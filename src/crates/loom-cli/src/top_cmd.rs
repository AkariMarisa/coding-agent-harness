//! `loom top`: live view over in-flight runs (`doc/15` §12.1).
//!
//! Reads only — never `Run::open` (it creates run directories) and never lock
//! reclamation (it deletes lock files): a watcher running in a second terminal
//! must not mutate the state it observes.
//!
//! Sources: event replay (which tasks are in flight), `run.json` (identity,
//! elapsed), `outbox/usage.jsonl` (steps, tokens, latency), native's
//! `logs/progress.json` (current phase — the `step.started` marker from
//! `doc/15` §12.1), `outbox/receipt.json` (outcome), `outbox/questions.json`
//! plus `.loom/answers/` (open questions), and `.loom/locks/`. Every read is
//! lenient: a torn line or a missing file degrades the row, never the view —
//! the watcher runs while those files are being written.

use std::fs;
use std::io::{IsTerminal, Write as _};
use std::path::Path;
use std::thread::sleep;
use std::time::Duration;

use anyhow::Result;
use loom_adapter::AdapterUsageRow;
use loom_core::{compact_utc, AnswersFile, QuestionsFile, Receipt, TaskStatus};
use loom_store::{now_ms, read_locks, read_task, task_statuses, Project, RunMeta};
use serde_json::{json, Value};

use super::run_util::find_run_for_task;
use super::Format;

/// Work dispatched but not yet settled. `passed`/`failed`/`merged` and the
/// rest belong to `loom board`; `top` answers "what is happening right now".
fn is_in_flight(state: TaskStatus) -> bool {
    matches!(
        state,
        TaskStatus::Dispatched | TaskStatus::Running | TaskStatus::Receipt | TaskStatus::NeedsInput
    )
}

/// Running first, then the states that wait on someone.
const fn rank(state: TaskStatus) -> u8 {
    match state {
        TaskStatus::Running => 0,
        TaskStatus::Dispatched => 1,
        TaskStatus::Receipt => 2,
        TaskStatus::NeedsInput => 3,
        _ => u8::MAX,
    }
}

#[derive(Debug, PartialEq, Eq)]
struct ReceiptInfo {
    status: String,
    pass: usize,
    fail: usize,
}

#[derive(Debug, PartialEq, Eq)]
struct QuestionInfo {
    open: usize,
    blocking: usize,
}

#[derive(Debug)]
struct Progress {
    task_id: String,
    run_id: String,
    step: u32,
    phase: String,
    tools: Vec<String>,
    started_ms: u64,
}

#[derive(Debug)]
struct Row {
    task_id: String,
    run_id: String,
    state: TaskStatus,
    adapter: Option<String>,
    model: Option<String>,
    elapsed_ms: Option<u64>,
    step: Option<u32>,
    max_steps: Option<u32>,
    phase: Option<String>,
    phase_age_ms: Option<u64>,
    tools: Vec<String>,
    input_tokens: Option<u64>,
    max_input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    last_latency_ms: Option<u64>,
    avg_latency_ms: Option<u64>,
    receipt: Option<ReceiptInfo>,
    questions: Option<QuestionInfo>,
    locks: Vec<String>,
}

impl Row {
    /// The one line a watcher reads: what this run is doing right now.
    fn action(&self) -> Option<String> {
        let mut parts: Vec<String> = Vec::new();

        if matches!(self.state, TaskStatus::Dispatched | TaskStatus::Running) {
            if let Some(phase) = &self.phase {
                let age = self.phase_age_ms.map(fmt_dur).unwrap_or_default();
                let text = match phase.as_str() {
                    "model" => format!("model call · {age}"),
                    "tools" if !self.tools.is_empty() => {
                        format!("tools {} · {age}", self.tools.join(","))
                    }
                    "tools" => format!("tools · {age}"),
                    "verify" => format!("acceptance · {age}"),
                    other => format!("{other} · {age}"),
                };
                parts.push(text);
            } else if self.step.is_some() {
                parts.push(format!(
                    "no live marker (step {} done)",
                    self.step.unwrap_or(0)
                ));
            } else {
                parts.push("starting".to_string());
            }
        }

        if self.state == TaskStatus::NeedsInput {
            parts.push("awaiting answer".to_string());
        } else if let Some(receipt) = &self.receipt {
            if receipt.pass + receipt.fail > 0 {
                parts.push(format!(
                    "acceptance {}/{} passed · awaiting verify",
                    receipt.pass,
                    receipt.pass + receipt.fail
                ));
            } else {
                parts.push(format!("receipt {} · awaiting verify", receipt.status));
            }
        }

        if let Some(questions) = &self.questions {
            if questions.open > 0 {
                parts.push(if questions.blocking > 0 {
                    format!("{} open ({} blocking)", questions.open, questions.blocking)
                } else {
                    format!("{} open", questions.open)
                });
            } else {
                parts.push("questions answered".to_string());
            }
        }

        (!parts.is_empty()).then(|| parts.join(" · "))
    }

