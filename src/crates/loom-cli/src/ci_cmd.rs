//! `loom ci` — the local CI entry point for target projects.
//!
//! Spec: `doc/15` §4, locked implementation subset `doc/16` §7. Plan is the
//! read-only detection view; run/doctor/report land with the executor.
//! Exit codes follow `doc/16` §7.5: 0 pass, 1 failed, 2 needs a human,
//! 3 timeout, 4 configuration/execution error.

use anyhow::Result;
use loom_ci::{detect, Detection, CI_CONFIG_NAME};

use super::{CiCommand, Format};

pub fn ci(project: &loom_store::Project, command: CiCommand, format: Format) -> Result<i32> {
    match command {
        CiCommand::Plan { profile, json } => plan(project, profile.as_deref(), json, format),
    }
}

fn plan(
    project: &loom_store::Project,
    profile: Option<&str>,
    json: bool,
    format: Format,
) -> Result<i32> {
    let detection = detect(&project.root);

    if let Some(profile) = profile {
        // A profile filter only makes sense against a generated file; asking
        // for one when there is nothing to filter is a configuration error.
        let file = detection.file.as_ref().ok_or_else(|| {
            anyhow::anyhow!("no {CI_CONFIG_NAME}: insufficient evidence, nothing to filter")
        })?;
        let jobs = file
            .profiles
            .get(profile)
            .ok_or_else(|| anyhow::anyhow!("unknown profile: {profile}"))?;
        if json || matches!(format, Format::Json) {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "v": 1,
                    "sufficient": detection.sufficient,
                    "profile": profile,
                    "jobs": jobs,
                }))?
            );
        } else {
            println!("profile {profile} (in order):");
            for job in jobs {
                println!("  {job}");
            }
        }
        return Ok(0);
    }

    if json || matches!(format, Format::Json) {
        let mut value = serde_json::to_value(&detection)?;
        value["v"] = serde_json::json!(1);
        println!("{}", serde_json::to_string_pretty(&value)?);
    } else {
        print!("{}", render_pretty(&detection));
    }
    // Exit 2 = "needs human handling": nothing would be generated because
    // the evidence does not support a guess (`doc/16` §7.5).
    Ok(if detection.sufficient { 0 } else { 2 })
}

fn render_pretty(detection: &Detection) -> String {
    let mut out = String::new();
    out.push_str("stack evidence:\n");
    if detection.evidence.is_empty() {
        out.push_str("  (none)\n");
    }
    for item in &detection.evidence {
        out.push_str(&format!("  {:<22} {}\n", item.path, item.detail));
    }
    for note in &detection.notes {
        out.push_str(&format!("  note: {note}\n"));
    }
    match &detection.file {
        None => out.push_str("suggestion: insufficient evidence — nothing would be written\n"),
        Some(file) => {
            if let Some(runner) = &file.runner {
                out.push_str(&format!("runner: {}\n", runner.argv.join(" ")));
            }
            out.push_str("jobs:\n");
            for job in &file.jobs {
                let fix = job
                    .fix_argv
                    .as_ref()
                    .map(|argv| format!("  (fix: {})", argv.join(" ")))
                    .unwrap_or_default();
                out.push_str(&format!("  {:<10} {}{fix}\n", job.id, job.argv.join(" ")));
            }
            out.push_str("profiles:\n");
            for (name, jobs) in &file.profiles {
                out.push_str(&format!("  {name:<10} {}\n", jobs.join(", ")));
            }
            out.push_str(&format!(
                "next: `loom init` writes {CI_CONFIG_NAME} (keeps an existing one)\n"
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pretty_render_names_evidence_jobs_and_profiles() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("Cargo.toml"), "[workspace]\n").expect("write");
        let detection = detect(dir.path());
        let text = render_pretty(&detection);
        assert!(text.contains("stack evidence:"));
        assert!(text.contains("Cargo.toml"));
        assert!(text.contains("fmt"));
        assert!(text.contains("clippy"));
        assert!(text.contains("fast"));
        assert!(
            text.contains("runner") == detection.file.as_ref().is_some_and(|f| f.runner.is_some())
        );
    }

    #[test]
    fn insufficient_detection_renders_a_suggestion_not_a_config() {
        let dir = tempfile::tempdir().expect("tempdir");
        let detection = detect(dir.path());
        assert!(!detection.sufficient);
        let text = render_pretty(&detection);
        assert!(text.contains("insufficient evidence"));
        assert!(!text.contains("jobs:"));
    }
}
