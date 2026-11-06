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
}
