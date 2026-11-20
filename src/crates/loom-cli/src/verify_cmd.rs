use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result};
use loom_core::{
    AttentionAction, AttentionKind, AttentionSource, EventType, Receipt, ReceiptStatus, Severity,
    TokenTotals, VerifyEvidence, VerifyResult,
};
use loom_store::{
    append_event, now_ms, open_item, read_task, sync_inbox, sync_index, NewItem, Run,
};
use serde_json::json;
use sha2::{Digest, Sha256};

use super::run_util::{append_blocked_event, find_run_for_task, git_head};
use super::Format;

/// Result of one verify run, for callers (e.g. `loom merge`) that need the
/// outcome without a second JSON document on stdout.
pub(crate) struct VerifyOutcome {
    pub(crate) run_id: String,
    pub(crate) receipt_path: PathBuf,
    pub(crate) receipt: Receipt,
    pub(crate) passed: bool,
    pub(crate) claim_done: bool,
    pub(crate) attempt: u32,
}

pub fn verify(
    project: &loom_store::Project,
    task_id: &str,
    attempt: u32,
    run_id: Option<&str>,
    format: Format,
) -> Result<i32> {
    let outcome = run_verify(project, task_id, attempt, run_id)?;
    let passed = outcome.passed;
    print_outcome(task_id, outcome, format)?;
    Ok(if passed { 0 } else { 1 })
}

fn print_outcome(task_id: &str, outcome: VerifyOutcome, format: Format) -> Result<()> {
    let VerifyOutcome {
        run_id,
        receipt_path,
        receipt,
        passed,
        claim_done,
        attempt,
    } = outcome;
    match format {
        Format::Json => {
            let payload = json!({
                "run_id": run_id,
                "task_id": task_id,
                "attempt": attempt,
                "passed": passed,
                "status": receipt.status.as_str(),
                "claim_done": claim_done,
                "verify": receipt.verify,
                "receipt": receipt_path.to_string_lossy(),
                "tokens": {
                    "input": receipt.tokens.input,
                    "cached_input": receipt.tokens.cached_input,
                    "output": receipt.tokens.output,
                    "cost_usd": receipt.tokens.cost_usd,
                },
            });
            println!("{}", serde_json::to_string_pretty(&payload)?);
        }
        Format::Pretty => {
            println!("{task_id} attempt {attempt} in run {run_id}");
            for item in &receipt.verify {
                println!(
                    "  [{}] {:<10} exit={} {:.1}s",
                    if item.result == VerifyResult::Pass {
                        "ok"
                    } else {
                        "FAIL"
                    },
                    item.id,
                    item.exit,
                    item.duration_s
                );
            }
            println!(
                "result:   {} ({})",
                if passed { "passed" } else { "failed" },
                receipt.status.as_str()
            );
            println!("receipt:  {}", receipt_path.display());
        }
    }
    Ok(())
}

