//! Deterministic role/tier routing.
//!
//! Routing is deliberately simple: a role may map to a tier, a task tier is the
//! fallback, and a worker is escalated only after real verify failures. No LLM
//! ever decides that a task "looks hard".

use std::collections::{BTreeMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::{find_model, ModelCapability, TaskRole, Tier};

/// Project-level routing policy. All fields are optional; defaults keep the
/// previous behavior (dispatch the task's own tier).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RoutingConfig {
    /// Consecutive task failures before a worker is escalated to frontier.
    pub escalate_after_failures: u32,
    /// Maximum `task.escalated` events per task.
    pub max_escalations_per_task: u32,
    /// Optional role -> tier overrides. Roles not listed keep the task tier.
    pub roles: BTreeMap<String, Tier>,
    /// Fallback aliases per tier, tried after the primary model.
    pub fallbacks: BTreeMap<String, Vec<String>>,
    /// Hard cost ceiling per tier, in USD per task.
    pub cost_limits: BTreeMap<String, f64>,
}

impl Default for RoutingConfig {
    fn default() -> Self {
        Self {
            escalate_after_failures: 2,
            max_escalations_per_task: 1,
            roles: BTreeMap::new(),
            fallbacks: BTreeMap::new(),
            cost_limits: BTreeMap::new(),
        }
    }
}

impl RoutingConfig {
    pub fn validate(&self, models: &[ModelCapability]) -> Result<(), String> {
        if self.escalate_after_failures == 0 {
            return Err("routing.escalate_after_failures must be > 0".to_string());
        }
        for role in self.roles.keys() {
            role.parse::<TaskRole>()
                .map_err(|message| format!("routing.roles.{role}: {message}"))?;
        }
        for (tier, aliases) in &self.fallbacks {
            let key = tier
                .parse::<Tier>()
                .map_err(|message| format!("routing.fallbacks.{tier}: {message}"))?;
            if aliases.is_empty() {
                return Err(format!("routing.fallbacks.{tier} must not be empty"));
            }
            let mut seen = HashSet::new();
            for alias in aliases {
                let model = find_model(models, alias).ok_or_else(|| {
                    format!("routing.fallbacks.{tier}: unknown model alias {alias}")
                })?;
                if model.tier != key {
                    return Err(format!(
                        "routing.fallbacks.{tier}: {alias} is a {} model; fallbacks stay in-tier, cross-tier changes must go through escalation",
                        model.tier.as_str()
                    ));
                }
                if !model.tool_loop_safe() {
                    return Err(format!(
                        "routing.fallbacks.{tier}: {alias} is not tool-loop-safe"
                    ));
                }
                if !seen.insert(alias.as_str()) {
                    return Err(format!("routing.fallbacks.{tier}: duplicate alias {alias}"));
                }
            }
        }
        for (tier, limit) in &self.cost_limits {
            tier.parse::<Tier>()
                .map_err(|message| format!("routing.cost_limits.{tier}: {message}"))?;
            if !limit.is_finite() || *limit < 0.0 {
                return Err(format!(
                    "routing.cost_limits.{tier} must be finite and >= 0"
                ));
            }
        }
        Ok(())
    }

    pub fn role_tier(&self, role: TaskRole) -> Option<Tier> {
        self.roles.get(role.as_str()).copied()
    }

    pub fn fallbacks_for(&self, tier: Tier) -> Vec<String> {
        self.fallbacks
            .get(tier.as_str())
            .cloned()
            .unwrap_or_default()
    }

    pub fn cost_limit(&self, tier: Tier) -> Option<f64> {
        self.cost_limits.get(tier.as_str()).copied()
    }

