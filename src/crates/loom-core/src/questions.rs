//! `questions.json` / answers contract (`doc/15` §2.4, §2.6).
//!
//! A question is a return value, not a pause: the executor writes
//! `outbox/questions.json` and exits 2, the control plane records
//! `task.needs_input`, and `loom answer` lands choices in
//! `.loom/answers/<task>.json` before the next dispatch.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

pub const QUESTIONS_VERSION: u32 = 1;
pub const ANSWERS_VERSION: u32 = 1;

/// A question stops being answerable by the worker alone once its scope
/// leaves the reversible/implementation corner (`doc/15` §2.6 matrix).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuestionScope {
    Implementation,
    Design,
    Contract,
    Security,
    Budget,
    Scope,
}

impl QuestionScope {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Implementation => "implementation",
            Self::Design => "design",
            Self::Contract => "contract",
            Self::Security => "security",
            Self::Budget => "budget",
            Self::Scope => "scope",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuestionOption {
    pub id: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consequence: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Question {
    pub id: String,
    pub question: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why_now: Option<String>,
    pub blocking: bool,
    pub scope: QuestionScope,
    pub options: Vec<QuestionOption>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recommended: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    #[serde(default)]
    pub reversible: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttl_s: Option<u64>,
}

/// What the executor wrote when it stopped for input (`doc/15` §2.4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuestionsFile {
    pub v: u32,
    pub task_id: String,
    pub run_id: String,
    pub questions: Vec<Question>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Answer {
    pub question_id: String,
    /// The run that asked the question. Question ids restart at `Q1` every
    /// run, so pairing without this would splice round 1's answer onto
    /// round 2's differently-worded `Q1`.
    pub run_id: String,
    pub choice: String,
    /// `human` | `judgment` | `policy` | `spec` (`doc/15` §2.6).
    pub by: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rationale: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assumption_id: Option<String>,
}

/// Control-plane side of the handshake (`.loom/answers/<task>.json`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnswersFile {
    pub v: u32,
    pub task_id: String,
    pub answers: Vec<Answer>,
}

/// At most 3 blocking questions per task; beyond that, merge or split
/// (`doc/15` §2.4). Returns the validation error for the caller to surface.
impl QuestionsFile {
    pub fn validate(&self) -> Result<(), String> {
        if self.v != QUESTIONS_VERSION {
            return Err(format!(
                "unsupported questions version: {} (expected {QUESTIONS_VERSION})",
                self.v
            ));
        }
        if self.questions.is_empty() {
            return Err("questions must not be empty".to_string());
        }
        let mut ids = HashSet::new();
        let mut blocking = 0usize;
        for question in &self.questions {
            if question.id.is_empty() {
                return Err("question id must not be empty".to_string());
            }
            if !ids.insert(question.id.as_str()) {
                return Err(format!("duplicate question id: {}", question.id));
            }
            if question.question.trim().is_empty() {
                return Err(format!("question {} has empty text", question.id));
            }
            if question.options.is_empty() {
                return Err(format!("question {} has no options", question.id));
            }
            let mut option_ids = HashSet::new();
            for option in &question.options {
                if option.id.is_empty() {
                    return Err(format!(
                        "question {} has an option with empty id",
                        question.id
                    ));
                }
                if !option_ids.insert(option.id.as_str()) {
                    return Err(format!(
                        "question {} has duplicate option id: {}",
                        question.id, option.id
                    ));
                }
            }
            for pointed in [&question.recommended, &question.default]
                .into_iter()
                .flatten()
            {
                if !option_ids.contains(pointed.as_str()) {
                    return Err(format!(
                        "question {} points at unknown option: {pointed}",
                        question.id
                    ));
                }
            }
            if question.blocking {
                blocking += 1;
            }
        }
        if blocking > 3 {
            return Err(format!(
                "{blocking} blocking questions; at most 3 are allowed — merge them or split the task"
            ));
        }
        Ok(())
    }
}

impl AnswersFile {
    /// Answering must hit a question that was actually asked, with an option
    /// that exists; anything else fails fast instead of silently rotting.
    ///
    /// Only answers for `questions.run_id` are checked — older rounds are
    /// history and were validated when they were written.
    pub fn validate_against(&self, questions: &QuestionsFile) -> Result<(), String> {
        if self.v != ANSWERS_VERSION {
            return Err(format!(
                "unsupported answers version: {} (expected {ANSWERS_VERSION})",
                self.v
            ));
        }
        if self.task_id != questions.task_id {
            return Err(format!(
                "answers target task {} but questions are for {}",
                self.task_id, questions.task_id
            ));
        }
        let mut seen = HashSet::new();
        for answer in self
            .answers
            .iter()
            .filter(|answer| answer.run_id == questions.run_id)
        {
            if !seen.insert(answer.question_id.as_str()) {
                return Err(format!("duplicate answer for {}", answer.question_id));
            }
            let question = questions
                .questions
                .iter()
                .find(|question| question.id == answer.question_id)
                .ok_or_else(|| {
                    format!("answer references unknown question: {}", answer.question_id)
                })?;
            if !question
                .options
                .iter()
                .any(|option| option.id == answer.choice)
            {
                return Err(format!(
                    "answer {}/{} is not one of the offered options",
                    answer.question_id, answer.choice
                ));
            }
            if !matches!(answer.by.as_str(), "human" | "judgment" | "policy" | "spec") {
                return Err(format!(
                    "answer {} has unknown `by`: {}",
                    answer.question_id, answer.by
                ));
            }
        }
        Ok(())
    }

