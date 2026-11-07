use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

use anyhow::{Context, Result};
use loom_core::{Event, EventFilter};

use crate::Project;

/// Append-only event log. This is the single fact source for task state.
pub fn events_path(project: &Project) -> PathBuf {
    project.loom_dir().join("events.jsonl")
}

pub fn append_event(project: &Project, event: &Event) -> Result<()> {
    event
        .validate()
        .map_err(|message| anyhow::anyhow!("invalid event: {message}"))?;

    let path = events_path(project);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| "create .loom dir")?;
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("open {}", path.display()))?;
    let mut line = serde_json::to_string(event)?;
    line.push('\n');
    // One syscall per record: concurrent appenders must never interleave the
    // halves of two JSONL lines (`doc/05` §6, no lost updates).
    file.write_all(line.as_bytes())
        .with_context(|| "append event")?;
    Ok(())
}

/// Read the whole log and filter in memory. Malformed lines fail loudly: state
/// reconstruction must never silently skip a corrupted transition.
pub fn read_events(project: &Project, filter: &EventFilter) -> Result<Vec<Event>> {
    let path = events_path(project);
    if !path.exists() {
        return Ok(Vec::new());
    }

    let text = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    let mut events = Vec::new();
    for (index, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let event = serde_json::from_str::<Event>(line).with_context(|| {
            format!("parse {} line {}", path.display(), index.saturating_add(1))
        })?;
        event.validate().map_err(|message| {
            anyhow::anyhow!(
                "validate {} line {}: {message}",
                path.display(),
                index.saturating_add(1)
            )
        })?;
        if filter.matches(&event) {
            events.push(event);
        }
    }
    Ok(events)
}

#[cfg(test)]
mod tests {
    use super::*;
    use loom_core::{EventType, RECEIPT_VERSION};
    use serde_json::json;

    fn event(task: &str, event_type: EventType) -> Event {
        Event::new("R-1", 1, event_type)
            .task(task)
            .data([("schema_version".to_string(), json!(RECEIPT_VERSION))])
    }

    #[test]
    fn missing_log_is_empty() {
        let dir = tempfile::tempdir().expect("tempdir");
        let project = Project::at(dir.path());
        assert!(read_events(&project, &EventFilter::default())
            .expect("read")
            .is_empty());
    }

    #[test]
    fn appends_and_filters() {
        let dir = tempfile::tempdir().expect("tempdir");
        let project = Project::at(dir.path());
        append_event(&project, &event("T-1", EventType::TaskCreated)).expect("append");
        append_event(&project, &event("T-1", EventType::TaskReady)).expect("append");
        append_event(&project, &event("T-2", EventType::TaskCreated)).expect("append");

        let all = read_events(&project, &EventFilter::default()).expect("read");
        assert_eq!(all.len(), 3);

        let filter = EventFilter {
            task_id: Some("T-1".into()),
            ..EventFilter::default()
        };
        assert_eq!(read_events(&project, &filter).expect("read").len(), 2);

        let filter = EventFilter {
            event_type: Some(EventType::TaskReady),
            ..EventFilter::default()
        };
        let ready = read_events(&project, &filter).expect("read");
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].task_id.as_deref(), Some("T-1"));
    }

    #[test]
    fn rejects_invalid_events() {
        let dir = tempfile::tempdir().expect("tempdir");
        let project = Project::at(dir.path());
        let mut bad = event("T-1", EventType::TaskStarted);
        bad.run_id.clear();
        assert!(append_event(&project, &bad).is_err());
    }

    #[test]
    fn malformed_line_fails_loudly() {
        let dir = tempfile::tempdir().expect("tempdir");
        let project = Project::at(dir.path());
        let path = events_path(&project);
        fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        fs::write(&path, "{not json}\n").expect("write");
        assert!(read_events(&project, &EventFilter::default()).is_err());
    }
}
