//! `loom config get/set`: comment-preserving edits of `.loom/config.toml`.
//!
//! `set` rewrites the target file through `toml_edit` (comments and key order
//! survive), then round-trips the *effective* config (base + local overlay)
//! before committing: a typo fails fast and nothing is written, and a base
//! edit may legitimately reference a model that only exists in the overlay.

use std::fs;

use anyhow::{anyhow, bail, Context as _, Result};
use loom_core::{apply_config_set, Config};
use loom_store::Project;
use serde_json::{json, Value};

use super::Format;

pub fn get(project: &Project, path: &str, format: Format) -> Result<i32> {
    let config = project.load_config()?;
    let effective = serde_json::to_value(&config).context("serialize effective config")?;
    let value = value_at(&effective, path).map_err(|message| anyhow!("{message}"))?;
    let source = source_of(project, path)?;
    match format {
        Format::Json => println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "path": path,
                "value": value,
                "source": source,
            }))?
        ),
        Format::Pretty => println!("{path} = {value} ({source})"),
    }
    Ok(0)
}

pub fn set(project: &Project, path: &str, raw: &str, local: bool, format: Format) -> Result<i32> {
    let base_path = project.config_path();
    if !base_path.exists() {
        bail!(
            "{} does not exist; run `loom init` first",
            base_path.display()
        );
    }
    let target = if local {
        project.config_local_path()
    } else {
        base_path.clone()
    };
    let original = fs::read_to_string(&target).unwrap_or_default();
    let new_text =
        apply_config_set(&original, path, raw).map_err(|message| anyhow::anyhow!("{message}"))?;

    fs::write(&target, &new_text)
        .with_context(|| format!("write {}", project.relative(&target)))?;
    if let Err(error) = Config::load_with_local(&base_path, &project.config_local_path()) {
        // Roll back: a half-valid config would break every later command.
        if original.is_empty() {
            let _ = fs::remove_file(&target);
        } else {
            let _ = fs::write(&target, &original);
        }
        bail!("config would be invalid ({error}); nothing written");
    }

    let mut warnings = Vec::new();
    if !local && shadows_local(project, path)? {
        warnings.push(format!(
            "{path} is also set in config.local.toml; the local value wins — remove it there to see this change"
        ));
    }
    match format {
        Format::Json => println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "path": path,
                "value": raw,
                "target": project.relative(&target),
                "warnings": warnings,
            }))?
        ),
        Format::Pretty => {
            println!("set {path} = {raw} in {}", project.relative(&target));
            for warning in &warnings {
                println!("warning: {warning}");
            }
        }
    }
    Ok(0)
}

/// Dotted-path lookup over a serialized config. `models.<alias>.<field>` walks
/// the `[[models]]` array by alias; other paths walk tables.
fn value_at(root: &Value, path: &str) -> Result<Value, String> {
    let parts: Vec<&str> = path.split('.').filter(|part| !part.is_empty()).collect();
    if parts.is_empty() {
        return Err("path is empty".to_string());
    }
    if parts[0] == "models" {
        if parts.len() != 3 {
            return Err("models path must be models.<alias>.<field>".to_string());
        }
        let models = root
            .get("models")
            .and_then(Value::as_array)
            .ok_or_else(|| "no [[models]] configured".to_string())?;
        let entry = models
            .iter()
            .find(|model| model.get("alias").and_then(Value::as_str) == Some(parts[1]))
            .ok_or_else(|| format!("model alias {} not found", parts[1]))?;
        return entry
            .get(parts[2])
            .cloned()
            .ok_or_else(|| format!("models.{}.{} is not set", parts[1], parts[2]));
    }
    let mut current = root;
    for segment in &parts {
        current = current
            .get(segment)
            .ok_or_else(|| format!("unknown path: {path}"))?;
    }
    Ok(current.clone())
}

/// Which file (if any) defines the path: `set` writes the base file, so a
/// shadowing local entry would silently win.
fn source_of(project: &Project, path: &str) -> Result<String> {
    let defines = |text: &str| -> bool {
        toml::from_str::<toml::Value>(text)
            .ok()
            .and_then(|value| serde_json::to_value(value).ok())
            .map(|json| value_at(&json, path).is_ok())
            .unwrap_or(false)
    };
    let base = fs::read_to_string(project.config_path()).unwrap_or_default();
    let local_path = project.config_local_path();
    let local = fs::read_to_string(&local_path).unwrap_or_default();
    let base_has = defines(&base);
    let local_has = local_path.exists() && defines(&local);
    Ok(match (base_has, local_has) {
        (true, true) => "base+local (local wins)".to_string(),
        (false, true) => "local".to_string(),
        (true, false) => "base".to_string(),
        (false, false) => "default".to_string(),
    })
}

fn shadows_local(project: &Project, path: &str) -> Result<bool> {
    let local_path = project.config_local_path();
    if !local_path.exists() {
        return Ok(false);
    }
    let text = fs::read_to_string(&local_path).unwrap_or_default();
    Ok(toml::from_str::<toml::Value>(&text)
        .ok()
        .and_then(|value| serde_json::to_value(value).ok())
        .map(|json| value_at(&json, path).is_ok())
        .unwrap_or(false))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> Value {
        json!({
            "lang": "zh-CN",
            "routing": {"roles": {"review": "frontier"}},
            "models": [
                {"alias": "a", "model_id": "model-a"},
                {"alias": "b", "model_id": "model-b", "reasoning_effort": "high"}
            ]
        })
    }

    #[test]
    fn value_at_walks_tables_and_models() {
        assert_eq!(value_at(&root(), "lang").expect("lang"), json!("zh-CN"));
        assert_eq!(
            value_at(&root(), "routing.roles.review").expect("role"),
            json!("frontier")
        );
        assert_eq!(
            value_at(&root(), "models.b.reasoning_effort").expect("effort"),
            json!("high")
        );
        assert!(
            value_at(&root(), "models.a.reasoning_effort").is_err(),
            "unset field"
        );
        assert!(
            value_at(&root(), "models.ghost.model_id").is_err(),
            "unknown alias"
        );
        assert!(value_at(&root(), "langg").is_err(), "unknown key");
        assert!(
            value_at(&root(), "models.b").is_err(),
            "alias-only path rejected"
        );
    }
}
