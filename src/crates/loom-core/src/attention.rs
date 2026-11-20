//! Attention inbox: pure types and the event reducer behind `.loom/inbox.json`.
//!
//! Spec: `doc/15` §2.2. The reducer is the single place that decides what a
//! human sees; task events keep deciding task state — the two dimensions are
//! never mixed (`doc/15` §2.2 linkage table). Like the rest of `loom-core`
//! this module does no I/O: `loom-store` feeds it events and writes the view.
//!
//! Item text comes from agents and is untrusted input: everything that lands
//! in an item passes through the `sanitize_*` helpers here (control characters
//! stripped, lengths bounded), so a hostile `title` can never inject ANSI or
//! a newline into the footer or a statusline.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::event::{Event, EventType};
use crate::text::truncate_chars;
use crate::time::{compact_utc, epoch_days, rfc3339_ms};

pub const INBOX_VERSION: u8 = 1;
/// `doc/15` §2.2 constraints.
pub const TITLE_MAX: usize = 120;
pub const DETAIL_MAX: usize = 2048;
pub const REFS_MAX: usize = 16;
pub const REF_MAX: usize = 512;
pub const ACTIONS_MAX: usize = 8;
const SINGLE_LINE_MAX: usize = 240;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttentionKind {
    Question,
    Approval,
    Operation,
    Review,
    Reminder,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttentionSource {
    Task,
    Ci,
    Skills,
    Env,
    Git,
    Human,
}

/// `blocking` work stopped; `soon` handle before the next commit/dispatch;
/// `info` full list only (`doc/15` §2.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Info,
    Soon,
    Blocking,
}

impl Severity {
    /// Footer/display rank: blocking first, then soon, then info.
    pub fn rank(self) -> u8 {
        match self {
            Self::Blocking => 0,
            Self::Soon => 1,
            Self::Info => 2,
        }
    }
}

/// `acked` means seen, not closed; `deferred` is derived from `defer_until`
/// rather than stored, so the persisted states stay four (`doc/15` §2.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemStatus {
    Open,
    Acked,
    Resolved,
    Expired,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttentionAction {
    pub id: String,
    pub label: String,
    /// Suggested command, displayed as text only — never executed, never
    /// shell-evaluated (`doc/15` §2.2 Security).
    pub cmd: String,
}

/// `changes` payload of `attention.updated`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AttentionChanges {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub severity: Option<Severity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub defer_until: Option<String>,
    /// Absolute value, not a delta: the writer knows the current count from
    /// the view, and an absolute set replays idempotently.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub occurrences: Option<u32>,
    /// Only `acked` is a legal status transition through `attention.updated`;
    /// closing goes through `attention.resolved` / `attention.expired`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<ItemStatus>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttentionItem {
    pub id: String,
    pub kind: AttentionKind,
    pub source: AttentionSource,
    pub severity: Severity,
    pub status: ItemStatus,
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub detail: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub refs: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    pub dedupe_key: String,
    pub occurrences: u32,
    pub created_at: String,
    pub last_seen_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub defer_until: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closed_at: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<AttentionAction>,
}

impl AttentionItem {
    /// Closed items are neither actionable nor counted.
    pub fn is_open(&self) -> bool {
        matches!(self.status, ItemStatus::Open | ItemStatus::Acked)
    }

