//! `loom inbox`: the resident attention list (`doc/15` §2.2).
//!
//! The read outlets serve the `.loom/inbox.json` view (refreshing it only
//! when `events.jsonl` moved — a virgin directory stays virgin). The mutating
//! subcommands append `attention.*` events and refresh the view; `acked` never
//! closes an item, `defer` refuses `blocking`, and `resolve` always states a
//! reason. `actions[].cmd` values are displayed as text only.

use anyhow::{bail, Result};
use loom_core::{
    parse_rfc3339_ms, rfc3339_ms, AttentionChanges, AttentionKind, InboxFile, ItemStatus,
};
use loom_store::{now_ms, resolve_item, sync_inbox, update_item, Project};

use super::{Format, InboxCommand};

#[derive(Debug, Default)]
pub struct InboxOpts {
    /// One statusline-ready line instead of the list.
    pub line: bool,
    /// Exit 2 when any blocking item exists (scripts/hooks).
    pub check: bool,
    /// Only this kind: question|approval|operation|review|reminder.
    pub kind: Option<String>,
    /// Only items of this task.
    pub task: Option<String>,
    /// Include resolved/expired items still inside the retention window.
    pub all: bool,
    /// Alias for `--format json`.
    pub json: bool,
}

pub fn inbox(
    project: &Project,
    command: Option<InboxCommand>,
    opts: InboxOpts,
    format: Format,
) -> Result<i32> {
    let format = if opts.json { Format::Json } else { format };

    if let Some(command) = command {
        return mutate(project, command, format);
    }

    // Self-healing read: refresh only when the log changed since the view.
    let view = sync_inbox(project)?;
    let view = filtered(view, &opts)?;

    if opts.check {
        // Pure exit-code probe: silent, so hooks capture nothing.
        return Ok(if view.counts.blocking > 0 { 2 } else { 0 });
    }
    if opts.line {
        println!("{}", render_line(&view));
        return Ok(0);
    }
    match format {
        Format::Json => println!("{}", to_json(&view)?),
        Format::Pretty => print!("{}", render(&view)),
    }
    Ok(0)
}

fn mutate(project: &Project, command: InboxCommand, format: Format) -> Result<i32> {
    let view = sync_inbox(project)?;
    let (item_id, action) = match &command {
        InboxCommand::Ack { item_id } => (item_id.clone(), "ack"),
        InboxCommand::Defer { item_id, .. } => (item_id.clone(), "defer"),
        InboxCommand::Resolve { item_id, .. } => (item_id.clone(), "resolve"),
    };
    let item = view
        .items
        .iter()
        .find(|i| i.id == item_id)
        .ok_or_else(|| anyhow::anyhow!("inbox item not found: {item_id}"))?;
    if !item.is_open() {
        bail!("inbox item {item_id} is already {}", item.status_str());
    }
    // The event belongs to the thread that raised the item; project-level
    // items without a run still need a non-empty envelope run_id.
    let run_id = item.run_id.clone().unwrap_or_else(|| "inbox".to_string());

    match command {
        InboxCommand::Ack { .. } => {
            if item.status == ItemStatus::Acked {
                // Seen is seen: a second ack would append a no-op event.
                bail!("inbox item {item_id} is already acked");
            }
            let changes = AttentionChanges {
                status: Some(ItemStatus::Acked),
                ..Default::default()
            };
            if !update_item(project, &run_id, &item_id, changes)? {
                bail!("inbox item {item_id} is not open anymore");
            }
        }
        InboxCommand::Defer { until, .. } => {
            // Default: an hour later. An explicit `--until` must be a stamp
            // this code can compare, or the defer would silently never end.
            let until = until.unwrap_or_else(|| rfc3339_ms(now_ms() + 3_600_000));
            if parse_rfc3339_ms(&until).is_none() {
                bail!("--until must be RFC 3339 UTC, e.g. 2026-11-21T09:00:00.000Z");
            }
            let changes = AttentionChanges {
                defer_until: Some(until.clone()),
                ..Default::default()
            };
            // `update_item` refuses `blocking`: handle-later must not swallow
            // work that has stopped.
            if !update_item(project, &run_id, &item_id, changes)? {
                bail!("inbox item {item_id} is not open anymore");
            }
            if matches!(format, Format::Pretty) {
                println!("deferred {item_id} until {until}");
            } else {
                println!(
                    "{}",
                    serde_json::json!({"ok": true, "action": "defer", "item_id": item_id, "until": until})
                );
            }
            return Ok(0);
        }
        InboxCommand::Resolve { reason, by, .. } => {
            if !resolve_item(project, &run_id, &item_id, &by, &reason)? {
                bail!("inbox item {item_id} is not open anymore");
            }
        }
    }

    let view = sync_inbox(project)?;
    let past = if action == "ack" { "acked" } else { "resolved" };
    match format {
        Format::Json => println!(
            "{}",
            serde_json::json!({
                "ok": true,
                "action": action,
                "item_id": item_id,
                "counts": view.counts,
            })
        ),
        Format::Pretty => println!(
            "{past} {item_id} · blocking {} · pending {}",
            view.counts.blocking, view.counts.soon
        ),
    }
    Ok(0)
}

