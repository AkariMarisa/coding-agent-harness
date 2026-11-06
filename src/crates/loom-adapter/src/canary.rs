use std::fs;
use std::path::Path;
use std::process::Command;

use anyhow::{Context as _, Result};
use serde_json::json;

use crate::mock::MockMode;
use crate::preset::{preset, AdapterPreset};
use crate::types::{AdapterKind, AdapterRequest};
use crate::{run, run_with_mode};

#[derive(Debug, Clone)]
pub struct CanaryCheck {
    pub name: String,
    pub ok: bool,
    pub detail: String,
}

#[derive(Debug, Clone)]
pub struct CanaryReport {
    pub adapter: AdapterKind,
    pub ok: bool,
    pub checks: Vec<CanaryCheck>,
}

impl CanaryReport {
    pub fn to_json(&self) -> serde_json::Value {
        json!({
            "adapter": self.adapter.as_str(),
            "ok": self.ok,
            "checks": self
                .checks
                .iter()
                .map(|check| json!({"name": check.name, "ok": check.ok, "detail": check.detail}))
                .collect::<Vec<_>>(),
        })
    }
}

fn check(name: &str, ok: bool, detail: impl Into<String>) -> CanaryCheck {
    CanaryCheck {
        name: name.to_string(),
        ok,
        detail: detail.into(),
    }
}

/// Run adapter contract checks. The mock adapter always self-tests; external
/// adapters check availability and version unless `live` is requested.
pub fn canary(
    kind: AdapterKind,
    live: bool,
    request: Option<&AdapterRequest>,
) -> Result<CanaryReport> {
    let preset = preset(kind);
    let available = preset.is_available();
    let mut checks = vec![check(
        "available",
        available,
        if available {
            format!("{} found", preset.resolve_executable())
        } else {
            format!("{} not found", preset.check_executable)
        },
    )];

    match kind {
        AdapterKind::Mock => mock_checks(&mut checks, request)?,
        _ => external_checks(&mut checks, &preset, live, request)?,
    }

    let ok = checks.iter().all(|check| check.ok);
    Ok(CanaryReport {
        adapter: kind,
        ok,
        checks,
    })
}

fn mock_checks(checks: &mut Vec<CanaryCheck>, request: Option<&AdapterRequest>) -> Result<()> {
    match request {
        Some(request) => {
            mock_checks_on(request, checks);
            Ok(())
        }
        None => {
            let dir = tempfile::tempdir().context("tempdir")?;
            let request = build_mock_request(dir.path());
            mock_checks_on(&request, checks);
            Ok(())
        }
    }
}

fn mock_checks_on(request: &AdapterRequest, checks: &mut Vec<CanaryCheck>) {
    match run_with_mode(request, MockMode::Ok) {
        Ok(run) => {
            let ok = run.exit_code == 0
                && run.receipt.status == loom_core::ReceiptStatus::Done
                && run.usage.len() == 1
                && request.output_dir.join("receipt.json").exists();
            checks.push(check(
                "mock ok run",
                ok,
                format!(
                    "exit={} status={} usage_rows={}",
                    run.exit_code,
                    run.receipt.status.as_str(),
                    run.usage.len()
                ),
            ));
        }
        Err(error) => checks.push(check("mock ok run", false, format!("{error:#}"))),
    }

    for (mode, expected_exit, expected_status) in [
        (
            MockMode::NeedsInput,
            2,
            loom_core::ReceiptStatus::NeedsInput,
        ),
        (
            MockMode::Budget,
            3,
            loom_core::ReceiptStatus::BudgetExceeded,
        ),
        (MockMode::AdapterError, 4, loom_core::ReceiptStatus::Failed),
    ] {
        let name = format!("mock exit {expected_exit}");
        match run_with_mode(request, mode) {
            Ok(run) => checks.push(check(
                &name,
                run.exit_code == expected_exit && run.receipt.status == expected_status,
                format!(
                    "exit={} status={}",
                    run.exit_code,
                    run.receipt.status.as_str()
                ),
            )),
            Err(error) => checks.push(check(&name, false, format!("{error:#}"))),
        }
    }

    match run_with_mode(request, MockMode::Empty) {
        Ok(run) => checks.push(check(
            "mock empty output",
            run.exit_code == 0 && run.receipt.status == loom_core::ReceiptStatus::Failed,
            format!("status={}", run.receipt.status.as_str()),
        )),
        Err(error) => checks.push(check("mock empty output", false, format!("{error:#}"))),
    }

    let mut timeout_request = request.clone();
    timeout_request.timeout_s = 1;
    match run_with_mode(&timeout_request, MockMode::Slow) {
        Ok(run) => checks.push(check(
            "mock timeout",
            run.timed_out && run.exit_code == 3,
            format!("timed_out={} exit={}", run.timed_out, run.exit_code),
        )),
        Err(error) => checks.push(check("mock timeout", false, format!("{error:#}"))),
    }
}