    /// Pending = open and not deferred into the future. `acked` stays pending:
    /// reading an item must never make it disappear from the counts.
    pub fn is_pending(&self, now_ms: u64) -> bool {
        if !self.is_open() {
            return false;
        }
        match &self.defer_until {
            Some(until) => !is_future_stamp(until, now_ms),
            None => true,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InboxCounts {
    pub blocking: u32,
    pub soon: u32,
    pub info: u32,
}

/// events.jsonl fingerprint so the footer can skip replaying when nothing
/// changed (`doc/16`: the corner must not full-replay on every command).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventsFingerprint {
    pub events_size: u64,
    pub events_mtime_ms: u64,
}

/// The `.loom/inbox.json` view. A view, never an input: regenerate from
/// events, do not hand-edit (`doc/15` §2.2).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InboxFile {
    pub v: u8,
    pub generated_at: String,
    pub counts: InboxCounts,
    pub items: Vec<AttentionItem>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<EventsFingerprint>,
}

impl InboxFile {
    pub fn validate(&self) -> Result<(), String> {
        if self.v != INBOX_VERSION {
            return Err(format!(
                "unsupported inbox version: {} (expected {INBOX_VERSION})",
                self.v
            ));
        }
        let mut keys = std::collections::HashSet::new();
        for item in &self.items {
            if item.id.trim().is_empty() {
                return Err("inbox item id must not be empty".into());
            }
            if item.title.chars().count() > TITLE_MAX {
                return Err(format!(
                    "inbox item {} title exceeds {TITLE_MAX} chars",
                    item.id
                ));
            }
            if item.detail.len() > DETAIL_MAX {
                return Err(format!(
                    "inbox item {} detail exceeds {DETAIL_MAX} bytes",
                    item.id
                ));
            }
            if item.is_open() && !keys.insert(item.dedupe_key.as_str()) {
                // Open dedupe keys are unique by construction; a duplicate
                // means the reducer or a writer has a bug.
                return Err(format!("duplicate open dedupe_key: {}", item.dedupe_key));
            }
        }
        Ok(())
    }
}

/// Knobs the reducer needs from `[attention]` config; passed in so `loom-core`
/// stays config- and I/O-free.
#[derive(Debug, Clone, Copy)]
pub struct ReduceOpts {
    pub now_ms: u64,
    /// Resolved/expired items stay in the view this many days (`doc/15` §2.2).
    pub retain_days: i64,
}

/// Replay attention events into an inbox view. Event order is the log order;
/// unknown or malformed payloads degrade the view, never the replay (the same
/// doctrine as `loom top`'s lenient readers).
pub fn reduce_inbox(events: &[Event], opts: &ReduceOpts) -> InboxFile {
    let mut items: Vec<AttentionItem> = Vec::new();

    for event in events {
        let ts_ms = event.ts_ms;
        let ts = rfc3339_ms(ts_ms);
        match event.event_type {
            EventType::AttentionOpened => {
                let Some(item_id) = str_field(&event.data, "item_id") else {
                    continue;
                };
                let (Some(kind), Some(source), Some(severity)) = (
                    json_field::<AttentionKind>(&event.data, "kind"),
                    json_field::<AttentionSource>(&event.data, "source"),
                    json_field::<Severity>(&event.data, "severity"),
                ) else {
                    continue;
                };
                let Some(dedupe_key) = str_field(&event.data, "dedupe_key") else {
                    continue;
                };
                let Some(title) = str_field(&event.data, "title") else {
                    continue;
                };

                // Dedupe among still-open items: a repeated occurrence only
                // accumulates, it never floods the list (`doc/15` §2.2).
                if let Some(existing) = items
                    .iter_mut()
                    .find(|i| i.is_open() && (i.dedupe_key == dedupe_key || i.id == item_id))
                {
                    existing.occurrences = existing.occurrences.saturating_add(1);
                    existing.last_seen_at = ts;
                    continue;
                }

                items.push(AttentionItem {
                    id: item_id.to_string(),
                    kind,
                    source,
                    severity,
                    status: ItemStatus::Open,
                    title: sanitize_title(title),
                    detail: str_field(&event.data, "detail")
                        .map(sanitize_detail)
                        .unwrap_or_default(),
                    refs: sanitize_refs(&event.data),
                    task_id: event.task_id.clone(),
                    run_id: Some(event.run_id.clone()),
                    dedupe_key: dedupe_key.to_string(),
                    occurrences: 1,
                    created_at: ts.clone(),
                    last_seen_at: ts,
                    defer_until: str_field(&event.data, "defer_until").map(sanitize_stamp),
                    expires_at: str_field(&event.data, "expires_at").map(sanitize_stamp),
                    closed_at: None,
                    actions: sanitize_actions(&event.data),
                });
            }
            EventType::AttentionUpdated => {
                let (Some(item_id), Some(changes)) = (
                    str_field(&event.data, "item_id"),
                    event
                        .data
                        .get("changes")
                        .and_then(|c| serde_json::from_value::<AttentionChanges>(c.clone()).ok()),
                ) else {
                    continue;
                };
                let Some(item) = items.iter_mut().find(|i| i.id == item_id && i.is_open()) else {
                    continue;
                };
                if let Some(severity) = changes.severity {
                    item.severity = severity;
                }
                if let Some(detail) = changes.detail {
                    item.detail = sanitize_detail(&detail);
                }
                if let Some(until) = changes.defer_until {
                    item.defer_until = Some(sanitize_stamp(&until));
                }
                if let Some(occurrences) = changes.occurrences {
                    if occurrences >= 1 {
                        item.occurrences = occurrences;
                    }
                }
                if let Some(status) = changes.status {
                    // Only `acked` may arrive through the generic update path.
                    if status == ItemStatus::Acked && item.status == ItemStatus::Open {
                        item.status = ItemStatus::Acked;
                    }
                }
                item.last_seen_at = ts;
            }
            EventType::AttentionResolved => {
                let Some(item_id) = str_field(&event.data, "item_id") else {
                    continue;
                };
                if let Some(item) = items.iter_mut().find(|i| i.id == item_id && i.is_open()) {
                    close_item(item, ItemStatus::Resolved, ts);
                }
            }
            EventType::AttentionExpired => {
                let Some(item_id) = str_field(&event.data, "item_id") else {
                    continue;
                };
                // An irreversible `blocking` item may not silently expire
                // (`doc/15` §2.2): the expiry event is ignored for everything
                // except reminders.
                let Some(item) = items.iter_mut().find(|i| i.id == item_id && i.is_open()) else {
                    continue;
                };
                if item.severity == Severity::Blocking && item.kind != AttentionKind::Reminder {
                    continue;
                }
                close_item(item, ItemStatus::Expired, ts);
            }
            _ => {}
        }
    }

    // Retention: closed items leave the view after `retain_days`; events are
    // kept forever, so the next replay recomputes this from scratch.
    let now_days = epoch_days(opts.now_ms);
    items.retain(|item| match item.status {
        ItemStatus::Open | ItemStatus::Acked => true,
        ItemStatus::Resolved | ItemStatus::Expired => item
            .closed_at
            .as_deref()
            .and_then(parse_epoch_days)
            .is_some_and(|closed| now_days - closed <= opts.retain_days),
    });

    // Display order lives in the view so every front end agrees:
    // blocking → soon → info, then most recently updated first.
    items.sort_by(|a, b| {
        a.severity
            .rank()
            .cmp(&b.severity.rank())
            .then(b.last_seen_at.cmp(&a.last_seen_at))
    });

    let mut counts = InboxCounts::default();
    for item in items.iter().filter(|i| i.is_pending(opts.now_ms)) {
        match item.severity {
            Severity::Blocking => counts.blocking += 1,
            Severity::Soon => counts.soon += 1,
            Severity::Info => counts.info += 1,
        }
    }

    InboxFile {
        v: INBOX_VERSION,
        generated_at: rfc3339_ms(opts.now_ms),
        counts,
        items,
        source: None,
    }
}

/// Next `ATT-YYYYMMDD-NNN` id, sequenced per UTC day across the log
/// (`doc/15` §2.2 data model).
pub fn next_attention_id(events: &[Event], now_ms: u64) -> String {
    let date = &compact_utc(now_ms)[..8];
    let prefix = format!("ATT-{date}-");
    let last = events
        .iter()
        .filter(|e| e.event_type == EventType::AttentionOpened)
        .filter_map(|e| str_field(&e.data, "item_id"))
        .filter_map(|id| id.strip_prefix(&prefix))
        .filter_map(|seq| seq.parse::<u32>().ok())
        .max()
        .unwrap_or(0);
    format!("ATT-{date}-{:03}", last + 1)
}

/// Strip control characters (including ANSI ESC) and bound to `TITLE_MAX`.
pub fn sanitize_title(input: &str) -> String {
    let cleaned: String = input.chars().filter(|c| !c.is_control()).collect();
    truncate_chars(&cleaned, TITLE_MAX).0.to_string()
}

/// Bound to `DETAIL_MAX`; newlines survive (the detail is a JSON field and a
/// pretty-printed block), every other control character does not.
pub fn sanitize_detail(input: &str) -> String {
    let cleaned: String = input
        .chars()
        .filter(|c| *c == '\n' || !c.is_control())
        .collect();
    truncate_chars(&cleaned, DETAIL_MAX).0.to_string()
}

fn sanitize_stamp(input: &str) -> String {
    let cleaned: String = input.chars().filter(|c| !c.is_control()).collect();
    truncate_chars(&cleaned, SINGLE_LINE_MAX).0.to_string()
}

fn sanitize_refs(data: &Map) -> Vec<String> {
    let Some(list) = data.get("refs").and_then(Value::as_array) else {
        return Vec::new();
    };
    list.iter()
        .filter_map(Value::as_str)
        .map(|r| {
            let cleaned: String = r.chars().filter(|c| !c.is_control()).collect();
            truncate_chars(&cleaned, REF_MAX).0.to_string()
        })
        .filter(|r| !r.trim().is_empty())
        .take(REFS_MAX)
        .collect()
}

fn sanitize_actions(data: &Map) -> Vec<AttentionAction> {
    let Some(list) = data.get("actions").and_then(Value::as_array) else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|a| {
            let obj = a.as_object()?;
            Some(AttentionAction {
                id: sanitize_line(obj.get("id")?.as_str()?),
                label: sanitize_line(obj.get("label")?.as_str()?),
                cmd: sanitize_line(obj.get("cmd")?.as_str()?),
            })
        })
        .filter(|a| !a.id.is_empty() && !a.cmd.is_empty())
        .take(ACTIONS_MAX)
        .collect()
}