    /// Resolve one route deterministically.
    pub fn plan(
        &self,
        models: &[ModelCapability],
        input: &RouteInput<'_>,
    ) -> Result<RouteDecision, String> {
        let mut warnings = Vec::new();
        let mut issues = Vec::new();

        if let Some(alias) = input.explicit_model {
            let model =
                find_model(models, alias).ok_or_else(|| format!("unknown model alias: {alias}"))?;
            if !model.tool_loop_safe() {
                issues.push(format!(
                    "{alias}: forced thinking requires reasoning_passthrough=required"
                ));
            }
            return Ok(self.decision(
                input,
                model.tier,
                Some(model),
                RouteReason::ExplicitModel,
                false,
                warnings,
                issues,
            ));
        }

        let requested_tier = self.role_tier(input.role).unwrap_or(input.task_tier);
        let mut tier = requested_tier;
        let mut reason = if self.role_tier(input.role).is_some() {
            RouteReason::RoleTier
        } else {
            RouteReason::TaskTier
        };
        let mut escalated = false;

        if requested_tier == Tier::Worker && input.failures >= self.escalate_after_failures {
            if input.escalations >= self.max_escalations_per_task {
                warnings.push(format!(
                    "worker failed {} time(s) but escalation budget is exhausted",
                    input.failures
                ));
            } else if select_model(models, Tier::Frontier).is_some() {
                tier = Tier::Frontier;
                reason = RouteReason::Escalated;
                escalated = true;
            } else {
                warnings.push(format!(
                    "worker failed {} time(s) but no frontier model is configured",
                    input.failures
                ));
            }
        }

        let selected = select_model(models, tier)
            .or_else(|| {
                if tier != input.task_tier {
                    select_model(models, input.task_tier)
                } else {
                    None
                }
            })
            .or_else(|| models.iter().find(|model| model.tool_loop_safe()));

        let selected = match selected {
            Some(model) => {
                if model.tier != tier {
                    warnings.push(format!(
                        "no {} model available; fell back to {} ({})",
                        tier.as_str(),
                        model.tier.as_str(),
                        model.alias
                    ));
                    reason = RouteReason::TierFallback;
                }
                Some(model)
            }
            None if models.is_empty() => {
                warnings.push(
                    "no models configured; events will record the tier name only".to_string(),
                );
                reason = RouteReason::NoModelConfigured;
                None
            }
            None => {
                return Err(
                    "no configured model is tool-loop-safe; run `loom capability check`"
                        .to_string(),
                );
            }
        };

        Ok(self.decision(input, tier, selected, reason, escalated, warnings, issues))
    }

    #[allow(clippy::too_many_arguments)]
    fn decision(
        &self,
        input: &RouteInput<'_>,
        tier: Tier,
        model: Option<&ModelCapability>,
        reason: RouteReason,
        escalated: bool,
        mut warnings: Vec<String>,
        mut issues: Vec<String>,
    ) -> RouteDecision {
        let alias = model
            .map(|model| model.alias.clone())
            .unwrap_or_else(|| tier.as_str().to_string());
        let model_id = model
            .and_then(|model| model.model_id.clone())
            .unwrap_or_default();
        let fallbacks = self
            .fallbacks_for(tier)
            .into_iter()
            .filter(|fallback| fallback != &alias)
            .collect();
        let tier_cost_limit_usd = self.cost_limit(tier);
        // An explicit `--model` pins one alias: failover would silently
        // override the operator's choice.
        let fallback_enabled = input.explicit_model.is_none();

        if let Some(limit) = tier_cost_limit_usd {
            if input.budget_usd > limit {
                issues.push(format!(
                    "task budget ${:.4} exceeds {} cost limit ${:.4}; lower budget.max_cost_usd or raise routing.cost_limits.{}",
                    input.budget_usd,
                    tier.as_str(),
                    limit,
                    tier.as_str()
                ));
            }
        }

        let estimated_cost_usd = model.and_then(|model| {
            model.estimate_cost(input.max_input_tokens, 0, input.max_output_tokens)
        });
        if let Some(estimated) = estimated_cost_usd {
            let cap = input
                .budget_usd
                .min(tier_cost_limit_usd.unwrap_or(f64::INFINITY));
            if estimated > cap {
                issues.push(format!(
                    "worst-case estimated cost ${estimated:.4} exceeds cap ${cap:.4}"
                ));
            }
        } else if model.is_some() {
            warnings.push(
                "prices are incomplete; cost cannot be estimated before dispatch".to_string(),
            );
        }

        RouteDecision {
            role: input.role,
            requested_tier: input.task_tier,
            tier,
            alias,
            model_id,
            reason,
            escalated,
            fallbacks,
            fallback_enabled,
            budget_usd: input.budget_usd,
            tier_cost_limit_usd,
            estimated_cost_usd,
            warnings,
            issues,
        }
    }
}

