use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::capability::find_model;
use crate::{Limits, ModelCapability};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub lang: String,
    pub tools: ToolsConfig,
    /// Alias -> real model + capability bits. Empty means "not configured yet";
    /// never invent a model or a price just to fill the ledger.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<ModelCapability>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            lang: "zh-CN".to_string(),
            tools: ToolsConfig::default(),
            models: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ToolsConfig {
    pub limits: Limits,
}

impl Config {
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        if !path.exists() {
            return Ok(Self::default());
        }

        let text = fs::read_to_string(path).map_err(|source| ConfigError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        let config: Self = toml::from_str(&text).map_err(|source| ConfigError::Parse {
            path: path.to_path_buf(),
            source,
        })?;
        let config = config.sanitized();
        config.validate()?;
        Ok(config)
    }

    pub fn sanitized(mut self) -> Self {
        self.tools.limits = self.tools.limits.sanitized();
        if self.lang.trim().is_empty() {
            self.lang = "zh-CN".to_string();
        }
        self
    }

    /// Reject ambiguous aliases and invalid capability bits. Prices may stay
    /// unknown; that is recorded as `unknown_cost`, not silently estimated.
    pub fn validate(&self) -> Result<(), ConfigError> {
        let mut seen = HashSet::new();
        for model in &self.models {
            model.validate().map_err(ConfigError::Validation)?;
            if !seen.insert(model.alias.as_str()) {
                return Err(ConfigError::Validation(format!(
                    "duplicate model alias: {}",
                    model.alias
                )));
            }
        }
        Ok(())
    }

    pub fn model(&self, alias: &str) -> Option<&ModelCapability> {
        find_model(&self.models, alias)
    }

    pub fn to_toml(&self) -> Result<String, ConfigError> {
        toml::to_string_pretty(self).map_err(ConfigError::Serialize)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("read config {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("parse config {path}: {source}")]
    Parse {
        path: PathBuf,
        source: toml::de::Error,
    },
    #[error("invalid config: {0}")]
    Validation(String),
    #[error("serialize config: {0}")]
    Serialize(#[from] toml::ser::Error),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Tier;

    #[test]
    fn missing_config_uses_defaults() {
        let config = Config::load(Path::new("/definitely/not/here.toml")).expect("load");
        assert_eq!(config.lang, "zh-CN");
        assert_eq!(config.tools.limits.call_bytes, 8 * 1024);
        assert!(config.models.is_empty());
    }

    #[test]
    fn partial_config_is_merged() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        fs::write(&path, "[tools.limits]\ncall_bytes = 4096\n").expect("write");

        let config = Config::load(&path).expect("load");
        assert_eq!(config.tools.limits.call_bytes, 4096);
        assert_eq!(config.tools.limits.search_results, 20);
    }

    #[test]
    fn parses_model_capabilities() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        fs::write(
            &path,
            r#"
[[models]]
alias = "worker"
provider = "deepseek"
model_id = "deepseek-v4-flash"
tier = "worker"
wire = "chat"
thinking_mode = "off"
reasoning_passthrough = "optional"
context_window = 128000
max_output = 8192
input_usd_per_mtok = 0.27
cached_input_usd_per_mtok = 0.07
output_usd_per_mtok = 1.10

[[models]]
alias = "frontier"
provider = "openai"
model_id = "gpt-6.1-sol"
tier = "frontier"
wire = "responses"
thinking_mode = "optional"
"#,
        )
        .expect("write");

        let config = Config::load(&path).expect("load");
        assert_eq!(config.models.len(), 2);
        let worker = config.model("worker").expect("worker");
        assert_eq!(worker.tier, Tier::Worker);
        assert_eq!(worker.model_id.as_deref(), Some("deepseek-v4-flash"));
        assert_eq!(
            config.model("frontier").expect("frontier").tier,
            Tier::Frontier
        );
        assert!(config.model("missing").is_none());
    }

    #[test]
    fn rejects_duplicate_aliases() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        fs::write(
            &path,
            r#"
[[models]]
alias = "worker"
tier = "worker"

[[models]]
alias = "worker"
tier = "frontier"
"#,
        )
        .expect("write");

        let error = Config::load(&path).expect_err("duplicate aliases must fail");
        assert!(error.to_string().contains("duplicate model alias"));
    }

    #[test]
    fn rejects_invalid_model_limits() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        fs::write(
            &path,
            r#"
[[models]]
alias = "worker"
context_window = 100
max_output = 200
"#,
        )
        .expect("write");

        let error = Config::load(&path).expect_err("invalid limits must fail");
        assert!(error.to_string().contains("max_output"));
    }

    #[test]
    fn rejects_unknown_model_fields() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        fs::write(
            &path,
            r#"
[[models]]
alias = "worker"
cache_price = 0.1
"#,
        )
        .expect("write");

        let error = Config::load(&path).expect_err("typo must not be silently ignored");
        assert!(error.to_string().contains("unknown field"));
    }
}