/// Re-run acceptance independently and persist the receipt plus events.
pub(crate) fn run_verify(
    project: &loom_store::Project,
    task_id: &str,
    attempt: u32,
    run_id: Option<&str>,
) -> Result<VerifyOutcome> {
    let spec = read_task(project, task_id)?;
    let run_id = find_run_for_task(project, task_id, run_id)?;
    let run = Run::open(project.clone(), Some(&run_id))?;
    let meta = run
        .read_meta()
        .with_context(|| format!("run {} has no metadata; dispatch the task first", run.id))?;
    if meta.task_id != task_id {
        anyhow::bail!("run {} belongs to {}, not {task_id}", run.id, meta.task_id);
    }
    let workspace = PathBuf::from(&meta.workspace);
    if !workspace.exists() {
        anyhow::bail!("workspace {} no longer exists", workspace.display());
    }

    // Re-run acceptance under the same path locks dispatch used, so an
    // in-place checkout cannot be written by another task mid-verify.
    let lease_s = spec.budget.max_wall_s.saturating_add(600);
    let _locks = match loom_store::LockGuard::acquire(
        project,
        task_id,
        &run.id,
        &spec.workspace.allowed_paths,
        lease_s,
    ) {
        Ok(guard) => guard,
        Err(loom_store::LockError::Conflict(conflict)) => {
            append_blocked_event(
                project,
                &run.id,
                task_id,
                attempt,
                vec![
                    ("code", json!("lock_owner")),
                    ("lock_owner", json!(conflict.held.owner_task)),
                    ("lock_run", json!(conflict.held.owner_run)),
                    ("lock_path", json!(conflict.held.path)),
                    ("detail", json!(conflict.to_string())),
                ],
            )?;
            anyhow::bail!("verify {task_id} blocked by path lock: {conflict}");
        }
        Err(loom_store::LockError::Io(error)) => return Err(error),
    };

    let claim = read_claim(&run);
    let head_sha = git_head(&workspace)
        .or_else(|| claim.as_ref().map(|receipt| receipt.head_sha.clone()))
        .unwrap_or_else(|| "unknown".to_string());
    let base_sha = claim
        .as_ref()
        .map(|receipt| receipt.base_sha.clone())
        .filter(|sha| !sha.trim().is_empty())
        .or_else(|| git_head(&workspace))
        .unwrap_or_else(|| "unknown".to_string());
    let branch = claim
        .as_ref()
        .map(|receipt| receipt.branch.clone())
        .filter(|branch| !branch.trim().is_empty())
        .unwrap_or_else(|| spec.workspace.branch.clone());

    let log_path = run.verify_log_path();
    let log_rel = format!(".loom/runs/{}/logs/verify.log", run.id);
    let mut log = String::new();
    log.push_str(&format!(
        "# verify {} attempt {} run {} git={head_sha}\n",
        task_id, attempt, run.id
    ));
    let env_hash = hash_env();

    let mut evidence = Vec::new();
    for item in &spec.acceptance {
        let outcome = run_command(&workspace, &item.cmd, item.timeout_s)?;
        let block = format!(
            "\n=== {} ===\ncmd: {}\nexit: {} timeout: {} duration_s: {:.3}\n--- stdout ---\n{}\n--- stderr ---\n{}\n",
            item.id,
            item.cmd,
            if outcome.timed_out { -1 } else { outcome.exit_code },
            outcome.timed_out,
            outcome.duration.as_secs_f64(),
            String::from_utf8_lossy(&outcome.stdout),
            String::from_utf8_lossy(&outcome.stderr),
        );
        let block_hash = sha256_hex(block.as_bytes());
        log.push_str(&block);

        let result = if outcome.timed_out {
            VerifyResult::Fail
        } else if outcome.exit_code == item.expect_exit {
            VerifyResult::Pass
        } else {
            VerifyResult::Fail
        };

        evidence.push(VerifyEvidence {
            id: item.id.clone(),
            cmd: item.cmd.clone(),
            exit: if outcome.timed_out {
                -1
            } else {
                outcome.exit_code
            },
            signal: None,
            duration_s: outcome.duration.as_secs_f64(),
            timeout_s: Some(item.timeout_s),
            log_path: log_rel.clone(),
            log_sha256: Some(block_hash),
            git_sha: Some(head_sha.clone()),
            env_hash: Some(env_hash.clone()),
            result,
        });
    }

    fs::write(&log_path, log).with_context(|| format!("write {}", log_path.display()))?;

    // Verdict consumption: review/judgment reports are parsed, not grepped.
    // A missing or unparseable verdict fails fast — for these roles the
    // verdict is the deliverable (AGENTS.md §10) — and the merge queue later
    // consumes the parsed value (`task.blocked(code=review_changes)`, doc/03).
    let report_text =
        fs::read_to_string(workspace.join(&spec.output.report_path)).unwrap_or_default();
    let verdict = match spec.role() {
        loom_core::TaskRole::Review => loom_core::parse_review_verdict(&report_text),
        loom_core::TaskRole::Judgment => loom_core::parse_judgment_decision(&report_text),
        _ => None,
    };
    let verdict_missing = matches!(
        spec.role(),
        loom_core::TaskRole::Review | loom_core::TaskRole::Judgment
    ) && verdict.is_none();

    let claim_status = claim.as_ref().map(|receipt| receipt.status);
    let claim_done = claim_status == Some(ReceiptStatus::Done);
    let all_pass = !evidence.is_empty()
        && evidence
            .iter()
            .all(|item| item.result == VerifyResult::Pass);
    // A review/judgment task without a parseable verdict has not passed, no
    // matter what its acceptance says — the verdict is the deliverable.
    let passed = all_pass && claim_done && !verdict_missing;

    // Keep the adapter's claim when verification cannot contradict it. A
    // `needs_input`/`blocked`/`budget_exceeded` run is not a model failure:
    // recording it as `task.failed` used to pollute the escalation counter
    // (doc/15 §2.1). A failed acceptance still wins over any claim.
    let status = if verdict_missing {
        // Acceptance can pass while the report itself is malformed; for a
        // review/judgment task the verdict is the deliverable.
        ReceiptStatus::Failed
    } else if passed || !all_pass {
        if passed {
            ReceiptStatus::Done
        } else {
            ReceiptStatus::Failed
        }
    } else {
        claim_status.unwrap_or(ReceiptStatus::Failed)
    };

    let mut receipt = Receipt::new(task_id, attempt);
    receipt.status = status;
    receipt.base_sha = base_sha;
    receipt.head_sha = head_sha;
    receipt.branch = branch;
    receipt.diffstat = loom_adapter::git_diffstat(&workspace);
    receipt.verify = evidence;
    receipt.tokens = aggregate_usage(&run)?;
    receipt.report_path = claim
        .as_ref()
        .map(|receipt| receipt.report_path.clone())
        .filter(|path| !path.trim().is_empty())
        .unwrap_or_else(|| run.report_path().to_string_lossy().into_owned());
    receipt.verdict = verdict;

    // Skills on the receipt come from the run's context manifest — compile is
    // the injection record (it bails on a missing/oversized skill), and this
    // is where consumers look (`doc/15` §12.3). A manifest from before the
    // feature parses to an empty list via serde default; a missing one (run
    // never compiled) degrades the same way rather than failing verify.
    receipt.skills = fs::read_to_string(run.manifest_path())
        .ok()
        .and_then(|text| serde_json::from_str::<loom_context::ContextManifest>(&text).ok())
        .map(|manifest| manifest.skills)
        .unwrap_or_default();

    // Required artifacts.
    let mut artifacts = Vec::new();
    if spec
        .output
        .required_artifacts
        .iter()
        .any(|item| item == "git_diff")
    {
        if let Some(diff) = write_patch(&run, &workspace)? {
            artifacts.push(diff);
        }
    }
    if spec
        .output
        .required_artifacts
        .iter()
        .any(|item| item == "verify_log")
    {
        let bytes = fs::read(&log_path).unwrap_or_default();
        artifacts.push(loom_core::ArtifactEvidence {
            path: log_rel.clone(),
            sha256: sha256_hex(&bytes),
            bytes: bytes.len() as u64,
        });
    }
    receipt.artifacts = artifacts;

    if verdict_missing {
        receipt.next_hint = format!(
            "{} report {} has no parseable verdict (review expects `verdict: pass|request_changes|human`; judgment expects a JSON `decision`)",
            spec.role().as_str(),
            spec.output.report_path
        );
    } else if !passed {
        if let Some(failure) = receipt
            .verify
            .iter()
            .find(|item| item.result == VerifyResult::Fail)
        {
            receipt.next_hint = format!(
                "acceptance {} failed (exit {}): {}",
                failure.id, failure.exit, failure.cmd
            );
        } else if !claim_done {
            receipt.next_hint = match status {
                ReceiptStatus::NeedsInput => {
                    "adapter needs input; answer, then dispatch the task again".to_string()
                }
                ReceiptStatus::Blocked => {
                    "adapter blocked; resolve the blocker, then dispatch the task again".to_string()
                }
                ReceiptStatus::BudgetExceeded => {
                    "adapter exceeded its budget; raise the limit or split the task".to_string()
                }
                _ => "adapter claim was not done; verify cannot mark the task passed".to_string(),
            };
        }
    }

    receipt
        .validate()
        .map_err(|message| anyhow::anyhow!("receipt is invalid: {message}"))?;
    let text = serde_json::to_string_pretty(&receipt)?;
    fs::write(run.receipt_path(), format!("{text}\n"))
        .with_context(|| format!("write {}", run.receipt_path().display()))?;

    for item in &receipt.verify {
        append_event(
            project,
            &loom_core::Event::new(&run.id, now_ms(), EventType::TaskVerify)
                .task(task_id)
                .attempt(attempt)
                .data([
                    ("acceptance_id".to_string(), json!(item.id)),
                    ("result".to_string(), json!(item.result.as_str())),
                    ("exit".to_string(), json!(item.exit)),
                    ("log_path".to_string(), json!(item.log_path)),
                ]),
        )?;
    }

    if let Some(verdict) = verdict {
        append_event(
            project,
            &loom_core::Event::new(&run.id, now_ms(), loom_core::EventType::TaskVerdict)
                .task(task_id)
                .attempt(attempt)
                .data([("verdict".to_string(), json!(verdict.as_str()))]),
        )?;
    }

    let verify_ids = json!(receipt
        .verify
        .iter()
        .filter(|item| item.result == VerifyResult::Fail)
        .map(|item| item.id.clone())
        .collect::<Vec<_>>());
    match status {
        ReceiptStatus::Done => append_event(
            project,
            &loom_core::Event::new(&run.id, now_ms(), EventType::TaskPassed)
                .task(task_id)
                .attempt(attempt)
                .data([(
                    "verify_ids".to_string(),
                    json!(receipt
                        .verify
                        .iter()
                        .map(|item| item.id.clone())
                        .collect::<Vec<_>>()),
                )]),
        )?,
        ReceiptStatus::NeedsInput => {
            // Enrich with the Q&A on disk so consumers (inbox, replay) see
            // what is being waited on without re-reading questions.json.
            let questions: loom_core::QuestionsFile =
                std::fs::read_to_string(run.outbox_dir().join("questions.json"))
                    .ok()
                    .and_then(|text| serde_json::from_str::<loom_core::QuestionsFile>(&text).ok())
                    .filter(|file| file.task_id == task_id)
                    .unwrap_or(loom_core::QuestionsFile {
                        v: loom_core::QUESTIONS_VERSION,
                        task_id: task_id.to_string(),
                        run_id: run.id.clone(),
                        questions: Vec::new(),
                    });
            append_event(
                project,
                &loom_core::Event::new(&run.id, now_ms(), EventType::TaskNeedsInput)
                    .task(task_id)
                    .attempt(attempt)
                    .data([
                        ("code".to_string(), json!("needs_input")),
                        ("verify_ids".to_string(), verify_ids),
                        (
                            "question_ids".to_string(),
                            json!(questions
                                .questions
                                .iter()
                                .map(|question| question.id.clone())
                                .collect::<Vec<_>>()),
                        ),
                        (
                            "scopes".to_string(),
                            json!(questions
                                .questions
                                .iter()
                                .map(|question| question.scope.as_str())
                                .collect::<Vec<_>>()),
                        ),
                        (
                            "blocking".to_string(),
                            json!(questions.questions.iter().any(|question| question.blocking)),
                        ),
                    ]),
            )?;
            // doc/15 §2.2: every question becomes a resident inbox item, so
            // "what needs me" survives past this conversation.
            if questions.questions.is_empty() {
                // Older ask without a questions file: one generic blocking
                // item instead of silence.
                open_item(
                    project,
                    &NewItem {
                        run_id: run.id.clone(),
                        task_id: Some(task_id.to_string()),
                        kind: AttentionKind::Question,
                        source: AttentionSource::Task,
                        severity: Severity::Blocking,
                        dedupe_key: format!("task:{task_id}:needs_input"),
                        title: format!("task {task_id} needs input"),
                        detail: Some(
                            "adapter returned needs_input without questions.json".to_string(),
                        ),
                        refs: Vec::new(),
                        actions: vec![AttentionAction {
                            id: "show".to_string(),
                            label: "Show task".to_string(),
                            cmd: format!("loom task show {task_id}"),
                        }],
                    },
                )?;
            } else {
                for question in &questions.questions {
                    open_item(
                        project,
                        &NewItem {
                            run_id: run.id.clone(),
                            task_id: Some(task_id.to_string()),
                            kind: AttentionKind::Question,
                            source: AttentionSource::Task,
                            severity: if question.blocking {
                                Severity::Blocking
                            } else {
                                Severity::Soon
                            },
                            dedupe_key: format!("task:{task_id}:question:{}", question.id),
                            title: question.question.clone(),
                            detail: question.why_now.clone(),
                            refs: question.evidence.clone(),
                            actions: vec![AttentionAction {
                                id: "answer".to_string(),
                                label: "Answer".to_string(),
                                cmd: format!("loom answer {task_id} -q {}", question.id),
                            }],
                        },
                    )?;
                }
            }
        }
        ReceiptStatus::Blocked | ReceiptStatus::BudgetExceeded => append_event(
            project,
            &loom_core::Event::new(&run.id, now_ms(), EventType::TaskBlocked)
                .task(task_id)
                .attempt(attempt)
                .data([
                    ("code".to_string(), json!(status.as_str())),
                    ("verify_ids".to_string(), verify_ids),
                ]),
        )?,
        ReceiptStatus::Failed => {
            let code = if claim_done {
                "verify_fail"
            } else {
                "claim_not_done"
            };
            append_event(
                project,
                &loom_core::Event::new(&run.id, now_ms(), EventType::TaskFailed)
                    .task(task_id)
                    .attempt(attempt)
                    .data([
                        ("code".to_string(), json!(code)),
                        ("verify_ids".to_string(), verify_ids),
                    ]),
            )?;
        }
    }
    sync_index(project)?;
    // Keep the resident view in step with any attention events written above.
    sync_inbox(project)?;

    let receipt_path = run.receipt_path();
    Ok(VerifyOutcome {
        run_id: run.id,
        receipt_path,
        receipt,
        passed,
        claim_done,
        attempt,
    })
}