/// Apply `--kind`/`--task`/`--all` and recompute the counts on what is left,
/// so every outlet reports the numbers it actually shows.
fn filtered(view: InboxFile, opts: &InboxOpts) -> Result<InboxFile> {
    let kind = match opts.kind.as_deref() {
        None => None,
        Some("question") => Some(AttentionKind::Question),
        Some("approval") => Some(AttentionKind::Approval),
        Some("operation") => Some(AttentionKind::Operation),
        Some("review") => Some(AttentionKind::Review),
        Some("reminder") => Some(AttentionKind::Reminder),
        Some(other) => {
            bail!("unknown --kind {other} (question|approval|operation|review|reminder)")
        }
    };
    // No early return: even with no flags the closed-item exclusion applies —
    // the default list is actionable items only, `--all` opts back in.
    let mut kept = Vec::new();
    for item in view.items {
        if !opts.all && !item.is_open() {
            continue;
        }
        if let Some(kind) = kind {
            if item.kind != kind {
                continue;
            }
        }
        if let Some(task) = &opts.task {
            if item.task_id.as_deref() != Some(task.as_str()) {
                continue;
            }
        }
        kept.push(item);
    }

    let mut counts = loom_core::InboxCounts::default();
    let now_ms_stamp = view.generated_at.as_str();
    for item in kept.iter().filter(|i| i.is_open()) {
        let deferred = item
            .defer_until
            .as_deref()
            .is_some_and(|until| until > now_ms_stamp);
        if deferred {
            continue;
        }
        match item.severity {
            loom_core::Severity::Blocking => counts.blocking += 1,
            loom_core::Severity::Soon => counts.soon += 1,
            loom_core::Severity::Info => counts.info += 1,
        }
    }

    Ok(InboxFile {
        v: view.v,
        generated_at: view.generated_at,
        counts,
        items: kept,
        source: view.source,
    })
}

/// `[!] 1 blocking · 2 pending` — one line for shell prompts and statuslines.
pub fn render_line(view: &InboxFile) -> String {
    let marker = if view.counts.blocking > 0 {
        "[!]"
    } else {
        "[ ]"
    };
    format!(
        "{marker} {} blocking · {} pending",
        view.counts.blocking, view.counts.soon
    )
}

pub fn to_json(view: &InboxFile) -> Result<String> {
    Ok(serde_json::to_string_pretty(view)?)
}

/// The full list: `blocking → soon → info`, then most recently updated —
/// the view already carries that order, this only renders it.
pub fn render(view: &InboxFile) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "loom inbox · blocking {} · pending {} · info {}\n",
        view.counts.blocking, view.counts.soon, view.counts.info
    ));
    for item in &view.items {
        let mut markers = String::new();
        if item.occurrences > 1 {
            markers.push_str(&format!(" ×{}", item.occurrences));
        }
        match item.status {
            ItemStatus::Open => {}
            ItemStatus::Acked => markers.push_str(" · acked"),
            ItemStatus::Resolved => markers.push_str(" · resolved"),
            ItemStatus::Expired => markers.push_str(" · expired"),
        }
        let deferred = item
            .defer_until
            .as_deref()
            .is_some_and(|until| until > view.generated_at.as_str());
        if deferred {
            markers.push_str(" · deferred");
        }
        let task = item
            .task_id
            .as_deref()
            .map(|t| format!(" {t}"))
            .unwrap_or_default();
        out.push_str(&format!(
            "[{}] {} {}{}{}\n",
            severity_str(item.severity),
            item.id,
            item.kind_str(),
            task,
            markers
        ));
        out.push_str(&format!("  {}\n", item.title));
        if !item.detail.is_empty() {
            let first_line = item.detail.lines().next().unwrap_or("");
            out.push_str(&format!("  {first_line}\n"));
        }
        if !item.refs.is_empty() {
            out.push_str(&format!("  refs: {}\n", item.refs.join(", ")));
        }
        for action in &item.actions {
            out.push_str(&format!("  → {}\n", action.cmd));
        }
    }
    out
}

