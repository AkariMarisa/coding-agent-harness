//! Process-level executor adapters for Loom.
//!
//! The stable contract is: a packet directory in, an outbox directory out, and
//! an exit code that tells the control plane what happened. No agent plugin API
//! is required.

mod canary;
mod mock;
mod normalize;
mod preset;
mod process;
mod types;

pub use canary::{canary, CanaryCheck, CanaryReport};
pub use normalize::{git_diffstat, git_head, read_usage_rows};
pub use preset::{list, preset, which, AdapterPreset};
pub use types::{AdapterKind, AdapterRequest, AdapterRun, AdapterUsageRow, ADAPTER_USAGE_VERSION};

use std::fs;
use std::time::Instant;

use anyhow::{Context as _, Result};
use loom_core::Receipt;

use mock::MockMode;

/// Run the selected adapter and normalize its output into a claim receipt.
pub fn run(request: &AdapterRequest) -> Result<AdapterRun> {
    run_with_mode(request, MockMode::from_env())
}

pub(crate) fn run_with_mode(request: &AdapterRequest, mode: MockMode) -> Result<AdapterRun> {
    let started = Instant::now();
    let outcome = match request.adapter {
        AdapterKind::Mock => {
            let mock = mock::produce(request, mode)?;
            process::ProcessOutcome {
                exit_code: mock.exit_code,
                timed_out: mock.timed_out,
                duration_ms: started.elapsed().as_millis() as u64,
                stdout_path: request.logs_dir.join("adapter.stdout.log"),
                stderr_path: request.logs_dir.join("adapter.stderr.log"),
            }
        }
        _ => process::execute(request)?,
    };

    let usage_path = request.output_dir.join("usage.jsonl");
    let usage = normalize::read_usage_rows(&usage_path)?;
    let receipt: Receipt =
        normalize::normalize(request, outcome.exit_code, outcome.timed_out, usage.clone())?;

    fs::create_dir_all(&request.output_dir).context("create outbox")?;
    let text = serde_json::to_string_pretty(&receipt)?;
    fs::write(request.output_dir.join("receipt.json"), format!("{text}\n"))
        .with_context(|| "write receipt.json")?;

    Ok(AdapterRun {
        adapter: request.adapter,
        exit_code: outcome.exit_code,
        timed_out: outcome.timed_out,
        duration_ms: outcome.duration_ms,
        stdout_path: outcome.stdout_path,
        stderr_path: outcome.stderr_path,
        receipt,
        usage,
    })
}
