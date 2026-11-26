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
    }
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
