//! Serial merge queue (`doc/05` §6, `doc/03` §13).
//!
//! One merge at a time: the queue holds a global `**` path lock, so no write
//! task can dispatch while the base branch is being rewritten. Merging is
//! rebase → merge → integration verify, with rollback and a `task.blocked`
//! event when anything fails.

use std::path::Path;
use std::process::Command;

use anyhow::{bail, Context, Result};
use loom_core::{EventFilter, EventType, TaskStatus, WorkspaceMode};
use loom_store::{
    append_event, read_events, read_task, sync_index, task_status, LockError, LockGuard, Project,
};
use serde_json::json;

use super::run_util::{append_blocked_event, find_run_for_task};
use super::verify_cmd::{run_command, run_verify};
use super::Format;

/// Block codes a previous failed merge may leave behind; only these let a
/// `blocked` task back into the merge queue.
const MERGE_BLOCK_CODES: [&str; 2] = ["merge_conflict", "integration_verify"];

pub fn merge(project: &Project, task_id: &str, attempt: u32, format: Format) -> Result<i32> {
    let spec = read_task(project, task_id)?;
    let status = task_status(project, task_id)?;
    match status {
        TaskStatus::Merged => bail!("task {task_id} is already merged"),
        TaskStatus::Passed => {}
        TaskStatus::Blocked
            if last_block_code(project, task_id)
                .as_deref()
                .is_some_and(|code| MERGE_BLOCK_CODES.contains(&code)) =>
        {
            // Retry after a failed merge (conflict or integration verify).
        }
        other => bail!(
            "task {task_id} is {}; merge only accepts a passed task (run `loom verify` first)",
            other.as_str()
        ),
    }

    if matches!(spec.workspace.mode, WorkspaceMode::InPlace) {
        bail!(
            "task {task_id} runs in place; its changes are already on the base branch, commit them directly"
        );
    }
    let worktree = project.root.join(".worktrees").join(&spec.task_id);
    if !worktree.exists() {
        bail!(
            "worktree {} no longer exists; re-dispatch the task",
            worktree.display()
        );
    }
    let head_branch = current_branch(&project.root)?.ok_or_else(|| {
        anyhow::anyhow!(
            "base checkout is on a detached HEAD; check out {} first",
            spec.workspace.base_ref
        )
    })?;
    if is_local_branch(&project.root, &spec.workspace.base_ref)?
        && head_branch != spec.workspace.base_ref
    {
        bail!(
            "base checkout is on {head_branch}, but this task merges into {}; switch branches first",
            spec.workspace.base_ref
        );
    }

    // Global queue lock: `**` overlaps every write set, so merging both
    // serializes merges and pauses dispatching write tasks.
    let run_id = find_run_for_task(project, task_id, None)?;
    let lease_s = spec.budget.max_wall_s.saturating_add(600);
    let _queue = match LockGuard::acquire(project, task_id, &run_id, &["**".to_string()], lease_s) {
        Ok(guard) => guard,
        Err(LockError::Conflict(conflict)) => {
            bail!("merge for {task_id} waits for other writers: {conflict}")
        }
        Err(LockError::Io(error)) => return Err(error),
    };

    // Pre-merge verify: the branch must still pass right before it lands.
    let outcome = run_verify(project, task_id, attempt, Some(&run_id))?;
    if !outcome.passed {
        let hint = if outcome.receipt.next_hint.is_empty() {
            "acceptance no longer passes"
        } else {
            &outcome.receipt.next_hint
        };
        bail!("pre-merge verify failed for {task_id}: {hint}");
    }

    let dirty = git(
        &project.root,
        &["status", "--porcelain", "--untracked-files=no"],
    )?;
    if !dirty.stdout.is_empty() {
        bail!(
            "base checkout has uncommitted changes; commit or stash them first:\n{}",
            String::from_utf8_lossy(&dirty.stdout)
        );
    }

    // Rebase first so a conflicted branch never reaches the base checkout.
    let rebase = git(&worktree, &["rebase", &spec.workspace.base_ref])?;
    if !rebase.status.success() {
        let _ = git(&worktree, &["rebase", "--abort"]);
        let detail = String::from_utf8_lossy(&rebase.stderr).trim().to_string();
        block_merge(
            project,
            &run_id,
            task_id,
            attempt,
            "merge_conflict",
            &json!({
                "base_ref": spec.workspace.base_ref,
                "detail": detail,
            }),
        )?;
        bail!(
            "task {task_id} conflicts with {}; rebase aborted: {detail}",
            spec.workspace.base_ref
        );
    }

    let before = git_rev(&project.root)?;
    let merge = git(
        &project.root,
        &["merge", "--no-edit", &spec.workspace.branch],
    )?;
    if !merge.status.success() {
        let _ = git(&project.root, &["merge", "--abort"]);
        let detail = String::from_utf8_lossy(&merge.stderr).trim().to_string();
        block_merge(
            project,
            &run_id,
            task_id,
            attempt,
            "merge_conflict",
            &json!({ "detail": detail }),
        )?;
        bail!(
            "merge of {} into {before} failed: {detail}",
            spec.workspace.branch
        );
    }
    let merge_sha = git_rev(&project.root)?;

    // Integration verify: the merged base must satisfy this task's acceptance.
    let mut failures = Vec::new();
    for item in &spec.acceptance {
        let outcome = run_command(&project.root, &item.cmd, item.timeout_s)?;
        if outcome.timed_out || outcome.exit_code != item.expect_exit {
            failures.push(format!(
                "{} (exit {}{})",
                item.id,
                outcome.exit_code,
                if outcome.timed_out { ", timeout" } else { "" }
            ));
        }
    }
    if !failures.is_empty() {
        let rollback = git(&project.root, &["reset", "--hard", &before])?;
        if !rollback.status.success() {
            eprintln!(
                "warning: rollback to {before} failed: {}",
                String::from_utf8_lossy(&rollback.stderr).trim()
            );
        }
        let detail = failures.join(", ");
        block_merge(
            project,
            &run_id,
            task_id,
            attempt,
            "integration_verify",
            &json!({ "detail": detail, "rolled_back_to": before }),
        )?;
        bail!(
            "integration verify failed after merging {task_id} ({detail}); rolled back to {before}"
        );
    }

    append_event(
        project,
        &loom_core::Event::new(&run_id, loom_store::now_ms(), EventType::TaskMerged)
            .task(task_id)
            .attempt(attempt)
            .data([
                ("merge_sha".to_string(), json!(merge_sha)),
                ("base_before".to_string(), json!(before)),
                ("branch".to_string(), json!(spec.workspace.branch)),
                ("integration_verify".to_string(), json!(true)),
            ]),
    )?;
    sync_index(project)?;

    // Best-effort cleanup: a dirty worktree stays put and is reported.
    let worktree_str = worktree.to_string_lossy().into_owned();
    let removed = git(&project.root, &["worktree", "remove", &worktree_str])
        .map(|output| output.status.success())
        .unwrap_or(false);
    if !removed {
        eprintln!(
            "warning: worktree {} was not removed (dirty?); run `git worktree prune` later",
            worktree.display()
        );
    }
    let _ = git(&project.root, &["worktree", "prune"]);
    let branch_deleted = git(&project.root, &["branch", "-d", &spec.workspace.branch])
        .map(|output| output.status.success())
        .unwrap_or(false);

    match format {
        Format::Json => {
            let payload = json!({
                "task_id": task_id,
                "merged": true,
                "merge_sha": merge_sha,
                "base_before": before,
                "branch": spec.workspace.branch,
                "integration_verify": true,
                "worktree_removed": removed,
                "branch_deleted": branch_deleted,
            });
            println!("{}", serde_json::to_string_pretty(&payload)?);
        }
        Format::Pretty => {
            println!("merged {task_id} into {}", spec.workspace.base_ref);
            println!("merge:    {merge_sha} (from {before})");
            println!("verify:   integration acceptance passed");
            if !removed || !branch_deleted {
                println!("cleanup:  worktree_removed={removed} branch_deleted={branch_deleted}");
            }
        }
    }
    Ok(0)
}

