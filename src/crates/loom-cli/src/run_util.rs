use std::fs;
use std::path::PathBuf;
use std::time::SystemTime;

use anyhow::{Context as _, Result};
use loom_core::compact_utc;
use loom_store::{now_ms, Project, Run, RunMeta, RUN_META_VERSION};

/// Generate a unique run id based on the current UTC second. The directory is
/// reserved with an atomic `mkdir`, so two dispatch processes starting in the
/// same second never end up sharing a run.
pub fn new_run_id(project: &Project, task_id: &str) -> String {
    let base = format!("R-{}", compact_utc(now_ms()));
    let runs_dir = project.loom_dir().join("runs");
    let _ = fs::create_dir_all(&runs_dir);

    let mut candidates = vec![base.clone(), format!("{base}-{task_id}")];
    for suffix in 2..1_000 {
        candidates.push(format!("{base}-{task_id}-{suffix}"));
    }
    for candidate in candidates {
        if fs::create_dir(runs_dir.join(&candidate)).is_ok() {
            return candidate;
        }
    }
    format!("{base}-{task_id}-{}-{}", now_ms(), std::process::id())
}

/// Resolve which run belongs to a task. Prefers an explicit run id, then the
/// most recent dispatch recorded in `run.json`.
pub fn find_run_for_task(project: &Project, task_id: &str, run_id: Option<&str>) -> Result<String> {
    if let Some(run_id) = run_id {
        return Ok(run_id.to_string());
    }

    let runs_dir = project.loom_dir().join("runs");
    let mut best: Option<(u64, String)> = None;
    if let Ok(entries) = fs::read_dir(&runs_dir) {
        for entry in entries.flatten() {
            let meta_path = entry.path().join("run.json");
            let Ok(text) = fs::read_to_string(&meta_path) else {
                continue;
            };
            let Ok(meta) = serde_json::from_str::<RunMeta>(&text) else {
                continue;
            };
            if meta.task_id != task_id {
                continue;
            }
            let stamp = meta
                .dispatched_at_ms
                .max(mtime_ms(&meta_path))
                .max(mtime_ms(&entry.path()));
            if best.as_ref().map_or(true, |(current, _)| stamp >= *current) {
                best = Some((stamp, meta.run_id.clone()));
            }
        }
    }

    Ok(best
        .map(|(_, run_id)| run_id)
        .unwrap_or_else(|| "local".to_string()))
}

fn mtime_ms(path: &std::path::Path) -> u64 {
    fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|time| time.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

/// Create (or reopen) a run and write its dispatch metadata.
#[allow(clippy::too_many_arguments)]
pub fn write_run_meta(
    project: &Project,
    run_id: &str,
    task_id: &str,
    adapter: &str,
    model: &str,
    model_id: &str,
    workspace: &std::path::Path,
    base_sha: &str,
    timeout_s: u64,
    packet_dir: &std::path::Path,
) -> Result<Run> {
    let run = Run::open(project.clone(), Some(run_id))?;
    let meta = RunMeta {
        v: RUN_META_VERSION,
        run_id: run.id.clone(),
        task_id: task_id.to_string(),
        adapter: adapter.to_string(),
        model: model.to_string(),
        model_id: model_id.to_string(),
        workspace: workspace.to_string_lossy().into_owned(),
        base_sha: base_sha.to_string(),
        dispatched_at_ms: now_ms(),
        timeout_s,
        packet_dir: packet_dir.to_string_lossy().into_owned(),
    };
    run.write_meta(&meta)?;
    Ok(run)
}

/// Append a `task.blocked` event. Used when dispatch/verify refuse to start
/// instead of silently failing: the block must be visible in the event log.
pub fn append_blocked_event(
    project: &Project,
    run_id: &str,
    task_id: &str,
    attempt: u32,
    data: Vec<(&str, serde_json::Value)>,
) -> Result<()> {
    let event = loom_core::Event::new(run_id, now_ms(), loom_core::EventType::TaskBlocked)
        .task(task_id)
        .attempt(attempt)
        .data(
            data.into_iter()
                .map(|(key, value)| (key.to_string(), value))
                .collect::<Vec<_>>(),
        );
    loom_store::append_event(project, &event)
}

pub fn git_head(path: &std::path::Path) -> Option<String> {
    let output = std::process::Command::new("git")
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

/// Resolve a task workspace, creating a git worktree when required.
pub fn prepare_workspace(
    project: &Project,
    spec: &loom_core::TaskSpec,
    force_in_place: bool,
) -> Result<PathBuf> {
    if force_in_place || matches!(spec.workspace.mode, loom_core::WorkspaceMode::InPlace) {
        return Ok(project.root.clone());
    }

    let worktree = project.root.join(".worktrees").join(&spec.task_id);
    if worktree.exists() {
        return Ok(worktree);
    }
    if let Some(parent) = worktree.parent() {
        fs::create_dir_all(parent).with_context(|| "create .worktrees")?;
    }

    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(&project.root)
        .arg("worktree")
        .arg("add")
        .arg("-b")
        .arg(&spec.workspace.branch)
        .arg(&worktree)
        .arg(&spec.workspace.base_ref)
        .output()
        .with_context(|| "git worktree add")?;
    if !output.status.success() {
        anyhow::bail!(
            "git worktree add failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(worktree)
}
