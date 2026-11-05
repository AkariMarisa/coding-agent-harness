use std::fs;

use anyhow::{Context as _, Result};
use loom_core::{TaskSpec, TaskStatus};
use loom_store::{
    mark_ready_tasks, read_task, read_tasks, register_task, sync_index, task_statuses,
};
use serde_json::json;

use super::{Format, TaskCommand};

pub fn task(
    project: &loom_store::Project,
    command: TaskCommand,
    run_id: Option<&str>,
    format: Format,
) -> Result<i32> {
    match command {
        TaskCommand::Add { file, json, force } => {
            let text = match (file, json) {
                (Some(path), _) => {
                    fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?
                }
                (None, Some(json)) => json,
                (None, None) => {
                    anyhow::bail!("provide --file or --json with a TaskSpec");
                }
            };
            let spec = serde_json::from_str::<TaskSpec>(&text).context("parse TaskSpec JSON")?;
            spec.validate()
                .map_err(|message| anyhow::anyhow!("invalid TaskSpec: {message}"))?;
            if force {
                loom_store::write_task(project, &spec, true)?;
                loom_store::append_event(
                    project,
                    &loom_core::Event::new(
                        run_id.unwrap_or("local"),
                        loom_store::now_ms(),
                        loom_core::EventType::TaskCreated,
                    )
                    .task(&spec.task_id)
                    .data([(
                        "spec_path".to_string(),
                        json!(loom_store::task_relative_path(&spec.task_id)),
                    )]),
                )?;
                sync_index(project)?;
            } else {
                register_task(project, run_id.unwrap_or("local"), &spec)?;
            }

            let status = loom_store::task_status(project, &spec.task_id)?;
            match format {
                Format::Json => {
                    let payload = json!({
                        "task_id": spec.task_id,
                        "status": status.as_str(),
                        "spec": loom_store::task_relative_path(&spec.task_id),
                    });
                    println!("{}", serde_json::to_string_pretty(&payload)?);
                }
                Format::Pretty => {
                    println!(
                        "registered {} [{}] {}",
                        spec.task_id,
                        status.as_str(),
                        spec.title
                    );
                    println!("spec: {}", loom_store::task_relative_path(&spec.task_id));
                }
            }
            Ok(0)
        }
        TaskCommand::List => {
            let tasks = read_tasks(project)?;
            let statuses = task_statuses(project)?;
            match format {
                Format::Json => {
                    let entries: Vec<_> = tasks
                        .iter()
                        .map(|spec| {
                            let status = statuses
                                .get(&spec.task_id)
                                .copied()
                                .unwrap_or(TaskStatus::Pending);
                            json!({
                                "task_id": spec.task_id,
                                "title": spec.title,
                                "role": spec.role().as_str(),
                                "tier": spec.tier.as_str(),
                                "status": status.as_str(),
                                "depends_on": spec.depends_on,
                            })
                        })
                        .collect();
                    println!("{}", serde_json::to_string_pretty(&entries)?);
                }
                Format::Pretty => {
                    if tasks.is_empty() {
                        println!("no tasks");
                        return Ok(0);
                    }
                    for spec in &tasks {
                        let status = statuses
                            .get(&spec.task_id)
                            .copied()
                            .unwrap_or(TaskStatus::Pending);
                        println!(
                            "{:<18} {:<11} {:<9} {}",
                            spec.task_id,
                            status.as_str(),
                            spec.tier.as_str(),
                            spec.title
                        );
                    }
                }
            }
            Ok(0)
        }
        TaskCommand::Show { task_id } => {
            let spec = read_task(project, &task_id)?;
            let status = loom_store::task_status(project, &task_id)?;
            match format {
                Format::Json => {
                    let payload = json!({
                        "status": status.as_str(),
                        "spec": spec,
                    });
                    println!("{}", serde_json::to_string_pretty(&payload)?);
                }
                Format::Pretty => {
                    println!("{} [{}] {}", spec.task_id, status.as_str(), spec.title);
                    println!(
                        "role={} tier={} goal={}",
                        spec.role().as_str(),
                        spec.tier.as_str(),
                        spec.goal_id
                    );
                    println!("depends_on: {}", spec.depends_on.join(", "));
                    for item in &spec.acceptance {
                        println!(
                            "  {} timeout={}s exit={} cmd={}",
                            item.id, item.timeout_s, item.expect_exit, item.cmd
                        );
                    }
                    println!("spec: {}", loom_store::task_relative_path(&spec.task_id));
                }
            }
            Ok(0)
        }
        TaskCommand::Ready => {
            let marked = mark_ready_tasks(project, run_id.unwrap_or("local"))?;
            let ready = loom_store::ready_tasks(project)?;
            match format {
                Format::Json => {
                    let payload = json!({
                        "marked": marked,
                        "ready": ready.iter().map(|task| task.task_id.clone()).collect::<Vec<_>>(),
                    });
                    println!("{}", serde_json::to_string_pretty(&payload)?);
                }
                Format::Pretty => {
                    if !marked.is_empty() {
                        println!("marked ready: {}", marked.join(", "));
                    }
                    if ready.is_empty() {
                        println!("no ready tasks");
                    } else {
                        for task in ready {
                            println!("{} [{}] {}", task.task_id, task.tier.as_str(), task.title);
                        }
                    }
                }
            }
            Ok(0)
        }
        TaskCommand::Graph => {
            let tasks = read_tasks(project)?;
            loom_core::validate_graph(&tasks)
                .map_err(|message| anyhow::anyhow!("invalid task graph: {message}"))?;
            let statuses = task_statuses(project)?;
            match format {
                Format::Json => {
                    let edges: Vec<_> = tasks
                        .iter()
                        .map(|spec| {
                            json!({
                                "task_id": spec.task_id,
                                "status": statuses
                                    .get(&spec.task_id)
                                    .copied()
                                    .unwrap_or(TaskStatus::Pending)
                                    .as_str(),
                                "depends_on": spec.depends_on,
                            })
                        })
                        .collect();
                    println!("{}", serde_json::to_string_pretty(&edges)?);
                }
                Format::Pretty => {
                    for spec in &tasks {
                        let status = statuses
                            .get(&spec.task_id)
                            .copied()
                            .unwrap_or(TaskStatus::Pending);
                        println!("{} [{}]", spec.task_id, status.as_str());
                        for dep in &spec.depends_on {
                            let dep_status =
                                statuses.get(dep).copied().unwrap_or(TaskStatus::Pending);
                            println!("  <- {dep} [{}]", dep_status.as_str());
                        }
                    }
                }
            }
            Ok(0)
        }
    }
}
