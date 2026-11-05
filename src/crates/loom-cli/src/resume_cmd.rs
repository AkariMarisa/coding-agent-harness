use anyhow::Result;
use loom_store::{mark_ready_tasks, read_tasks, ready_tasks, sync_index, task_statuses};
use serde_json::json;

use super::dispatch_cmd::{self, DispatchArgs};
use super::Format;

pub fn resume(
    project: &loom_store::Project,
    dispatch: bool,
    adapter: &str,
    run_id: Option<&str>,
    format: Format,
) -> Result<i32> {
    let event_run = run_id.unwrap_or("local");
    let marked = mark_ready_tasks(project, event_run)?;
    let index = sync_index(project)?;

    let statuses = task_statuses(project)?;
    let ready = ready_tasks(project)?;
    let open: Vec<_> = statuses
        .iter()
        .filter(|(_, status)| status.is_open())
        .map(|(task_id, status)| (task_id.clone(), *status))
        .collect();

    let mut dispatched = Vec::new();
    if dispatch {
        for task in &ready {
            let args = DispatchArgs {
                task_id: task.task_id.clone(),
                adapter: adapter.to_string(),
                model: None,
                timeout_s: None,
                attempt: 1,
                in_place: false,
            };
            match dispatch_cmd::dispatch(project, &args, None, Format::Json) {
                Ok(exit) => dispatched.push((task.task_id.clone(), exit)),
                Err(error) => {
                    dispatched.push((task.task_id.clone(), 4));
                    eprintln!("loom: dispatch {} failed: {error:#}", task.task_id);
                }
            }
        }
    }

    let total = read_tasks(project)?.len();
    match format {
        Format::Json => {
            let payload = json!({
                "marked_ready": marked,
                "total_tasks": total,
                "open": open
                    .iter()
                    .map(|(task_id, status)| json!({"task_id": task_id, "status": status.as_str()}))
                    .collect::<Vec<_>>(),
                "ready": ready
                    .iter()
                    .map(|task| task.task_id.clone())
                    .collect::<Vec<_>>(),
                "dispatched": dispatched
                    .iter()
                    .map(|(task_id, exit)| json!({"task_id": task_id, "exit_code": exit}))
                    .collect::<Vec<_>>(),
                "index": index.updated_at,
            });
            println!("{}", serde_json::to_string_pretty(&payload)?);
        }
        Format::Pretty => {
            if !marked.is_empty() {
                println!("marked ready: {}", marked.join(", "));
            }
            println!(
                "tasks: {} total, {} open, {} ready",
                total,
                open.len(),
                ready.len()
            );
            for (task_id, status) in &open {
                println!("  open  {task_id} [{}]", status.as_str());
            }
            for task in &ready {
                println!(
                    "  ready {} [{}] {}",
                    task.task_id,
                    task.tier.as_str(),
                    task.title
                );
            }
            if dispatch {
                for (task_id, exit) in &dispatched {
                    println!("  dispatched {task_id} exit={exit}");
                }
            }
            if open.is_empty() && ready.is_empty() {
                println!("nothing to resume");
            }
        }
    }

    let failed = dispatched.iter().any(|(_, exit)| !matches!(exit, 0 | 2));
    Ok(if failed { 1 } else { 0 })
}
