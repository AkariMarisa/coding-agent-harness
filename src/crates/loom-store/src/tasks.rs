use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use loom_core::{
    rfc3339_ms, status_after_event, validate_graph, Event, EventFilter, TaskIndex, TaskIndexEntry,
    TaskSpec, TaskStatus, TASK_VERSION,
};

use crate::events::{append_event, read_events};
use crate::telemetry::now_ms;
use crate::Project;

pub fn tasks_dir(project: &Project) -> PathBuf {
    project.loom_dir().join("tasks")
}

pub fn index_path(project: &Project) -> PathBuf {
    project.loom_dir().join("tasks.json")
}

pub fn task_path(project: &Project, task_id: &str) -> PathBuf {
    tasks_dir(project).join(format!("{task_id}.json"))
}

pub fn task_relative_path(task_id: &str) -> String {
    format!(".loom/tasks/{task_id}.json")
}

/// Write a TaskSpec after validating its shape. Task spec paths are immutable
/// unless `overwrite` is explicitly requested.
pub fn write_task(project: &Project, spec: &TaskSpec, overwrite: bool) -> Result<()> {
    spec.validate()
        .map_err(|message| anyhow::anyhow!("invalid task spec: {message}"))?;

    let path = task_path(project, &spec.task_id);
    if path.exists() && !overwrite {
        bail!("task spec already exists: {}", path.display());
    }
    fs::create_dir_all(tasks_dir(project)).with_context(|| "create .loom/tasks")?;
    let text = serde_json::to_string_pretty(spec)?;
    fs::write(&path, format!("{text}\n")).with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

pub fn read_task(project: &Project, task_id: &str) -> Result<TaskSpec> {
    let path = task_path(project, task_id);
    let text = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    let spec = serde_json::from_str::<TaskSpec>(&text)
        .with_context(|| format!("parse {}", path.display()))?;
    spec.validate()
        .map_err(|message| anyhow::anyhow!("validate {}: {message}", path.display()))?;
    Ok(spec)
}

/// Read all task specs in id order. `tasks.json` is a view, never the input.
pub fn read_tasks(project: &Project) -> Result<Vec<TaskSpec>> {
    let dir = tasks_dir(project);
    if !dir.exists() {
        return Ok(Vec::new());
    }

    let mut paths = Vec::new();
    for entry in fs::read_dir(&dir).with_context(|| format!("read {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) == Some("json") {
            paths.push(path);
        }
    }
    paths.sort();

    let mut tasks = Vec::new();
    for path in paths {
        let text = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
        let spec = serde_json::from_str::<TaskSpec>(&text)
            .with_context(|| format!("parse {}", path.display()))?;
        spec.validate()
            .map_err(|message| anyhow::anyhow!("validate {}: {message}", path.display()))?;
        tasks.push(spec);
    }
    tasks.sort_by(|left, right| left.task_id.cmp(&right.task_id));
    Ok(tasks)
}

/// Replay events into a task -> status map.
pub fn replay_statuses(events: &[Event]) -> HashMap<String, TaskStatus> {
    let mut statuses = HashMap::new();
    for event in events {
        let Some(task_id) = &event.task_id else {
            continue;
        };
        let current = statuses
            .get(task_id)
            .copied()
            .unwrap_or(TaskStatus::Pending);
        statuses.insert(
            task_id.clone(),
            status_after_event(current, event.event_type),
        );
    }
    statuses
}

pub fn task_statuses(project: &Project) -> Result<HashMap<String, TaskStatus>> {
    let events = read_events(project, &EventFilter::default())?;
    Ok(replay_statuses(&events))
}

pub fn task_status(project: &Project, task_id: &str) -> Result<TaskStatus> {
    let filter = EventFilter {
        task_id: Some(task_id.to_string()),
        ..EventFilter::default()
    };
    let events = read_events(project, &filter)?;
    Ok(replay_statuses(&events)
        .remove(task_id)
        .unwrap_or(TaskStatus::Pending))
}

/// Validate the graph and atomically replace the lightweight index view.
pub fn sync_index(project: &Project) -> Result<TaskIndex> {
    let tasks = read_tasks(project)?;
    validate_graph(&tasks).map_err(|message| anyhow::anyhow!("invalid task graph: {message}"))?;

    let statuses = task_statuses(project)?;
    let goal_id = tasks
        .first()
        .map(|task| task.goal_id.clone())
        .or_else(|| read_index(project).ok().map(|index| index.goal_id))
        .unwrap_or_else(|| "G-none".to_string());

    let entries = tasks
        .iter()
        .map(|spec| TaskIndexEntry {
            task_id: spec.task_id.clone(),
            title: spec.title.clone(),
            status: statuses
                .get(&spec.task_id)
                .copied()
                .unwrap_or(TaskStatus::Pending),
            tier: spec.tier,
            depends_on: spec.depends_on.clone(),
            spec: task_relative_path(&spec.task_id),
        })
        .collect();

    let index = TaskIndex {
        schema_version: TASK_VERSION.to_string(),
        goal_id,
        updated_at: rfc3339_ms(now_ms()),
        tasks: entries,
    };
    write_index(project, &index)?;
    Ok(index)
}

pub fn write_index(project: &Project, index: &TaskIndex) -> Result<()> {
    index
        .validate()
        .map_err(|message| anyhow::anyhow!("invalid tasks index: {message}"))?;
    let path = index_path(project);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| "create .loom dir")?;
    }
    let temp = path.with_extension(format!("json.tmp-{}", std::process::id()));
    let text = serde_json::to_string_pretty(index)?;
    fs::write(&temp, format!("{text}\n")).with_context(|| format!("write {}", temp.display()))?;
    fs::rename(&temp, &path).with_context(|| format!("replace {}", path.display()))?;
    Ok(())
}

