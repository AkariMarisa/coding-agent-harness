//! `loom skill` — skills governance (`doc/15` §3).
//!
//! v1 scope follows `doc/16` §9: profile scan, catalog/recommend, integrity
//! lock, static checks and progressive disclosure — never a second loader.

use anyhow::Result;

use loom_skill::{scan, STACK_PROFILE_NAME};

use super::{Format, SkillCommand};

pub fn skill(project: &loom_store::Project, command: SkillCommand, format: Format) -> Result<i32> {
    match command {
        SkillCommand::Scan => scan_profile(project, format),
        SkillCommand::Recommend { task } => recommend(project, &task, format),
        SkillCommand::Verify { write_lock } => verify(project, write_lock, format),
        SkillCommand::Show { id, json } => show(project, &id, json),
    }
}

/// `loom skill show <id>`: the other half of progressive disclosure
/// (`doc/15` §3.6) — plain text by default (the consumer is a model reading
/// a tool result), `--json` for structure. Bodies are capped at 4KB.
fn show(project: &loom_store::Project, id: &str, json: bool) -> Result<i32> {
    let skill =
        loom_skill::read_skill(&project.root, id).map_err(|error| anyhow::anyhow!(error))?;
    if json {
        let value = serde_json::json!({
            "v": 1,
            "id": skill.id,
            "path": skill.path,
            "tokens": skill.tokens,
            "truncated": skill.truncated,
            "total_bytes": skill.total_bytes,
            "body": skill.body,
            "references": skill.references,
        });
        println!("{}", serde_json::to_string_pretty(&value)?);
        return Ok(0);
    }

    print!("{}", skill.body);
    if !skill.body.ends_with('\n') {
        println!();
    }
    if skill.truncated {
        println!(
            "… [truncated: showing {} of {} bytes; full body at {}]",
            skill.body.len(),
            skill.total_bytes,
            skill.path
        );
    }
    if !skill.references.is_empty() {
        println!(
            "references (read on demand): {}",
            skill.references.join(", ")
        );
    }
    Ok(0)
}

/// `loom skill verify`: the L0 gate (`doc/15` §3.7, `doc/16` §9.3).
/// Exit 0 = green (warnings allowed), 1 = findings, 4 = configuration error.
fn verify(project: &loom_store::Project, write_lock: bool, format: Format) -> Result<i32> {
    let outcome =
        loom_skill::verify(&project.root, write_lock).map_err(|error| anyhow::anyhow!(error))?;
    match format {
        Format::Json => {
            println!("{}", serde_json::to_string_pretty(&outcome)?);
        }
        Format::Pretty => {
            let fails = outcome
                .findings
                .iter()
                .filter(|finding| finding.level == loom_skill::Level::Fail)
                .count();
            let warns = outcome.findings.len() - fails;
            let state = if outcome.ok { "PASS" } else { "FAIL" };
            println!(
                "skills verify: {state} — {} skill(s), {fails} failure(s), {warns} warning(s)",
                outcome.checked
            );
            if outcome.lock_written > 0 {
                println!(
                    "  lock: wrote {} entries to {}",
                    outcome.lock_written,
                    loom_skill::LOCK_REL
                );
            }
            for finding in &outcome.findings {
                let marker = match finding.level {
                    loom_skill::Level::Fail => "fail",
                    loom_skill::Level::Warn => "warn",
                };
                println!("  {marker} {}: {}", finding.skill, finding.message);
            }
        }
    }
    Ok(if outcome.ok { 0 } else { 1 })
}

