use std::fs;
use std::thread;
use std::time::Duration;

use anyhow::{Context as _, Result};
use loom_core::TaskSpec;

use crate::normalize::git_head;
use crate::types::{AdapterRequest, AdapterUsageRow, ADAPTER_USAGE_VERSION};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MockMode {
    Ok,
    NeedsInput,
    Blocked,
    Budget,
    AdapterError,
    Empty,
    ParseError,
    Slow,
}

#[derive(Debug, Clone, Copy)]
pub struct MockOutcome {
    pub exit_code: i32,
    pub timed_out: bool,
}

impl MockMode {
    pub fn from_env() -> Self {
        match std::env::var("LOOM_MOCK_MODE")
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str()
        {
            "needs_input" => Self::NeedsInput,
            "blocked" => Self::Blocked,
            "budget" | "budget_exceeded" => Self::Budget,
            "adapter_error" | "crash" => Self::AdapterError,
            "empty" => Self::Empty,
            "parse_error" | "malformed" => Self::ParseError,
            "slow" | "timeout" => Self::Slow,
            _ => Self::Ok,
        }
    }
}

pub fn produce(request: &AdapterRequest, mode: MockMode) -> Result<MockOutcome> {
    fs::create_dir_all(&request.output_dir).context("create outbox")?;
    fs::create_dir_all(&request.logs_dir).context("create logs")?;

    let spec = read_task_spec(request);
    let acceptance_ids = spec
        .as_ref()
        .map(|spec| {
            spec.acceptance
                .iter()
                .map(|item| item.id.clone())
                .collect::<Vec<_>>()
        })
        .filter(|ids| !ids.is_empty())
        .unwrap_or_else(|| vec!["A1".to_string()]);

    let log_path = format!(".loom/runs/{}/logs/verify.log", request.run_id);
    let report_path = format!(".loom/runs/{}/outbox/report.md", request.run_id);
    let head = git_head(&request.worktree).unwrap_or_else(|| "unknown".to_string());

    match mode {
        MockMode::Slow => {
            // Simulate a slow adapter that exceeds the orchestrator timeout.
            let started = std::time::Instant::now();
            while started.elapsed().as_secs() < request.timeout_s {
                thread::sleep(Duration::from_millis(50));
            }
            Ok(MockOutcome {
                exit_code: 3,
                timed_out: true,
            })
        }
        MockMode::AdapterError => {
            fs::write(request.output_dir.join("final.md"), "").ok();
            write_usage(request)?;
            Ok(MockOutcome {
                exit_code: 4,
                timed_out: false,
            })
        }
        MockMode::Empty => {
            fs::write(request.output_dir.join("final.md"), "").ok();
            write_usage(request)?;
            Ok(MockOutcome {
                exit_code: 0,
                timed_out: false,
            })
        }
        MockMode::ParseError => {
            write_final(request, "status: done\n\nbranch: b@sha\n")?;
            write_usage(request)?;
            Ok(MockOutcome {
                exit_code: 0,
                timed_out: false,
            })
        }
        MockMode::NeedsInput => {
            write_final(
                request,
                &receipt_lines(
                    "needs_input",
                    &request.branch,
                    &head,
                    &acceptance_ids,
                    "skip",
                    &log_path,
                    &report_path,
                ),
            )?;
            write_usage(request)?;
            Ok(MockOutcome {
                exit_code: 2,
                timed_out: false,
            })
        }
        MockMode::Blocked => {
            write_final(
                request,
                &receipt_lines(
                    "blocked",
                    &request.branch,
                    &head,
                    &acceptance_ids,
                    "skip",
                    &log_path,
                    &report_path,
                ),
            )?;
            write_usage(request)?;
            Ok(MockOutcome {
                exit_code: 2,
                timed_out: false,
            })
        }
        MockMode::Budget => {
            write_final(
                request,
                &receipt_lines(
                    "budget_exceeded",
                    &request.branch,
                    &head,
                    &acceptance_ids,
                    "skip",
                    &log_path,
                    &report_path,
                ),
            )?;
            write_usage(request)?;
            Ok(MockOutcome {
                exit_code: 3,
                timed_out: false,
            })
        }
        MockMode::Ok => {
            write_final(
                request,
                &receipt_lines(
                    "done",
                    &request.branch,
                    &head,
                    &acceptance_ids,
                    "pass",
                    &log_path,
                    &report_path,
                ),
            )?;
            write_usage(request)?;
            Ok(MockOutcome {
                exit_code: 0,
                timed_out: false,
            })
        }
    }
}

fn read_task_spec(request: &AdapterRequest) -> Option<TaskSpec> {
    let path = request.packet_dir.join("task.json");
    let text = fs::read_to_string(path).ok()?;
    serde_json::from_str::<TaskSpec>(&text).ok()
}

fn receipt_lines(
    status: &str,
    branch: &str,
    head: &str,
    acceptance_ids: &[String],
    result: &str,
    log_path: &str,
    report_path: &str,
) -> String {
    let results = acceptance_ids
        .iter()
        .map(|id| format!("{id}={result}"))
        .collect::<Vec<_>>()
        .join(",");
    format!("status: {status}\nbranch: {branch}@{head}\nverify: {results}; log={log_path}\nreport: {report_path}\n")
}

fn write_final(request: &AdapterRequest, text: &str) -> Result<()> {
    fs::write(request.output_dir.join("final.md"), text).with_context(|| "write final.md")?;
    fs::write(
        request.output_dir.join("report.md"),
        "# report\n\nmock adapter report\n",
    )
    .with_context(|| "write report.md")?;
    Ok(())
}

fn write_usage(request: &AdapterRequest) -> Result<()> {
    let row = AdapterUsageRow {
        v: ADAPTER_USAGE_VERSION,
        run_id: request.run_id.clone(),
        task_id: request.task_id.clone(),
        model: Some(request.model.clone()),
        input_tokens: Some(12_000),
        cached_input_tokens: Some(8_000),
        output_tokens: Some(800),
        reasoning_tokens: Some(100),
        cost_usd: Some(0.002),
        latency_ms: Some(1_000),
        status: Some(200),
        source: Some("runtime".to_string()),
    };
    row.validate().map_err(|message| anyhow::anyhow!(message))?;
    let line = serde_json::to_string(&row)?;
    fs::write(request.output_dir.join("usage.jsonl"), format!("{line}\n"))
        .with_context(|| "write usage.jsonl")?;
    Ok(())
}