    /// The choice recorded for a question in `run_id`, if any.
    pub fn choice_of(&self, question_id: &str, run_id: &str) -> Option<&str> {
        self.answers
            .iter()
            .find(|answer| answer.question_id == question_id && answer.run_id == run_id)
            .map(|answer| answer.choice.as_str())
    }

    /// The rationale recorded alongside [`Self::choice_of`].
    pub fn rationale_of(&self, question_id: &str, run_id: &str) -> Option<&str> {
        self.answers
            .iter()
            .find(|answer| answer.question_id == question_id && answer.run_id == run_id)
            .and_then(|answer| answer.rationale.as_deref())
    }

    /// Insert or replace the answer for `(question_id, run_id)`.
    pub fn record(&mut self, answer: Answer) {
        self.answers.retain(|existing| {
            !(existing.question_id == answer.question_id && existing.run_id == answer.run_id)
        });
        self.answers.push(answer);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn option(id: &str) -> QuestionOption {
        QuestionOption {
            id: id.to_string(),
            label: format!("label {id}"),
            consequence: None,
        }
    }

    fn question(id: &str, blocking: bool) -> Question {
        Question {
            id: id.to_string(),
            question: "Which way?".to_string(),
            why_now: None,
            blocking,
            scope: QuestionScope::Implementation,
            options: vec![option("A"), option("B")],
            recommended: Some("A".to_string()),
            default: Some("A".to_string()),
            reversible: true,
            evidence: Vec::new(),
            ttl_s: None,
        }
    }

    fn file(questions: Vec<Question>) -> QuestionsFile {
        QuestionsFile {
            v: QUESTIONS_VERSION,
            task_id: "T-1".to_string(),
            run_id: "R-1".to_string(),
            questions,
        }
    }

    #[test]
    fn accepts_a_well_formed_file() {
        assert!(file(vec![question("Q1", true)]).validate().is_ok());
    }

    #[test]
    fn rejects_empty_duplicate_and_unanswerable_shapes() {
        assert!(file(vec![]).validate().is_err());
        assert!(file(vec![question("Q1", false), question("Q1", false)])
            .validate()
            .is_err());

        let mut no_options = question("Q1", false);
        no_options.options.clear();
        assert!(file(vec![no_options]).validate().is_err());

        let mut bad_pointers = question("Q1", false);
        bad_pointers.default = Some("Z".to_string());
        assert!(file(vec![bad_pointers]).validate().is_err());

        let mut empty_text = question("Q1", false);
        empty_text.question = "  ".to_string();
        assert!(file(vec![empty_text]).validate().is_err());
    }

    #[test]
    fn caps_blocking_questions_at_three() {
        let three = file(vec![
            question("Q1", true),
            question("Q2", true),
            question("Q3", true),
        ]);
        assert!(three.validate().is_ok());

        let four = file(vec![
            question("Q1", true),
            question("Q2", true),
            question("Q3", true),
            question("Q4", true),
        ]);
        let error = four.validate().expect_err("four blocking must fail");
        assert!(error.contains("at most 3"), "{error}");

        // Non-blocking questions do not count toward the cap.
        let many = file(vec![
            question("Q1", false),
            question("Q2", false),
            question("Q3", false),
            question("Q4", false),
            question("Q5", false),
        ]);
        assert!(many.validate().is_ok());
    }

    #[test]
    fn answers_must_reference_asked_questions_and_offered_options() {
        let questions = file(vec![question("Q1", true)]);
        let answers = AnswersFile {
            v: ANSWERS_VERSION,
            task_id: "T-1".to_string(),
            answers: vec![Answer {
                question_id: "Q1".to_string(),
                run_id: "R-1".to_string(),
                choice: "A".to_string(),
                by: "human".to_string(),
                rationale: Some("keep rollback".to_string()),
                assumption_id: None,
            }],
        };
        assert!(answers.validate_against(&questions).is_ok());
        assert_eq!(answers.choice_of("Q1", "R-1"), Some("A"));
        assert_eq!(answers.choice_of("Q2", "R-1"), None);
        // Same question id from another run is a different question.
        assert_eq!(answers.choice_of("Q1", "R-2"), None);
        assert_eq!(answers.rationale_of("Q1", "R-1"), Some("keep rollback"));

        // An answer written for an older run is history: it is not re-checked
        // against the current questions and never pairs with them.
        let stale = AnswersFile {
            answers: vec![Answer {
                run_id: "R-0".to_string(),
                ..answers.answers[0].clone()
            }],
            ..answers.clone()
        };
        assert!(stale.validate_against(&questions).is_ok());
        assert_eq!(stale.choice_of("Q1", "R-1"), None);

        let unknown_question = AnswersFile {
            answers: vec![Answer {
                question_id: "Q9".to_string(),
                ..answers.answers[0].clone()
            }],
            ..answers.clone()
        };
        assert!(unknown_question
            .validate_against(&questions)
            .unwrap_err()
            .contains("unknown question"));

        let unknown_choice = AnswersFile {
            answers: vec![Answer {
                choice: "Z".to_string(),
                ..answers.answers[0].clone()
            }],
            ..answers.clone()
        };
        assert!(unknown_choice
            .validate_against(&questions)
            .unwrap_err()
            .contains("not one of the offered options"));

        let unknown_by = AnswersFile {
            answers: vec![Answer {
                by: "oracle".to_string(),
                ..answers.answers[0].clone()
            }],
            ..answers.clone()
        };
        assert!(unknown_by
            .validate_against(&questions)
            .unwrap_err()
            .contains("unknown `by`"));

        let wrong_task = AnswersFile {
            task_id: "T-2".to_string(),
            ..answers.clone()
        };
        assert!(wrong_task.validate_against(&questions).is_err());
    }

    #[test]
    fn record_replaces_same_question_in_same_run_only() {
        let mut answers = AnswersFile {
            v: ANSWERS_VERSION,
            task_id: "T-1".to_string(),
            answers: Vec::new(),
        };
        let make = |run_id: &str, choice: &str| Answer {
            question_id: "Q1".to_string(),
            run_id: run_id.to_string(),
            choice: choice.to_string(),
            by: "human".to_string(),
            rationale: None,
            assumption_id: None,
        };
        answers.record(make("R-1", "A"));
        answers.record(make("R-2", "B"));
        assert_eq!(answers.answers.len(), 2);

        // Re-answering in the same run replaces, never duplicates.
        answers.record(make("R-1", "B"));
        assert_eq!(answers.answers.len(), 2);
        assert_eq!(answers.choice_of("Q1", "R-1"), Some("B"));
        assert_eq!(answers.choice_of("Q1", "R-2"), Some("B"));
    }

    #[test]
    fn questions_roundtrip_through_json() {
        let original = file(vec![question("Q1", true)]);
        let text = serde_json::to_string_pretty(&original).expect("json");
        assert!(text.contains("\"blocking\": true"));
        let parsed: QuestionsFile = serde_json::from_str(&text).expect("parse");
        assert_eq!(parsed, original);
        assert_eq!(parsed.questions[0].scope.as_str(), "implementation");

        // Unknown fields are rejected: the contract is strict on the wire.
        let mut strict = serde_json::to_value(&original).expect("value");
        strict["extra"] = serde_json::json!("nope");
        assert!(serde_json::from_value::<QuestionsFile>(strict).is_err());
    }
}
