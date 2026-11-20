//! `.loom/inbox.json` — the resident attention view (`doc/15` §2.2).
//!
//! View, never input: it is regenerated from `events.jsonl` and must not be
//! hand-edited. The events fingerprint lets a caller reuse a fresh view
//! instead of replaying the log on every command (`doc/16`: the corner must
//! not full-replay per invocation).

use std::fs;
use std::path::PathBuf;
use std::time::UNIX_EPOCH;

use anyhow::{bail, Context, Result};
use loom_core::{
    next_attention_id, parse_rfc3339_ms, reduce_inbox, rfc3339_ms, AttentionAction,
    AttentionChanges, AttentionKind, AttentionSource, Event, EventFilter, EventType,
    EventsFingerprint, InboxFile, ReduceOpts, Severity,
};
use serde_json::{json, Map, Value};

use crate::events::{append_event, events_path, read_events};
use crate::telemetry::now_ms;
use crate::Project;

pub fn inbox_path(project: &Project) -> PathBuf {
    project.loom_dir().join("inbox.json")
}

fn fingerprint(project: &Project) -> Option<EventsFingerprint> {
    // A missing log is a real state (size 0, no mtime): the first append
    // then changes the fingerprint and forces the first replay.
    let Ok(meta) = fs::metadata(events_path(project)) else {
        return Some(EventsFingerprint {
            events_size: 0,
            events_mtime_ms: 0,
        });
    };
    let mtime_ms = meta
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_millis() as u64;
    Some(EventsFingerprint {
        events_size: meta.len(),
        events_mtime_ms: mtime_ms,
    })
}

/// Regenerate `.loom/inbox.json` from the event log, unless the log is
/// unchanged since the last sync (then the existing view is returned as-is).
pub fn sync_inbox(project: &Project) -> Result<InboxFile> {
    let fp = fingerprint(project);
    if let (Some(fp), Ok(existing)) = (&fp, read_inbox(project)) {
        if existing.source.as_ref() == Some(fp) {
            return Ok(existing);
        }
    }

    let config = project.load_config().unwrap_or_default();
    let events = read_events(project, &EventFilter::default())?;
    let mut inbox = reduce_inbox(
        &events,
        &ReduceOpts {
            now_ms: now_ms(),
            retain_days: config.attention.retain_days,
        },
    );
    if events.is_empty() && !inbox_path(project).exists() {
        // A reader on a virgin directory must not materialize `.loom`
        // (`loom inbox --line` runs from shell prompts). The fingerprint is
        // still recorded so the first real event forces a replay+write.
        inbox.source = fp;
        return Ok(inbox);
    }
    inbox.source = fp;
    write_inbox(project, &inbox)?;
    Ok(inbox)
}

pub fn read_inbox(project: &Project) -> Result<InboxFile> {
    let path = inbox_path(project);
    let text = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    let inbox: InboxFile =
        serde_json::from_str(&text).with_context(|| format!("parse {}", path.display()))?;
    inbox
        .validate()
        .map_err(|message| anyhow::anyhow!("invalid inbox view: {message}"))?;
    Ok(inbox)
}

/// Lenient reader for the footer: a missing or torn view degrades to an empty
/// inbox, never to an error (same doctrine as `loom top`).
pub fn read_inbox_lenient(project: &Project) -> InboxFile {
    read_inbox(project).unwrap_or(InboxFile {
        v: loom_core::INBOX_VERSION,
        generated_at: rfc3339_ms(now_ms()),
        counts: Default::default(),
        items: Vec::new(),
        source: None,
    })
}

fn write_inbox(project: &Project, inbox: &InboxFile) -> Result<()> {
    inbox
        .validate()
        .map_err(|message| anyhow::anyhow!("invalid inbox view: {message}"))?;
    let path = inbox_path(project);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| "create .loom dir")?;
    }
    let temp = path.with_extension(format!("json.tmp-{}", std::process::id()));
    let text = serde_json::to_string_pretty(inbox)?;
    fs::write(&temp, format!("{text}\n")).with_context(|| format!("write {}", temp.display()))?;
    fs::rename(&temp, &path).with_context(|| format!("replace {}", path.display()))?;
    Ok(())
}

