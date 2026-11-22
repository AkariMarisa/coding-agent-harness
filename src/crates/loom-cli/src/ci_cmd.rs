//! `loom ci` — the local CI entry point for target projects.
//!
//! Spec: `doc/15` §4, locked implementation subset `doc/16` §7. Plan is the
//! read-only detection view; run/doctor/report land with the executor.
//! Exit codes follow `doc/16` §7.5: 0 pass, 1 failed, 2 needs a human,
//! 3 timeout, 4 configuration/execution error.

use anyhow::{Context as _, Result};
use loom_ci::{detect, Detection, CI_CONFIG_NAME};

use super::{CiCommand, Format};

pub fn ci(project: &loom_store::Project, command: CiCommand, format: Format) -> Result<i32> {
    match command {
        CiCommand::Plan { profile, json } => plan(project, profile.as_deref(), json, format),
        CiCommand::Run {
            profile,
            keep_going,
            fix,
            json,
        } => run_profile(project, profile.as_deref(), keep_going, fix, json, format),
    }
}

fn run_profile(
    project: &loom_store::Project,
    profile: Option<&str>,
    keep_going: bool,
    fix: bool,
    json: bool,
    format: Format,
) -> Result<i32> {
    let config = project.load_config()?;
    let ci_file = loom_ci::load_ci_file(&project.root)
        .map_err(|error| anyhow::anyhow!(error))?
        .ok_or_else(|| {
            anyhow::anyhow!(
                "no {} — run `loom ci plan` (or `loom init`) first",
                loom_ci::CI_CONFIG_NAME
            )
        })?;
    let profile = profile
        .map(str::to_owned)
        .unwrap_or_else(|| config.ci.default_profile.clone());

    let outcome = loom_ci::run(loom_ci::RunRequest {
        root: &project.root,
        file: &ci_file,
        profile: &profile,
        keep_going,
        fix,
        now_ms: loom_store::now_ms(),
    })?;

    // `doc/16` §7.9: failures are resident items, green resolves them.
    link_attention(project, &outcome)?;

    if json || matches!(format, Format::Json) {
        println!(
            "{}",
            serde_json::to_string_pretty(&outcome.report).context("serialize CI report")?
        );
    } else {
        print!("{}", render_run(&outcome));
    }
    Ok(outcome.exit_code)
}

/// `doc/16` §7.9: a failed job opens a `review` item keyed
/// `ci:<job>:<tree>`; a green job on the same tree resolves it; a missing
/// tool opens a blocking `operation`.
fn link_attention(project: &loom_store::Project, outcome: &loom_ci::RunOutcome) -> Result<()> {
    let tree = loom_ci::tree_id(&outcome.report.tree);
    for execution in &outcome.executions {
        let dedupe_key = format!("ci:{}:{tree}", execution.report.id);
        if execution.passed {
            loom_store::resolve_by_key(
                project,
                "loom-ci",
                &dedupe_key,
                "ci",
                "green on this tree",
            )?;
        } else {
            loom_store::open_item(
                project,
                &loom_store::NewItem {
                    run_id: "loom-ci".to_string(),
                    task_id: None,
                    kind: loom_core::AttentionKind::Review,
                    source: loom_core::AttentionSource::Ci,
                    severity: loom_core::Severity::Soon,
                    dedupe_key,
                    title: format!(
                        "CI job {} failed on {}",
                        execution.report.id, outcome.report.profile
                    ),
                    detail: Some(execution.summary.clone()),
                    refs: vec![execution.report.log_path.clone()],
                    actions: vec![loom_core::AttentionAction {
                        id: "rerun".to_string(),
                        label: "Re-run the profile".to_string(),
                        cmd: format!("loom ci run --profile {}", outcome.report.profile),
                    }],
                },
            )?;
        }
    }
    if let Some(tool) = &outcome.missing_tool {
        loom_store::open_item(
            project,
            &loom_store::NewItem {
                run_id: "loom-ci".to_string(),
                task_id: None,
                kind: loom_core::AttentionKind::Operation,
                source: loom_core::AttentionSource::Env,
                severity: loom_core::Severity::Blocking,
                dedupe_key: format!("env:missing-tool:{tool}"),
                title: format!("CI needs {tool}, which is not on PATH"),
                detail: Some(
                    "install it or point the runner at an environment that has it".to_string(),
                ),
                refs: vec![loom_ci::CI_CONFIG_NAME.to_string()],
                actions: vec![loom_core::AttentionAction {
                    id: "plan".to_string(),
                    label: "Review the plan".to_string(),
                    cmd: "loom ci plan".to_string(),
                }],
            },
        )?;
    }
    loom_store::sync_inbox(project)?;
    Ok(())
}

fn render_run(outcome: &loom_ci::RunOutcome) -> String {
    let mut out = format!("{}  profile={}\n", outcome.run_id, outcome.report.profile);
    for execution in &outcome.executions {
        let (state, detail) = if execution.passed {
            ("ok", String::new())
        } else if execution.report.timed_out {
            (
                "TIMEOUT",
                format!("exit after {}ms", execution.report.duration_ms),
            )
        } else {
            (
                "FAIL",
                format!(
                    "exit {}",
                    execution
                        .report
                        .exit
                        .map(|code| code.to_string())
                        .unwrap_or_else(|| "signal".to_string())
                ),
            )
        };
        out.push_str(&format!(
            "  {state:<7} {:<12} {:<10} {:<22} {}\n",
            execution.report.id,
            format!("{}ms", execution.report.duration_ms),
            detail,
            execution.report.log_path
        ));
        if !execution.passed && !execution.summary.is_empty() {
            let lines: Vec<&str> = execution.summary.lines().collect();
            let tail: Vec<&str> = lines.iter().rev().take(30).copied().rev().collect();
            for line in tail {
                out.push_str(&format!("          | {line}\n"));
            }
        }
    }
    if let Some(tool) = &outcome.missing_tool {
        out.push_str(&format!("  missing tool: {tool}\n"));
    }
    if let Some(diff) = &outcome.diff_stat {
        out.push_str("--- git diff --stat (nothing staged, nothing committed) ---\n");
        out.push_str(diff);
        if !diff.ends_with('\n') {
            out.push('\n');
        }
    }
    out.push_str(&format!(
        "report: {}\n",
        outcome
            .report_path
            .strip_prefix(&outcome.root)
            .unwrap_or(&outcome.report_path)
            .display()
    ));
    out
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