fn read_claim(run: &Run) -> Option<Receipt> {
    let text = fs::read_to_string(run.receipt_path()).ok()?;
    serde_json::from_str::<Receipt>(&text).ok()
}

fn aggregate_usage(run: &Run) -> Result<TokenTotals> {
    let rows = loom_adapter::read_usage_rows(&run.usage_path())?;
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
    Ok(totals)
}

fn write_patch(run: &Run, workspace: &Path) -> Result<Option<loom_core::ArtifactEvidence>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(workspace)
        .args(["diff", "--binary"])
        .output()
        .ok();
    let Some(output) = output else {
        return Ok(None);
    };
    if !output.status.success() || output.stdout.is_empty() {
        return Ok(None);
    }
    let path = run.dir().join("patch.diff");
    fs::write(&path, &output.stdout).with_context(|| format!("write {}", path.display()))?;
    Ok(Some(loom_core::ArtifactEvidence {
        path: format!(".loom/runs/{}/patch.diff", run.id),
        sha256: sha256_hex(&output.stdout),
        bytes: output.stdout.len() as u64,
    }))
}

pub(crate) struct CommandOutcome {
    pub(crate) exit_code: i32,
    pub(crate) timed_out: bool,
    pub(crate) duration: Duration,
    pub(crate) stdout: Vec<u8>,
    pub(crate) stderr: Vec<u8>,
}

