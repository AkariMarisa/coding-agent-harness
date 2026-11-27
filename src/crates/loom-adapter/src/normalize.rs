use std::fs;
use std::path::Path;
use std::process::Command;

use anyhow::{Context as _, Result};
use loom_core::{parse_four_line, Diffstat, Receipt, ReceiptStatus, TokenTotals, VerifyEvidence};

use crate::types::{AdapterRequest, AdapterUsageRow};

pub fn git_head(path: &Path) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let head = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!head.is_empty()).then_some(head)
}

pub fn git_diffstat(path: &Path) -> Option<Diffstat> {
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["diff", "--numstat"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut stat = Diffstat::default();
    let mut seen = false;
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        let (Some(insertions), Some(deletions), Some(files)) =
            (parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        let Ok(insertions) = insertions.parse::<u64>() else {
            continue;
        };
        let Ok(deletions) = deletions.parse::<u64>() else {
            continue;
        };
        stat.files += 1;
        stat.insertions += insertions;
        stat.deletions += deletions;
        let _ = files;
        seen = true;
    }
    seen.then_some(stat)
}

pub fn read_usage_rows(path: &Path) -> Result<Vec<AdapterUsageRow>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let text = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let mut rows = Vec::new();
    for (index, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let row = serde_json::from_str::<AdapterUsageRow>(line).with_context(|| {
            format!("parse {} line {}", path.display(), index.saturating_add(1))
        })?;
        row.validate().map_err(|message| {
            anyhow::anyhow!(
                "validate {} line {}: {message}",
                path.display(),
                index.saturating_add(1)
            )
        })?;
        rows.push(row);
    }
    Ok(rows)
}

fn aggregate_tokens(rows: &[AdapterUsageRow]) -> TokenTotals {
    let mut totals = TokenTotals::default();
    for row in rows {
        totals.input += row.input_tokens.unwrap_or(0);
        totals.cached_input += row.cached_input_tokens.unwrap_or(0);
        totals.output += row.output_tokens.unwrap_or(0);
        if let Some(reasoning) = row.reasoning_tokens {
            totals.reasoning = Some(totals.reasoning.unwrap_or(0) + reasoning);
        }
        if let Some(cost) = row.cost_usd {
            totals.cost_usd = Some(totals.cost_usd.unwrap_or(0.0) + cost);
        }
    }
    if totals.cached_input > totals.input {
        totals.cached_input = totals.input;
    }
    totals
}

fn status_for(exit_code: i32, timed_out: bool, claim: Option<ReceiptStatus>) -> ReceiptStatus {
    if timed_out || exit_code == 3 {
        return ReceiptStatus::BudgetExceeded;
    }
    // Exit 5: the churn watchdog stopped a gross loop. Blocked, not failed —
    // a human inspects the loop before redispatch (`doc/15` §1.6).
    if exit_code == 5 {
        return ReceiptStatus::Blocked;
    }
    match exit_code {
        4 => ReceiptStatus::Failed,
        2 => match claim {
            Some(ReceiptStatus::NeedsInput) | Some(ReceiptStatus::Blocked) => claim.unwrap(),
            _ => ReceiptStatus::Blocked,
        },
        _ => claim.unwrap_or(ReceiptStatus::Failed),
    }
}

