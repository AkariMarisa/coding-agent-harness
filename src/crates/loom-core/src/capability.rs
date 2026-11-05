use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// Routing tier. Keep the set small; a third tier needs CPVT evidence first.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tier {
    Frontier,
    #[default]
    Worker,
}

impl Tier {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Frontier => "frontier",
            Self::Worker => "worker",
        }
    }
}

impl FromStr for Tier {
    type Err = String;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        match input.trim().to_ascii_lowercase().as_str() {
            "frontier" => Ok(Self::Frontier),
            "worker" => Ok(Self::Worker),
            other => Err(format!("unknown tier: {other}")),
        }
    }
}

/// Wire protocol a model speaks. This decides whether gateways and
/// compression layers fully cover the request.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Wire {
    #[default]
    Chat,
    Responses,
    Messages,
}

impl Wire {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Chat => "chat",
            Self::Responses => "responses",
            Self::Messages => "messages",
        }
    }
}

impl FromStr for Wire {
    type Err = String;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        match input.trim().to_ascii_lowercase().as_str() {
            "chat" => Ok(Self::Chat),
            "responses" => Ok(Self::Responses),
            "messages" => Ok(Self::Messages),
            other => Err(format!("unknown wire: {other}")),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThinkingMode {
    Off,
    #[default]
    Optional,
    Forced,
}

impl ThinkingMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Optional => "optional",
            Self::Forced => "forced",
        }
    }
}

impl FromStr for ThinkingMode {
    type Err = String;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        match input.trim().to_ascii_lowercase().as_str() {
            "off" => Ok(Self::Off),
            "optional" => Ok(Self::Optional),
            "forced" => Ok(Self::Forced),
            other => Err(format!("unknown thinking mode: {other}")),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningPassthrough {
    Required,
    #[default]
    Optional,
}

impl ReasoningPassthrough {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Required => "required",
            Self::Optional => "optional",
        }
    }
}

impl FromStr for ReasoningPassthrough {
    type Err = String;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        match input.trim().to_ascii_lowercase().as_str() {
            "required" => Ok(Self::Required),
            "optional" => Ok(Self::Optional),
            other => Err(format!("unknown reasoning passthrough: {other}")),
        }
    }
}

/// Alias → real model + capability bits.
///
/// Prices are USD per million tokens. Unknown values stay `None`; never guess
/// a provider price just to make the ledger look complete.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ModelCapability {
    pub alias: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_id: Option<String>,
    pub tier: Tier,
    pub wire: Wire,
    pub thinking_mode: ThinkingMode,
    pub reasoning_passthrough: ReasoningPassthrough,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_window: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub streaming: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_usd_per_mtok: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cached_input_usd_per_mtok: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_write_usd_per_mtok: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_usd_per_mtok: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pricing_version: Option<String>,
}