fn external_checks(
    checks: &mut Vec<CanaryCheck>,
    preset: &AdapterPreset,
    live: bool,
    request: Option<&AdapterRequest>,
) -> Result<()> {
    let (executable, version_args) = preset.version_command();
    let version = Command::new(&executable)
        .args(&version_args)
        .output()
        .with_context(|| format!("run {executable} --version"));
    match version {
        Ok(output) => checks.push(check(
            "version",
            output.status.success(),
            String::from_utf8_lossy(&output.stdout).trim().to_string(),
        )),
        Err(error) => checks.push(check("version", false, format!("{error:#}"))),
    }

    if let Some(request) = request {
        let args = preset.expand(request);
        let uses_packet = preset.stdin_packet || args.iter().any(|arg| arg.contains("packet"));
        checks.push(check(
            "packet argument",
            uses_packet,
            if uses_packet {
                format!("args={args:?}")
            } else {
                "packet is not referenced".to_string()
            },
        ));
    }

    if live {
        match request {
            Some(request) => match run(request) {
                Ok(run) => checks.push(check(
                    "live run",
                    (0..=4).contains(&run.exit_code)
                        && request.output_dir.join("receipt.json").exists(),
                    format!(
                        "exit={} status={}",
                        run.exit_code,
                        run.receipt.status.as_str()
                    ),
                )),
                Err(error) => checks.push(check("live run", false, format!("{error:#}"))),
            },
            None => checks.push(check(
                "live run",
                false,
                "live canary requires a prepared request",
            )),
        }
    }

    Ok(())
}

fn build_mock_request(dir: &Path) -> AdapterRequest {
    let packet_dir = dir.join("context");
    let worktree = dir.join("worktree");
    let output_dir = dir.join("outbox");
    let logs_dir = dir.join("logs");
    fs::create_dir_all(&packet_dir).expect("mkdir packet");
    fs::create_dir_all(&worktree).expect("mkdir worktree");
    fs::create_dir_all(&output_dir).expect("mkdir outbox");
    fs::create_dir_all(&logs_dir).expect("mkdir logs");

    let spec = loom_core::TaskSpec {
        schema_version: loom_core::TASK_VERSION.to_string(),
        task_id: "T-20261104-001".to_string(),
        goal_id: "G-20261104-auth".to_string(),
        title: "canary".to_string(),
        acceptance: vec![loom_core::Acceptance {
            id: "A1".to_string(),
            cmd: "true".to_string(),
            expect_exit: 0,
            timeout_s: 30,
        }],
        ..loom_core::TaskSpec::default()
    };
    let text = serde_json::to_string_pretty(&spec).expect("serialize spec");
    fs::write(packet_dir.join("task.json"), format!("{text}\n")).expect("write spec");

    AdapterRequest {
        adapter: AdapterKind::Mock,
        run_id: "R-canary".to_string(),
        task_id: spec.task_id.clone(),
        attempt: 1,
        packet_dir,
        worktree,
        output_dir,
        logs_dir,
        model: "worker".to_string(),
        model_id: "worker".to_string(),
        branch: "agent/canary".to_string(),
        timeout_s: 10,
        max_tokens: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_canary_passes() {
        let report = canary(AdapterKind::Mock, false, None).expect("canary");
        assert!(report.ok, "{:?}", report.checks);
        assert!(report.checks.len() >= 5);
        assert!(report.to_json().get("ok").is_some());
    }

    #[test]
    fn external_canary_reports_missing_binary() {
        // PATH should not contain a fake `goose` in CI; if it does, this still
        // must not panic.
        let report = canary(AdapterKind::Goose, false, None).expect("canary");
        assert!(!report.checks.is_empty());
    }
}