fn sanitize_line(input: &str) -> String {
    let cleaned: String = input.chars().filter(|c| !c.is_control()).collect();
    truncate_chars(&cleaned, SINGLE_LINE_MAX).0.to_string()
}

fn close_item(item: &mut AttentionItem, status: ItemStatus, ts: String) {
    // Retention is applied once, after the loop, from `closed_at` — closing
    // here only records the state so old replays cannot resurrect the item.
    item.status = status;
    item.closed_at = Some(ts);
}

type Map = serde_json::Map<String, Value>;

fn str_field<'a>(data: &'a Map, key: &str) -> Option<&'a str> {
    data.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
}

fn json_field<T: serde::de::DeserializeOwned>(data: &Map, key: &str) -> Option<T> {
    serde_json::from_value(data.get(key)?.clone()).ok()
}

/// RFC 3339 stamps compare lexicographically; this only needs a coarse
/// "is `stamp` after now" for deferral.
fn is_future_stamp(stamp: &str, now_ms: u64) -> bool {
    stamp > rfc3339_ms(now_ms).as_str()
}

/// Parse the date part of an RFC 3339 stamp back to epoch days for retention.
fn parse_epoch_days(stamp: &str) -> Option<i64> {
    let date = stamp.get(..10)?;
    let mut parts = date.split('-');
    let year: i64 = parts.next()?.parse().ok()?;
    let month: i64 = parts.next()?.parse().ok()?;
    let day: i64 = parts.next()?.parse().ok()?;
    Some(crate::time::days_from_civil(year, month, day))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const DAY: u64 = 86_400_000;
    // 2026-11-20T00:00:00Z
    const NOW: u64 = 1_795_180_800_000;

    fn opened(ts: u64, id: &str, key: &str, severity: &str, kind: &str) -> Event {
        Event::new("R-1", ts, EventType::AttentionOpened).data([
            ("item_id".into(), json!(id)),
            ("kind".into(), json!(kind)),
            ("source".into(), json!("task")),
            ("severity".into(), json!(severity)),
            ("dedupe_key".into(), json!(key)),
            ("title".into(), json!(format!("title for {id}"))),
        ])
    }

    fn opts(now_ms: u64) -> ReduceOpts {
        ReduceOpts {
            now_ms,
            retain_days: 7,
        }
    }

    #[test]
    fn opens_counts_and_sorts_blocking_first() {
        let events = vec![
            opened(NOW, "ATT-20261120-001", "k1", "info", "reminder"),
            opened(NOW, "ATT-20261120-002", "k2", "blocking", "question"),
            opened(NOW, "ATT-20261120-003", "k3", "soon", "review"),
        ];
        let inbox = reduce_inbox(&events, &opts(NOW));
        assert_eq!(
            inbox.counts,
            InboxCounts {
                blocking: 1,
                soon: 1,
                info: 1
            }
        );
        assert_eq!(inbox.items[0].id, "ATT-20261120-002");
        assert_eq!(inbox.items[2].id, "ATT-20261120-001");
        inbox.validate().unwrap();
    }

    #[test]
    fn open_duplicate_dedupe_key_only_accumulates() {
        let events = vec![
            opened(
                NOW,
                "ATT-20261120-001",
                "task:T-1:question:Q1",
                "blocking",
                "question",
            ),
            opened(
                NOW + 1000,
                "ATT-20261120-002",
                "task:T-1:question:Q1",
                "blocking",
                "question",
            ),
        ];
        let inbox = reduce_inbox(&events, &opts(NOW));
        assert_eq!(inbox.items.len(), 1);
        assert_eq!(inbox.items[0].occurrences, 2);
        assert_eq!(inbox.counts.blocking, 1);
    }

    #[test]
    fn ack_keeps_the_item_counted() {
        let events = vec![
            opened(NOW, "ATT-20261120-001", "k1", "blocking", "question"),
            Event::new("R-1", NOW + 1, EventType::AttentionUpdated).data([
                ("item_id".into(), json!("ATT-20261120-001")),
                ("changes".into(), json!({"status": "acked"})),
            ]),
        ];
        let inbox = reduce_inbox(&events, &opts(NOW));
        assert_eq!(inbox.items[0].status, ItemStatus::Acked);
        assert_eq!(inbox.counts.blocking, 1, "acked must not lower the count");
    }

    #[test]
    fn resolved_item_stays_within_retention_and_leaves_after() {
        let events = vec![
            opened(NOW, "ATT-20261120-001", "k1", "soon", "question"),
            Event::new("R-1", NOW + 2, EventType::AttentionResolved).data([
                ("item_id".into(), json!("ATT-20261120-001")),
                ("by".into(), json!("human")),
                ("reason".into(), json!("answered")),
            ]),
        ];
        let inbox = reduce_inbox(&events, &opts(NOW + DAY));
        assert_eq!(inbox.items[0].status, ItemStatus::Resolved);
        assert_eq!(inbox.counts.soon, 0, "closed items are not counted");

        let after = reduce_inbox(&events, &opts(NOW + 8 * DAY));
        assert!(after.items.is_empty(), "retention is 7 days");
    }

    #[test]
    fn blocking_items_never_silently_expire() {
        let events = vec![
            opened(NOW, "ATT-20261120-001", "k1", "blocking", "question"),
            opened(NOW, "ATT-20261120-002", "k2", "info", "reminder"),
            Event::new("R-1", NOW + 1, EventType::AttentionExpired).data([
                ("item_id".into(), json!("ATT-20261120-001")),
                ("policy".into(), json!("ttl")),
            ]),
            Event::new("R-1", NOW + 1, EventType::AttentionExpired).data([
                ("item_id".into(), json!("ATT-20261120-002")),
                ("policy".into(), json!("ttl")),
            ]),
        ];
        let inbox = reduce_inbox(&events, &opts(NOW));
        assert_eq!(inbox.items[0].status, ItemStatus::Open, "blocking survives");
        assert_eq!(inbox.items[1].status, ItemStatus::Expired);
    }

    #[test]
    fn deferred_soon_items_leave_the_counts_until_the_defer_passes() {
        let events = vec![
            opened(NOW, "ATT-20261120-001", "k1", "soon", "review"),
            Event::new("R-1", NOW + 1, EventType::AttentionUpdated).data([
                ("item_id".into(), json!("ATT-20261120-001")),
                (
                    "changes".into(),
                    json!({"defer_until": rfc3339_ms(NOW + DAY)}),
                ),
            ]),
        ];
        let inbox = reduce_inbox(&events, &opts(NOW));
        assert_eq!(inbox.counts.soon, 0, "deferred out of the counts");
        assert!(inbox.items[0].is_open());

        let later = reduce_inbox(&events, &opts(NOW + 2 * DAY));
        assert_eq!(later.counts.soon, 1, "defer expired, back in the counts");
    }

    #[test]
    fn sanitizes_untrusted_title_and_detail() {
        let hostile = format!(
            "\u{1b}[31mred\u{1b}[0m\n{}\u{7}x",
            "t".repeat(TITLE_MAX + 50)
        );
        let events = vec![Event::new("R-1", NOW, EventType::AttentionOpened).data([
            ("item_id".into(), json!("ATT-20261120-001")),
            ("kind".into(), json!("operation")),
            ("source".into(), json!("env")),
            ("severity".into(), json!("blocking")),
            ("dedupe_key".into(), json!("env:missing-tool:x")),
            ("title".into(), json!(hostile)),
            ("detail".into(), json!("line1\u{1b}[2Jline2")),
        ])];
        let inbox = reduce_inbox(&events, &opts(NOW));
        let item = &inbox.items[0];
        assert!(!item.title.contains('\u{1b}'), "ANSI stripped");
        assert!(!item.title.contains('\u{7}'), "BEL stripped");
        assert!(!item.title.contains('\n'), "title is one line");
        assert!(item.title.chars().count() <= TITLE_MAX);
        assert_eq!(
            item.detail, "line1[2Jline2",
            "detail keeps newlines, strips ESC"
        );
        inbox.validate().unwrap();
    }

    #[test]
    fn next_id_sequences_per_utc_day() {
        let day1 = NOW;
        let day2 = NOW + DAY;
        let events = vec![
            opened(day1, "ATT-20261120-001", "a", "info", "reminder"),
            opened(day1, "ATT-20261120-002", "b", "info", "reminder"),
        ];
        assert_eq!(next_attention_id(&events, day1), "ATT-20261120-003");
        assert_eq!(next_attention_id(&events, day2), "ATT-20261121-001");
        assert_eq!(next_attention_id(&[], day1), "ATT-20261120-001");
    }

    #[test]
    fn attention_events_never_change_task_state() {
        use crate::task::{status_after_event, TaskStatus};
        for event_type in [
            EventType::AttentionOpened,
            EventType::AttentionUpdated,
            EventType::AttentionResolved,
            EventType::AttentionExpired,
        ] {
            assert_eq!(
                status_after_event(TaskStatus::Running, event_type),
                TaskStatus::Running
            );
        }
    }
}