    fn render(&self) -> String {
        let step = match (self.step, self.max_steps) {
            (Some(step), Some(max)) => format!("{step}/{max}"),
            (Some(step), None) => step.to_string(),
            _ => "-".to_string(),
        };
        let tokens = match (self.input_tokens, self.max_input_tokens) {
            (Some(used), Some(max)) => format!("{}/{}", fmt_tokens(used), fmt_tokens(max)),
            (Some(used), None) => fmt_tokens(used),
            _ => "-".to_string(),
        };
        let elapsed = self
            .elapsed_ms
            .map(fmt_dur)
            .unwrap_or_else(|| "-".to_string());
        let latency = match (self.last_latency_ms, self.avg_latency_ms) {
            (Some(last), Some(avg)) => format!("{}/{}", fmt_ms(last), fmt_ms(avg)),
            (Some(last), None) => fmt_ms(last),
            _ => "-".to_string(),
        };

        let mut tail: Vec<String> = Vec::new();
        if let (Some(adapter), Some(model)) = (&self.adapter, &self.model) {
            tail.push(format!("{adapter}/{model}"));
        }
        if let Some(action) = self.action() {
            tail.push(action);
        }
        if !self.locks.is_empty() {
            tail.push(format!("lock {}", self.locks.join(",")));
        }

        format!(
            "{:<14} {:<11} {:>5}  {:>10}  {:>8}  {:>11}\n  {}\n",
            self.task_id,
            self.state.as_str(),
            step,
            tokens,
            elapsed,
            latency,
            tail.join(" · "),
        )
    }

    fn to_json(&self) -> Value {
        json!({
            "task_id": self.task_id,
            "run_id": self.run_id,
            "state": self.state.as_str(),
            "adapter": self.adapter,
            "model": self.model,
            "elapsed_ms": self.elapsed_ms,
            "step": self.step,
            "max_steps": self.max_steps,
            "phase": self.phase,
            "phase_age_ms": self.phase_age_ms,
            "tools": self.tools,
            "input_tokens": self.input_tokens,
            "max_input_tokens": self.max_input_tokens,
            "output_tokens": self.output_tokens,
            "last_latency_ms": self.last_latency_ms,
            "avg_latency_ms": self.avg_latency_ms,
            "receipt": self.receipt.as_ref().map(|receipt| json!({
                "status": receipt.status,
                "verify_pass": receipt.pass,
                "verify_fail": receipt.fail,
            })),
            "questions": self.questions.as_ref().map(|questions| json!({
                "open": questions.open,
                "blocking": questions.blocking,
            })),
            "locks": self.locks,
        })
    }
}

struct Snapshot {
    now_ms: u64,
    max_running: usize,
    live_locks: usize,
    rows: Vec<Row>,
}

impl Snapshot {
    fn count(&self, state: TaskStatus) -> usize {
        self.rows.iter().filter(|row| row.state == state).count()
    }

    fn render(&self) -> String {
        let mut head = format!(
            "loom top {} · running {}/{}",
            compact_utc(self.now_ms),
            self.count(TaskStatus::Running),
            self.max_running,
        );
        for (state, label) in [
            (TaskStatus::Dispatched, "dispatched"),
            (TaskStatus::Receipt, "receipt"),
            (TaskStatus::NeedsInput, "needs_input"),
        ] {
            let count = self.count(state);
            if count > 0 {
                head.push_str(&format!(" · {label} {count}"));
            }
        }
        if self.live_locks > 0 {
            head.push_str(&format!(" · locks {}", self.live_locks));
        }
        if self.rows.is_empty() {
            head.push_str(" · idle");
        }

        let mut out = format!("{head}\n");
        for row in &self.rows {
            out.push_str(&row.render());
        }
        out
    }

