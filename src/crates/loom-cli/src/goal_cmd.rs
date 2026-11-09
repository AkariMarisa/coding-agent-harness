//! `loom goal` / `loom constraints`: model-drafted contract files.
//!
//! Tool drafts, human reviews the diff (AGENTS.md §10). There are no fill-in
//! placeholders: `loom init` never asks anyone to write an md file by hand.

use std::fs;
use std::path::Path;

use anyhow::{bail, Context as _, Result};
use loom_context::ContextKind;
use loom_core::{estimate_tokens, Config};
use loom_native::{ChatMessage, ChatRequest, OpenAiProvider, Provider};
use loom_store::Project;
use serde_json::json;

use super::Format;

/// Which contract file is being drafted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Intent {
    Goal,
    Constraints,
}

impl Intent {
    const fn file_name(self) -> &'static str {
        match self {
            Self::Goal => "goal.md",
            Self::Constraints => "constraints.md",
        }
    }

    fn template(self) -> &'static str {
        match self {
            Self::Goal => include_str!("../prompts/goal-draft@1.md"),
            Self::Constraints => include_str!("../prompts/constraints-draft@1.md"),
        }
    }
}

/// Evidence byte budget: same "summary + address" rule the tools follow —
/// never ship a whole repo into one prompt.
const EVIDENCE_CAP: usize = 12 * 1024;

/// Per-file caps so one long file cannot eat the whole evidence budget.
const README_CAP: usize = 4_000;
const MANIFEST_CAP: usize = 2_500;
const SUPPORT_CAP: usize = 1_500;
const ALIGN_CAP: usize = 800;

pub fn draft(
    project: &Project,
    intent: Intent,
    model: Option<&str>,
    force: bool,
    format: Format,
) -> Result<i32> {
    let config = project
        .load_config()
        .context("load .loom/config.toml (run `loom init` first)")?;
    let alias = pick_alias(&config, model)?;
    let capability = config.model(alias);
    let model_id = capability
        .and_then(|capability| capability.model_id.clone())
        .unwrap_or_else(|| alias.to_string());

    let target = project.loom_dir().join(intent.file_name());
    if target.exists() && !force {
        let existing = fs::read_to_string(&target).unwrap_or_default();
        bail!(
            "{} already exists ({} tokens); review it, or rerun with --force to regenerate (uncommitted edits would be lost)",
            project.relative(&target),
            estimate_tokens(&existing)
        );
    }

    let evidence = gather_evidence(&project.root, intent);
    let system = intent.template().replace("{{evidence}}", &evidence);

    let mut provider = OpenAiProvider::from_env_with_capability(capability)?;
    let response = provider
        .complete(&ChatRequest {
            model: model_id.clone(),
            messages: vec![
                ChatMessage::system(system),
                ChatMessage::user("直接输出 Markdown 正文，不要代码围栏，不要解释。"),
            ],
            tools: json!([]),
            max_tokens: 2_000,
            temperature: 0.2,
        })
        .with_context(|| format!("draft {} via {alias}", intent.file_name()))?;

    let text = strip_fence(response.message.content.unwrap_or_default().trim());
    if text.is_empty() {
        bail!("provider returned an empty draft");
    }

    let tokens = estimate_tokens(&text);
    let cap = ContextKind::Worker.budget().goal;
    let mut warnings = Vec::new();
    if tokens > cap {
        warnings.push(format!(
            "draft is {tokens} tokens > worker cap {cap}; worker packets will truncate it — trim before commit"
        ));
    }

    fs::write(&target, format!("{text}\n"))
        .with_context(|| format!("write {}", project.relative(&target)))?;

    let path = project.relative(&target);
    match format {
        Format::Json => println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "path": path,
                "alias": alias,
                "model_id": model_id,
                "tokens": tokens,
                "cap_tokens": cap,
                "warnings": warnings,
            }))?
        ),
        Format::Pretty => {
            println!("drafted {path}");
            println!("  model:  {alias} ({model_id})");
            println!("  tokens: {tokens} (worker cap {cap})");
            for warning in &warnings {
                println!("  warning: {warning}");
            }
            println!("review:  git diff {path}");
        }
    }
    Ok(0)
}