/// Everything a caller needs to open one inbox item.
#[derive(Debug, Clone)]
pub struct NewItem {
    pub run_id: String,
    pub task_id: Option<String>,
    pub kind: AttentionKind,
    pub source: AttentionSource,
    pub severity: Severity,
    pub dedupe_key: String,
    pub title: String,
    pub detail: Option<String>,
    pub refs: Vec<String>,
    pub actions: Vec<AttentionAction>,
}

/// Append `attention.opened` for a new problem, or fold a repeat into the
/// still-open item with the same `dedupe_key`.
///
/// Returns the item id, or `None` when the dedupe cooldown suppressed the
/// repeat entirely (`doc/15` §2.2 noise reduction). Appends only — the
/// caller refreshes the view with [`sync_inbox`] once, after its batch.
pub fn open_item(project: &Project, item: &NewItem) -> Result<Option<String>> {
    if item.dedupe_key.trim().is_empty() || item.title.trim().is_empty() {
        bail!("attention item needs a dedupe_key and a title");
    }
    let config = project.load_config().unwrap_or_default();
    // Freshen first: earlier appends in this batch are invisible
    // to a cached view, and a stale view would open duplicates.
    let view = sync_inbox(project)?;

    if let Some(existing) = view
        .items
        .iter()
        .find(|i| i.is_open() && i.dedupe_key == item.dedupe_key)
    {
        let cooldown_ms = config.attention.dedupe_cooldown_s.saturating_mul(1000);
        if cooldown_ms > 0 {
            if let Some(last_seen) = parse_rfc3339_ms(&existing.last_seen_at) {
                if now_ms().saturating_sub(last_seen) < cooldown_ms {
                    // Same problem inside the window: not counted again.
                    return Ok(None);
                }
            }
        }
        let mut changes = Map::new();
        changes.insert(
            "occurrences".to_string(),
            json!(existing.occurrences.saturating_add(1)),
        );
        let mut data = Map::new();
        data.insert("item_id".to_string(), json!(existing.id));
        data.insert("changes".to_string(), Value::Object(changes));
        let mut event = Event::new(&item.run_id, now_ms(), EventType::AttentionUpdated).data(data);
        if let Some(task_id) = &item.task_id {
            event = event.task(task_id);
        }
        append_event(project, &event)?;
        return Ok(Some(existing.id.clone()));
    }

    let events = read_events(project, &EventFilter::default())?;
    let item_id = next_attention_id(&events, now_ms());

    let mut data = Map::new();
    data.insert("item_id".to_string(), json!(item_id));
    data.insert("kind".to_string(), json!(item.kind));
    data.insert("source".to_string(), json!(item.source));
    data.insert("severity".to_string(), json!(item.severity));
    data.insert("dedupe_key".to_string(), json!(item.dedupe_key));
    data.insert("title".to_string(), json!(item.title));
    if let Some(detail) = &item.detail {
        data.insert("detail".to_string(), json!(detail));
    }
    if !item.refs.is_empty() {
        data.insert("refs".to_string(), json!(item.refs));
    }
    if !item.actions.is_empty() {
        data.insert("actions".to_string(), json!(item.actions));
    }
    let mut event = Event::new(&item.run_id, now_ms(), EventType::AttentionOpened).data(data);
    if let Some(task_id) = &item.task_id {
        event = event.task(task_id);
    }
    append_event(project, &event)?;
    Ok(Some(item_id))
}

