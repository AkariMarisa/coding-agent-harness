//! `loom model init`: guided `[[models]]` onboarding (AGENTS.md §10).
//!
//! Detects `<PREFIX>_API_BASE` / `<PREFIX>_API_KEY` pairs in the environment,
//! then either asks (TTY), writes from hints (`--yes`), or prints a suggested
//! block (non-TTY default). Keys never enter the file — only their names.

use std::collections::HashSet;
use std::fs;
use std::io::{IsTerminal, Write as _};
use std::str::FromStr;

use anyhow::{anyhow, bail, Context as _, Result};
use loom_core::{Config, ModelCapability, Tier, Wire};
use loom_store::Project;
use serde_json::json;

use super::Format;

/// One detected provider endpoint: `<PREFIX>_API_BASE` + optional key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Pair {
    /// Full env prefix, e.g. `LOOM_DEEPSEEK`.
    pub prefix: String,
    /// Prefix with a leading `LOOM_` stripped: `DEEPSEEK`. Used for alias and
    /// for the `LOOM_LIVE_<SHORT>_{MODEL,WIRE}` hints.
    pub short: String,
    pub base_url: String,
    pub key_env: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Draft {
    pub alias: String,
    pub model_id: Option<String>,
    pub tier: Tier,
    pub wire: Wire,
    pub base_url: String,
    pub key_env: Option<String>,
}

/// Scan `<PREFIX>_API_BASE` variables. The generic `LOOM_API_BASE` (prefix
/// `LOOM`) is the single-provider shortcut and becomes alias `default`.
pub(crate) fn detect(vars: &[(String, String)]) -> Vec<Pair> {
    let map: std::collections::BTreeMap<&str, &str> =
        vars.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    let mut pairs = Vec::new();
    for (name, base) in &map {
        let Some(prefix) = name.strip_suffix("_API_BASE") else {
            continue;
        };
        if prefix.is_empty() || base.trim().is_empty() {
            continue;
        }
        let key_name = format!("{prefix}_API_KEY");
        let key_env = map
            .get(key_name.as_str())
            .filter(|value| !value.trim().is_empty())
            .map(|_| key_name);
        let short = if prefix == "LOOM" {
            "DEFAULT".to_string()
        } else {
            prefix.strip_prefix("LOOM_").unwrap_or(prefix).to_string()
        };
        pairs.push(Pair {
            prefix: prefix.to_string(),
            short,
            base_url: base.trim().to_string(),
            key_env,
        });
    }
    pairs.sort_by(|a, b| a.prefix.cmp(&b.prefix));
    pairs
}

