use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::{ModelCapability, Tier, Wire};

pub const USAGE_VERSION: u8 = 1;

/// Where a usage row came from. Provider billing wins over local estimates.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UsageSource {
    Provider,
    Litellm,
    Runtime,
    Headroom,
    Manual,
    #[default]
    Unknown,
}

impl UsageSource {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Provider => "provider",
            Self::Litellm => "litellm",
            Self::Runtime => "runtime",
            Self::Headroom => "headroom",
            Self::Manual => "manual",
            Self::Unknown => "unknown",
        }
    }
}

impl FromStr for UsageSource {
    type Err = String;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        match input.trim().to_ascii_lowercase().as_str() {
            "provider" => Ok(Self::Provider),
            "litellm" => Ok(Self::Litellm),
            "runtime" => Ok(Self::Runtime),
            "headroom" => Ok(Self::Headroom),
            "manual" => Ok(Self::Manual),
            "unknown" => Ok(Self::Unknown),
            other => Err(format!("unknown usage source: {other}")),
        }
    }
}

/// How `cost_usd` was obtained. Never mix a local estimate with a provider
/// invoice without keeping this marker.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CostSource {
    Provider,
    PricingTable,
    Manual,
    #[default]
    Unknown,
}

impl CostSource {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Provider => "provider",
            Self::PricingTable => "pricing_table",
            Self::Manual => "manual",
            Self::Unknown => "unknown",
        }
    }
}

impl FromStr for CostSource {
    type Err = String;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        match input.trim().to_ascii_lowercase().as_str() {
            "provider" => Ok(Self::Provider),
            "pricing_table" | "pricing-table" => Ok(Self::PricingTable),
            "manual" => Ok(Self::Manual),
            "unknown" => Ok(Self::Unknown),
            other => Err(format!("unknown cost source: {other}")),
        }
    }
}

/// One request-level ledger row. Mirrors `doc/03` §12.1, with `ts_ms` and
/// capability bits added for deterministic local accounting.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct UsageRecord {
    #[serde(default = "default_version")]
    pub v: u8,
    pub ts_ms: u64,
    pub run_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attempt: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tier: Option<Tier>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_alias: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wire: Option<Wire>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cached_input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    pub cost_source: CostSource,
    pub source: UsageSource,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    /// 1-based step inside the run; absent for one-shot requests.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_names: Option<Vec<String>>,
    /// Files this step wrote in full (not `edit`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub write_paths: Option<Vec<String>>,
    /// Full rewrites of already-written files in this step.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rewrite_count: Option<u32>,
    /// Chat history bytes before projection.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub history_bytes: Option<u64>,
    /// Chat history bytes actually sent after projection.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub projected_bytes: Option<u64>,
}

fn default_version() -> u8 {
    USAGE_VERSION
}

impl Default for UsageRecord {
    fn default() -> Self {
        Self {
            v: USAGE_VERSION,
            ts_ms: 0,
            run_id: String::new(),
            task_id: None,
            attempt: None,
            tier: None,
            model_alias: None,
            model_id: None,
            provider: None,
            wire: None,
            input_tokens: None,
            cached_input_tokens: None,
            output_tokens: None,
            reasoning_tokens: None,
            cost_usd: None,
            cost_source: CostSource::Unknown,
            source: UsageSource::Unknown,
            latency_ms: None,
            status: None,
            step: None,
            tool_names: None,
            write_paths: None,
            rewrite_count: None,
            history_bytes: None,
            projected_bytes: None,
        }
    }
}

impl UsageRecord {
    pub fn new(run_id: impl Into<String>) -> Self {
        Self {
            v: USAGE_VERSION,
            run_id: run_id.into(),
            ..Self::default()
        }
    }

    /// Validate the canonical row shape before it enters the append-only ledger.
    ///
    /// `input_tokens` includes cached tokens; providers that report cache reads
    /// outside the prompt count must be normalized by the adapter first.
    pub fn validate(&self) -> Result<(), String> {
        if self.v != USAGE_VERSION {
            return Err(format!(
                "unsupported usage version: {} (expected {USAGE_VERSION})",
                self.v
            ));
        }
        if self.run_id.trim().is_empty() {
            return Err("run_id must not be empty".to_string());
        }
        if let Some(cost) = self.cost_usd {
            if !cost.is_finite() || cost < 0.0 {
                return Err("cost_usd must be finite and >= 0".to_string());
            }
            if self.cost_source == CostSource::Unknown {
                return Err("cost_source must not be unknown when cost_usd is present".to_string());
            }
        }
        if let (Some(input), Some(cached)) = (self.input_tokens, self.cached_input_tokens) {
            if cached > input {
                return Err(format!(
                    "cached_input_tokens ({cached}) exceeds input_tokens ({input})"
                ));
            }
        }
        if self.step == Some(0) {
            return Err("step is 1-based".to_string());
        }
        if let (Some(history), Some(projected)) = (self.history_bytes, self.projected_bytes) {
            if projected > history {
                return Err(format!(
                    "projected_bytes ({projected}) exceeds history_bytes ({history})"
                ));
            }
        }
        Ok(())
    }