impl ModelCapability {
    pub fn new(alias: impl Into<String>, tier: Tier) -> Self {
        Self {
            alias: alias.into(),
            tier,
            ..Self::default()
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.alias.trim().is_empty() {
            return Err("model alias must not be empty".to_string());
        }
        if self
            .model_id
            .as_deref()
            .is_some_and(|id| id.trim().is_empty())
        {
            return Err(format!("{}: model_id must not be empty", self.alias));
        }
        if self.context_window == Some(0) {
            return Err(format!("{}: context_window must be > 0", self.alias));
        }
        if self.max_output == Some(0) {
            return Err(format!("{}: max_output must be > 0", self.alias));
        }
        if let (Some(context), Some(max_output)) = (self.context_window, self.max_output) {
            if max_output > context {
                return Err(format!(
                    "{}: max_output ({max_output}) exceeds context_window ({context})",
                    self.alias
                ));
            }
        }
        for (name, price) in [
            ("input_usd_per_mtok", self.input_usd_per_mtok),
            ("cached_input_usd_per_mtok", self.cached_input_usd_per_mtok),
            ("cache_write_usd_per_mtok", self.cache_write_usd_per_mtok),
            ("output_usd_per_mtok", self.output_usd_per_mtok),
        ] {
            if let Some(value) = price {
                if !value.is_finite() || value < 0.0 {
                    return Err(format!("{}: {name} must be finite and >= 0", self.alias));
                }
            }
        }
        Ok(())
    }

    /// Whether this model can safely drive a multi-turn tool loop.
    pub fn tool_loop_safe(&self) -> bool {
        self.thinking_mode != ThinkingMode::Forced
            || self.reasoning_passthrough == ReasoningPassthrough::Required
    }

    pub fn display_model(&self) -> &str {
        self.model_id.as_deref().unwrap_or(&self.alias)
    }

    /// Estimate request cost from the local pricing table.
    ///
    /// Returns `None` when the needed prices are unknown. `reasoning_tokens`
    /// are intentionally not added: they are normally part of output tokens
    /// and double counting would inflate CPVT.
    pub fn estimate_cost(
        &self,
        input_tokens: u64,
        cached_input_tokens: u64,
        output_tokens: u64,
    ) -> Option<f64> {
        let cached = cached_input_tokens.min(input_tokens);
        let uncached = input_tokens.saturating_sub(cached);
        let input_price = self.input_usd_per_mtok?;
        let output_price = self.output_usd_per_mtok?;
        let cache_price = if cached > 0 {
            self.cached_input_usd_per_mtok?
        } else {
            0.0
        };

        Some(
            (uncached as f64 * input_price
                + cached as f64 * cache_price
                + output_tokens as f64 * output_price)
                / 1_000_000.0,
        )
    }
}

pub fn find_model<'a>(models: &'a [ModelCapability], alias: &str) -> Option<&'a ModelCapability> {
    models.iter().find(|model| model.alias == alias)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_capability_values() {
        assert_eq!("frontier".parse::<Tier>().expect("tier"), Tier::Frontier);
        assert_eq!("responses".parse::<Wire>().expect("wire"), Wire::Responses);
        assert_eq!(
            "forced".parse::<ThinkingMode>().expect("thinking"),
            ThinkingMode::Forced
        );
        assert!("nope".parse::<Tier>().is_err());
    }

    #[test]
    fn validates_context_and_prices() {
        let mut model = ModelCapability::new("worker", Tier::Worker);
        assert!(model.validate().is_ok());

        model.max_output = Some(100);
        model.context_window = Some(50);
        assert!(model.validate().is_err());

        model.context_window = Some(100);
        model.input_usd_per_mtok = Some(-1.0);
        assert!(model.validate().is_err());
    }

    #[test]
    fn estimates_cost_with_cache() {
        let model = ModelCapability {
            alias: "worker".into(),
            input_usd_per_mtok: Some(1.0),
            cached_input_usd_per_mtok: Some(0.1),
            output_usd_per_mtok: Some(2.0),
            ..ModelCapability::new("worker", Tier::Worker)
        };

        let cost = model
            .estimate_cost(1_000_000, 400_000, 100_000)
            .expect("cost");
        assert!(
            (cost - (600_000.0 * 1.0 + 400_000.0 * 0.1 + 100_000.0 * 2.0) / 1_000_000.0).abs()
                < 1e-12
        );

        let missing = ModelCapability::new("worker", Tier::Worker);
        assert!(missing.estimate_cost(10, 0, 1).is_none());
    }

    #[test]
    fn forced_thinking_needs_reasoning_passthrough() {
        let mut model = ModelCapability::new("worker", Tier::Worker);
        model.thinking_mode = ThinkingMode::Forced;
        model.reasoning_passthrough = ReasoningPassthrough::Optional;
        assert!(!model.tool_loop_safe());

        model.reasoning_passthrough = ReasoningPassthrough::Required;
        assert!(model.tool_loop_safe());
    }
}
