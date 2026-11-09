use std::str::FromStr;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub const EVENT_VERSION: u8 = 1;

/// Append-only event types from `doc/03` §11.2.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EventType {
    #[serde(rename = "run.started")]
    RunStarted,
    #[serde(rename = "goal.loaded")]
    GoalLoaded,
    #[serde(rename = "task.created")]
    TaskCreated,
    #[serde(rename = "task.ready")]
    TaskReady,
    #[serde(rename = "task.dispatched")]
    TaskDispatched,
    #[serde(rename = "task.started")]
    TaskStarted,
    #[serde(rename = "task.receipt")]
    TaskReceipt,
    #[serde(rename = "task.verify")]
    TaskVerify,
    #[serde(rename = "task.passed")]
    TaskPassed,
    #[serde(rename = "task.failed")]
    TaskFailed,
    #[serde(rename = "task.escalated")]
    TaskEscalated,
    #[serde(rename = "task.fallback")]
    TaskFallback,
    #[serde(rename = "task.merged")]
    TaskMerged,
    #[serde(rename = "task.blocked")]
    TaskBlocked,
    #[serde(rename = "task.needs_input")]
    TaskNeedsInput,
    #[serde(rename = "budget.warning")]
    BudgetWarning,
    #[serde(rename = "budget.exceeded")]
    BudgetExceeded,
    #[serde(rename = "human.approved")]
    HumanApproved,
    #[serde(rename = "human.aborted")]
    HumanAborted,
    #[serde(rename = "run.finished")]
    RunFinished,
}

impl EventType {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RunStarted => "run.started",
            Self::GoalLoaded => "goal.loaded",
            Self::TaskCreated => "task.created",
            Self::TaskReady => "task.ready",
            Self::TaskDispatched => "task.dispatched",
            Self::TaskStarted => "task.started",
            Self::TaskReceipt => "task.receipt",
            Self::TaskVerify => "task.verify",
            Self::TaskPassed => "task.passed",
            Self::TaskFailed => "task.failed",
            Self::TaskEscalated => "task.escalated",
            Self::TaskFallback => "task.fallback",
            Self::TaskMerged => "task.merged",
            Self::TaskBlocked => "task.blocked",
            Self::TaskNeedsInput => "task.needs_input",
            Self::BudgetWarning => "budget.warning",
            Self::BudgetExceeded => "budget.exceeded",
            Self::HumanApproved => "human.approved",
            Self::HumanAborted => "human.aborted",
            Self::RunFinished => "run.finished",
        }
    }
}

