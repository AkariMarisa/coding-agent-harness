//! Advisory path locks for parallel dispatch (`doc/03` §13).
//!
//! One lock file per write-set pattern under `.loom/locks/`. Acquisition is
//! two-phase (scan, then write) and best-effort: a TOCTOU window remains, so
//! the no-lost-update guarantee still comes from per-task worktrees plus the
//! serial merge queue. Locks only stop a second writer from starting.

use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};

use crate::{now_ms, short_hash, Project};

pub const LOCK_VERSION: u8 = 1;
const LOCK_MODE: &str = "write";

/// One held write lock, stored as `.loom/locks/<sha256(path)[..12]>.lock`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PathLock {
    pub v: u8,
    /// Write-set pattern from `workspace.allowed_paths`, e.g. `src/**`.
    pub path: String,
    pub mode: String,
    pub owner_task: String,
    pub owner_run: String,
    pub owner_pid: u32,
    /// Unix milliseconds; the lock may be reclaimed once the owner is gone.
    pub lease_until_ms: u64,
}

impl PathLock {
    /// `Some(false)` means the owner process is provably gone, `None` means
    /// the platform cannot tell (no `/proc`), in which case only the lease
    /// decides.
    pub fn owner_alive(&self) -> Option<bool> {
        pid_alive(self.owner_pid)
    }

    pub fn is_expired(&self, now_ms_value: u64) -> bool {
        self.lease_until_ms <= now_ms_value
    }

    /// A lock may be taken over when the owner is dead, or when the platform
    /// cannot check liveness and the lease has expired. An expired lease whose
    /// owner is still alive stays a conflict: reclaiming it could produce two
    /// live writers.
    pub fn is_reclaimable(&self, now_ms_value: u64) -> bool {
        match self.owner_alive() {
            Some(false) => true,
            Some(true) => false,
            None => self.is_expired(now_ms_value),
        }
    }

    /// Expired but the owner process is still running: needs a human.
    pub fn is_stuck(&self, now_ms_value: u64) -> bool {
        self.is_expired(now_ms_value) && self.owner_alive() == Some(true)
    }
}

fn pid_alive(pid: u32) -> Option<bool> {
    if pid == 0 {
        return None;
    }
    let proc = std::path::Path::new("/proc");
    if !proc.is_dir() {
        return None;
    }
    Some(proc.join(pid.to_string()).exists())
}

/// Two patterns conflict when one static prefix contains the other. This is
/// deliberately over-approximating: `src` vs `srcx` reports a conflict, but
/// `src/**` vs `migrations/**` does not, and a wildcard-first pattern such as
/// `**` or `*.sql` conflicts with everything.
pub fn overlaps(a: &str, b: &str) -> bool {
    let a = static_prefix(a);
    let b = static_prefix(b);
    if a.is_empty() || b.is_empty() {
        return true;
    }
    a.starts_with(b) || b.starts_with(a)
}

fn static_prefix(pattern: &str) -> &str {
    let pattern = pattern.trim_start_matches("./");
    match pattern.find(['*', '?']) {
        Some(index) => &pattern[..index],
        None => pattern,
    }
}

#[derive(Debug, Clone)]
pub struct LockConflict {
    /// The requested pattern that could not be locked.
    pub requested: String,
    pub held: PathLock,
    pub lease_expired: bool,
}

impl std::fmt::Display for LockConflict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "`{}` is locked by {} (run {}, pid {}, lease {}{})",
            self.held.path,
            self.held.owner_task,
            self.held.owner_run,
            self.held.owner_pid,
            loom_core::rfc3339_ms(self.held.lease_until_ms),
            if self.lease_expired {
                ", expired but the owner is still alive"
            } else {
                ""
            }
        )
    }
}

#[derive(Debug)]
pub enum LockError {
    Conflict(Box<LockConflict>),
    Io(anyhow::Error),
}

impl std::fmt::Display for LockError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Conflict(conflict) => write!(f, "{conflict}"),
            Self::Io(error) => write!(f, "lock io: {error:#}"),
        }
    }
}

impl From<anyhow::Error> for LockError {
    fn from(error: anyhow::Error) -> Self {
        Self::Io(error)
    }
}

pub fn locks_dir(project: &Project) -> PathBuf {
    project.loom_dir().join("locks")
}

pub fn lock_file_name(path: &str) -> String {
    format!("{}.lock", short_hash(path))
}

fn lock_file(project: &Project, path: &str) -> PathBuf {
    locks_dir(project).join(lock_file_name(path))
}