/// `loom skill recommend --task T-…`: score the catalog against the repo
/// profile and one task; the output is advice for the human slicer, never
/// an auto-declaration (`doc/15` §3.4, §12.3 rule 1).
fn recommend(project: &loom_store::Project, task_id: &str, format: Format) -> Result<i32> {
    let spec = loom_store::read_task(project, task_id)?;
    // Refresh the profile first (cached unless the tree moved).
    let profile = scan(&project.root)?.profile;
    let catalog = loom_skill::load_catalog(&project.root)
        .map_err(|error| anyhow::anyhow!(error))?
        .ok_or_else(|| {
            anyhow::anyhow!(
                "no {} — create it to enable recommendations",
                loom_skill::CATALOG_REL
            )
        })?;
    let suggestions = loom_skill::recommend(&loom_skill::RecommendInput {
        root: &project.root,
        profile: &profile,
        catalog: &catalog,
        role: spec.role.as_str(),
        loaded: &spec.skills,
    });

    match format {
        Format::Json => {
            let value = serde_json::json!({
                "v": 1,
                "task_id": spec.task_id,
                "role": spec.role.as_str(),
                "threshold": loom_skill::AUTO_THRESHOLD,
                "max_suggestions": loom_skill::MAX_SUGGESTIONS,
                "max_auto": loom_skill::MAX_AUTO,
                "suggestions": suggestions,
            });
            println!("{}", serde_json::to_string_pretty(&value)?);
        }
        Format::Pretty => {
            println!(
                "recommendations for {} (role={}, threshold {}, auto ≤ {}):",
                spec.task_id,
                spec.role.as_str(),
                loom_skill::AUTO_THRESHOLD,
                loom_skill::MAX_AUTO
            );
            if suggestions.is_empty() {
                println!("  (nothing above the floor)");
            }
            for suggestion in &suggestions {
                let label = if suggestion.auto {
                    "auto   "
                } else {
                    "suggest"
                };
                println!(
                    "  {label} {:<24} score {:>5}  {} tok",
                    suggestion.id, suggestion.score, suggestion.tokens
                );
                for why in &suggestion.why {
                    println!("           - {why}");
                }
            }
        }
    }
    Ok(0)
}

/// `loom skill scan`: deterministic repo profile into `.loom/stack-profile.json`.
fn scan_profile(project: &loom_store::Project, format: Format) -> Result<i32> {
    let outcome = scan(&project.root)?;
    match format {
        Format::Json => {
            println!("{}", serde_json::to_string_pretty(&outcome.profile)?);
        }
        Format::Pretty => {
            let profile = &outcome.profile;
            let state = if outcome.written { "updated" } else { "cached" };
            let path = loom_skill::profile_path(&project.root);
            println!(
                "stack profile: {} ({state})",
                path.strip_prefix(&project.root).unwrap_or(&path).display()
            );
            let join = |items: &[String]| {
                if items.is_empty() {
                    "-".to_string()
                } else {
                    items.join(", ")
                }
            };
            println!("  lang:       {}", join(&profile.lang));
            println!("  infra:      {}", join(&profile.infra));
            println!("  tests:      {}", join(&profile.tests));
            if !profile.toolchain.is_empty() {
                let pins: Vec<String> = profile
                    .toolchain
                    .iter()
                    .map(|(name, value)| format!("{name}={value}"))
                    .collect();
                println!("  toolchain:  {}", pins.join(", "));
            }
            println!(
                "  git_head:   {}",
                profile.git_head.as_deref().unwrap_or("(not a repository)")
            );
            for (tag, files) in &profile.evidence {
                println!("  evidence:   {tag} ← {}", files.join(", "));
            }
            println!("  hash:       {}", profile.hash);
        }
    }
    Ok(0)
}

/// Used by `loom init` to keep the profile in step (`doc/15` §4.4).
pub fn refresh_profile_line(project: &loom_store::Project) -> Result<String> {
    let outcome = scan(&project.root)?;
    let state = if outcome.written { "updated" } else { "cached" };
    let lang = if outcome.profile.lang.is_empty() {
        "-".to_string()
    } else {
        outcome.profile.lang.join(", ")
    };
    Ok(format!("{STACK_PROFILE_NAME} ({state}): lang={lang}"))
}