/// Normalize adapter output into a claim receipt. The runner never trusts this
/// as acceptance; `loom verify` re-runs the commands.
pub fn normalize(
    request: &AdapterRequest,
    exit_code: i32,
    timed_out: bool,
    churn: Option<&crate::watch::ChurnStop>,
    usage: Vec<AdapterUsageRow>,
) -> Result<Receipt> {
    let final_path = request.output_dir.join("final.md");
    let final_text = fs::read_to_string(&final_path).ok().unwrap_or_default();

    let parsed = parse_four_line(&final_text).ok();
    let claim_status = parsed.as_ref().map(|receipt| receipt.status);
    let status = status_for(exit_code, timed_out, claim_status);

    let log_path = format!(".loom/runs/{}/logs/verify.log", request.run_id);
    let report_path = parsed
        .as_ref()
        .map(|receipt| receipt.report_path.clone())
        .filter(|path| !path.trim().is_empty())
        .unwrap_or_else(|| format!(".loom/runs/{}/outbox/report.md", request.run_id));

    let head_sha = parsed
        .as_ref()
        .map(|receipt| receipt.head_sha.clone())
        .filter(|sha| !sha.trim().is_empty())
        .or_else(|| git_head(&request.worktree))
        .unwrap_or_else(|| "unknown".to_string());
    let branch = parsed
        .as_ref()
        .map(|receipt| receipt.branch.clone())
        .filter(|branch| !branch.trim().is_empty())
        .unwrap_or_else(|| request.branch.clone());

    let verify: Vec<VerifyEvidence> = parsed
        .as_ref()
        .map(|receipt| {
            receipt
                .verify
                .iter()
                .map(|claim| VerifyEvidence {
                    id: claim.id.clone(),
                    cmd: String::new(),
                    exit: -1,
                    signal: None,
                    duration_s: 0.0,
                    timeout_s: None,
                    log_path: log_path.clone(),
                    log_sha256: None,
                    git_sha: Some(head_sha.clone()),
                    env_hash: None,
                    result: claim.result,
                })
                .collect()
        })
        .unwrap_or_default();

    let mut receipt = Receipt::new(&request.task_id, request.attempt);
    receipt.status = status;
    receipt.base_sha = git_head(&request.worktree).unwrap_or_else(|| "unknown".to_string());
    receipt.head_sha = head_sha;
    receipt.branch = branch;
    receipt.diffstat = git_diffstat(&request.worktree);
    receipt.verify = verify;
    receipt.artifacts = Vec::new();
    receipt.tokens = aggregate_tokens(&usage);
    receipt.decisions = Vec::new();
    receipt.blockers = Vec::new();
    if let Some(stop) = churn {
        // The loop evidence travels on the receipt so verify and the inbox
        // can both see why the run stopped.
        receipt.blockers.push(format!(
            "churn watchdog: {} changed in {} consecutive samples ({} bytes)",
            stop.file, stop.changes, stop.bytes_touched
        ));
    }
    receipt.report_path = report_path;

    if receipt.status == ReceiptStatus::Failed && parsed.is_none() {
        receipt.next_hint = "adapter produced no parseable four-line receipt".to_string();
    } else if timed_out {
        receipt.next_hint = "adapter timed out".to_string();
    } else if exit_code == 2 {
        receipt.next_hint = "adapter exit 2: needs human input".to_string();
    } else if exit_code == 3 {
        receipt.next_hint = "adapter exit 3: budget or timeout".to_string();
    } else if exit_code == 5 {
        receipt.next_hint =
            "churn watchdog stopped the adapter; inspect the loop before redispatching".to_string();
    } else if exit_code == 4 {
        receipt.next_hint = "adapter exit 4: adapter failure".to_string();
    } else {
        receipt.next_hint = String::new();
    }

    receipt
        .validate()
        .map_err(|message| anyhow::anyhow!("normalized receipt is invalid: {message}"))?;
    Ok(receipt)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{AdapterKind, AdapterRequest};

    fn request(dir: &Path) -> AdapterRequest {
        AdapterRequest {
            adapter: AdapterKind::Mock,
            run_id: "R-1".into(),
            task_id: "T-20261104-001".into(),
            packet_dir: dir.join("context"),
            worktree: dir.to_path_buf(),
            output_dir: dir.join("outbox"),
            logs_dir: dir.join("logs"),
            model: "worker".into(),
            model_id: "worker".into(),
            model_capability: None,
            branch: "agent/T-20261104-001".into(),
            timeout_s: 10,
            max_tokens: None,
            attempt: 1,
            skills: Vec::new(),
            churn: Default::default(),
        }
    }

    #[test]
    fn churn_stop_maps_to_blocked_with_evidence() {
        let dir = tempfile::tempdir().expect("tempdir");
        let req = request(dir.path());
        let stop = crate::watch::ChurnStop {
            file: "src/lib.rs".to_string(),
            changes: 5,
            bytes_touched: 12_345,
        };
        let receipt = normalize(&req, 5, false, Some(&stop), vec![]).expect("normalize");
        assert_eq!(receipt.status, ReceiptStatus::Blocked);
        assert!(receipt
            .blockers
            .iter()
            .any(|blocker| blocker.contains("churn watchdog") && blocker.contains("src/lib.rs")));
        assert!(receipt.next_hint.contains("churn watchdog"));

        // Exit 5 with no detail still blocks (defensive), and plain codes
        // are untouched.
        let bare = normalize(&req, 5, false, None, vec![]).expect("normalize");
        assert_eq!(bare.status, ReceiptStatus::Blocked);
        let ok = normalize(&req, 0, false, None, vec![]).expect("normalize");
        assert_ne!(ok.status, ReceiptStatus::Blocked);
    }

    #[test]
    fn maps_exit_codes_and_claims() {
        let dir = tempfile::tempdir().expect("tempdir");
        let req = request(dir.path());
        let receipt = normalize(&req, 3, false, None, vec![]).expect("normalize");
        assert_eq!(receipt.status, ReceiptStatus::BudgetExceeded);
        assert!(!receipt.report_path.is_empty());

        let receipt = normalize(&req, 4, false, None, vec![]).expect("normalize");
        assert_eq!(receipt.status, ReceiptStatus::Failed);
        assert!(receipt.next_hint.contains("adapter"));
    }

    #[test]
    fn parses_final_and_aggregates_usage() {
        let dir = tempfile::tempdir().expect("tempdir");
        let req = request(dir.path());
        fs::create_dir_all(&req.output_dir).expect("mkdir");
        fs::write(
            req.output_dir.join("final.md"),
            "status: done\nbranch: agent/T@123abc\nverify: A1=pass; log=.loom/logs/v.log\nreport: .loom/reports/T.md\n",
        )
        .expect("write final");
        let usage = vec![
            AdapterUsageRow {
                input_tokens: Some(1000),
                cached_input_tokens: Some(400),
                output_tokens: Some(100),
                cost_usd: Some(0.001),
                ..AdapterUsageRow::new("R-1", "T-20261104-001")
            },
            AdapterUsageRow {
                input_tokens: Some(500),
                output_tokens: Some(50),
                ..AdapterUsageRow::new("R-1", "T-20261104-001")
            },
        ];
        let receipt = normalize(&req, 0, false, None, usage).expect("normalize");
        assert_eq!(receipt.status, ReceiptStatus::Done);
        assert_eq!(receipt.verify.len(), 1);
        assert_eq!(receipt.tokens.input, 1500);
        assert_eq!(receipt.tokens.cached_input, 400);
        assert_eq!(receipt.tokens.output, 150);
        assert!((receipt.tokens.cost_usd.unwrap() - 0.001).abs() < 1e-12);
        assert!(receipt.derived_status() == ReceiptStatus::Done);
    }
}
