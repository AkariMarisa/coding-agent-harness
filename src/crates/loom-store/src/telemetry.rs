use std::fs::{self, OpenOptions};
use std::io::Write;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::Run;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolEvent {
    pub ts_ms: u64,
    pub run: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub invocation: Option<String>,
    pub tool: String,
    pub args_hash: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    pub items: usize,
    pub bytes: usize,
    pub tok_est: usize,
    pub truncated: bool,
    pub spill: bool,
    pub dedupe: bool,
    pub stale: usize,
    pub ms: u64,
    pub exit: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub budget_at: Option<String>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Totals {
    pub run_bytes: usize,
    pub invocation_bytes: usize,
}

pub fn append_event(run: &Run, event: &ToolEvent) -> Result<()> {
    fs::create_dir_all(run.logs_dir()).with_context(|| "create logs dir")?;
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(run.telemetry_path())
        .with_context(|| "open tools.jsonl")?;
    let line = serde_json::to_string(event)?;
    writeln!(file, "{line}").with_context(|| "append telemetry")?;
    Ok(())
}

pub fn totals(run: &Run, invocation: Option<&str>) -> Result<Totals> {
    let path = run.telemetry_path();
    if !path.exists() {
        return Ok(Totals::default());
    }

    let text = fs::read_to_string(&path).with_context(|| "read tools.jsonl")?;
    let mut totals = Totals::default();

    for line in text.lines() {
        let Ok(event) = serde_json::from_str::<ToolEvent>(line) else {
            continue;
        };
        totals.run_bytes = totals.run_bytes.saturating_add(event.bytes);
        if invocation.is_some() && event.invocation.as_deref() == invocation {
            totals.invocation_bytes = totals.invocation_bytes.saturating_add(event.bytes);
        }
    }

    Ok(totals)
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}