pub fn read_index(project: &Project) -> Result<TaskIndex> {
    let path = index_path(project);
    let text = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    let index = serde_json::from_str::<TaskIndex>(&text)
        .with_context(|| format!("parse {}", path.display()))?;
    index
        .validate()
        .map_err(|message| anyhow::anyhow!("validate {}: {message}", path.display()))?;
    Ok(index)
}

/// Tasks whose dependencies are verified and which have not started yet.
pub fn ready_tasks(project: &Project) -> Result<Vec<TaskSpec>> {
    let tasks = read_tasks(project)?;
    validate_graph(&tasks).map_err(|message| anyhow::anyhow!("invalid task graph: {message}"))?;
    let statuses = task_statuses(project)?;
    let by_id: HashMap<&str, &TaskSpec> = tasks
        .iter()
        .map(|task| (task.task_id.as_str(), task))
        .collect();

    let mut ready = Vec::new();
    for task in &tasks {
        let status = statuses
            .get(&task.task_id)
            .copied()
            .unwrap_or(TaskStatus::Pending);
        if !matches!(status, TaskStatus::Pending | TaskStatus::Ready) {
            continue;
        }
        let deps_ok = task.depends_on.iter().all(|dep| {
            statuses
                .get(dep)
                .copied()
                .is_some_and(TaskStatus::satisfies_dependency)
        });
        if deps_ok && by_id.contains_key(task.task_id.as_str()) {
            ready.push(task.clone());
        }
    }
    Ok(ready)
}

/// Append `task.created` and refresh `tasks.json`.
pub fn register_task(project: &Project, run_id: &str, spec: &TaskSpec) -> Result<()> {
    write_task(project, spec, false)?;
    let event = Event::new(run_id, now_ms(), loom_core::EventType::TaskCreated)
        .task(&spec.task_id)
        .data([
            (
                "spec_path".to_string(),
                serde_json::json!(task_relative_path(&spec.task_id)),
            ),
            ("depends_on".to_string(), serde_json::json!(spec.depends_on)),
        ]);
    append_event(project, &event)?;
    sync_index(project)?;
    Ok(())
}

/// Mark every dependency-satisfied pending task as ready. This is deterministic
/// and safe to call from `status`, `resume`, and `dispatch`.
pub fn mark_ready_tasks(project: &Project, run_id: &str) -> Result<Vec<String>> {
    let ready = ready_tasks(project)?;
    let statuses = task_statuses(project)?;
    let mut marked = Vec::new();
    for task in ready {
        let current = statuses
            .get(&task.task_id)
            .copied()
            .unwrap_or(TaskStatus::Pending);
        if current != TaskStatus::Pending {
            continue;
        }
        let deps_done: Vec<String> = task
            .depends_on
            .iter()
            .filter(|dep| {
                statuses
                    .get(*dep)
                    .copied()
                    .is_some_and(TaskStatus::satisfies_dependency)
            })
            .cloned()
            .collect();
        let event = Event::new(run_id, now_ms(), loom_core::EventType::TaskReady)
            .task(&task.task_id)
            .data([("deps_done".to_string(), serde_json::json!(deps_done))]);
        append_event(project, &event)?;
        marked.push(task.task_id);
    }
    if !marked.is_empty() {
        sync_index(project)?;
    }
    Ok(marked)
}