fn select_model(models: &[ModelCapability], tier: Tier) -> Option<&ModelCapability> {
    models
        .iter()
        .find(|model| model.tier == tier && model.tool_loop_safe())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RouteReason {
    ExplicitModel,
    TaskTier,
    RoleTier,
    Escalated,
    TierFallback,
    NoModelConfigured,
}

impl RouteReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ExplicitModel => "explicit_model",
            Self::TaskTier => "task_tier",
            Self::RoleTier => "role_tier",
            Self::Escalated => "escalated",
            Self::TierFallback => "tier_fallback",
            Self::NoModelConfigured => "no_model_configured",
        }
    }
}

#[derive(Debug, Clone)]
pub struct RouteInput<'a> {
    pub role: TaskRole,
    /// Tier declared by the TaskSpec; used as the fallback when the role has no
    /// explicit mapping.
    pub task_tier: Tier,
    pub failures: u32,
    pub escalations: u32,
    pub explicit_model: Option<&'a str>,
    pub budget_usd: f64,
    pub max_input_tokens: u64,
    pub max_output_tokens: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteDecision {
    pub role: TaskRole,
    pub requested_tier: Tier,
    pub tier: Tier,
    pub alias: String,
    pub model_id: String,
    pub reason: RouteReason,
    pub escalated: bool,
    pub fallbacks: Vec<String>,
    /// `false` when the operator pinned `--model`; dispatch then never fails
    /// over to `fallbacks`.
    #[serde(default)]
    pub fallback_enabled: bool,
    pub budget_usd: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tier_cost_limit_usd: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub estimated_cost_usd: Option<f64>,
    pub warnings: Vec<String>,
    pub issues: Vec<String>,
}

impl RouteDecision {
    pub fn is_dispatchable(&self) -> bool {
        self.issues.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ModelCapability, TaskRole, Tier};

    fn models() -> Vec<ModelCapability> {
        vec![
            ModelCapability {
                alias: "worker".into(),
                model_id: Some("deepseek-v4-flash".into()),
                tier: Tier::Worker,
                input_usd_per_mtok: Some(0.27),
                output_usd_per_mtok: Some(1.10),
                ..ModelCapability::new("worker", Tier::Worker)
            },
            ModelCapability {
                alias: "frontier".into(),
                model_id: Some("gpt-6.1-sol".into()),
                tier: Tier::Frontier,
                input_usd_per_mtok: Some(2.50),
                output_usd_per_mtok: Some(10.0),
                ..ModelCapability::new("frontier", Tier::Frontier)
            },
        ]
    }