/// Append `attention.updated` for an open item. Refuses to defer a blocking
/// item: "handle it later" must never swallow work that has stopped.
pub fn update_item(
    project: &Project,
    run_id: &str,
    item_id: &str,
    changes: AttentionChanges,
) -> Result<bool> {
    // Freshen first: earlier appends in this batch are invisible
    // to a cached view, and a stale view would open duplicates.
    let view = sync_inbox(project)?;
    let Some(existing) = view.items.iter().find(|i| i.id == item_id) else {
        return Ok(false);
    };
    if !existing.is_open() {
        return Ok(false);
    }
    if changes.defer_until.is_some() && existing.severity == Severity::Blocking {
        bail!("blocking item {item_id} cannot be deferred");
    }
    let mut data = Map::new();
    data.insert("item_id".to_string(), json!(item_id));
    data.insert("changes".to_string(), serde_json::to_value(&changes)?);
    let mut event = Event::new(run_id, now_ms(), EventType::AttentionUpdated).data(data);
    if let Some(task_id) = &existing.task_id {
        event = event.task(task_id);
    }
    append_event(project, &event)?;
    Ok(true)
}

/// Append `attention.resolved` for an open item by id.
pub fn resolve_item(
    project: &Project,
    run_id: &str,
    item_id: &str,
    by: &str,
    reason: &str,
) -> Result<bool> {
    // Freshen first: earlier appends in this batch are invisible
    // to a cached view, and a stale view would open duplicates.
    let view = sync_inbox(project)?;
    let Some(existing) = view.items.iter().find(|i| i.id == item_id) else {
        return Ok(false);
    };
    if !existing.is_open() {
        return Ok(false);
    }
    let mut data = Map::new();
    data.insert("item_id".to_string(), json!(item_id));
    data.insert("by".to_string(), json!(by));
    data.insert("reason".to_string(), json!(reason));
    let mut event = Event::new(run_id, now_ms(), EventType::AttentionResolved).data(data);
    if let Some(task_id) = &existing.task_id {
        event = event.task(task_id);
    }
    append_event(project, &event)?;
    Ok(true)
}

/// Resolve every open `question` item of `task_id` whose `question_id` was
/// just answered. Appends only; the caller syncs once after the batch.
pub fn resolve_question_items(
    project: &Project,
    run_id: &str,
    task_id: &str,
    question_ids: &[String],
    by: &str,
) -> Result<u32> {
    // Freshen first: earlier appends in this batch are invisible
    // to a cached view, and a stale view would open duplicates.
    let view = sync_inbox(project)?;
    let mut resolved = 0;
    for question_id in question_ids {
        let dedupe_key = format!("task:{task_id}:question:{question_id}");
        let Some(existing) = view
            .items
            .iter()
            .find(|i| i.is_open() && i.dedupe_key == dedupe_key)
        else {
            continue;
        };
        let item_id = existing.id.clone();
        if resolve_item(project, run_id, &item_id, by, "answered")? {
            resolved += 1;
        }
    }
    Ok(resolved)
}

#[cfg(test)]
mod tests {
    use super::*;
    use loom_core::{AttentionKind, AttentionSource, EventType, Severity};

    fn project() -> (tempfile::TempDir, Project) {
        let dir = tempfile::tempdir().expect("tempdir");
        let project = Project::at(dir.path());
        (dir, project)
    }

    fn open(question: &str, key: &str, severity: Severity) -> NewItem {
        NewItem {
            run_id: "R-1".to_string(),
            task_id: Some("T-1".to_string()),
            kind: AttentionKind::Question,
            source: AttentionSource::Task,
            severity,
            dedupe_key: key.to_string(),
            title: question.to_string(),
            detail: None,
            refs: Vec::new(),
            actions: Vec::new(),
        }
    }

    #[test]
    fn sync_writes_view_and_fingerprint_skips_replay() {
        let (_dir, project) = project();
        let inbox = sync_inbox(&project).expect("sync");
        assert_eq!(inbox.counts.blocking, 0);
        assert!(
            inbox.source.is_some(),
            "view records its events fingerprint"
        );

        append_event(
            &project,
            &Event::new("R-1", now_ms(), EventType::AttentionOpened).data([
                ("item_id".to_string(), json!("ATT-20261120-001")),
                ("kind".to_string(), json!("operation")),
                ("source".to_string(), json!("env")),
                ("severity".to_string(), json!("blocking")),
                ("dedupe_key".to_string(), json!("env:missing-tool:x")),
                ("title".to_string(), json!("x is missing")),
            ]),
        )
        .expect("append");
        let inbox = sync_inbox(&project).expect("sync");
        assert_eq!(inbox.counts.blocking, 1);
        assert!(inbox_path(&project).exists());
        inbox.validate().expect("valid view");
    }