pub(crate) fn run_command(cwd: &Path, cmd: &str, timeout_s: u64) -> Result<CommandOutcome> {
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .current_dir(cwd)
        .env("LOOM_VERIFY", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("spawn verify command: {cmd}"))?;

    let mut stdout_pipe = child.stdout.take();
    let mut stderr_pipe = child.stderr.take();
    let stdout_thread = thread::spawn(move || {
        let mut buffer = Vec::new();
        if let Some(pipe) = stdout_pipe.as_mut() {
            let _ = pipe.read_to_end(&mut buffer);
        }
        buffer
    });
    let stderr_thread = thread::spawn(move || {
        let mut buffer = Vec::new();
        if let Some(pipe) = stderr_pipe.as_mut() {
            let _ = pipe.read_to_end(&mut buffer);
        }
        buffer
    });

    let started = Instant::now();
    let mut timed_out = false;
    let mut status = None;
    loop {
        match child
            .try_wait()
            .with_context(|| "wait for verify command")?
        {
            Some(waited) => {
                status = Some(waited);
                break;
            }
            None => {
                if started.elapsed().as_secs() >= timeout_s {
                    timed_out = true;
                    let _ = child.kill();
                    let _ = child.wait();
                    break;
                }
                thread::sleep(Duration::from_millis(20));
            }
        }
    }

    let stdout = stdout_thread.join().unwrap_or_default();
    let stderr = stderr_thread.join().unwrap_or_default();
    let exit_code = if timed_out {
        -1
    } else {
        status
            .as_ref()
            .and_then(|status| status.code())
            .unwrap_or(-1)
    };

    Ok(CommandOutcome {
        exit_code,
        timed_out,
        duration: started.elapsed(),
        stdout,
        stderr,
    })
}

fn hash_env() -> String {
    let mut entries: Vec<(String, String)> = std::env::vars()
        .filter(|(key, _)| (key.starts_with("LOOM_") || key == "PATH") && !key.contains("API_KEY"))
        .collect();
    entries.sort();
    let text = entries
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join("\n");
    sha256_hex(text.as_bytes())
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}