/// Record why a merge is blocked. The task stays retryable: the merge gate
/// accepts `blocked` only for these codes.
fn block_merge(
    project: &Project,
    run_id: &str,
    task_id: &str,
    attempt: u32,
    code: &str,
    extra: &serde_json::Value,
) -> Result<()> {
    let mut data = vec![("code", json!(code))];
    if let Some(map) = extra.as_object() {
        for (key, value) in map {
            data.push((key.as_str(), value.clone()));
        }
    }
    append_blocked_event(project, run_id, task_id, attempt, data)
}

fn last_block_code(project: &Project, task_id: &str) -> Option<String> {
    let filter = EventFilter {
        task_id: Some(task_id.to_string()),
        event_type: Some(EventType::TaskBlocked),
        ..EventFilter::default()
    };
    let events = read_events(project, &filter).ok()?;
    events
        .last()?
        .data
        .get("code")
        .and_then(|value| value.as_str())
        .map(str::to_string)
}

fn git(dir: &Path, args: &[&str]) -> Result<std::process::Output> {
    Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .with_context(|| format!("git {}", args.join(" ")))
}

fn git_rev(dir: &Path) -> Result<String> {
    let output = git(dir, &["rev-parse", "HEAD"])?;
    if !output.status.success() {
        bail!(
            "git rev-parse HEAD failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn current_branch(dir: &Path) -> Result<Option<String>> {
    let output = git(dir, &["symbolic-ref", "--quiet", "--short", "HEAD"])?;
    if !output.status.success() {
        return Ok(None);
    }
    let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok((!branch.is_empty()).then_some(branch))
}

fn is_local_branch(dir: &Path, name: &str) -> Result<bool> {
    let output = git(
        dir,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/{name}"),
        ],
    )?;
    Ok(output.status.success())
}