    fn to_json(&self) -> Value {
        json!({
            "now": compact_utc(self.now_ms),
            "ts_ms": self.now_ms,
            "max_running": self.max_running,
            "counts": {
                "running": self.count(TaskStatus::Running),
                "dispatched": self.count(TaskStatus::Dispatched),
                "receipt": self.count(TaskStatus::Receipt),
                "needs_input": self.count(TaskStatus::NeedsInput),
                "locks": self.live_locks,
            },
            "rows": self.rows.iter().map(Row::to_json).collect::<Vec<_>>(),
        })
    }
}

/// Parse a whole file leniently: missing or mid-write files read as absent.
fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Option<T> {
    let text = fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

/// Usage rows as they land: a torn last line (the writer may be mid-append)
/// is skipped rather than failing the whole snapshot.
fn read_usage(path: &Path) -> Vec<AdapterUsageRow> {
    let Ok(text) = fs::read_to_string(path) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|line| serde_json::from_str::<AdapterUsageRow>(line).ok())
        .collect()
}

fn read_progress(path: &Path) -> Option<Progress> {
    let value: Value = serde_json::from_str(&fs::read_to_string(path).ok()?).ok()?;
    Some(Progress {
        task_id: value.get("task_id")?.as_str()?.to_string(),
        run_id: value.get("run_id")?.as_str()?.to_string(),
        step: value.get("step")?.as_u64()? as u32,
        phase: value.get("phase")?.as_str()?.to_string(),
        tools: value
            .get("tools")
            .and_then(Value::as_array)
            .map(|tools| {
                tools
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default(),
        started_ms: value.get("started_ms")?.as_u64()?,
    })
}

/// Open questions for this run: asked minus answered. Stale answers from an
/// older round never pair with a newer run (answers carry `run_id`).
fn read_questions(project: &Project, task_id: &str, run_dir: &Path) -> Option<QuestionInfo> {
    let questions: QuestionsFile = read_json(&run_dir.join("outbox").join("questions.json"))?;
    if questions.task_id != task_id {
        return None;
    }
    let answers: AnswersFile = read_json(
        &project
            .loom_dir()
            .join("answers")
            .join(format!("{task_id}.json")),
    )
    .unwrap_or_default();
    let open = |question: &loom_core::Question| {
        answers.choice_of(&question.id, &questions.run_id).is_none()
    };
    Some(QuestionInfo {
        open: questions.questions.iter().filter(|q| open(q)).count(),
        blocking: questions
            .questions
            .iter()
            .filter(|q| q.blocking && open(q))
            .count(),
    })
}

fn collect_row(
    project: &Project,
    task_id: &str,
    state: TaskStatus,
    now: u64,
    live_locks: &[(String, String)],
) -> Row {
    let run_id = find_run_for_task(project, task_id, None).unwrap_or_else(|_| "local".to_string());
    let run_dir = project.loom_dir().join("runs").join(&run_id);

    // Deliberately NOT `Run::open`: opening to read would create the run
    // directory of a run that does not exist, turning a watch into a write.
    let meta: Option<RunMeta> = read_json(&run_dir.join("run.json"));
    let usage = read_usage(&run_dir.join("outbox").join("usage.jsonl"));
    // The marker is only trusted for this task's this run; a mismatched file
    // (copied, or a run reusing the directory) reads as absent.
    let progress = read_progress(&run_dir.join("logs").join("progress.json"))
        .filter(|marker| marker.task_id == task_id && marker.run_id == run_id);
    let receipt: Option<Receipt> = read_json(&run_dir.join("outbox").join("receipt.json"));
    let questions = read_questions(project, task_id, &run_dir);
    let budget = read_task(project, task_id)
        .ok()
        .map(|spec| (spec.budget.max_steps, spec.budget.max_input_tokens));

    let steps_done = usage
        .iter()
        .filter_map(|row| row.step)
        .max()
        .unwrap_or(if usage.is_empty() {
            0
        } else {
            usage.len() as u32
        });
    let step = match state {
        TaskStatus::Running | TaskStatus::Dispatched => match &progress {
            // The in-flight step leads the flushed rows by one by design.
            Some(marker) => Some(marker.step.max(steps_done)),
            None => (steps_done > 0).then_some(steps_done),
        },
        _ => (steps_done > 0).then_some(steps_done),
    };

    let latencies: Vec<u64> = usage.iter().filter_map(|row| row.latency_ms).collect();
    let avg_latency = if latencies.is_empty() {
        None
    } else {
        Some(latencies.iter().sum::<u64>() / latencies.len() as u64)
    };

    Row {
        task_id: task_id.to_string(),
        run_id,
        state,
        adapter: meta.as_ref().map(|meta| meta.adapter.clone()),
        model: meta.as_ref().map(|meta| meta.model.clone()),
        elapsed_ms: meta
            .as_ref()
            .filter(|meta| meta.dispatched_at_ms > 0)
            .map(|meta| now.saturating_sub(meta.dispatched_at_ms)),
        step,
        max_steps: budget.map(|(max_steps, _)| max_steps),
        phase: progress.as_ref().map(|marker| marker.phase.clone()),
        phase_age_ms: progress
            .as_ref()
            .map(|marker| now.saturating_sub(marker.started_ms)),
        tools: progress
            .as_ref()
            .map(|marker| marker.tools.clone())
            .unwrap_or_default(),
        input_tokens: (!usage.is_empty())
            .then(|| usage.iter().map(|row| row.input_tokens.unwrap_or(0)).sum()),
        max_input_tokens: budget.map(|(_, max_input)| max_input),
        output_tokens: (!usage.is_empty())
            .then(|| usage.iter().map(|row| row.output_tokens.unwrap_or(0)).sum()),
        last_latency_ms: latencies.last().copied(),
        avg_latency_ms: avg_latency,
        receipt: receipt.map(|receipt| ReceiptInfo {
            status: receipt.status.as_str().to_string(),
            pass: receipt
                .verify
                .iter()
                .filter(|item| item.result == loom_core::VerifyResult::Pass)
                .count(),
            fail: receipt
                .verify
                .iter()
                .filter(|item| item.result == loom_core::VerifyResult::Fail)
                .count(),
        }),
        questions,
        locks: live_locks
            .iter()
            .filter(|(owner, _)| owner == task_id)
            .map(|(_, path)| path.clone())
            .collect(),
    }
}

fn collect(project: &Project) -> Result<Snapshot> {
    let now = now_ms();
    let statuses = task_statuses(project)?;
    let config = project.load_config()?;

    // Read-only counterpart of `live_locks`: reclaiming is board/dispatch's
    // job, a watcher must not delete anything.
    let live_locks: Vec<(String, String)> = read_locks(project)?
        .into_iter()
        .filter(|lock| !lock.is_reclaimable(now))
        .map(|lock| (lock.owner_task, lock.path))
        .collect();

    let mut in_flight: Vec<(String, TaskStatus)> = statuses
        .into_iter()
        .filter(|(_, state)| is_in_flight(*state))
        .collect();
    in_flight.sort_by(|a, b| rank(a.1).cmp(&rank(b.1)).then_with(|| a.0.cmp(&b.0)));

    let rows = in_flight
        .iter()
        .map(|(task_id, state)| collect_row(project, task_id, *state, now, &live_locks))
        .collect();

    Ok(Snapshot {
        now_ms: now,
        max_running: config.parallel.max_running,
        live_locks: live_locks.len(),
        rows,
    })
}

/// Live on a TTY with `--format pretty`; everything else prints one snapshot
/// (JSON is a query, and a pipe must not receive an ANSI refresh loop).
pub fn top(project: &Project, once: bool, interval: u64, format: Format) -> Result<i32> {
    let watch = matches!(format, Format::Pretty) && !once && std::io::stdout().is_terminal();
    loop {
        let snapshot = collect(project)?;
        match format {
            Format::Json => {
                println!("{}", serde_json::to_string_pretty(&snapshot.to_json())?);
                return Ok(0);
            }
            Format::Pretty => {
                if watch {
                    print!("\x1b[2J\x1b[H");
                }
                print!("{}", snapshot.render());
                if !watch {
                    return Ok(0);
                }
                std::io::stdout().flush()?;
            }
        }
        sleep(Duration::from_secs(interval.max(1)));
    }
}

fn fmt_tokens(count: u64) -> String {
    if count >= 1_000_000 {
        format!("{:.1}M", count as f64 / 1_000_000.0)
    } else if count >= 10_000 {
        format!("{:.0}k", count as f64 / 1_000.0)
    } else if count >= 1_000 {
        format!("{:.1}k", count as f64 / 1_000.0)
    } else {
        count.to_string()
    }
}

/// Whole seconds for ages: `850ms`, `8s`, `1m12s`, `2h05m`.
fn fmt_dur(ms: u64) -> String {
    if ms < 1_000 {
        format!("{ms}ms")
    } else if ms < 60_000 {
        format!("{}s", ms / 1_000)
    } else if ms < 3_600_000 {
        format!("{}m{:02}s", ms / 60_000, (ms % 60_000) / 1_000)
    } else {
        format!("{}h{:02}m", ms / 3_600_000, (ms % 3_600_000) / 60_000)
    }
}

/// Latency keeps sub-second precision: `850ms`, `1.2s`.
fn fmt_ms(ms: u64) -> String {
    if ms < 1_000 {
        format!("{ms}ms")
    } else {
        format!("{:.1}s", ms as f64 / 1_000.0)
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use loom_adapter::ADAPTER_USAGE_VERSION;
    use loom_core::{
        Answer, AnswersFile, Event, EventType, Question, QuestionOption, QuestionScope,
        QuestionsFile, Receipt, ReceiptStatus, TaskRole, TaskSpec, TaskStatus, Tier,
        VerifyEvidence, VerifyResult, ANSWERS_VERSION, QUESTIONS_VERSION, TASK_VERSION,
    };
    use loom_store::{append_event, now_ms, write_task, Project, RunMeta, RUN_META_VERSION};

    use super::*;

    const TASK: &str = "T-20261120-001";

    fn spec(task_id: &str) -> TaskSpec {
        TaskSpec {
            schema_version: TASK_VERSION.into(),
            task_id: task_id.into(),
            goal_id: "G-20261120-top".into(),
            title: "top test".into(),
            role: TaskRole::Implement,
            tier: Tier::Worker,
            workspace: loom_core::TaskWorkspace {
                mode: loom_core::WorkspaceMode::InPlace,
                base_ref: "main".into(),
                branch: format!("agent/{task_id}"),
                allowed_paths: vec!["src/**".into()],
                forbidden_paths: vec![],
            },
            acceptance: vec![loom_core::Acceptance {
                id: "A1".into(),
                cmd: "true".into(),
                expect_exit: 0,
                timeout_s: 5,
            }],
            budget: loom_core::TaskBudget {
                max_input_tokens: 140_000,
                max_output_tokens: 20_000,
                max_steps: 5,
                max_wall_s: 300,
                max_cost_usd: 1.0,
            },
            output: loom_core::TaskOutput {
                report_path: ".loom/runs/local/outbox/report.md".into(),
                ..loom_core::TaskOutput::default()
            },
            ..TaskSpec::default()
        }
    }

    fn emit(project: &Project, run_id: &str, task_id: &str, kind: EventType) {
        append_event(
            project,
            &Event::new(run_id, now_ms(), kind).task(task_id).attempt(1),
        )
        .expect("append event");
    }

    fn run_dir(project: &Project, run_id: &str) -> PathBuf {
        let dir = project.loom_dir().join("runs").join(run_id);
        fs::create_dir_all(dir.join("outbox")).expect("outbox");
        fs::create_dir_all(dir.join("logs")).expect("logs");
        dir
    }

    fn write_meta(project: &Project, run_id: &str, task_id: &str, dispatched_at_ms: u64) {
        let meta = RunMeta {
            v: RUN_META_VERSION,
            run_id: run_id.into(),
            task_id: task_id.into(),
            adapter: "native".into(),
            model: "mimo-v2.6-flash".into(),
            model_id: "mimo".into(),
            workspace: "/tmp/ws".into(),
            base_sha: "abc".into(),
            dispatched_at_ms,
            timeout_s: 300,
            packet_dir: "/tmp/ws/.loom/packet".into(),
        };
        fs::write(
            project
                .loom_dir()
                .join("runs")
                .join(run_id)
                .join("run.json"),
            serde_json::to_string_pretty(&meta).expect("serialize meta"),
        )
        .expect("write run.json");
    }

    fn usage_row(
        run_id: &str,
        task_id: &str,
        step: u32,
        latency_ms: u64,
        input: u64,
        output: u64,
        tools: &[&str],
    ) -> AdapterUsageRow {
        AdapterUsageRow {
            v: ADAPTER_USAGE_VERSION,
            run_id: run_id.into(),
            task_id: task_id.into(),
            model: Some("mimo".into()),
            input_tokens: Some(input),
            output_tokens: Some(output),
            latency_ms: Some(latency_ms),
            step: Some(step),
            tool_names: Some(tools.iter().map(|tool| (*tool).to_string()).collect()),
            ..AdapterUsageRow::default()
        }
    }

    fn write_usage(project: &Project, run_id: &str, rows: &[AdapterUsageRow]) {
        let text: String = rows
            .iter()
            .map(|row| serde_json::to_string(row).expect("row"))
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(
            project
                .loom_dir()
                .join("runs")
                .join(run_id)
                .join("outbox")
                .join("usage.jsonl"),
            format!("{text}\n"),
        )
        .expect("write usage");
    }

    /// Full fixture: a running native run with flushed rows, a live phase
    /// marker, and a held write lock.
    fn running_fixture(project: &Project) {
        fs::create_dir_all(project.loom_dir()).expect("loom dir");
        write_task(project, &spec(TASK), false).expect("task");
        emit(project, "R-1", TASK, EventType::TaskDispatched);
        emit(project, "R-1", TASK, EventType::TaskStarted);

        let now = now_ms();
        run_dir(project, "R-1");
        write_meta(project, "R-1", TASK, now.saturating_sub(61_000));
        write_usage(
            project,
            "R-1",
            &[
                usage_row("R-1", TASK, 1, 800, 9_000, 400, &["read"]),
                usage_row("R-1", TASK, 2, 1_200, 9_400, 400, &["write"]),
            ],
        );
        fs::write(
            project
                .loom_dir()
                .join("runs")
                .join("R-1")
                .join("logs")
                .join("progress.json"),
            json!({
                "v": 1,
                "task_id": TASK,
                "run_id": "R-1",
                "step": 3,
                "phase": "tools",
                "tools": ["bash", "edit"],
                "started_ms": now.saturating_sub(8_000),
            })
            .to_string(),
        )
        .expect("write progress");

        fs::create_dir_all(project.loom_dir().join("locks")).expect("locks dir");
        fs::write(
            project.loom_dir().join("locks").join("src.lock"),
            json!({
                "v": 1,
                "path": "src/**",
                "mode": "write",
                "owner_task": TASK,
                "owner_run": "R-1",
                "owner_pid": std::process::id(),
                "lease_until_ms": now.saturating_add(600_000),
            })
            .to_string(),
        )
        .expect("write lock");
    }

    #[test]
    fn running_row_carries_marker_progress_and_metrics() {
        let dir = tempfile::tempdir().expect("tempdir");
        let project = Project::at(dir.path());
        running_fixture(&project);

        let snapshot = collect(&project).expect("collect");
        assert_eq!(snapshot.rows.len(), 1);
        let row = &snapshot.rows[0];

        assert_eq!(row.state, TaskStatus::Running);
        assert_eq!(row.run_id, "R-1");
        assert_eq!(row.adapter.as_deref(), Some("native"));
        assert_eq!(row.model.as_deref(), Some("mimo-v2.6-flash"));
        // The in-flight step leads the flushed rows: 3 > 2.
        assert_eq!(row.step, Some(3));
        assert_eq!(row.max_steps, Some(5));
        assert_eq!(row.phase.as_deref(), Some("tools"));
        assert_eq!(row.tools, ["bash", "edit"]);
        assert_eq!(row.input_tokens, Some(18_400));
        assert_eq!(row.max_input_tokens, Some(140_000));
        assert_eq!(row.output_tokens, Some(800));
        assert_eq!(row.last_latency_ms, Some(1_200));
        assert_eq!(row.avg_latency_ms, Some(1_000));
        assert!(row.elapsed_ms.unwrap_or(0) >= 61_000, "run elapsed");
        assert!(row.phase_age_ms.unwrap_or(0) >= 8_000, "phase age");
        assert_eq!(row.locks, ["src/**"]);
        assert_eq!(row.receipt, None);

        let rendered = snapshot.render();
        assert!(rendered.contains("loom top"), "{rendered}");
        assert!(rendered.contains(TASK), "{rendered}");
        assert!(rendered.contains("tools bash,edit"), "{rendered}");

        let value = snapshot.to_json();
        assert_eq!(value["rows"][0]["state"], "running");
        assert_eq!(value["rows"][0]["step"], 3);
        assert_eq!(value["counts"]["running"], 1);
    }

    #[test]
    fn watching_never_creates_run_directories() {
        let dir = tempfile::tempdir().expect("tempdir");
        let project = Project::at(dir.path());
        fs::create_dir_all(project.loom_dir()).expect("loom dir");
        write_task(&project, &spec(TASK), false).expect("task");
        emit(&project, "R-ghost", TASK, EventType::TaskDispatched);

        // No runs directory at all: the row must still appear (the event says
        // work is in flight) without `Run::open` materializing `runs/local`.
        let snapshot = collect(&project).expect("collect");
        assert_eq!(snapshot.rows.len(), 1);
        let row = &snapshot.rows[0];
        assert_eq!(row.state, TaskStatus::Dispatched);
        assert_eq!(row.adapter, None);
        assert_eq!(row.step, None);
        assert!(row.action().is_some(), "degraded row still explains itself");
        assert!(
            !project.loom_dir().join("runs").exists(),
            "collect must not create run directories"
        );
    }

    #[test]
    fn torn_usage_and_stale_marker_degrade_instead_of_failing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let project = Project::at(dir.path());
        fs::create_dir_all(project.loom_dir()).expect("loom dir");
        let task_id = "T-20261120-002";
        write_task(&project, &spec(task_id), false).expect("task");
        emit(&project, "R-2", task_id, EventType::TaskStarted);

        let dir_run = run_dir(&project, "R-2");
        write_meta(&project, "R-2", task_id, now_ms());
        // A torn half-line, as caught mid-append by a concurrent writer.
        fs::write(
            dir_run.join("outbox").join("usage.jsonl"),
            format!(
                "{}\n{}",
                serde_json::to_string(&usage_row("R-2", task_id, 1, 500, 4_000, 100, &["read"]))
                    .expect("row"),
                r#"{"v":1,"run"#,
            ),
        )
        .expect("write usage");
        // Malformed marker: absent, not fatal.
        fs::write(dir_run.join("logs").join("progress.json"), "{half").expect("write");

        let snapshot = collect(&project).expect("collect");
        let row = &snapshot.rows[0];
        assert_eq!(row.input_tokens, Some(4_000), "torn line skipped");
        assert_eq!(row.step, Some(1));
        assert_eq!(row.phase, None, "malformed marker reads as absent");
        let action = row.action().expect("action");
        assert!(action.contains("no live marker"), "{action}");
    }

    #[test]
    fn needs_input_reports_open_questions_scoped_to_the_run() {
        let dir = tempfile::tempdir().expect("tempdir");
        let project = Project::at(dir.path());
        fs::create_dir_all(project.loom_dir()).expect("loom dir");
        let task_id = "T-20261120-003";
        write_task(&project, &spec(task_id), false).expect("task");
        emit(&project, "R-3", task_id, EventType::TaskNeedsInput);
        let dir_run = run_dir(&project, "R-3");
        write_meta(&project, "R-3", task_id, now_ms());

        let question = |id: &str, blocking: bool| Question {
            id: id.into(),
            question: format!("Question {id}?"),
            why_now: None,
            blocking,
            scope: QuestionScope::Design,
            options: vec![QuestionOption {
                id: "A".into(),
                label: "a".into(),
                consequence: None,
            }],
            recommended: None,
            default: Some("A".into()),
            reversible: false,
            evidence: Vec::new(),
            ttl_s: None,
        };
        fs::write(
            dir_run.join("outbox").join("questions.json"),
            serde_json::to_string_pretty(&QuestionsFile {
                v: QUESTIONS_VERSION,
                task_id: task_id.into(),
                run_id: "R-3".into(),
                questions: vec![question("Q1", true), question("Q2", false)],
            })
            .expect("serialize questions"),
        )
        .expect("write questions");

        // Nothing answered: both open, one blocking.
        let snapshot = collect(&project).expect("collect");
        let row = &snapshot.rows[0];
        assert_eq!(row.state, TaskStatus::NeedsInput);
        assert_eq!(
            row.questions,
            Some(QuestionInfo {
                open: 2,
                blocking: 1
            })
        );
        let action = row.action().expect("action");
        assert!(action.contains("2 open (1 blocking)"), "{action}");
        assert!(action.contains("awaiting answer"), "{action}");

        // Answer Q1 for THIS run; Q2 stays open. An answer tagged with an
        // older run must not pair with the current round's Q1.
        let answers_path = project
            .loom_dir()
            .join("answers")
            .join(format!("{task_id}.json"));
        fs::create_dir_all(answers_path.parent().expect("answers dir")).expect("mkdir");
        fs::write(
            &answers_path,
            serde_json::to_string_pretty(&AnswersFile {
                v: ANSWERS_VERSION,
                task_id: task_id.into(),
                answers: vec![
                    Answer {
                        question_id: "Q1".into(),
                        run_id: "R-3".into(),
                        choice: "A".into(),
                        by: "human".into(),
                        rationale: None,
                        assumption_id: None,
                    },
                    Answer {
                        question_id: "Q2".into(),
                        run_id: "R-old".into(),
                        choice: "A".into(),
                        by: "human".into(),
                        rationale: None,
                        assumption_id: None,
                    },
                ],
            })
            .expect("serialize answers"),
        )
        .expect("write answers");

        let snapshot = collect(&project).expect("collect");
        assert_eq!(
            snapshot.rows[0].questions,
            Some(QuestionInfo {
                open: 1,
                blocking: 0
            }),
            "stale run's answer must not close the current round"
        );
    }

    #[test]
    fn receipt_row_reports_acceptance_outcome() {
        let dir = tempfile::tempdir().expect("tempdir");
        let project = Project::at(dir.path());
        fs::create_dir_all(project.loom_dir()).expect("loom dir");
        let task_id = "T-20261120-004";
        write_task(&project, &spec(task_id), false).expect("task");
        emit(&project, "R-4", task_id, EventType::TaskReceipt);
        let dir_run = run_dir(&project, "R-4");
        write_meta(&project, "R-4", task_id, now_ms());

        let mut receipt = Receipt::new(task_id, 1);
        receipt.status = ReceiptStatus::Done;
        receipt.verify = vec![
            VerifyEvidence {
                id: "A1".into(),
                result: VerifyResult::Pass,
                ..VerifyEvidence::default()
            },
            VerifyEvidence {
                id: "A2".into(),
                result: VerifyResult::Fail,
                ..VerifyEvidence::default()
            },
        ];
        fs::write(
            dir_run.join("outbox").join("receipt.json"),
            serde_json::to_string_pretty(&receipt).expect("serialize receipt"),
        )
        .expect("write receipt");

        let snapshot = collect(&project).expect("collect");
        let row = &snapshot.rows[0];
        assert_eq!(row.state, TaskStatus::Receipt);
        assert_eq!(
            row.receipt,
            Some(ReceiptInfo {
                status: "done".into(),
                pass: 1,
                fail: 1
            })
        );
        let action = row.action().expect("action");
        assert!(action.contains("acceptance 1/2 passed"), "{action}");
        assert!(action.contains("awaiting verify"), "{action}");
    }

    #[test]
    fn fmt_helpers_render_human_units() {
        assert_eq!(fmt_tokens(950), "950");
        assert_eq!(fmt_tokens(1_500), "1.5k");
        assert_eq!(fmt_tokens(18_400), "18k");
        assert_eq!(fmt_tokens(140_000), "140k");
        assert_eq!(fmt_tokens(2_500_000), "2.5M");
        assert_eq!(fmt_dur(850), "850ms");
        assert_eq!(fmt_dur(8_000), "8s");
        assert_eq!(fmt_dur(72_000), "1m12s");
        assert_eq!(fmt_dur(7_500_000), "2h05m");
        assert_eq!(fmt_ms(850), "850ms");
        assert_eq!(fmt_ms(1_200), "1.2s");
    }
}