fn hint(vars: &[(String, String)], short: &str, kind: &str) -> Option<String> {
    let name = format!("LOOM_LIVE_{short}_{kind}");
    vars.iter()
        .find(|(key, _)| key == &name)
        .map(|(_, value)| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// Non-interactive drafts from `LOOM_LIVE_<SHORT>_MODEL` hints. Pairs without
/// a model_id hint are skipped with a note instead of guessing a model name.
pub(crate) fn auto_drafts(
    pairs: &[Pair],
    vars: &[(String, String)],
    existing: &HashSet<String>,
) -> (Vec<Draft>, Vec<String>) {
    let mut drafts = Vec::new();
    let mut notes = Vec::new();
    let mut seen = HashSet::new();
    for pair in pairs {
        let alias = pair.short.to_lowercase();
        if existing.contains(&alias) || !seen.insert(alias.clone()) {
            notes.push(format!("alias {alias} already configured; skipped"));
            continue;
        }
        let Some(model_id) = hint(vars, &pair.short, "MODEL") else {
            notes.push(format!(
                "no model_id hint for {alias} (set LOOM_LIVE_{}_MODEL, or run `loom model init` in a TTY)",
                pair.short
            ));
            continue;
        };
        let wire = hint(vars, &pair.short, "WIRE")
            .and_then(|value| {
                let parsed = Wire::from_str(&value);
                if parsed.is_err() {
                    notes.push(format!(
                        "unknown wire hint {value:?} for {alias}; defaulting to chat"
                    ));
                }
                parsed.ok()
            })
            .unwrap_or_default();
        drafts.push(Draft {
            alias,
            model_id: Some(model_id),
            tier: Tier::Worker,
            wire,
            base_url: pair.base_url.clone(),
            key_env: pair.key_env.clone(),
        });
    }
    (drafts, notes)
}

/// Append drafted entries as TOML. The result is validated by the caller via
/// `Config::load` before it is kept.
pub(crate) fn render_block(drafts: &[Draft]) -> Result<String> {
    let mut out = String::from("\n# loom model init\n");
    for draft in drafts {
        let model = ModelCapability {
            alias: draft.alias.clone(),
            model_id: draft.model_id.clone(),
            tier: draft.tier,
            wire: draft.wire,
            base_url: Some(draft.base_url.clone()),
            key_env: draft.key_env.clone(),
            ..ModelCapability::default()
        };
        let body = toml::to_string_pretty(&model).context("serialize model entry")?;
        out.push_str("[[models]]\n");
        out.push_str(&body);
        if !body.ends_with('\n') {
            out.push('\n');
        }
    }
    Ok(out)
}

pub fn init(project: &Project, yes: bool, format: Format) -> Result<i32> {
    if !project.loom_dir().is_dir() {
        bail!(
            "{} does not exist; run `loom init` first",
            project.loom_dir().display()
        );
    }
    let vars: Vec<(String, String)> = std::env::vars().collect();
    let pairs = detect(&vars);
    if pairs.is_empty() {
        bail!(
            "no *_API_BASE variables found; set e.g. LOOM_DEEPSEEK_API_BASE + LOOM_DEEPSEEK_API_KEY in .env.local (see .env.example), then rerun"
        );
    }
    let config = project.load_config().context("load .loom/config.toml")?;
    let existing: HashSet<String> = config.models.iter().map(|m| m.alias.clone()).collect();

    let interactive = !yes && std::io::stdin().is_terminal();
    let (drafts, notes) = if interactive {
        interactive_drafts(&pairs, &vars, &existing)?
    } else {
        auto_drafts(&pairs, &vars, &existing)
    };

    if drafts.is_empty() {
        bail!(
            "nothing to write: {}",
            if notes.is_empty() {
                "no drafts produced".to_string()
            } else {
                notes.join("; ")
            }
        );
    }
    let block = render_block(&drafts)?;
    let written: Vec<String> = drafts.iter().map(|draft| draft.alias.clone()).collect();

    // Preview (non-TTY without --yes): never write without being asked.
    if !interactive && !yes {
        match format {
            Format::Json => println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "preview": true,
                    "suggested": block,
                    "skipped": notes,
                }))?
            ),
            Format::Pretty => {
                println!("{block}");
                for note in &notes {
                    println!("# skipped: {note}");
                }
            }
        }
        eprintln!("preview only; run in a TTY to confirm, or pass --yes to write");
        return Ok(0);
    }

    let path = project.config_path();
    let original = fs::read_to_string(&path).unwrap_or_default();
    let mut new_text = original.clone();
    if !new_text.is_empty() && !new_text.ends_with('\n') {
        new_text.push('\n');
    }
    new_text.push_str(&block);
    fs::write(&path, &new_text).context("write .loom/config.toml")?;
    if let Err(error) = Config::load(&path) {
        // Roll back: a half-valid config would break every later command.
        if original.is_empty() {
            let _ = fs::remove_file(&path);
        } else {
            let _ = fs::write(&path, &original);
        }
        bail!("drafted config is invalid ({error}); nothing written");
    }

    match format {
        Format::Json => println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "path": project.relative(&path),
                "written": written,
                "skipped": notes,
            }))?
        ),
        Format::Pretty => {
            println!(
                "wrote {} entries to {}",
                written.len(),
                project.relative(&path)
            );
            for alias in &written {
                println!("  + {alias}");
            }
            for note in &notes {
                println!("  # skipped: {note}");
            }
            println!("next: loom capability list");
        }
    }
    Ok(0)
}

/// Prompt-driven drafts. Questions go to stderr; stdout stays parseable.
fn interactive_drafts(
    pairs: &[Pair],
    vars: &[(String, String)],
    existing: &HashSet<String>,
) -> Result<(Vec<Draft>, Vec<String>)> {
    let mut drafts = Vec::new();
    let mut notes = Vec::new();
    let mut seen = existing.clone();
    for pair in pairs {
        eprintln!(
            "检测到 {}_API_BASE → {} (key: {})",
            pair.prefix,
            pair.base_url,
            pair.key_env.as_deref().unwrap_or("(env fallback)")
        );
        let default_alias = pair.short.to_lowercase();
        let alias = prompt("alias", Some(&default_alias))?;
        if !seen.insert(alias.clone()) {
            notes.push(format!("alias {alias} already configured; skipped"));
            continue;
        }
        let default_model = hint(vars, &pair.short, "MODEL");
        let model_id = match default_model.as_deref() {
            Some(value) => prompt("model_id", Some(value))?,
            None => prompt_required("model_id")?,
        };
        let tier_value = prompt("tier (worker/frontier)", Some("worker"))?;
        let tier = parse_value("tier", &tier_value, Tier::from_str)?;
        let default_wire = hint(vars, &pair.short, "WIRE").unwrap_or_else(|| "chat".to_string());
        let wire_value = prompt("wire (chat/messages)", Some(&default_wire))?;
        let wire = parse_value("wire", &wire_value, Wire::from_str)?;
        drafts.push(Draft {
            alias,
            model_id: Some(model_id),
            tier,
            wire,
            base_url: pair.base_url.clone(),
            key_env: pair.key_env.clone(),
        });
    }
    Ok((drafts, notes))
}

