//! Filesystem layout and persistence for `.loom/`.

mod artifact;
mod events;
mod ledger;
mod locks;
mod project;
mod run;
mod seen;
mod tasks;
mod telemetry;

pub use artifact::{read_artifact, write_artifact};
pub use events::{append_event, events_path, read_events};
pub use ledger::{aggregate_usage, append_usage, ledger_path, read_usage};
pub use locks::{
    live_lock_owners, lock_file_name, locks_dir, read_locks, reclaim_stale_locks, release_locks,
    LockConflict, LockError, LockGuard, PathLock,
};
pub use project::{InitReport, Project};
pub use run::{Run, RunMeta, RUN_META_VERSION};
pub use seen::Seen;
pub use tasks::{
    index_path, mark_ready_tasks, missing_dependencies, read_index, read_task, read_tasks,
    ready_tasks, register_task, replay_statuses, sync_index, task_path, task_relative_path,
    task_status, task_statuses, tasks_dir, write_index, write_task,
};
pub use telemetry::{append_tool_event, now_ms, totals, ToolEvent, Totals};

use sha2::{Digest, Sha256};

pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

pub fn short_hash(text: &str) -> String {
    sha256_hex(text.as_bytes())[..12].to_string()
}
