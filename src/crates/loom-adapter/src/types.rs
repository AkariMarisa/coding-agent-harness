use std::path::PathBuf;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// Supported executor adapters. The contract is process + files + JSON + exit
/// code; no plugin API is required.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AdapterKind {
    Mock,
    Goose,
    Opencode,
}

impl AdapterKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Mock => "mock",
            Self::Goose => "goose",
            Self::Opencode => "opencode",
        }
    }
}

impl FromStr for AdapterKind {
    type Err = String;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        match input.trim().to_ascii_lowercase().as_str() {
            "mock" => Ok(Self::Mock),
            "goose" => Ok(Self::Goose),
            "opencode" | "oc" => Ok(Self::Opencode),
            other => Err(format!("unknown adapter: {other}")),
        }
    }
}

#[derive(Debug, Clone)]
pub struct AdapterRequest {
    pub adapter: AdapterKind,
    pub run_id: String,
    pub task_id: String,
    pub attempt: u32,
    pub packet_dir: PathBuf,
    pub worktree: PathBuf,
    pub output_dir: PathBuf,
    pub logs_dir: PathBuf,
    pub model: String,
    pub branch: String,
    pub timeout_s: u64,
    pub max_tokens: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AdapterUsageRow {
    pub v: u8,
    pub run_id: String,
    pub task_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

pub const ADAPTER_USAGE_VERSION: u8 = 1;

impl Default for AdapterUsageRow {
    fn default() -> Self {
        Self {
            v: ADAPTER_USAGE_VERSION,
            run_id: String::new(),
            task_id: String::new(),
            model: None,
            input_tokens: None,
            cached_input_tokens: None,
            output_tokens: None,
            reasoning_tokens: None,
            cost_usd: None,
            latency_ms: None,
            status: None,
            source: None,
        }
    }
}

impl AdapterUsageRow {
    pub fn new(run_id: impl Into<String>, task_id: impl Into<String>) -> Self {
        Self {
            run_id: run_id.into(),
            task_id: task_id.into(),
            ..Self::default()
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.v != ADAPTER_USAGE_VERSION {
            return Err(format!("unsupported adapter usage version: {}", self.v));
        }
        if self.run_id.trim().is_empty() || self.task_id.trim().is_empty() {
            return Err("adapter usage requires run_id and task_id".to_string());
        }
        if let (Some(input), Some(cached)) = (self.input_tokens, self.cached_input_tokens) {
            if cached > input {
                return Err(format!(
                    "cached_input_tokens ({cached}) exceeds input_tokens ({input})"
                ));
            }
        }
        if let Some(cost) = self.cost_usd {
            if !cost.is_finite() || cost < 0.0 {
                return Err("cost_usd must be finite and >= 0".to_string());
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct AdapterRun {
    pub adapter: AdapterKind,
    pub exit_code: i32,
    pub timed_out: bool,
    pub duration_ms: u64,
    pub stdout_path: PathBuf,
    pub stderr_path: PathBuf,
    pub receipt: loom_core::Receipt,
    pub usage: Vec<AdapterUsageRow>,
}