    /// Fill `cost_usd` from the local model capability table when provider
    /// cost is absent. Existing provider/manual cost is never overwritten.
    pub fn fill_estimated_cost(&mut self, capability: Option<&ModelCapability>) {
        if self.cost_usd.is_some() {
            return;
        }
        let Some(capability) = capability else {
            return;
        };
        let (Some(input), Some(output)) = (self.input_tokens, self.output_tokens) else {
            return;
        };
        if let Some(cost) =
            capability.estimate_cost(input, self.cached_input_tokens.unwrap_or(0), output)
        {
            self.cost_usd = Some(cost);
            self.cost_source = CostSource::PricingTable;
        }
    }

    pub fn billed_tokens(&self) -> u64 {
        self.input_tokens.unwrap_or(0) + self.output_tokens.unwrap_or(0)
    }

    pub fn is_attributed(&self) -> bool {
        !self.run_id.trim().is_empty()
            && self
                .task_id
                .as_deref()
                .is_some_and(|task| !task.trim().is_empty())
    }

    pub fn is_failed(&self) -> bool {
        self.status.is_some_and(|status| status >= 400)
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct UsageAggregate {
    pub requests: u64,
    pub input_tokens: u64,
    pub cached_input_tokens: u64,
    pub output_tokens: u64,
    pub reasoning_tokens: u64,
    pub cost_usd: f64,
    pub unknown_cost_requests: u64,
    pub unknown_token_requests: u64,
    pub unattributed_requests: u64,
    pub failed_requests: u64,
    pub latency_ms: u64,
}

impl UsageAggregate {
    pub fn add(&mut self, record: &UsageRecord) {
        self.requests += 1;
        self.input_tokens += record.input_tokens.unwrap_or(0);
        self.cached_input_tokens += record.cached_input_tokens.unwrap_or(0);
        self.output_tokens += record.output_tokens.unwrap_or(0);
        self.reasoning_tokens += record.reasoning_tokens.unwrap_or(0);
        match record.cost_usd {
            Some(cost) => self.cost_usd += cost,
            None => self.unknown_cost_requests += 1,
        }
        if record.input_tokens.is_none() && record.output_tokens.is_none() {
            self.unknown_token_requests += 1;
        }
        if !record.is_attributed() {
            self.unattributed_requests += 1;
        }
        if record.is_failed() {
            self.failed_requests += 1;
        }
        self.latency_ms += record.latency_ms.unwrap_or(0);
    }

    pub fn billed_tokens(&self) -> u64 {
        self.input_tokens + self.output_tokens
    }

    pub fn cache_hit_ratio(&self) -> f64 {
        if self.input_tokens == 0 {
            0.0
        } else {
            self.cached_input_tokens as f64 / self.input_tokens as f64
        }
    }

    /// Cost per verified task. `None` when no task passed, no requests were
    /// recorded, or the slice contains unattributed/unknown-cost rows.
    pub fn cpvt(&self, passed_tasks: u64) -> Option<f64> {
        if passed_tasks == 0
            || self.requests == 0
            || self.unattributed_requests > 0
            || self.unknown_cost_requests > 0
        {
            return None;
        }
        Some(self.cost_usd / passed_tasks as f64)
    }

    /// Billed tokens per verified task. `None` when the slice contains
    /// unattributed or token-unknown rows.
    pub fn tpvt(&self, passed_tasks: u64) -> Option<f64> {
        if passed_tasks == 0
            || self.requests == 0
            || self.unattributed_requests > 0
            || self.unknown_token_requests > 0
        {
            return None;
        }
        Some(self.billed_tokens() as f64 / passed_tasks as f64)
    }

    /// Wall-clock seconds per verified task.
    pub fn wpt(wall_seconds: f64, passed_tasks: u64) -> Option<f64> {
        if passed_tasks == 0 {
            return None;
        }
        Some(wall_seconds / passed_tasks as f64)
    }
}

#[derive(Debug, Clone, Default)]
pub struct UsageFilter {
    pub run_id: Option<String>,
    pub task_id: Option<String>,
    pub tier: Option<Tier>,
    pub model_alias: Option<String>,
}

impl UsageFilter {
    pub fn matches(&self, record: &UsageRecord) -> bool {
        if let Some(run_id) = &self.run_id {
            if &record.run_id != run_id {
                return false;
            }
        }
        if let Some(task_id) = &self.task_id {
            if record.task_id.as_deref() != Some(task_id.as_str()) {
                return false;
            }
        }
        if let Some(tier) = self.tier {
            if record.tier != Some(tier) {
                return false;
            }
        }
        if let Some(model_alias) = &self.model_alias {
            let alias_matches = record.model_alias.as_deref() == Some(model_alias.as_str());
            let model_matches = record.model_id.as_deref() == Some(model_alias.as_str());
            if !alias_matches && !model_matches {
                return false;
            }
        }
        true
    }
}

pub fn aggregate_usage(records: &[UsageRecord], filter: &UsageFilter) -> UsageAggregate {
    let mut aggregate = UsageAggregate::default();
    for record in records.iter().filter(|record| filter.matches(record)) {
        aggregate.add(record);
    }
    aggregate
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(cost: Option<f64>, attributed: bool) -> UsageRecord {
        UsageRecord {
            v: USAGE_VERSION,
            run_id: "R-1".into(),
            task_id: attributed.then(|| "T-1".into()),
            input_tokens: Some(1000),
            cached_input_tokens: Some(400),
            output_tokens: Some(200),
            cost_usd: cost,
            cost_source: if cost.is_some() {
                CostSource::Provider
            } else {
                CostSource::Unknown
            },
            ..UsageRecord::default()
        }
    }

    #[test]
    fn aggregates_and_computes_metrics() {
        let mut aggregate = UsageAggregate::default();
        aggregate.add(&record(Some(0.01), true));
        aggregate.add(&record(Some(0.03), true));

        assert_eq!(aggregate.requests, 2);
        assert_eq!(aggregate.billed_tokens(), 2400);
        assert!((aggregate.cache_hit_ratio() - 0.4).abs() < 1e-12);
        assert!((aggregate.cpvt(2).expect("cpvt") - 0.02).abs() < 1e-12);
        assert!((aggregate.tpvt(2).expect("tpvt") - 1200.0).abs() < 1e-12);
        assert_eq!(aggregate.unattributed_requests, 0);

        aggregate.add(&record(Some(0.01), false));
        assert_eq!(aggregate.unattributed_requests, 1);
        assert!(aggregate.cpvt(2).is_none());
        assert!(aggregate.tpvt(2).is_none());
    }

    #[test]
    fn metrics_need_requests_and_tasks() {
        let aggregate = UsageAggregate::default();
        assert!(aggregate.cpvt(1).is_none());
        assert!(aggregate.tpvt(1).is_none());
        assert!(aggregate.cpvt(0).is_none());
    }

    #[test]
    fn tpvt_is_unknown_with_missing_tokens() {
        let mut aggregate = UsageAggregate::default();
        aggregate.add(&UsageRecord {
            run_id: "R-1".into(),
            task_id: Some("T-1".into()),
            cost_usd: Some(0.0),
            ..UsageRecord::default()
        });
        assert!(aggregate.tpvt(1).is_none());
        assert_eq!(aggregate.unknown_token_requests, 1);
    }

    #[test]
    fn cpvt_is_unknown_with_missing_cost() {
        let mut aggregate = UsageAggregate::default();
        aggregate.add(&record(None, true));
        assert!(aggregate.cpvt(1).is_none());
        assert_eq!(aggregate.unknown_cost_requests, 1);
    }

    #[test]
    fn fills_cost_from_capability() {
        let capability = ModelCapability {
            alias: "worker".into(),
            input_usd_per_mtok: Some(1.0),
            cached_input_usd_per_mtok: Some(0.1),
            output_usd_per_mtok: Some(2.0),
            ..ModelCapability::new("worker", Tier::Worker)
        };
        let mut row = record(None, true);
        row.fill_estimated_cost(Some(&capability));
        assert_eq!(row.cost_source, CostSource::PricingTable);
        assert!(row.cost_usd.is_some());

        let provider = UsageRecord {
            cost_usd: Some(1.0),
            cost_source: CostSource::Provider,
            ..record(None, true)
        };
        let mut provider = provider;
        provider.fill_estimated_cost(Some(&capability));
        assert_eq!(provider.cost_usd, Some(1.0));
        assert_eq!(provider.cost_source, CostSource::Provider);
    }

    #[test]
    fn filters_rows() {
        let filter = UsageFilter {
            task_id: Some("T-1".into()),
            tier: Some(Tier::Worker),
            ..UsageFilter::default()
        };
        let mut row = record(Some(0.0), true);
        assert!(!filter.matches(&row));
        row.tier = Some(Tier::Worker);
        assert!(filter.matches(&row));
    }

    #[test]
    fn aggregates_with_filter() {
        let mut worker = record(Some(0.01), true);
        worker.tier = Some(Tier::Worker);
        let mut frontier = record(Some(0.50), true);
        frontier.tier = Some(Tier::Frontier);
        let rows = vec![worker, frontier];

        let filter = UsageFilter {
            tier: Some(Tier::Worker),
            ..UsageFilter::default()
        };
        let aggregate = aggregate_usage(&rows, &filter);
        assert_eq!(aggregate.requests, 1);
        assert!((aggregate.cost_usd - 0.01).abs() < 1e-12);
    }

    #[test]
    fn validates_rows() {
        let mut row = record(Some(0.01), true);
        assert!(row.validate().is_ok());

        row.run_id.clear();
        assert!(row.validate().is_err());

        let mut row = record(Some(0.01), true);
        row.cost_usd = Some(f64::NAN);
        assert!(row.validate().is_err());

        let mut row = record(Some(0.01), true);
        row.cached_input_tokens = Some(row.input_tokens.unwrap_or(0) + 1);
        assert!(row.validate().is_err());

        let mut row = record(None, true);
        row.cost_usd = Some(0.01);
        assert!(row.validate().is_err());
    }

    #[test]
    fn default_row_has_current_version() {
        assert_eq!(UsageRecord::default().v, USAGE_VERSION);
    }
}
