//! `loom answer`: land human choices in `.loom/answers/<task>.json`.
//!
//! The handshake (`doc/15` §2.6): an adapter asked via `outbox/questions.json`
//! and exited 2, verify recorded `task.needs_input`, and this command writes
//! the answers and emits `task.answered`, which moves the task back to
//! `ready`. The event fires only once no blocking question is left open —
//! `blocking` means exactly that the task may not resume yet.

use std::fs;

use anyhow::{bail, Context as _, Result};
use loom_core::{
    status_after_event, Answer, AnswersFile, EventType, QuestionsFile, TaskStatus, ANSWERS_VERSION,
};
use loom_store::{append_event, now_ms, read_task, sync_index, task_status, Project};
use serde_json::json;

use super::run_util::find_run_for_task;
use super::Format;

/// Answer questions for `task_id`.
///
/// `-q`/`-c` may repeat and are paired in order; `--rationale`/`--by` apply
/// to every pair in this invocation.
pub fn answer(
    project: &Project,
    task_id: &str,
    question_ids: &[String],
    choices: &[String],
    rationale: Option<&str>,
    by: &str,
    format: Format,
) -> Result<i32> {
    if question_ids.is_empty() {
        bail!("pass at least one -q <question-id> with its -c <choice>");
    }
    if question_ids.len() != choices.len() {
        bail!(
            "{} -q but {} -c; pair them in order",
            question_ids.len(),
            choices.len()
        );
    }
    read_task(project, task_id)?;

    let status = task_status(project, task_id)?;
    if matches!(
        status,
        TaskStatus::Passed | TaskStatus::Merged | TaskStatus::Cancelled
    ) {
        bail!(
            "task {task_id} is {}; its questions are closed, an answer would have no effect",
            status.as_str()
        );
    }

    // The questions live with the run that asked them; question ids restart
    // at Q1 every round, so answers are keyed by (question, run).
    let run_id = find_run_for_task(project, task_id, None)?;
    let questions_path = project
        .loom_dir()
        .join("runs")
        .join(&run_id)
        .join("outbox")
        .join("questions.json");
    let text = fs::read_to_string(&questions_path).with_context(|| {
        format!(
            "read {}; no run of {task_id} has asked anything",
            project.relative(&questions_path)
        )
    })?;
    let questions: QuestionsFile =
        serde_json::from_str(&text).context("parse questions.json (it is a protocol file)")?;
    if questions.task_id != task_id {
        bail!(
            "{} asks about {}, not {task_id}",
            project.relative(&questions_path),
            questions.task_id
        );
    }

    let answers_path = project
        .loom_dir()
        .join("answers")
        .join(format!("{task_id}.json"));
    let mut answers: AnswersFile = match fs::read_to_string(&answers_path) {
        Ok(text) => serde_json::from_str(&text)
            .with_context(|| format!("parse {}", project.relative(&answers_path)))?,
        Err(_) => AnswersFile {
            v: ANSWERS_VERSION,
            task_id: task_id.to_string(),
            answers: Vec::new(),
        },
    };
    if answers.v != ANSWERS_VERSION {
        answers.v = ANSWERS_VERSION;
    }

    for (question_id, choice) in question_ids.iter().zip(choices) {
        answers.record(Answer {
            question_id: question_id.clone(),
            run_id: run_id.clone(),
            choice: choice.clone(),
            by: by.to_string(),
            rationale: rationale.map(str::to_owned),
            assumption_id: None,
        });
    }
    // Validate the whole current-round view, not just what was typed: a bad
    // earlier answer must not survive into a file this command is about to
    // rewrite.
    answers
        .validate_against(&questions)
        .map_err(|message| anyhow::anyhow!("{message}"))?;

    fs::create_dir_all(answers_path.parent().expect("answers dir"))
        .context("create .loom/answers")?;
    let text = serde_json::to_string_pretty(&answers).context("serialize answers")?;
    fs::write(&answers_path, format!("{text}\n"))
        .with_context(|| format!("write {}", project.relative(&answers_path)))?;

    // Blocking questions gate the resume: while one is open the task stays
    // `needs_input`, and only the last answer emits `task.answered`.
    let pending_blocking: Vec<String> = questions
        .questions
        .iter()
        .filter(|question| question.blocking)
        .filter(|question| answers.choice_of(&question.id, &run_id).is_none())
        .map(|question| question.id.clone())
        .collect();

    let mut emitted = false;
    if pending_blocking.is_empty() {
        append_event(
            project,
            &loom_core::Event::new(&run_id, now_ms(), EventType::TaskAnswered)
                .task(task_id)
                .data([
                    ("question_ids".to_string(), json!(question_ids)),
                    ("by".to_string(), json!(by)),
                    ("source".to_string(), json!("cli")),
                    ("assumption_ids".to_string(), json!([])),
                ]),
        )?;
        sync_index(project)?;
        emitted = true;
    }

    let after = if emitted {
        status_after_event(status, EventType::TaskAnswered)
    } else {
        status
    };
    let event = if emitted { Some("task.answered") } else { None };

    match format {
        Format::Json => println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "task_id": task_id,
                "run_id": run_id,
                "answered": question_ids,
                "pending_blocking": pending_blocking,
                "event": event,
                "status": after.as_str(),
                "answers_path": project.relative(&answers_path),
            }))?
        ),
        Format::Pretty => {
            for (question_id, choice) in question_ids.iter().zip(choices) {
                println!("answered {task_id} {question_id} = {choice} ({by})");
            }
            if emitted {
                println!("{}: task.answered → {}", task_id, after.as_str());
            } else {
                println!(
                    "{} is still waiting: {} blocking question(s) open: {}",
                    task_id,
                    pending_blocking.len(),
                    pending_blocking.join(", ")
                );
            }
        }
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use loom_core::{TaskRole, TaskSpec, Tier, TASK_VERSION};
    use loom_store::{append_event, now_ms, Project, Run, RunMeta, RUN_META_VERSION};

    use super::*;

    const TASK: &str = "T-20261107-002";
    const RUN: &str = "R-20261107-001";

    fn spec() -> TaskSpec {
        TaskSpec {
            schema_version: TASK_VERSION.into(),
            task_id: TASK.into(),
            goal_id: "G-20261107-qa".into(),
            title: "answer handshake".into(),
            role: TaskRole::Implement,
            tier: Tier::Worker,
            workspace: loom_core::TaskWorkspace {
                mode: loom_core::WorkspaceMode::InPlace,
                base_ref: "main".into(),
                branch: "agent/T-20261107-002".into(),
                allowed_paths: vec!["src/**".into()],
                forbidden_paths: vec![],
            },
            acceptance: vec![loom_core::Acceptance {
                id: "A1".into(),
                cmd: "true".into(),
                expect_exit: 0,
                timeout_s: 5,
            }],
            output: loom_core::TaskOutput {
                report_path: ".loom/runs/local/outbox/report.md".into(),
                ..loom_core::TaskOutput::default()
            },
            ..TaskSpec::default()
        }
    }

    fn questions(blocking: &[&str]) -> QuestionsFile {
        QuestionsFile {
            v: loom_core::QUESTIONS_VERSION,
            task_id: TASK.into(),
            run_id: RUN.into(),
            questions: blocking
                .iter()
                .map(|id| loom_core::Question {
                    id: (*id).into(),
                    question: format!("Question {id}?"),
                    why_now: None,
                    blocking: true,
                    scope: loom_core::QuestionScope::Design,
                    options: vec![
                        loom_core::QuestionOption {
                            id: "A".into(),
                            label: "first".into(),
                            consequence: None,
                        },
                        loom_core::QuestionOption {
                            id: "B".into(),
                            label: "second".into(),
                            consequence: None,
                        },
                    ],
                    recommended: Some("A".into()),
                    default: Some("A".into()),
                    reversible: false,
                    evidence: Vec::new(),
                    ttl_s: None,
                })
                .collect(),
        }
    }

    /// Task registered, dispatched, and waiting: exactly the state `loom
    /// answer` is meant to find.
    fn fixture(project: &Project, questions: &QuestionsFile) {
        fs::create_dir_all(project.loom_dir()).expect("loom dir");
        loom_store::register_task(project, "local", &spec()).expect("register");
        let run = Run::open(project.clone(), Some(RUN)).expect("run");
        run.write_meta(&RunMeta {
            v: RUN_META_VERSION,
            run_id: RUN.into(),
            task_id: TASK.into(),
            adapter: "native".into(),
            workspace: ".".into(),
            dispatched_at_ms: 1,
            ..RunMeta::default()
        })
        .expect("meta");
        fs::create_dir_all(run.outbox_dir()).expect("outbox");
        fs::write(
            run.outbox_dir().join("questions.json"),
            serde_json::to_string_pretty(questions).expect("serialize"),
        )
        .expect("questions");
        append_event(
            project,
            &loom_core::Event::new(RUN, now_ms(), EventType::TaskNeedsInput)
                .task(TASK)
                .attempt(1),
        )
        .expect("needs_input event");
    }

    fn answer_json(project: &Project, args: &[&str]) -> Result<serde_json::Value> {
        // Capture stdout? No: the command prints, we assert on state instead.
        // This helper only runs the command for its side effects.
        let question_ids: Vec<String> = args
            .iter()
            .step_by(2)
            .map(|value| (*value).to_string())
            .collect();
        let choices: Vec<String> = args
            .iter()
            .skip(1)
            .step_by(2)
            .map(|value| (*value).to_string())
            .collect();
        answer(
            project,
            TASK,
            &question_ids,
            &choices,
            Some("keep the rollback path"),
            "human",
            Format::Json,
        )?;
        let path = project
            .loom_dir()
            .join("answers")
            .join(format!("{TASK}.json"));
        let text = fs::read_to_string(&path).expect("answers file");
        Ok(serde_json::from_str(&text).expect("parse answers"))
    }

    #[test]
    fn answering_the_last_blocking_question_resumes_the_task() {
        let dir = tempfile::tempdir().expect("tempdir");
        let project = Project::at(dir.path());
        fixture(&project, &questions(&["Q1"]));

        let answers = answer_json(&project, &["Q1", "A"]).expect("answer");
        assert_eq!(answers["task_id"], TASK);
        assert_eq!(answers["answers"][0]["question_id"], "Q1");
        assert_eq!(answers["answers"][0]["choice"], "A");
        assert_eq!(answers["answers"][0]["run_id"], RUN);
        assert_eq!(answers["answers"][0]["by"], "human");

        // The event moved needs_input → ready (guarded arm).
        assert_eq!(
            task_status(&project, TASK).expect("status"),
            TaskStatus::Ready
        );
        let events = loom_store::read_events(
            &project,
            &loom_core::EventFilter {
                task_id: Some(TASK.into()),
                ..loom_core::EventFilter::default()
            },
        )
        .expect("events");
        assert!(events
            .iter()
            .any(|event| event.event_type == EventType::TaskAnswered));
    }

    #[test]
    fn a_still_open_blocking_question_keeps_the_task_waiting() {
        let dir = tempfile::tempdir().expect("tempdir");
        let project = Project::at(dir.path());
        fixture(&project, &questions(&["Q1", "Q2"]));

        let answers = answer_json(&project, &["Q1", "B"]).expect("answer");
        assert_eq!(answers["answers"].as_array().expect("array").len(), 1);

        // Q2 is still open: no event, no state change.
        assert_eq!(
            task_status(&project, TASK).expect("status"),
            TaskStatus::NeedsInput
        );
        let events = loom_store::read_events(
            &project,
            &loom_core::EventFilter {
                task_id: Some(TASK.into()),
                ..loom_core::EventFilter::default()
            },
        )
        .expect("events");
        assert!(!events
            .iter()
            .any(|event| event.event_type == EventType::TaskAnswered));

        // Answering the second one resumes.
        answer_json(&project, &["Q2", "A"]).expect("answer");
        assert_eq!(
            task_status(&project, TASK).expect("status"),
            TaskStatus::Ready
        );
    }

    #[test]
    fn rejects_unknown_questions_choices_and_pairing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let project = Project::at(dir.path());
        fixture(&project, &questions(&["Q1"]));

        let error = answer(
            &project,
            TASK,
            &["Q9".to_string()],
            &["A".to_string()],
            None,
            "human",
            Format::Json,
        )
        .expect_err("unknown question");
        assert!(error.to_string().contains("unknown question"), "{error}");

        let error = answer(
            &project,
            TASK,
            &["Q1".to_string()],
            &["Z".to_string()],
            None,
            "human",
            Format::Json,
        )
        .expect_err("unknown choice");
        assert!(
            error.to_string().contains("not one of the offered options"),
            "{error}"
        );

        let error = answer(
            &project,
            TASK,
            &["Q1".to_string(), "Q2".to_string()],
            &["A".to_string()],
            None,
            "human",
            Format::Json,
        )
        .expect_err("mismatched pairs");
        assert!(error.to_string().contains("pair them"), "{error}");

        let error = answer(&project, TASK, &[], &[], None, "human", Format::Json)
            .expect_err("nothing to answer");
        assert!(error.to_string().contains("-q"), "{error}");

        let error = answer(
            &project,
            TASK,
            &["Q1".to_string()],
            &["A".to_string()],
            None,
            "oracle",
            Format::Json,
        )
        .expect_err("unknown by");
        assert!(error.to_string().contains("unknown `by`"), "{error}");

        // None of the failures wrote an answers file or moved the task.
        assert!(!project
            .loom_dir()
            .join("answers")
            .join(format!("{TASK}.json"))
            .exists());
        assert_eq!(
            task_status(&project, TASK).expect("status"),
            TaskStatus::NeedsInput
        );
    }

    #[test]
    fn fails_fast_when_nothing_was_asked() {
        let dir = tempfile::tempdir().expect("tempdir");
        let project = Project::at(dir.path());
        fs::create_dir_all(project.loom_dir()).expect("loom dir");
        loom_store::register_task(&project, "local", &spec()).expect("register");

        let error = answer(
            &project,
            TASK,
            &["Q1".to_string()],
            &["A".to_string()],
            None,
            "human",
            Format::Json,
        )
        .expect_err("no questions");
        assert!(error.to_string().contains("has asked anything"), "{error}");
    }

    #[test]
    fn closed_tasks_refuse_answers() {
        let dir = tempfile::tempdir().expect("tempdir");
        let project = Project::at(dir.path());
        fixture(&project, &questions(&["Q1"]));
        append_event(
            &project,
            &loom_core::Event::new(RUN, now_ms(), EventType::TaskPassed).task(TASK),
        )
        .expect("passed event");

        let error = answer(
            &project,
            TASK,
            &["Q1".to_string()],
            &["A".to_string()],
            None,
            "human",
            Format::Json,
        )
        .expect_err("closed");
        assert!(error.to_string().contains("no effect"), "{error}");
    }
}
