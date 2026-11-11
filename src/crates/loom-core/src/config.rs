use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::capability::find_model;
use crate::routing::RoutingConfig;
use crate::{Limits, ModelCapability};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub lang: String,
    pub tools: ToolsConfig,
    /// Alias -> real model + capability bits. Empty means "not configured yet";
    /// never invent a model or a price just to fill the ledger.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<ModelCapability>,
    /// Role/tier mapping, escalation, and cost guardrails.
    pub routing: RoutingConfig,
    /// How many tasks may hold path locks (i.e. actually run) at once.
    pub parallel: ParallelConfig,
}

/// M4 starts at 2 parallel workers; 3+ needs an A/B showing WPT/CPVT do not
/// regress (`doc/05` §6).
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ParallelConfig {
    pub max_running: usize,
    /// Worst-case sum of `budget.max_cost_usd` across concurrently running
    /// tasks plus the new one; 0 disables the cap.
    pub max_inflight_cost_usd: f64,
}

impl Default for ParallelConfig {
    fn default() -> Self {
        Self {
            max_running: 2,
            max_inflight_cost_usd: 0.0,
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            lang: "zh-CN".to_string(),
            tools: ToolsConfig::default(),
            models: Vec::new(),
            routing: RoutingConfig::default(),
            parallel: ParallelConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
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
        self.routing
            .validate(&self.models)
            .map_err(ConfigError::Validation)?;
        if self.parallel.max_running == 0 {
            return Err(ConfigError::Validation(
                "parallel.max_running must be >= 1".to_string(),
            ));
        }
        if !self.parallel.max_inflight_cost_usd.is_finite()
            || self.parallel.max_inflight_cost_usd < 0.0
        {
            return Err(ConfigError::Validation(
                "parallel.max_inflight_cost_usd must be finite and >= 0".to_string(),
            ));
        }
        Ok(())
    }

    pub fn model(&self, alias: &str) -> Option<&ModelCapability> {
        find_model(&self.models, alias)
    }

    /// Load `base_path` and deep-merge `local_path` over it when present
    /// (`.loom/config.local.toml`, never committed): tables merge recursively,
    /// other values and arrays are replaced — except `models`, which merges by
    /// `alias` so a local experiment can override or add one entry without
    /// re-listing the rest. Secrets stay where they are: `key_env` only ever
    /// names an environment variable.
    pub fn load_with_local(base_path: &Path, local_path: &Path) -> Result<Self, ConfigError> {
        let mut base = read_value(base_path)?;
        if local_path.exists() {
            let local = read_value(local_path)?;
            merge_value(&mut base, local);
        }
        let config: Self = Self::deserialize(base).map_err(|source| ConfigError::Parse {
            path: base_path.to_path_buf(),
            source,
        })?;
        let config = config.sanitized();
        config.validate()?;
        Ok(config)
    }

    pub fn to_toml(&self) -> Result<String, ConfigError> {
        toml::to_string_pretty(self).map_err(ConfigError::Serialize)
    }
}

/// Rewrite a config document at a dotted path, preserving comments and key
/// order (toml_edit). `models.<alias>.<field>` addresses `[[models]]` entries
/// by alias; every other path walks or creates tables. This is text-level
/// editing only: the caller round-trips the result through `Config::load*` so
/// schema validation (including `deny_unknown_fields`) rejects typos before
/// anything is committed.
pub fn apply_config_set(text: &str, path: &str, raw: &str) -> Result<String, String> {
    let parsed = parse_config_value(raw)?;
    let mut doc: toml_edit::DocumentMut = text
        .parse()
        .map_err(|error| format!("parse config: {error}"))?;

    let parts: Vec<&str> = path.split('.').filter(|part| !part.is_empty()).collect();
    if parts.is_empty() {
        return Err("path is empty".to_string());
    }
    if parts[0] == "models" {
        if parts.len() != 3 {
            return Err("models path must be models.<alias>.<field>".to_string());
        }
        set_model_field(&mut doc, parts[1], parts[2], parsed)?;
        return Ok(doc.to_string());
    }

    let (key, parents) = parts.split_last().expect("parts is not empty");
    let mut table: &mut toml_edit::Table = doc.as_table_mut();
    for (index, segment) in parents.iter().enumerate() {
        let slot = table
            .entry(segment)
            .or_insert(toml_edit::Item::Table(toml_edit::Table::new()));
        match slot {
            toml_edit::Item::Table(nested) => table = nested,
            _ => return Err(format!("{} is not a table", parents[..=index].join("."))),
        }
    }
    table[*key] = parsed;
    Ok(doc.to_string())
}

fn set_model_field(
    doc: &mut toml_edit::DocumentMut,
    alias: &str,
    field: &str,
    parsed: toml_edit::Item,
) -> Result<(), String> {
    let Some(item) = doc.get_mut("models") else {
        return Err("no [[models]] in config; run `loom model init` first".to_string());
    };
    let Some(models) = item.as_array_of_tables_mut() else {
        return Err("models is not an array of tables".to_string());
    };
    let mut aliases = Vec::new();
    for table in models.iter_mut() {
        let entry_alias = table
            .get("alias")
            .and_then(|item| item.as_value())
            .and_then(|value| value.as_str());
        aliases.push(entry_alias.unwrap_or("?").to_string());
        if entry_alias == Some(alias) {
            table[field] = parsed;
            return Ok(());
        }
    }
    Err(format!(
        "model alias {alias} not found; configured: {}",
        aliases.join(", ")
    ))
}

/// Parse a CLI value: booleans, integers, finite floats, JSON arrays of
/// scalars, quoted strings (force string semantics), otherwise plain string.
pub fn parse_config_value(raw: &str) -> Result<toml_edit::Item, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("value is empty".to_string());
    }
    if trimmed.starts_with('"') {
        let text: String = serde_json::from_str(trimmed)
            .map_err(|error| format!("quoted value must be a valid JSON string: {error}"))?;
        return Ok(toml_edit::value(text));
    }
    match trimmed {
        "true" => return Ok(toml_edit::value(true)),
        "false" => return Ok(toml_edit::value(false)),
        _ => {}
    }
    if let Ok(int) = trimmed.parse::<i64>() {
        return Ok(toml_edit::value(int));
    }
    if let Ok(float) = trimmed.parse::<f64>() {
        if float.is_finite() {
            return Ok(toml_edit::value(float));
        }
    }
    if trimmed.starts_with('[') {
        let elements: Vec<serde_json::Value> = serde_json::from_str(trimmed)
            .map_err(|error| format!("array must look like [\"a\", \"b\"]: {error}"))?;
        let mut array = toml_edit::Array::new();
        for element in elements {
            array.push(json_scalar(element)?);
        }
        return Ok(toml_edit::Item::Value(toml_edit::Value::Array(array)));
    }
    if trimmed.starts_with('{') {
        return Err("object values are not supported; set a dotted path instead".to_string());
    }
    Ok(toml_edit::value(trimmed.to_string()))
}