/// Read every parseable lock. Unparseable files are skipped: a corrupted lock
/// cannot claim anything, and refusing to run on it would wedge the project.
pub fn read_locks(project: &Project) -> Result<Vec<PathLock>> {
    let dir = locks_dir(project);
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut locks = Vec::new();
    for entry in fs::read_dir(&dir).with_context(|| format!("read {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("lock") {
            continue;
        }
        let text = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
        if let Ok(lock) = serde_json::from_str::<PathLock>(&text) {
            locks.push(lock);
        }
    }
    locks.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(locks)
}

fn remove_lock_file(project: &Project, path: &str) -> Result<()> {
    let file = lock_file(project, path);
    match fs::remove_file(&file) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("remove {}", file.display())),
    }
}

/// RAII holder: locks are released when the guard drops, including on error
/// paths, so a failed dispatch does not keep its write set locked.
#[derive(Debug)]
pub struct LockGuard {
    project: Project,
    owner_task: String,
    owner_run: String,
}

impl LockGuard {
    /// Acquire write locks for every pattern. Re-entrant for the same task:
    /// existing locks owned by `owner_task` are refreshed with the new lease
    /// and run id. On conflict nothing of ours is left behind.
    pub fn acquire(
        project: &Project,
        owner_task: &str,
        owner_run: &str,
        patterns: &[String],
        lease_s: u64,
    ) -> Result<Self, LockError> {
        let now = now_ms();
        let lease_until_ms = now.saturating_add(lease_s.saturating_mul(1_000));
        let dir = locks_dir(project);
        fs::create_dir_all(&dir).map_err(|error| {
            LockError::Io(anyhow!(error).context(format!("create {}", dir.display())))
        })?;

        let mut seen = HashSet::new();
        let patterns: Vec<&str> = patterns
            .iter()
            .map(String::as_str)
            .filter(|pattern| !pattern.trim().is_empty() && seen.insert(*pattern))
            .collect();

        // Phase 1: prune dead locks, then scan for conflicts. Nothing of ours
        // is written until the whole set is known to be free.
        for held in read_locks(project)? {
            if held.owner_task == owner_task {
                continue;
            }
            let Some(requested) = patterns
                .iter()
                .find(|pattern| overlaps(pattern, &held.path))
            else {
                continue;
            };
            if held.is_reclaimable(now) {
                remove_lock_file(project, &held.path)?;
                continue;
            }
            return Err(LockError::Conflict(Box::new(LockConflict {
                requested: (*requested).to_string(),
                lease_expired: held.is_expired(now),
                held,
            })));
        }

        // Phase 2: write our locks atomically (temp + rename).
        for pattern in &patterns {
            let file = lock_file(project, pattern);
            if let Ok(text) = fs::read_to_string(&file) {
                if let Ok(other) = serde_json::from_str::<PathLock>(&text) {
                    if other.owner_task != owner_task && !other.is_reclaimable(now) {
                        let conflict = LockError::Conflict(Box::new(LockConflict {
                            requested: (*pattern).to_string(),
                            lease_expired: other.is_expired(now),
                            held: other,
                        }));
                        let _ = release_locks(project, owner_task, owner_run);
                        return Err(conflict);
                    }
                }
            }
            let record = PathLock {
                v: LOCK_VERSION,
                path: (*pattern).to_string(),
                mode: LOCK_MODE.to_string(),
                owner_task: owner_task.to_string(),
                owner_run: owner_run.to_string(),
                owner_pid: std::process::id(),
                lease_until_ms,
            };
            let text = serde_json::to_string_pretty(&record)
                .map_err(|error| LockError::Io(anyhow!(error)))?;
            let tmp = file.with_extension(format!("tmp-{}", std::process::id()));
            fs::write(&tmp, format!("{text}\n")).map_err(|error| {
                LockError::Io(anyhow!(error).context(format!("write {}", tmp.display())))
            })?;
            fs::rename(&tmp, &file).map_err(|error| {
                LockError::Io(anyhow!(error).context(format!("rename {}", file.display())))
            })?;
        }

        Ok(Self {
            project: project.clone(),
            owner_task: owner_task.to_string(),
            owner_run: owner_run.to_string(),
        })
    }
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        let _ = release_locks(&self.project, &self.owner_task, &self.owner_run);
    }
}

/// Release the locks held by one (task, run) pair. Matching on both fields
/// lets a newer holder of the same task (e.g. `loom verify` after dispatch)
/// keep its locks when an older guard drops.
pub fn release_locks(project: &Project, owner_task: &str, owner_run: &str) -> Result<usize> {
    let mut released = 0;
    for lock in read_locks(project)? {
        if lock.owner_task == owner_task && lock.owner_run == owner_run {
            remove_lock_file(project, &lock.path)?;
            released += 1;
        }
    }
    Ok(released)
}

