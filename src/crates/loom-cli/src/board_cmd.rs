//! Kanban view over the replayed task statuses (`doc/08` §9.5).

use anyhow::Result;
use loom_core::TaskStatus;
use loom_store::{live_lock_owners, read_locks, task_statuses, Project};
use serde_json::json;

use super::Format;

const COLUMNS: [(&str, TaskStatus); 12] = [
    ("pending", TaskStatus::Pending),
    ("ready", TaskStatus::Ready),
    ("dispatched", TaskStatus::Dispatched),
    ("running", TaskStatus::Running),
    ("receipt", TaskStatus::Receipt),
    ("passed", TaskStatus::Passed),
    ("merged", TaskStatus::Merged),
    ("blocked", TaskStatus::Blocked),
    ("needs_input", TaskStatus::NeedsInput),
    ("failed", TaskStatus::Failed),
    ("escalated", TaskStatus::Escalated),
    ("cancelled", TaskStatus::Cancelled),
];

pub fn board(project: &Project, format: Format) -> Result<i32> {
    let statuses = task_statuses(project)?;
    let config = project.load_config()?;
    let owners = live_lock_owners(project)?;
    let locks = read_locks(project)?;

    let mut columns = serde_json::Map::new();
    let mut rows = Vec::new();
    for (name, status) in COLUMNS {
        let mut ids: Vec<String> = statuses
            .iter()
            .filter(|(_, candidate)| **candidate == status)
            .map(|(task_id, _)| task_id.clone())
            .collect();
        ids.sort();
        columns.insert(name.to_string(), json!(ids));
        rows.push((name, ids));
    }

    match format {
        Format::Json => {
            let payload = json!({
                "tasks": statuses.len(),
                "running": owners.len(),
                "max_running": config.parallel.max_running,
                "lock_owners": owners,
                "locks": locks.len(),
                "columns": columns,
            });
            println!("{}", serde_json::to_string_pretty(&payload)?);
        }
        Format::Pretty => {
            println!(
                "board: {} tasks, running {}/{}",
                statuses.len(),
                owners.len(),
                config.parallel.max_running
            );
            if !locks.is_empty() {
                println!("locks:    {} held by {}", locks.len(), owners.join(", "));
            }
            for (name, ids) in rows {
                let list = if ids.is_empty() {
                    "-".to_string()
                } else {
                    ids.join(" ")
                };
                println!("{name:<11} {list}");
            }
        }
    }
    Ok(0)
}
