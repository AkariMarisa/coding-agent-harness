use std::collections::BTreeMap;

use anyhow::{Context as _, Result};
use loom_core::{Config, EventFilter, EventType, RouteDecision, RouteInput, TaskSpec, TaskStatus};
use loom_store::{read_events, read_task, task_status, Project};
use serde_json::json;

use super::Format;

/// Explain the deterministic route for one task.
pub fn route(project: &Project, task_id: &str, model: Option<&str>, format: Format) -> Result<i32> {
    let spec = read_task(project, task_id)?;
    let status = task_status(project, task_id)?;
    let decision = plan_for_task(project, &spec, model)?;

    match format {
        Format::Json => {
            let payload = json!({
                "task_id": spec.task_id,
                "status": status.as_str(),
                "route": decision,
            });
            println!("{}", serde_json::to_string_pretty(&payload)?);
        }
        Format::Pretty => print_route(&spec, status, &decision),
    }

    Ok(if decision.is_dispatchable() { 0 } else { 1 })
}

/// Summarize the tier of every dispatch recorded in the event log.
///
/// `tasks` counts each task once using its latest dispatch, which is the number
/// the M3 exit criterion ("worker >= 50%") talks about. `dispatches` counts
/// retries as well and is useful for spotting escalation churn.
pub fn stats(project: &Project, format: Format) -> Result<i32> {
    let stats = collect_stats(project)?;

    match format {
        Format::Json => println!("{}", serde_json::to_string_pretty(&stats.as_json())?),
        Format::Pretty => {
            println!("tasks:");
            println!(
                "  total {}  worker {}  frontier {}  unknown {}",
                stats.tasks.total(),
                stats.tasks.worker,
                stats.tasks.frontier,
                stats.tasks.unknown,
            );
            println!(
                "  worker share: {}",
                format_share(stats.tasks.worker_share())
            );
            println!("dispatches:");
            println!(
                "  total {}  worker {}  frontier {}  unknown {}",
                stats.dispatches.total(),
                stats.dispatches.worker,
                stats.dispatches.frontier,
                stats.dispatches.unknown,
            );
            println!(
                "  worker share: {}",
                format_share(stats.dispatches.worker_share())
            );
            println!("escalated:    {}", stats.escalated);
            if stats.reasons.is_empty() {
                println!("reasons:      -");
            } else {
                let reasons = stats
                    .reasons
                    .iter()
                    .map(|(reason, count)| format!("{reason}={count}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                println!("reasons:      {reasons}");
            }
        }
    }

    Ok(0)
}

fn format_share(share: Option<f64>) -> String {
    share
        .map(|share| format!("{:.1}%", share * 100.0))
        .unwrap_or_else(|| "n/a".to_string())
}

#[derive(Debug, Default, Clone, Copy)]
struct TierCounts {
    worker: u64,
    frontier: u64,
    unknown: u64,
}

impl TierCounts {
    fn add(&mut self, tier: &str) {
        match tier {
            "worker" => self.worker += 1,
            "frontier" => self.frontier += 1,
            _ => self.unknown += 1,
        }
    }

    fn total(&self) -> u64 {
        self.worker + self.frontier + self.unknown
    }

    fn worker_share(&self) -> Option<f64> {
        (self.total() > 0).then(|| self.worker as f64 / self.total() as f64)
    }

    fn as_json(&self) -> serde_json::Value {
        json!({
            "total": self.total(),
            "worker": self.worker,
            "frontier": self.frontier,
            "unknown": self.unknown,
            "worker_share": self.worker_share(),
        })
    }
}

#[derive(Debug, Default)]
struct RouteStats {
    tasks: TierCounts,
    dispatches: TierCounts,
    escalated: u64,
    reasons: BTreeMap<String, u64>,
}

impl RouteStats {
    fn as_json(&self) -> serde_json::Value {
        json!({
            "tasks": self.tasks.as_json(),
            "dispatches": self.dispatches.as_json(),
            "escalated": self.escalated,
            "reasons": self.reasons,
        })
    }
}

fn collect_stats(project: &Project) -> Result<RouteStats> {
    let events = read_events(project, &EventFilter::default())
        .with_context(|| "read routing events for stats")?;
    let mut stats = RouteStats::default();
    let mut latest_tier: BTreeMap<String, String> = BTreeMap::new();

    for event in &events {
        if event.event_type != EventType::TaskDispatched {
            continue;
        }
        let route = event.data.get("route");
        let tier = route
            .and_then(|route| route.get("tier"))
            .and_then(|tier| tier.as_str())
            // Older events recorded only the task tier.
            .or_else(|| event.data.get("tier").and_then(|tier| tier.as_str()))
            .unwrap_or("unknown");
        stats.dispatches.add(tier);
        if let Some(task_id) = &event.task_id {
            latest_tier.insert(task_id.clone(), tier.to_string());
        }

        let reason = route
            .and_then(|route| route.get("reason"))
            .and_then(|reason| reason.as_str())
            .unwrap_or("unknown");
        *stats.reasons.entry(reason.to_string()).or_insert(0) += 1;

        if route
            .and_then(|route| route.get("escalated"))
            .and_then(|escalated| escalated.as_bool())
            .unwrap_or(false)
        {
            stats.escalated += 1;
        }
    }

    for tier in latest_tier.values() {
        stats.tasks.add(tier);
    }

    Ok(stats)
}

pub(crate) fn plan_for_task(
    project: &Project,
    spec: &TaskSpec,
    explicit_model: Option<&str>,
) -> Result<RouteDecision> {
    let config = project.load_config()?;
    plan_with_config(project, &config, spec, explicit_model)
}

pub(crate) fn plan_with_config(
    project: &Project,
    config: &Config,
    spec: &TaskSpec,
    explicit_model: Option<&str>,
) -> Result<RouteDecision> {
    let (failures, escalations) = history(project, &spec.task_id)?;
    let input = RouteInput {
        role: spec.role,
        task_tier: spec.tier,
        failures,
        escalations,
        explicit_model,
        budget_usd: spec.budget.max_cost_usd,
        max_input_tokens: spec.budget.max_input_tokens,
        max_output_tokens: spec.budget.max_output_tokens,
    };
    config
        .routing
        .plan(&config.models, &input)
        .map_err(|message| anyhow::anyhow!("route {}: {message}", spec.task_id))
}

fn history(project: &Project, task_id: &str) -> Result<(u32, u32)> {
    let filter = EventFilter {
        task_id: Some(task_id.to_string()),
        ..EventFilter::default()
    };
    let events = read_events(project, &filter)
        .with_context(|| format!("read routing history for {task_id}"))?;
    let failures = events
        .iter()
        .filter(|event| event.event_type == EventType::TaskFailed)
        .count() as u32;
    let escalations = events
        .iter()
        .filter(|event| event.event_type == EventType::TaskEscalated)
        .count() as u32;
    Ok((failures, escalations))
}

fn print_route(spec: &TaskSpec, status: TaskStatus, decision: &RouteDecision) {
    println!(
        "{} [{}] role={} task_tier={}",
        spec.task_id,
        status.as_str(),
        decision.role.as_str(),
        decision.requested_tier.as_str(),
    );
    println!(
        "route:    {} -> {} ({}) via {}",
        decision.requested_tier.as_str(),
        decision.tier.as_str(),
        decision.reason.as_str(),
        decision.alias,
    );
    println!(
        "model:    {} ({}){}",
        decision.alias,
        if decision.model_id.is_empty() {
            "-"
        } else {
            &decision.model_id
        },
        if decision.escalated {
            " [escalated]"
        } else {
            ""
        },
    );
    if !decision.fallbacks.is_empty() {
        println!("fallback: {}", decision.fallbacks.join(", "));
    }
    println!(
        "budget:   ${:.4} task{}",
        decision.budget_usd,
        decision
            .tier_cost_limit_usd
            .map(|limit| format!(" / ${limit:.4} {} cap", decision.tier.as_str()))
            .unwrap_or_default(),
    );
    println!(
        "estimate: {}",
        decision
            .estimated_cost_usd
            .map(|cost| format!("${cost:.4} worst case"))
            .unwrap_or_else(|| "unknown".to_string()),
    );
    for warning in &decision.warnings {
        println!("  warn:  {warning}");
    }
    for issue in &decision.issues {
        println!("  issue: {issue}");
    }
    if !decision.is_dispatchable() {
        println!("dispatch would be rejected; fix the issues above");
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use loom_core::{TaskRole, Tier, TASK_VERSION};
    use loom_store::Project;

    use super::*;

    fn spec() -> TaskSpec {
        TaskSpec {
            schema_version: TASK_VERSION.into(),
            task_id: "T-20261106-001".into(),
            goal_id: "G-20261106-routing".into(),
            title: "route test".into(),
            role: TaskRole::Implement,
            tier: Tier::Worker,
            depends_on: Vec::new(),
            workspace: loom_core::TaskWorkspace {
                mode: loom_core::WorkspaceMode::InPlace,
                base_ref: "main".into(),
                branch: "agent/T-20261106-001".into(),
                allowed_paths: vec!["src/**".into()],
                forbidden_paths: vec![],
            },
            inputs: vec![],
            constraints: loom_core::TaskConstraints::default(),
            acceptance: vec![loom_core::Acceptance {
                id: "A1".into(),
                cmd: "true".into(),
                timeout_s: 5,
                expect_exit: 0,
            }],
            budget: loom_core::TaskBudget::default(),
            output: loom_core::TaskOutput::default(),
            context_policy: None,
            notes: None,
        }
    }

    #[test]
    fn history_counts_failures_and_escalations() {
        let dir = tempfile::tempdir().expect("tempdir");
        let project = Project::at(dir.path());
        fs::create_dir_all(project.loom_dir()).expect("loom dir");
        for event_type in [
            EventType::TaskFailed,
            EventType::TaskFailed,
            EventType::TaskEscalated,
        ] {
            let event = loom_core::Event::new("R-1", 1, event_type)
                .task("T-20261106-001")
                .data([("code".to_string(), json!("verify_fail"))]);
            loom_store::append_event(&project, &event).expect("append");
        }

        assert_eq!(
            history(&project, "T-20261106-001").expect("history"),
            (2, 1)
        );
    }

    #[test]
    fn plan_for_task_uses_configured_role_mapping() {
        let dir = tempfile::tempdir().expect("tempdir");
        let project = Project::at(dir.path());
        fs::create_dir_all(project.loom_dir()).expect("loom dir");
        fs::write(
            project.config_path(),
            r#"
[[models]]
alias = "worker"
tier = "worker"
model_id = "deepseek-v4-flash"

[[models]]
alias = "frontier"
tier = "frontier"
model_id = "gpt-6.1-sol"

[routing.roles]
review = "frontier"
"#,
        )
        .expect("config");

        let mut task = spec();
        task.role = TaskRole::Review;
        let decision = plan_for_task(&project, &task, None).expect("route");
        assert_eq!(decision.tier, Tier::Frontier);
        assert_eq!(decision.alias, "frontier");
        assert_eq!(decision.reason, loom_core::RouteReason::RoleTier);
    }

    #[test]
    fn stats_use_latest_tier_per_task() {
        let dir = tempfile::tempdir().expect("tempdir");
        let project = Project::at(dir.path());
        fs::create_dir_all(project.loom_dir()).expect("loom dir");
        for (task_id, tier, reason, escalated) in [
            ("T-1", "worker", "task_tier", false),
            ("T-1", "frontier", "escalated", true),
            ("T-2", "worker", "task_tier", false),
        ] {
            let event = loom_core::Event::new("R-1", 1, EventType::TaskDispatched)
                .task(task_id)
                .data([(
                    "route".to_string(),
                    json!({
                        "tier": tier,
                        "reason": reason,
                        "escalated": escalated,
                    }),
                )]);
            loom_store::append_event(&project, &event).expect("append");
        }

        let stats = collect_stats(&project).expect("stats");
        assert_eq!(stats.tasks.total(), 2);
        assert_eq!(stats.tasks.worker, 1);
        assert_eq!(stats.tasks.frontier, 1);
        assert_eq!(stats.tasks.unknown, 0);
        assert_eq!(stats.tasks.worker_share(), Some(0.5));

        // Retries count as dispatches, not as extra tasks.
        assert_eq!(stats.dispatches.total(), 3);
        assert_eq!(stats.dispatches.worker, 2);
        assert_eq!(stats.dispatches.frontier, 1);
        assert_eq!(stats.escalated, 1);
        assert_eq!(stats.reasons.get("task_tier"), Some(&2));
        assert_eq!(stats.reasons.get("escalated"), Some(&1));
    }

    #[test]
    fn stats_fall_back_to_legacy_tier_field() {
        let dir = tempfile::tempdir().expect("tempdir");
        let project = Project::at(dir.path());
        fs::create_dir_all(project.loom_dir()).expect("loom dir");
        let event = loom_core::Event::new("R-1", 1, EventType::TaskDispatched)
            .task("T-1")
            .data([("tier".to_string(), json!("frontier"))]);
        loom_store::append_event(&project, &event).expect("append");

        let stats = collect_stats(&project).expect("stats");
        assert_eq!(stats.tasks.frontier, 1);
        assert_eq!(stats.dispatches.frontier, 1);
        assert_eq!(stats.reasons.get("unknown"), Some(&1));
    }
}