impl FromStr for EventType {
    type Err = String;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        match input.trim() {
            "run.started" => Ok(Self::RunStarted),
            "goal.loaded" => Ok(Self::GoalLoaded),
            "task.created" => Ok(Self::TaskCreated),
            "task.ready" => Ok(Self::TaskReady),
            "task.dispatched" => Ok(Self::TaskDispatched),
            "task.started" => Ok(Self::TaskStarted),
            "task.receipt" => Ok(Self::TaskReceipt),
            "task.verify" => Ok(Self::TaskVerify),
            "task.passed" => Ok(Self::TaskPassed),
            "task.failed" => Ok(Self::TaskFailed),
            "task.escalated" => Ok(Self::TaskEscalated),
            "task.fallback" => Ok(Self::TaskFallback),
            "task.merged" => Ok(Self::TaskMerged),
            "task.blocked" => Ok(Self::TaskBlocked),
            "task.needs_input" => Ok(Self::TaskNeedsInput),
            "budget.warning" => Ok(Self::BudgetWarning),
            "budget.exceeded" => Ok(Self::BudgetExceeded),
            "human.approved" => Ok(Self::HumanApproved),
            "human.aborted" => Ok(Self::HumanAborted),
            "run.finished" => Ok(Self::RunFinished),
            other => Err(format!("unknown event type: {other}")),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Event {
    #[serde(default = "default_version")]
    pub v: u8,
    pub ts_ms: u64,
    pub run_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    #[serde(rename = "type")]
    pub event_type: EventType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attempt: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_run_id: Option<String>,
    pub data: Map<String, Value>,
}

fn default_version() -> u8 {
    EVENT_VERSION
}

impl Default for Event {
    fn default() -> Self {
        Self {
            v: EVENT_VERSION,
            ts_ms: 0,
            run_id: String::new(),
            task_id: None,
            event_type: EventType::RunStarted,
            attempt: None,
            parent_run_id: None,
            data: Map::new(),
        }
    }
}

impl Event {
    pub fn new(run_id: impl Into<String>, ts_ms: u64, event_type: EventType) -> Self {
        Self {
            v: EVENT_VERSION,
            ts_ms,
            run_id: run_id.into(),
            task_id: None,
            event_type,
            attempt: None,
            parent_run_id: None,
            data: Map::new(),
        }
    }

    pub fn task(mut self, task_id: impl Into<String>) -> Self {
        self.task_id = Some(task_id.into());
        self
    }

    pub fn attempt(mut self, attempt: u32) -> Self {
        self.attempt = Some(attempt);
        self
    }

    /// Link this event to the run it came from, e.g. a `task.fallback` that
    /// switches away from the run recorded here.
    pub fn parent_run_id(mut self, run_id: impl Into<String>) -> Self {
        self.parent_run_id = Some(run_id.into());
        self
    }

    pub fn data(mut self, data: impl IntoIterator<Item = (String, Value)>) -> Self {
        self.data = data.into_iter().collect();
        self
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.v != EVENT_VERSION {
            return Err(format!(
                "unsupported event version: {} (expected {EVENT_VERSION})",
                self.v
            ));
        }
        if self.run_id.trim().is_empty() {
            return Err("event run_id must not be empty".to_string());
        }
        if self
            .task_id
            .as_deref()
            .is_some_and(|task| task.trim().is_empty())
        {
            return Err("event task_id must not be empty when present".to_string());
        }
        if self.attempt == Some(0) {
            return Err("event attempt must be >= 1".to_string());
        }
        let needs_task = matches!(
            self.event_type,
            EventType::TaskCreated
                | EventType::TaskReady
                | EventType::TaskDispatched
                | EventType::TaskStarted
                | EventType::TaskReceipt
                | EventType::TaskVerify
                | EventType::TaskPassed
                | EventType::TaskFailed
                | EventType::TaskEscalated
                | EventType::TaskFallback
                | EventType::TaskMerged
                | EventType::TaskBlocked
                | EventType::TaskNeedsInput
        );
        if needs_task && self.task_id.is_none() {
            return Err(format!(
                "event {} requires task_id",
                self.event_type.as_str()
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Default)]
pub struct EventFilter {
    pub run_id: Option<String>,
    pub task_id: Option<String>,
    pub event_type: Option<EventType>,
}

impl EventFilter {
    pub fn matches(&self, event: &Event) -> bool {
        if let Some(run_id) = &self.run_id {
            if &event.run_id != run_id {
                return false;
            }
        }
        if let Some(task_id) = &self.task_id {
            if event.task_id.as_deref() != Some(task_id.as_str()) {
                return false;
            }
        }
        if let Some(event_type) = self.event_type {
            if event.event_type != event_type {
                return false;
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_all_documented_types() {
        for name in [
            "run.started",
            "goal.loaded",
            "task.created",
            "task.ready",
            "task.dispatched",
            "task.started",
            "task.receipt",
            "task.verify",
            "task.passed",
            "task.failed",
            "task.escalated",
            "task.fallback",
            "task.merged",
            "task.blocked",
            "budget.warning",
            "budget.exceeded",
            "human.approved",
            "human.aborted",
            "run.finished",
        ] {
            let parsed = name.parse::<EventType>().expect(name);
            assert_eq!(parsed.as_str(), name);
        }
    }

    #[test]
    fn validates_task_events() {
        let event = Event::new("R-1", 1, EventType::TaskStarted);
        assert!(event.validate().is_err());

        let event = event.task("T-20261104-001").attempt(1);
        assert!(event.validate().is_ok());

        let mut bad = event.clone();
        bad.v = 2;
        assert!(bad.validate().is_err());
    }

    #[test]
    fn fallback_events_carry_task_and_parent_run() {
        let event = Event::new("R-2", 2, EventType::TaskFallback)
            .task("T-1")
            .attempt(1)
            .parent_run_id("R-1")
            .data([("to".to_string(), serde_json::json!("worker-b"))]);
        assert!(event.validate().is_ok());
        assert_eq!(event.parent_run_id.as_deref(), Some("R-1"));

        let no_task = Event::new("R-2", 2, EventType::TaskFallback);
        assert!(no_task.validate().is_err());
    }

    #[test]
    fn filters_events() {
        let event = Event::new("R-1", 1, EventType::TaskPassed).task("T-1");
        let filter = EventFilter {
            run_id: Some("R-1".into()),
            task_id: Some("T-1".into()),
            event_type: Some(EventType::TaskPassed),
        };
        assert!(filter.matches(&event));
        assert!(!EventFilter {
            task_id: Some("T-2".into()),
            ..filter
        }
        .matches(&event));
    }
}