    fn input<'a>(role: TaskRole, task_tier: Tier, explicit: Option<&'a str>) -> RouteInput<'a> {
        RouteInput {
            role,
            task_tier,
            failures: 0,
            escalations: 0,
            explicit_model: explicit,
            budget_usd: 0.25,
            max_input_tokens: 120_000,
            max_output_tokens: 20_000,
        }
    }

    #[test]
    fn role_mapping_overrides_task_tier_when_configured() {
        let mut routing = RoutingConfig::default();
        routing.roles.insert("review".into(), Tier::Frontier);
        let decision = routing
            .plan(&models(), &input(TaskRole::Review, Tier::Worker, None))
            .expect("route");
        assert_eq!(decision.tier, Tier::Frontier);
        assert_eq!(decision.alias, "frontier");
        assert_eq!(decision.reason, RouteReason::RoleTier);
        assert!(!decision.escalated);
    }

    #[test]
    fn task_tier_is_used_without_role_mapping() {
        let decision = RoutingConfig::default()
            .plan(&models(), &input(TaskRole::Implement, Tier::Worker, None))
            .expect("route");
        assert_eq!(decision.tier, Tier::Worker);
        assert_eq!(decision.alias, "worker");
        assert_eq!(decision.reason, RouteReason::TaskTier);
    }

    #[test]
    fn escalates_worker_after_failures() {
        let routing = RoutingConfig {
            escalate_after_failures: 2,
            ..RoutingConfig::default()
        };
        let mut route_input = input(TaskRole::Implement, Tier::Worker, None);
        route_input.failures = 2;

        let decision = routing.plan(&models(), &route_input).expect("route");
        assert_eq!(decision.tier, Tier::Frontier);
        assert_eq!(decision.alias, "frontier");
        assert!(decision.escalated);
        assert_eq!(decision.reason, RouteReason::Escalated);
    }

    #[test]
    fn stops_escalating_after_the_limit() {
        let routing = RoutingConfig {
            escalate_after_failures: 1,
            max_escalations_per_task: 1,
            ..RoutingConfig::default()
        };
        let mut route_input = input(TaskRole::Implement, Tier::Worker, None);
        route_input.failures = 3;
        route_input.escalations = 1;

        let decision = routing.plan(&models(), &route_input).expect("route");
        assert_eq!(decision.tier, Tier::Worker);
        assert!(!decision.escalated);
        assert!(decision
            .warnings
            .iter()
            .any(|warning| warning.contains("escalation budget")));
    }

    #[test]
    fn explicit_model_wins() {
        let decision = RoutingConfig::default()
            .plan(
                &models(),
                &input(TaskRole::Review, Tier::Worker, Some("worker")),
            )
            .expect("route");
        assert_eq!(decision.alias, "worker");
        assert_eq!(decision.reason, RouteReason::ExplicitModel);
    }

    #[test]
    fn unknown_explicit_model_fails() {
        let error = RoutingConfig::default()
            .plan(
                &models(),
                &input(TaskRole::Review, Tier::Worker, Some("missing")),
            )
            .expect_err("unknown alias");
        assert!(error.contains("unknown model alias"));
    }

    #[test]
    fn cost_guard_rejects_over_budget() {
        let mut routing = RoutingConfig::default();
        routing.cost_limits.insert("worker".into(), 0.01);
        let decision = routing
            .plan(&models(), &input(TaskRole::Implement, Tier::Worker, None))
            .expect("route");
        assert!(!decision.is_dispatchable());
        assert!(decision
            .issues
            .iter()
            .any(|issue| issue.contains("exceeds worker cost limit")));
    }

    #[test]
    fn validates_routing_keys_and_aliases() {
        let mut routing = RoutingConfig::default();
        routing.roles.insert("nope".into(), Tier::Worker);
        assert!(routing.validate(&models()).is_err());

        let mut routing = RoutingConfig::default();
        routing
            .fallbacks
            .insert("worker".into(), vec!["missing".into()]);
        assert!(routing.validate(&models()).is_err());

        let mut routing = RoutingConfig::default();
        routing.cost_limits.insert("worker".into(), -1.0);
        assert!(routing.validate(&models()).is_err());
    }

    #[test]
    fn rejects_cross_tier_and_duplicate_fallbacks() {
        let mut routing = RoutingConfig::default();
        routing
            .fallbacks
            .insert("worker".into(), vec!["frontier".into()]);
        let error = routing.validate(&models()).expect_err("cross-tier");
        assert!(error.contains("cross-tier"), "{error}");

        let mut routing = RoutingConfig::default();
        routing
            .fallbacks
            .insert("worker".into(), vec!["worker".into(), "worker".into()]);
        let error = routing.validate(&models()).expect_err("duplicate");
        assert!(error.contains("duplicate alias worker"), "{error}");

        let mut routing = RoutingConfig::default();
        routing.fallbacks.insert("worker".into(), Vec::new());
        assert!(routing.validate(&models()).is_err());
    }

    #[test]
    fn explicit_model_pinning_disables_failover() {
        let mut routing = RoutingConfig::default();
        routing
            .fallbacks
            .insert("worker".into(), vec!["worker-b".into(), "worker-c".into()]);
        let mut model_list = models();
        model_list.push(ModelCapability::new("worker-b", Tier::Worker));
        model_list.push(ModelCapability::new("worker-c", Tier::Worker));

        let pinned = routing
            .plan(
                &model_list,
                &input(TaskRole::Implement, Tier::Worker, Some("worker-b")),
            )
            .expect("route");
        assert_eq!(pinned.alias, "worker-b");
        assert!(!pinned.fallback_enabled);
        // The list still shows what is configured; the flag says dispatch will
        // not use it while the alias is pinned.
        assert_eq!(pinned.fallbacks, ["worker-c"]);

        let routed = routing
            .plan(&model_list, &input(TaskRole::Implement, Tier::Worker, None))
            .expect("route");
        assert!(routed.fallback_enabled);
        assert_eq!(routed.fallbacks, ["worker-b", "worker-c"]);
    }
}