    #[test]
    fn open_item_folds_repeats_and_honors_cooldown() {
        let (_dir, project) = project();
        // Default cooldown (600s) suppresses the immediate repeat.
        let first = open_item(
            &project,
            &open("Q1?", "task:T-1:question:Q1", Severity::Blocking),
        )
        .expect("open")
        .expect("id");
        let repeat = open_item(
            &project,
            &open("Q1?", "task:T-1:question:Q1", Severity::Blocking),
        )
        .expect("open");
        assert_eq!(repeat, None, "cooldown suppresses the repeat");

        // Cooldown off: the repeat folds into the open item.
        fs::write(
            project.loom_dir().join("config.toml"),
            "[attention]\ndedupe_cooldown_s = 0\n",
        )
        .expect("write config");
        let folded = open_item(
            &project,
            &open("Q1?", "task:T-1:question:Q1", Severity::Blocking),
        )
        .expect("open")
        .expect("id");
        assert_eq!(folded, first, "same identity, never a second item");
        let inbox = sync_inbox(&project).expect("sync");
        assert_eq!(inbox.items.len(), 1);
        assert_eq!(inbox.items[0].occurrences, 2);
        assert_eq!(inbox.counts.blocking, 1);
    }

    #[test]
    fn resolve_question_items_closes_only_answered_keys() {
        let (_dir, project) = project();
        open_item(
            &project,
            &open("Q1?", "task:T-1:question:Q1", Severity::Blocking),
        )
        .expect("open");
        open_item(
            &project,
            &open("Q2?", "task:T-1:question:Q2", Severity::Soon),
        )
        .expect("open");
        open_item(
            &project,
            &open("other", "task:T-9:question:Q1", Severity::Blocking),
        )
        .expect("open");
        sync_inbox(&project).expect("sync");

        let resolved = resolve_question_items(
            &project,
            "R-1",
            "T-1",
            &["Q1".to_string(), "Q2".to_string()],
            "human",
        )
        .expect("resolve");
        assert_eq!(resolved, 2);

        let inbox = sync_inbox(&project).expect("sync");
        assert_eq!(inbox.counts.blocking, 1, "the other task's item stays open");
        assert_eq!(inbox.items.iter().filter(|i| i.is_open()).count(), 1);
    }

    #[test]
    fn update_item_refuses_to_defer_blocking() {
        let (_dir, project) = project();
        let id = open_item(&project, &open("Q1?", "k", Severity::Blocking))
            .expect("open")
            .expect("id");
        sync_inbox(&project).expect("sync");

        let defer = AttentionChanges {
            defer_until: Some(rfc3339_ms(now_ms() + 60_000)),
            ..Default::default()
        };
        assert!(update_item(&project, "R-1", &id, defer).is_err());

        let ack = AttentionChanges {
            status: Some(loom_core::ItemStatus::Acked),
            ..Default::default()
        };
        assert!(update_item(&project, "R-1", &id, ack).expect("ack"));
        let inbox = sync_inbox(&project).expect("sync");
        assert_eq!(inbox.items[0].status, loom_core::ItemStatus::Acked);
        assert_eq!(inbox.counts.blocking, 1, "acked still counts");
    }

    #[test]
    fn resolve_item_is_idempotent_by_state() {
        let (_dir, project) = project();
        let id = open_item(&project, &open("Q1?", "k", Severity::Soon))
            .expect("open")
            .expect("id");
        assert!(resolve_item(&project, "R-1", &id, "human", "done").expect("first"));
        assert!(
            !resolve_item(&project, "R-1", &id, "human", "again").expect("second"),
            "a closed item never re-closes"
        );
        assert!(!resolve_item(&project, "R-1", "ATT-none", "human", "x").expect("missing"));
        let inbox = sync_inbox(&project).expect("sync");
        assert_eq!(inbox.counts.soon, 0);
    }
}