fn json_scalar(element: serde_json::Value) -> Result<toml_edit::Value, String> {
    let item = match element {
        serde_json::Value::String(text) => toml_edit::value(text),
        serde_json::Value::Bool(flag) => toml_edit::value(flag),
        serde_json::Value::Number(number) => {
            if let Some(int) = number.as_i64() {
                toml_edit::value(int)
            } else if let Some(float) = number.as_f64() {
                toml_edit::value(float)
            } else {
                return Err("array numbers must be i64 or f64".to_string());
            }
        }
        _ => return Err("arrays may only contain strings, numbers, or booleans".to_string()),
    };
    match item {
        toml_edit::Item::Value(value) => Ok(value),
        other => Err(format!("unexpected value shape: {other:?}")),
    }
}

/// Read a TOML document as a generic value; a missing or empty file is an
/// empty table so `load_with_local` can start from either side.
fn read_value(path: &Path) -> Result<toml::Value, ConfigError> {
    if !path.exists() {
        return Ok(toml::Value::Table(toml::Table::new()));
    }
    let text = fs::read_to_string(path).map_err(|source| ConfigError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if text.trim().is_empty() {
        return Ok(toml::Value::Table(toml::Table::new()));
    }
    toml::from_str(&text).map_err(|source| ConfigError::Parse {
        path: path.to_path_buf(),
        source,
    })
}

fn merge_value(base: &mut toml::Value, local: toml::Value) {
    match (base, local) {
        (toml::Value::Table(base_table), toml::Value::Table(local_table)) => {
            for (key, local_value) in local_table {
                match base_table.get_mut(&key) {
                    Some(base_value) if key == "models" => merge_models(base_value, local_value),
                    Some(base_value) => merge_value(base_value, local_value),
                    None => {
                        base_table.insert(key, local_value);
                    }
                }
            }
        }
        (slot, local_value) => *slot = local_value,
    }
}

/// `[[models]]` merges by `alias`: a local entry with the same alias overlays
/// its fields onto the base entry (tables merge, scalars replace), new aliases
/// are appended, unrelated entries are untouched. An effort-only local entry
/// therefore does not need to repeat the base URL.
fn merge_models(base: &mut toml::Value, local: toml::Value) {
    let local_entries = match local {
        toml::Value::Array(entries) => entries,
        other => {
            *base = other;
            return;
        }
    };
    let base_entries = match base {
        toml::Value::Array(entries) => entries,
        _ => {
            *base = toml::Value::Array(local_entries);
            return;
        }
    };
    for local_entry in local_entries {
        match entry_alias(&local_entry) {
            Some(alias) => match base_entries
                .iter_mut()
                .position(|entry| entry_alias(entry).as_deref() == Some(alias.as_str()))
            {
                Some(index) => merge_value(&mut base_entries[index], local_entry),
                None => base_entries.push(local_entry),
            },
            None => base_entries.push(local_entry),
        }
    }
}

fn entry_alias(entry: &toml::Value) -> Option<String> {
    entry.as_table()?.get("alias")?.as_str().map(str::to_string)
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
        assert_eq!(config.routing.escalate_after_failures, 2);
        assert_eq!(config.routing.max_escalations_per_task, 1);
        assert_eq!(config.parallel.max_running, 2);
        assert_eq!(config.parallel.max_inflight_cost_usd, 0.0);
    }

    #[test]
    fn local_overlays_scalars_and_merges_models_by_alias() {
        let dir = tempfile::tempdir().expect("tempdir");
        let base = dir.path().join("config.toml");
        let local = dir.path().join("config.local.toml");
        fs::write(
            &base,
            r#"
lang = "en-US"

[[models]]
alias = "a"
model_id = "model-a"
tier = "worker"

[[models]]
alias = "b"
model_id = "model-b-base"
tier = "worker"
base_url = "https://base.example/v1"

[routing.fallbacks]
worker = ["b"]
"#,
        )
        .expect("write base");
        fs::write(
            &local,
            r#"
lang = "en-GB"

[[models]]
alias = "b"
model_id = "model-b-local"
tier = "worker"

[[models]]
alias = "c"
model_id = "model-c"
tier = "worker"
"#,
        )
        .expect("write local");

        let config = Config::load_with_local(&base, &local).expect("load");
        // Scalar: local wins.
        assert_eq!(config.lang, "en-GB");
        // Models merge by alias: base keeps `a`, `b` comes from local, `c` is new.
        let ids: Vec<&str> = config
            .models
            .iter()
            .map(|model| model.model_id.as_deref().unwrap_or_default())
            .collect();
        assert_eq!(ids, ["model-a", "model-b-local", "model-c"]);
        // Field-level merge: `b`'s local entry only sets model_id, so the base
        // entry's base_url survives the overlay.
        let b = config.model("b").expect("b");
        assert_eq!(b.base_url.as_deref(), Some("https://base.example/v1"));
        // Deep merge keeps base routing that local did not mention.
        assert_eq!(config.routing.fallbacks["worker"], ["b"]);
        config.validate().expect("valid");
    }

    #[test]
    fn local_deep_merges_tables_and_replaces_other_arrays() {
        let dir = tempfile::tempdir().expect("tempdir");
        let base = dir.path().join("config.toml");
        let local = dir.path().join("config.local.toml");
        fs::write(
            &base,
            "[routing]\nescalate_after_failures = 3\n[routing.cost_limits]\nworker = 0.5\n",
        )
        .expect("write base");
        fs::write(&local, "[routing.cost_limits]\nworker = 9.0\n").expect("write local");

        let config = Config::load_with_local(&base, &local).expect("load");
        assert_eq!(config.routing.escalate_after_failures, 3);
        assert_eq!(config.routing.cost_limit(crate::Tier::Worker), Some(9.0));
    }

    #[test]
    fn invalid_local_is_reported_with_its_own_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        let base = dir.path().join("config.toml");
        let local = dir.path().join("config.local.toml");
        fs::write(&local, "not toml {{{").expect("write local");

        let error = Config::load_with_local(&base, &local).expect_err("must fail");
        assert!(matches!(&error, ConfigError::Parse { path, .. } if path == &local));
    }

    #[test]
    fn config_set_preserves_comments_and_order() {
        let text = "# keep me\nlang = \"en-US\"\n\n[parallel]\nmax_running = 2\nmax_inflight_cost_usd = 0.5\n";
        let out = apply_config_set(text, "parallel.max_running", "4").expect("set");
        assert!(out.contains("# keep me"), "comment survives: {out}");
        assert!(
            out.contains("lang = \"en-US\""),
            "key order survives: {out}"
        );
        assert!(out.contains("max_running = 4"), "value lands: {out}");
        let config: Config = toml::from_str(&out).expect("parse");
        assert_eq!(config.parallel.max_running, 4);
        assert_eq!(config.lang, "en-US");
    }

    #[test]
    fn config_set_creates_missing_tables() {
        let out = apply_config_set("lang = \"zh-CN\"\n", "routing.roles.review", "frontier")
            .expect("set");
        let config: Config = toml::from_str(&out).expect("parse");
        assert_eq!(config.routing.roles["review"], Tier::Frontier);
    }

    #[test]
    fn config_set_addresses_models_by_alias() {
        let text = r#"
[[models]]
alias = "a"
tier = "worker"
base_url = "https://a.example"

[[models]]
alias = "b"
tier = "worker"
base_url = "https://b.example"
"#;
        let out = apply_config_set(text, "models.b.reasoning_effort", "high").expect("set");
        let config: Config = toml::from_str(&out).expect("parse");
        let b = config.model("b").expect("b");
        assert_eq!(b.reasoning_effort.as_deref(), Some("high"));
        assert_eq!(
            b.base_url.as_deref(),
            Some("https://b.example"),
            "untouched fields survive"
        );
        assert!(config.model("a").expect("a").reasoning_effort.is_none());

        let missing = apply_config_set(text, "models.ghost.reasoning_effort", "high")
            .expect_err("alias must exist");
        assert!(missing.contains("ghost"), "{missing}");
        assert!(missing.contains("configured: a, b"), "{missing}");
        assert!(
            apply_config_set(text, "models.b", "x").is_err(),
            "alias-only path rejected"
        );
    }

    #[test]
    fn config_set_value_shapes() {
        // Item renders as TOML, which sidesteps Formatted<> internals.
        assert_eq!(
            parse_config_value("true").expect("bool").to_string(),
            "true"
        );
        assert_eq!(parse_config_value("4").expect("int").to_string(), "4");
        assert_eq!(parse_config_value("0.3").expect("float").to_string(), "0.3");
        assert_eq!(
            parse_config_value("\"123\"").expect("quoted").to_string(),
            "\"123\"",
            "quoted input stays a string"
        );
        assert_eq!(
            parse_config_value("ja").expect("plain").to_string(),
            "\"ja\""
        );
        let array = parse_config_value("[\"mimo\", \"x\"]")
            .expect("array")
            .to_string();
        assert!(
            array.starts_with('[') && array.contains("mimo") && array.contains('x'),
            "{array}"
        );
        assert!(parse_config_value("{}").is_err(), "objects rejected");
        assert!(parse_config_value("").is_err(), "empty rejected");
        assert!(
            parse_config_value("[\"a\", 1, true]").is_ok(),
            "scalar mix allowed"
        );
        assert!(
            parse_config_value("[{ \"a\": 1 }]").is_err(),
            "nested objects rejected"
        );
    }

    #[test]
    fn unknown_keys_fail_validation_instead_of_silently_ignored() {
        let base = "lang = \"zh-CN\"\n";
        let typo = apply_config_set(base, "langg", "x").expect("apply");
        assert!(
            toml::from_str::<Config>(&typo).is_err(),
            "top-level typo must not parse"
        );
        let nested = apply_config_set(base, "tools.limits.callbyes", "10").expect("apply");
        assert!(
            toml::from_str::<Config>(&nested).is_err(),
            "nested typo must not parse"
        );
        let wrong_type = apply_config_set(base, "tools.limits.call_bytes", "fast").expect("apply");
        assert!(
            toml::from_str::<Config>(&wrong_type).is_err(),
            "wrong type must not parse"
        );
        let bad_enum = apply_config_set(base, "routing.roles.review", "ultra").expect("apply");
        assert!(
            toml::from_str::<Config>(&bad_enum).is_err(),
            "bad enum must not parse"
        );
    }

    #[test]
    fn parallel_max_running_must_be_positive() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        fs::write(&path, "[parallel]\nmax_running = 0\n").expect("write");
        assert!(matches!(
            Config::load(&path),
            Err(ConfigError::Validation(_))
        ));

        fs::write(
            &path,
            "[parallel]\nmax_running = 3\nmax_inflight_cost_usd = 1.5\n",
        )
        .expect("write");
        let config = Config::load(&path).expect("load");
        assert_eq!(config.parallel.max_running, 3);
        assert_eq!(config.parallel.max_inflight_cost_usd, 1.5);

        fs::write(
            &path,
            "[parallel]\nmax_running = 2\nmax_inflight_cost_usd = -1\n",
        )
        .expect("write");
        assert!(matches!(
            Config::load(&path),
            Err(ConfigError::Validation(_))
        ));
    }

    #[test]
    fn parses_routing_policy() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        fs::write(
            &path,
            r#"
[[models]]
alias = "worker"
tier = "worker"
model_id = "deepseek-v4-flash"

[[models]]
alias = "frontier"
tier = "frontier"
model_id = "gpt-6.1-sol"

[[models]]
alias = "worker-b"
tier = "worker"
model_id = "deepseek-v4-flash-b"

[routing]
escalate_after_failures = 3
max_escalations_per_task = 2

[routing.roles]
review = "frontier"
implement = "worker"

[routing.fallbacks]
worker = ["worker-b"]

[routing.cost_limits]
worker = 0.25
frontier = 1.0
"#,
        )
        .expect("write");

        let config = Config::load(&path).expect("load");
        assert_eq!(config.routing.escalate_after_failures, 3);
        assert_eq!(
            config.routing.role_tier(crate::TaskRole::Review),
            Some(Tier::Frontier)
        );
        assert_eq!(config.routing.fallbacks_for(Tier::Worker), ["worker-b"]);
        assert_eq!(config.routing.cost_limit(Tier::Frontier), Some(1.0));
    }

    #[test]
    fn rejects_cross_tier_fallback() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        fs::write(
            &path,
            r#"
[[models]]
alias = "worker"
tier = "worker"

[[models]]
alias = "frontier"
tier = "frontier"

[routing.fallbacks]
worker = ["frontier"]
"#,
        )
        .expect("write");

        let error = Config::load(&path).expect_err("cross-tier must fail");
        assert!(error.to_string().contains("cross-tier"), "{error}");
    }

    #[test]
    fn rejects_unknown_routing_role() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        fs::write(&path, "[routing.roles]\nnope = \"worker\"\n").expect("write");

        let error = Config::load(&path).expect_err("unknown role must fail");
        assert!(error.to_string().contains("routing.roles.nope"));
    }

    #[test]
    fn rejects_unknown_fallback_alias() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        fs::write(&path, "[routing.fallbacks]\nworker = [\"missing\"]\n").expect("write");

        let error = Config::load(&path).expect_err("unknown fallback must fail");
        assert!(error.to_string().contains("unknown model alias missing"));
    }

    #[test]
    fn rejects_unknown_routing_fields() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        fs::write(&path, "[routing]\nescalate_after_failure = 2\n").expect("write");

        let error = Config::load(&path).expect_err("typo must not be silently ignored");
        assert!(error.to_string().contains("unknown field"));
    }

    #[test]
    fn default_config_round_trips() {
        let text = Config::default().to_toml().expect("serialize");
        let parsed: Config = toml::from_str(&text).expect("parse");
        assert_eq!(parsed.lang, "zh-CN");
        assert_eq!(parsed.routing.escalate_after_failures, 2);
        assert_eq!(parsed.routing.max_escalations_per_task, 1);
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