fn prompt(label: &str, default: Option<&str>) -> Result<String> {
    match default {
        Some(value) => eprint!("? {label} [{value}]: "),
        None => eprint!("? {label}: "),
    }
    std::io::stderr().flush().ok();
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line)? == 0 {
        bail!("stdin closed");
    }
    let value = line.trim().to_string();
    if value.is_empty() {
        if let Some(default) = default {
            return Ok(default.to_string());
        }
        return prompt_required(label);
    }
    Ok(value)
}

fn prompt_required(label: &str) -> Result<String> {
    loop {
        eprint!("? {label}: ");
        std::io::stderr().flush().ok();
        let mut line = String::new();
        if std::io::stdin().read_line(&mut line)? == 0 {
            bail!("{label} is required; stdin closed");
        }
        let value = line.trim().to_string();
        if !value.is_empty() {
            return Ok(value);
        }
        eprintln!("  不能为空");
    }
}

fn parse_value<T>(
    label: &str,
    value: &str,
    parse: fn(&str) -> std::result::Result<T, String>,
) -> Result<T> {
    parse(value).map_err(|message| anyhow!("{label}: {message}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn detect_pairs_and_hints() {
        let vars = vars(&[
            ("LOOM_DEEPSEEK_API_BASE", "https://api.deepseek.com/v1"),
            ("LOOM_DEEPSEEK_API_KEY", "sk-x"),
            ("LOOM_GPT_API_BASE", "https://relay.example/v1"),
            ("LOOM_GPT_API_KEY", ""),
            ("LOOM_LIVE_DEEPSEEK_MODEL", "deepseek-v4-flash"),
            ("LOOM_LIVE_DEEPSEEK_WIRE", "chat"),
            ("UNRELATED", "1"),
        ]);
        let pairs = detect(&vars);
        assert_eq!(pairs.len(), 2);
        assert_eq!(pairs[0].short, "DEEPSEEK");
        assert_eq!(pairs[0].key_env.as_deref(), Some("LOOM_DEEPSEEK_API_KEY"));
        // Empty key value counts as absent.
        assert_eq!(pairs[1].short, "GPT");
        assert_eq!(pairs[1].key_env, None);

        let (drafts, notes) = auto_drafts(&pairs, &vars, &HashSet::new());
        // GPT has no model_id hint → skipped with a note.
        assert_eq!(drafts.len(), 1);
        assert_eq!(drafts[0].alias, "deepseek");
        assert_eq!(drafts[0].model_id.as_deref(), Some("deepseek-v4-flash"));
        assert!(notes[0].contains("no model_id hint for gpt"));
    }

    #[test]
    fn generic_base_becomes_default_alias() {
        let vars = vars(&[("LOOM_API_BASE", "https://api.openai.com/v1")]);
        let pairs = detect(&vars);
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].short, "DEFAULT");
        let (drafts, _) = auto_drafts(&pairs, &vars, &HashSet::new());
        // No model hint and no key → skipped, never guessed.
        assert!(drafts.is_empty());
    }

    #[test]
    fn block_is_valid_toml_and_round_trips() {
        let draft = Draft {
            alias: "deepseek".into(),
            model_id: Some("deepseek-v4-flash".into()),
            tier: Tier::Worker,
            wire: Wire::Chat,
            base_url: "https://api.deepseek.com/v1".into(),
            key_env: Some("DEEPSEEK_API_KEY".into()),
        };
        let block = render_block(&[draft]).expect("render");
        assert!(block.contains("[[models]]"));
        // Comments are valid TOML, so the whole block parses as a Config.
        let config: Config = toml::from_str(&block).expect("parse block");
        assert_eq!(config.models.len(), 1);
        assert_eq!(config.models[0].alias, "deepseek");
        assert_eq!(
            config.models[0].key_env.as_deref(),
            Some("DEEPSEEK_API_KEY")
        );
        config.validate().expect("valid");
    }

    #[test]
    fn existing_alias_is_skipped() {
        let vars = vars(&[
            ("LOOM_DEEPSEEK_API_BASE", "https://api.deepseek.com/v1"),
            ("LOOM_DEEPSEEK_API_KEY", "sk-x"),
            ("LOOM_LIVE_DEEPSEEK_MODEL", "deepseek-v4-flash"),
        ]);
        let pairs = detect(&vars);
        let mut existing = HashSet::new();
        existing.insert("deepseek".to_string());
        let (drafts, notes) = auto_drafts(&pairs, &vars, &existing);
        assert!(drafts.is_empty());
        assert!(notes[0].contains("already configured"));
    }
}