fn severity_str(severity: loom_core::Severity) -> &'static str {
    match severity {
        loom_core::Severity::Blocking => "blocking",
        loom_core::Severity::Soon => "soon",
        loom_core::Severity::Info => "info",
    }
}

trait ItemView {
    fn kind_str(&self) -> &'static str;
    fn status_str(&self) -> &'static str;
}

impl ItemView for loom_core::AttentionItem {
    fn kind_str(&self) -> &'static str {
        match self.kind {
            AttentionKind::Question => "question",
            AttentionKind::Approval => "approval",
            AttentionKind::Operation => "operation",
            AttentionKind::Review => "review",
            AttentionKind::Reminder => "reminder",
        }
    }

    fn status_str(&self) -> &'static str {
        match self.status {
            ItemStatus::Open => "open",
            ItemStatus::Acked => "acked",
            ItemStatus::Resolved => "resolved",
            ItemStatus::Expired => "expired",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use loom_core::{AttentionAction, AttentionSource, Severity};
    use loom_store::{open_item, NewItem};

    fn fixture() -> (tempfile::TempDir, Project) {
        let dir = tempfile::tempdir().expect("tempdir");
        let project = Project::at(dir.path());
        open_item(
            &project,
            &NewItem {
                run_id: "R-1".into(),
                task_id: Some("T-1".into()),
                kind: AttentionKind::Question,
                source: AttentionSource::Task,
                severity: Severity::Blocking,
                dedupe_key: "task:T-1:question:Q1".into(),
                title: "Must the migration stay compatible?".into(),
                detail: Some("the rollback plan depends on it".into()),
                refs: vec!["src/db/migrate.rs:40".into()],
                actions: vec![AttentionAction {
                    id: "answer".into(),
                    label: "Answer".into(),
                    cmd: "loom answer T-1 -q Q1".into(),
                }],
            },
        )
        .expect("open");
        open_item(
            &project,
            &NewItem {
                run_id: "R-1".into(),
                task_id: Some("T-1".into()),
                kind: AttentionKind::Review,
                source: AttentionSource::Ci,
                severity: Severity::Soon,
                dedupe_key: "ci:clippy:tree1".into(),
                title: "clippy is red on the baseline".into(),
                detail: None,
                refs: Vec::new(),
                actions: Vec::new(),
            },
        )
        .expect("open");
        sync_inbox(&project).expect("sync");
        (dir, project)
    }

    #[test]
    fn list_filters_and_recounts() {
        let (_dir, project) = fixture();
        let view = sync_inbox(&project).expect("sync");
        assert_eq!(view.counts.blocking, 1);
        assert_eq!(view.counts.soon, 1);

        let only_review = filtered(
            view.clone(),
            &InboxOpts {
                kind: Some("review".into()),
                ..Default::default()
            },
        )
        .expect("filter");
        assert_eq!(only_review.items.len(), 1);
        assert_eq!(only_review.counts.blocking, 0, "counts follow the filter");

        let wrong_task = filtered(
            view,
            &InboxOpts {
                task: Some("T-9".into()),
                ..Default::default()
            },
        )
        .expect("filter");
        assert!(wrong_task.items.is_empty());
        assert_eq!(wrong_task.counts.blocking, 0);

        assert!(filtered(
            sync_inbox(&project).expect("sync"),
            &InboxOpts {
                kind: Some("nope".into()),
                ..Default::default()
            },
        )
        .is_err());
    }

    #[test]
    fn check_probes_blocking_with_exit_two() {
        let (_dir, project) = fixture();
        let code = inbox(
            &project,
            None,
            InboxOpts {
                check: true,
                ..Default::default()
            },
            Format::Json,
        )
        .expect("check");
        assert_eq!(code, 2);

        // Resolve the blocker → probe passes.
        let view = sync_inbox(&project).expect("sync");
        let blocking = view
            .items
            .iter()
            .find(|i| i.severity == Severity::Blocking)
            .expect("blocking item");
        inbox(
            &project,
            Some(InboxCommand::Resolve {
                item_id: blocking.id.clone(),
                reason: "answered".into(),
                by: "human".into(),
            }),
            InboxOpts::default(),
            Format::Json,
        )
        .expect("resolve");
        let code = inbox(
            &project,
            None,
            InboxOpts {
                check: true,
                ..Default::default()
            },
            Format::Json,
        )
        .expect("check");
        assert_eq!(code, 0);
    }

    #[test]
    fn line_is_one_short_line() {
        let (_dir, project) = fixture();
        let view = sync_inbox(&project).expect("sync");
        let line = render_line(&view);
        assert_eq!(line, "[!] 1 blocking · 1 pending");
        assert_eq!(line.lines().count(), 1);
    }

    #[test]
    fn ack_keeps_counting_and_resolve_closes() {
        let (_dir, project) = fixture();
        let view = sync_inbox(&project).expect("sync");
        let id = view.items[0].id.clone();

        inbox(
            &project,
            Some(InboxCommand::Ack {
                item_id: id.clone(),
            }),
            InboxOpts::default(),
            Format::Json,
        )
        .expect("ack");
        let view = sync_inbox(&project).expect("sync");
        assert_eq!(view.items[0].status, ItemStatus::Acked);
        assert_eq!(view.counts.blocking, 1, "acked still counts");

        // Acking again is refused: the state machine does not loop.
        assert!(inbox(
            &project,
            Some(InboxCommand::Ack {
                item_id: id.clone()
            }),
            InboxOpts::default(),
            Format::Json,
        )
        .is_err());

        inbox(
            &project,
            Some(InboxCommand::Resolve {
                item_id: id.clone(),
                reason: "done".into(),
                by: "human".into(),
            }),
            InboxOpts::default(),
            Format::Json,
        )
        .expect("resolve");
        let view = sync_inbox(&project).expect("sync");
        assert_eq!(view.items[0].status, ItemStatus::Resolved);
        assert_eq!(view.counts.blocking, 0);

        // The resolved item vanishes from the default list (the open review
        // stays), and both remain visible under --all.
        let default_list = filtered(view.clone(), &InboxOpts::default()).expect("filter");
        assert_eq!(default_list.items.len(), 1, "only the open review remains");
        assert_eq!(default_list.items[0].severity, Severity::Soon);
        let all = filtered(
            view,
            &InboxOpts {
                all: true,
                ..Default::default()
            },
        )
        .expect("filter");
        assert_eq!(all.items.len(), 2, "resolved + open, both under --all");
    }

    #[test]
    fn defer_accepts_soon_and_refuses_blocking() {
        let (_dir, project) = fixture();
        let view = sync_inbox(&project).expect("sync");
        let blocking = view
            .items
            .iter()
            .find(|i| i.severity == Severity::Blocking)
            .expect("blocking");
        let soon = view
            .items
            .iter()
            .find(|i| i.severity == Severity::Soon)
            .expect("soon");

        let err = inbox(
            &project,
            Some(InboxCommand::Defer {
                item_id: blocking.id.clone(),
                until: None,
            }),
            InboxOpts::default(),
            Format::Json,
        );
        assert!(err.is_err(), "blocking must never be deferred");

        assert!(inbox(
            &project,
            Some(InboxCommand::Defer {
                item_id: soon.id.clone(),
                until: None,
            }),
            InboxOpts::default(),
            Format::Json,
        )
        .is_ok());
        let view = sync_inbox(&project).expect("sync");
        let soon = view.items.iter().find(|i| i.id == soon.id).expect("soon");
        assert!(soon.defer_until.is_some());
        assert_eq!(view.counts.soon, 0, "deferred leaves the counts");

        assert!(inbox(
            &project,
            Some(InboxCommand::Defer {
                item_id: soon.id.clone(),
                until: Some("tomorrow".into()),
            }),
            InboxOpts::default(),
            Format::Json,
        )
        .is_err());
    }

    #[test]
    fn unknown_item_is_an_error_and_line_renders_clean() {
        let (_dir, project) = fixture();
        assert!(inbox(
            &project,
            Some(InboxCommand::Ack {
                item_id: "ATT-none".into()
            }),
            InboxOpts::default(),
            Format::Json,
        )
        .is_err());

        let view = sync_inbox(&project).expect("sync");
        let text = render(&view);
        assert!(text.starts_with("loom inbox · blocking 1 · pending 1"));
        assert!(
            text.contains("loom answer T-1 -q Q1"),
            "action shown as text"
        );
        assert!(!text.contains('\u{1b}'), "no ANSI in the list");
        serde_json::from_str::<InboxFile>(&to_json(&view).expect("json")).expect("valid json");
    }

    #[test]
    fn virgin_directory_stays_unwritten_for_readers() {
        let dir = tempfile::tempdir().expect("tempdir");
        let project = Project::at(dir.path());
        // No .loom, no events: the read outlet must not materialize state.
        let code = inbox(&project, None, InboxOpts::default(), Format::Json).expect("read");
        assert_eq!(code, 0);
        assert!(!project.loom_dir().exists(), "a reader creates nothing");
    }
}