/// One configured model is used automatically; otherwise the alias must be
/// given. Fail fast with the available list instead of guessing.
fn pick_alias<'a>(config: &'a Config, requested: Option<&'a str>) -> Result<&'a str> {
    let available = || {
        config
            .models
            .iter()
            .map(|model| model.alias.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    };
    match requested {
        Some(alias) => {
            if config.model(alias).is_some() {
                Ok(alias)
            } else {
                bail!(
                    "model alias {alias} is not in .loom/config.toml; configured: {}",
                    available()
                )
            }
        }
        None => match config.models.len() {
            0 => bail!("no [[models]] in .loom/config.toml; add one or pass --model"),
            1 => Ok(config.models[0].alias.as_str()),
            count => bail!(
                "{count} models configured; pass --model <alias> ({})",
                available()
            ),
        },
    }
}

/// Models sometimes wrap answers in a fence despite instructions.
fn strip_fence(text: &str) -> String {
    let mut text = text;
    if text.starts_with("```") {
        if let Some(index) = text.find('\n') {
            text = &text[index + 1..];
        }
        if let Some(index) = text.rfind("```") {
            text = &text[..index];
        }
    }
    text.trim().to_string()
}

/// Bounded repo evidence: README, manifests, layout, and for constraints the
/// lint/CI/entry-point files. `.env*` is never read (only `.env.example`).
pub(crate) fn gather_evidence(root: &Path, intent: Intent) -> String {
    let mut out = String::new();
    let mut budget = EVIDENCE_CAP;

    let name = root
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default();
    out.push_str(&format!("# 仓库根目录：{name}\n"));

    for candidate in ["README.md", "readme.md", "README.rst", "README"] {
        if root.join(candidate).is_file() {
            push_file(&mut out, &mut budget, root, candidate, README_CAP);
            break;
        }
    }

    let mut manifests = 0;
    for candidate in [
        "pyproject.toml",
        "package.json",
        "Cargo.toml",
        "go.mod",
        "setup.py",
        "Gemfile",
        "composer.json",
    ] {
        if root.join(candidate).is_file() {
            push_file(&mut out, &mut budget, root, candidate, MANIFEST_CAP);
            manifests += 1;
            if manifests == 2 {
                break;
            }
        }
    }

    push_listing(&mut out, &mut budget, root, ".", 100);

    match intent {
        Intent::Goal => {
            // Keep the goal draft aligned with already-agreed constraints.
            push_file(
                &mut out,
                &mut budget,
                root,
                ".loom/constraints.md",
                ALIGN_CAP,
            );
        }
        Intent::Constraints => {
            for candidate in [
                ".flake8",
                "ruff.toml",
                "setup.cfg",
                ".eslintrc",
                ".eslintrc.json",
                "rustfmt.toml",
                ".editorconfig",
                ".pre-commit-config.yaml",
                ".gitlab-ci.yml",
            ] {
                if root.join(candidate).is_file() {
                    push_file(&mut out, &mut budget, root, candidate, SUPPORT_CAP);
                }
            }
            // Example env only: real env files carry credentials.
            push_file(&mut out, &mut budget, root, ".env.example", ALIGN_CAP);
            push_listing(&mut out, &mut budget, root, ".github/workflows", 20);
            push_file(&mut out, &mut budget, root, ".loom/goal.md", ALIGN_CAP);
            push_file(
                &mut out,
                &mut budget,
                root,
                ".loom/constraints.md",
                ALIGN_CAP,
            );
        }
    }
    out
}

fn push_section(out: &mut String, budget: &mut usize, header: &str, body: &str) {
    if *budget == 0 || body.trim().is_empty() {
        return;
    }
    let take = take_bytes(body, *budget);
    out.push_str(&format!("\n## {header}\n```\n{take}\n"));
    if take.len() < body.len() {
        out.push_str("[…truncated]\n");
    }
    out.push_str("```\n");
    *budget -= take.len();
}

fn push_file(out: &mut String, budget: &mut usize, root: &Path, relative: &str, max: usize) {
    let Ok(body) = fs::read_to_string(root.join(relative)) else {
        return;
    };
    let capped = take_bytes(&body, max);
    if *budget == 0 || capped.is_empty() {
        return;
    }
    push_section(out, budget, relative, capped);
    if capped.len() < body.len() {
        out.push_str("[…truncated]\n");
    }
}

/// Names only — layout hints, not file contents.
fn push_listing(out: &mut String, budget: &mut usize, root: &Path, relative: &str, limit: usize) {
    let Ok(entries) = fs::read_dir(root.join(relative)) else {
        return;
    };
    let mut names: Vec<String> = entries
        .flatten()
        .map(|entry| {
            let mut name = entry.file_name().to_string_lossy().to_string();
            if entry.path().is_dir() {
                name.push('/');
            }
            name
        })
        .filter(|name| name != ".git/")
        .collect();
    names.sort();
    names.truncate(limit);
    push_section(out, budget, relative, &names.join("\n"));
}

fn take_bytes(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evidence_captures_readme_and_manifest() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(dir.path().join("README.md"), "hello readme").expect("write");
        fs::write(dir.path().join("pyproject.toml"), "[project]").expect("write");
        fs::create_dir(dir.path().join("src")).expect("mkdir");

        let text = gather_evidence(dir.path(), Intent::Goal);
        assert!(text.contains("hello readme"));
        assert!(text.contains("pyproject.toml"));
        assert!(text.contains("src/"));
        // Goal drafts align with existing constraints, not with lint configs.
        assert!(!text.contains(".flake8"));
    }

    #[test]
    fn evidence_stays_within_cap() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(dir.path().join("README.md"), "a".repeat(100_000)).expect("write");

        let text = gather_evidence(dir.path(), Intent::Constraints);
        assert!(
            text.len() <= EVIDENCE_CAP + 1024,
            "evidence {} bytes exceeds cap {}",
            text.len(),
            EVIDENCE_CAP
        );
        assert!(text.contains("[…truncated]"));
    }

    #[test]
    fn strips_wrapping_fence() {
        assert_eq!(strip_fence("```markdown\n# Goal\n```"), "# Goal");
        assert_eq!(strip_fence("# Goal"), "# Goal");
    }
}