/// Distinct tasks currently holding a live lock, pruning dead locks first.
/// This is the input for `[parallel] max_running`.
pub fn live_lock_owners(project: &Project) -> Result<Vec<String>> {
    let now = now_ms();
    let mut owners = HashSet::new();
    for lock in read_locks(project)? {
        if lock.is_reclaimable(now) {
            remove_lock_file(project, &lock.path)?;
            continue;
        }
        owners.insert(lock.owner_task);
    }
    let mut owners: Vec<String> = owners.into_iter().collect();
    owners.sort();
    Ok(owners)
}

/// Remove locks whose owner is gone (or whose lease expired on a platform
/// without pid checks). Returns what was removed so `loom doctor` can report.
pub fn reclaim_stale_locks(project: &Project) -> Result<Vec<PathLock>> {
    let now = now_ms();
    let mut removed = Vec::new();
    for lock in read_locks(project)? {
        if lock.is_reclaimable(now) {
            remove_lock_file(project, &lock.path)?;
            removed.push(lock);
        }
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn setup() -> (tempfile::TempDir, Project) {
        let dir = tempfile::tempdir().expect("tempdir");
        let project = Project::at(dir.path());
        fs::create_dir_all(locks_dir(&project)).expect("locks dir");
        (dir, project)
    }

    fn write_foreign_lock(project: &Project, path: &str, task: &str, pid: u32, lease_ms: u64) {
        let record = PathLock {
            v: LOCK_VERSION,
            path: path.to_string(),
            mode: LOCK_MODE.to_string(),
            owner_task: task.to_string(),
            owner_run: "R-foreign".to_string(),
            owner_pid: pid,
            lease_until_ms: lease_ms,
        };
        fs::write(
            lock_file(project, path),
            format!("{}\n", serde_json::to_string_pretty(&record).expect("json")),
        )
        .expect("write lock");
    }

    fn dead_pid() -> u32 {
        let mut child = Command::new("true").spawn().expect("spawn");
        let pid = child.id();
        child.wait().expect("wait");
        pid
    }

    #[test]
    fn acquire_holds_and_releases() {
        let (_dir, project) = setup();
        let guard = LockGuard::acquire(&project, "T-1", "R-1", &["src/**".to_string()], 600)
            .expect("acquire");
        let locks = read_locks(&project).expect("read");
        assert_eq!(locks.len(), 1);
        assert_eq!(locks[0].owner_task, "T-1");
        assert_eq!(live_lock_owners(&project).expect("owners"), vec!["T-1"]);

        drop(guard);
        assert!(read_locks(&project).expect("read").is_empty());
    }

    #[test]
    fn overlapping_write_sets_conflict() {
        let (_dir, project) = setup();
        // Foreign lock: live pid, lease in the future.
        write_foreign_lock(
            &project,
            "src/api/**",
            "T-2",
            std::process::id(),
            now_ms() + 600_000,
        );

        let error = LockGuard::acquire(&project, "T-1", "R-1", &["src/**".to_string()], 600)
            .expect_err("must conflict");
        match error {
            LockError::Conflict(conflict) => {
                assert_eq!(conflict.held.owner_task, "T-2");
                assert_eq!(conflict.requested, "src/**");
                assert!(!conflict.lease_expired);
            }
            other => panic!("expected conflict, got {other}"),
        }
        // Nothing of ours was written.
        let locks = read_locks(&project).expect("read");
        assert_eq!(locks.len(), 1);
        assert_eq!(locks[0].owner_task, "T-2");
    }

    #[test]
    fn disjoint_write_sets_do_not_conflict() {
        let (_dir, project) = setup();
        write_foreign_lock(
            &project,
            "migrations/**",
            "T-2",
            std::process::id(),
            now_ms() + 600_000,
        );
        let _guard = LockGuard::acquire(&project, "T-1", "R-1", &["src/**".to_string()], 600)
            .expect("disjoint acquire");
        assert_eq!(read_locks(&project).expect("read").len(), 2);
    }

    #[test]
    fn same_task_is_reentrant_and_newer_run_wins_release() {
        let (_dir, project) = setup();
        let first = LockGuard::acquire(&project, "T-1", "R-1", &["src/**".to_string()], 600)
            .expect("first");
        let second = LockGuard::acquire(&project, "T-1", "R-2", &["src/**".to_string()], 600)
            .expect("second");
        let locks = read_locks(&project).expect("read");
        assert_eq!(locks.len(), 1, "re-entrant acquire reuses one file");
        assert_eq!(locks[0].owner_run, "R-2");

        drop(first);
        assert_eq!(
            read_locks(&project).expect("read").len(),
            1,
            "older run must not release the newer holder"
        );
        drop(second);
        assert!(read_locks(&project).expect("read").is_empty());
    }

    #[test]
    fn dead_owner_is_reclaimed_even_with_future_lease() {
        let (_dir, project) = setup();
        write_foreign_lock(&project, "src/**", "T-2", dead_pid(), now_ms() + 600_000);
        let _guard = LockGuard::acquire(&project, "T-1", "R-1", &["src/**".to_string()], 600)
            .expect("dead owner is reclaimable");
        let locks = read_locks(&project).expect("read");
        assert_eq!(locks.len(), 1);
        assert_eq!(locks[0].owner_task, "T-1");
    }

    #[test]
    fn expired_but_alive_owner_still_conflicts() {
        let (_dir, project) = setup();
        write_foreign_lock(
            &project,
            "src/**",
            "T-2",
            std::process::id(),
            now_ms().saturating_sub(1),
        );
        let error = LockGuard::acquire(&project, "T-1", "R-1", &["src/**".to_string()], 600)
            .expect_err("expired lease with live owner stays locked");
        match error {
            LockError::Conflict(conflict) => {
                assert!(conflict.lease_expired);
                assert!(conflict.held.is_stuck(now_ms()));
            }
            other => panic!("expected conflict, got {other}"),
        }
    }

    #[test]
    fn expired_lock_without_pid_check_is_reclaimed() {
        let (_dir, project) = setup();
        // pid 0 means "cannot check": reclaim only after the lease expires.
        write_foreign_lock(&project, "src/**", "T-2", 0, now_ms().saturating_sub(1));
        let _guard = LockGuard::acquire(&project, "T-1", "R-1", &["src/**".to_string()], 600)
            .expect("expired unknown owner is reclaimable");
        assert_eq!(read_locks(&project).expect("read")[0].owner_task, "T-1");

        // Not expired yet: the same lock blocks.
        let (_dir2, project2) = setup();
        write_foreign_lock(&project2, "src/**", "T-2", 0, now_ms() + 600_000);
        assert!(matches!(
            LockGuard::acquire(&project2, "T-1", "R-1", &["src/**".to_string()], 600),
            Err(LockError::Conflict(_))
        ));
    }

    #[test]
    fn live_owners_prune_dead_locks() {
        let (_dir, project) = setup();
        write_foreign_lock(&project, "src/**", "T-dead", dead_pid(), now_ms() + 600_000);
        write_foreign_lock(
            &project,
            "migrations/**",
            "T-live",
            std::process::id(),
            now_ms() + 600_000,
        );
        assert_eq!(live_lock_owners(&project).expect("owners"), vec!["T-live"]);
        let locks = read_locks(&project).expect("read");
        assert_eq!(locks.len(), 1, "dead lock pruned");
        assert_eq!(locks[0].owner_task, "T-live");
    }

    #[test]
    fn overlap_heuristic_is_conservative() {
        assert!(overlaps("src/**", "src/api/**"));
        assert!(overlaps("src/api", "src/api2"), "pure prefix conflicts");
        assert!(overlaps("**", "anything/at/all"));
        assert!(overlaps("*.sql", "migrations/**"));
        assert!(!overlaps("src/**", "migrations/**"));
        assert!(!overlaps("docs/**", "src/**"));
        assert!(overlaps("./src/**", "src/x"));
    }

    #[test]
    fn lock_file_name_is_stable_and_hashed() {
        assert_eq!(
            lock_file_name("src/**"),
            format!("{}.lock", short_hash("src/**"))
        );
        assert!(!lock_file_name("src/api/**").contains('/'));
    }

    #[test]
    fn reclaim_stale_returns_only_dead_locks() {
        let (_dir, project) = setup();
        write_foreign_lock(&project, "a/**", "T-dead", dead_pid(), now_ms() + 600_000);
        write_foreign_lock(
            &project,
            "b/**",
            "T-live",
            std::process::id(),
            now_ms() + 600_000,
        );
        let removed = reclaim_stale_locks(&project).expect("reclaim");
        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0].owner_task, "T-dead");
        let locks = read_locks(&project).expect("read");
        assert_eq!(locks.len(), 1);
        assert!(!locks[0].is_stuck(now_ms()));
    }
}
