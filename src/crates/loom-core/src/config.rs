use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::Limits;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub lang: String,
    pub tools: ToolsConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            lang: "zh-CN".to_string(),
            tools: ToolsConfig::default(),
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
        Ok(config.sanitized())
    }

    pub fn sanitized(mut self) -> Self {
        self.tools.limits = self.tools.limits.sanitized();
        if self.lang.trim().is_empty() {
            self.lang = "zh-CN".to_string();
        }
        self
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
    #[error("serialize config: {0}")]
    Serialize(#[from] toml::ser::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_config_uses_defaults() {
        let config = Config::load(Path::new("/definitely/not/here.toml")).expect("load");
        assert_eq!(config.lang, "zh-CN");
        assert_eq!(config.tools.limits.call_bytes, 8 * 1024);
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
}