/// Small helper used by status views.
pub fn missing_dependencies<M>(task: &TaskSpec, statuses: &M) -> Vec<String>
where
    M: for<'a> std::ops::Index<&'a String, Output = TaskStatus> + ?Sized,
{
    task.depends_on
        .iter()
        .filter(|dep| !statuses.index(dep).satisfies_dependency())
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use loom_core::{
        Acceptance, EventType, TaskOutput, TaskRole, TaskWorkspace, Tier, WorkspaceMode,
    };

    fn task(id: &str) -> TaskSpec {
        TaskSpec {
            schema_version: TASK_VERSION.into(),
            task_id: id.into(),
            goal_id: "G-20261104-auth".into(),
            title: "test".into(),
            role: TaskRole::Implement,
            tier: Tier::Worker,
            workspace: TaskWorkspace {
                mode: WorkspaceMode::InPlace,
                base_ref: "main".into(),
                branch: "agent/test".into(),
                allowed_paths: vec!["src/**".into()],
                forbidden_paths: vec![],
            },
            acceptance: vec![Acceptance {
                id: "A1".into(),
                cmd: "true".into(),
                expect_exit: 0,
                timeout_s: 30,
            }],
            output: TaskOutput {
                report_path: ".loom/reports/test.md".into(),
                ..TaskOutput::default()
            },
            ..TaskSpec::default()
        }
    }

    #[test]
    fn writes_reads_and_indexes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let project = Project::at(dir.path());
        let spec = task("T-20261104-001");
        register_task(&project, "R-1", &spec).expect("register");

        assert!(task_path(&project, &spec.task_id).exists());
        assert_eq!(
            read_task(&project, &spec.task_id).expect("read").task_id,
            spec.task_id
        );
        let index = read_index(&project).expect("index");
        assert_eq!(index.tasks.len(), 1);
        assert_eq!(index.tasks[0].status, TaskStatus::Pending);

        assert!(write_task(&project, &spec, false).is_err());
        write_task(&project, &spec, true).expect("overwrite");
    }

    #[test]
    fn rejects_duplicate_ids_and_cycles() {
        let dir = tempfile::tempdir().expect("tempdir");
        let project = Project::at(dir.path());
        let one = task("T-20261104-001");
        write_task(&project, &one, false).expect("write");
        assert!(write_task(&project, &one, false).is_err());

        let mut two = task("T-20261104-002");
        two.depends_on = vec![one.task_id.clone()];
        write_task(&project, &two, false).expect("write");
        sync_index(&project).expect("graph ok");

        let mut cycle = one;
        cycle.depends_on = vec![two.task_id.clone()];
        write_task(&project, &cycle, true).expect("write");
        assert!(sync_index(&project).is_err());
    }

    #[test]
    fn replays_statuses_from_events() {
        let dir = tempfile::tempdir().expect("tempdir");
        let project = Project::at(dir.path());
        append_event(
            &project,
            &Event::new("R-1", 1, EventType::TaskCreated).task("T-1"),
        )
        .expect("append");
        append_event(
            &project,
            &Event::new("R-1", 2, EventType::TaskReady).task("T-1"),
        )
        .expect("append");
        append_event(
            &project,
            &Event::new("R-1", 3, EventType::TaskPassed).task("T-1"),
        )
        .expect("append");

        assert_eq!(
            task_status(&project, "T-1").expect("status"),
            TaskStatus::Passed
        );
    }

    #[test]
    fn ready_requires_verified_dependencies() {
        let dir = tempfile::tempdir().expect("tempdir");
        let project = Project::at(dir.path());
        let one = task("T-20261104-001");
        let mut two = task("T-20261104-002");
        two.depends_on = vec![one.task_id.clone()];
        write_task(&project, &one, false).expect("write");
        write_task(&project, &two, false).expect("write");

        let ready = ready_tasks(&project).expect("ready");
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].task_id, one.task_id);

        for (ts, event_type) in [(1, EventType::TaskCreated), (2, EventType::TaskPassed)] {
            append_event(
                &project,
                &Event::new("R-1", ts, event_type).task(&one.task_id),
            )
            .expect("append");
        }
        let ready = ready_tasks(&project).expect("ready");
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].task_id, two.task_id);
    }
}
