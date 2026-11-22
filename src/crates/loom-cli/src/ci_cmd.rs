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
        CiCommand::Doctor { json } => doctor(project, json, format),
        CiCommand::Report { run_id, json } => report(project, run_id.as_deref(), json, format),
        CiCommand::VerifyAttest { path, json } => verify_attest(project, &path, json, format),
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
    let mut detection = detect(&project.root);
    // An existing tracked configuration wins over fresh suggestions: plan
    // must show what will actually run (`doc/16` §7.6 keeps the file by
    // default; detection alone would drift from the committed policy).
    if let Some(file) =
        loom_ci::load_ci_file(&project.root).map_err(|error| anyhow::anyhow!(error))?
    {
        detection.file = Some(file);
        detection.sufficient = true;
        detection.notes.push(format!(
            "existing {CI_CONFIG_NAME} shown; detection suggestions ignored"
        ));
    }

    if let Some(profile) = profile {
        // A profile filter only makes sense against a file to run; asking
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

pub(crate) fn render_pretty(detection: &Detection) -> String {
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

/// `loom ci doctor`: toolchain, config, and last-run diagnosis. Advisory —
/// it prints problems and exits 2 when a human is needed, never 4.
fn doctor(project: &loom_store::Project, json: bool, format: Format) -> Result<i32> {
    let mut missing: Vec<String> = Vec::new();
    let mut config_error: Option<String> = None;
    let mut config_info: Option<serde_json::Value> = None;

    // A broken config is a diagnosis, not a crash: doctor reports it.
    match loom_ci::load_ci_file(&project.root) {
        Err(error) => config_error = Some(error),
        Ok(None) => {
            config_error = Some(format!("{CI_CONFIG_NAME} not found — run `loom init`"));
        }
        Ok(Some(file)) => {
            let runner = file.runner.as_ref().map(|runner| runner.argv.clone());
            for job in &file.jobs {
                if let Some(tool) =
                    loom_ci::missing_tool(runner.as_deref(), &job.argv[0], &job.required_tools)
                {
                    if !missing.contains(&tool) {
                        missing.push(tool);
                    }
                }
            }
            config_info = Some(serde_json::json!({
                "path": CI_CONFIG_NAME,
                "jobs": file.jobs.len(),
                "profiles": file.profiles.len(),
                "runner": runner,
            }));
        }
    }

    let detection = detect(&project.root);
    let last_run = latest_report(&project.loom_dir().join("ci"));
    let ok = config_error.is_none() && missing.is_empty();

    if json || matches!(format, Format::Json) {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "v": 1,
                "ok": ok,
                "config": config_info,
                "config_error": config_error,
                "stack": detection.evidence,
                "missing_tools": missing,
                "last_run": last_run,
                "hooks": "not_installed",
            }))?
        );
    } else {
        match (&config_info, &config_error) {
            (Some(info), _) => println!(
                "config:   {} — {} jobs, {} profiles, runner {}",
                info["path"],
                info["jobs"],
                info["profiles"],
                info["runner"]
                    .as_array()
                    .map(|argv| argv
                        .iter()
                        .filter_map(|v| v.as_str())
                        .collect::<Vec<_>>()
                        .join(" "))
                    .filter(|text| !text.is_empty())
                    .unwrap_or_else(|| "(none, jobs run directly)".to_string())
            ),
            (None, Some(error)) => println!("config:   {error}"),
            (None, None) => println!("config:   missing"),
        }
        if missing.is_empty() && config_info.is_some() {
            println!("tools:    all present");
        } else if !missing.is_empty() {
            println!("tools:    missing: {}", missing.join(", "));
        }
        for item in &detection.evidence {
            println!("stack:    {} — {}", item.path, item.detail);
        }
        match &last_run {
            Some(run) => println!(
                "last run: {} {} ok={} ({}ms)",
                run["run_id"].as_str().unwrap_or("?"),
                run["profile"].as_str().unwrap_or("?"),
                run["ok"],
                run["duration_ms"].as_u64().unwrap_or(0)
            ),
            None => println!("last run: none yet"),
        }
        println!("hooks:    not installed — `loom ci install-hooks` lands with the hooks phase");
    }
    // Exit 2 = needs human handling (missing config or tools), per §7.5.
    Ok(if ok { 0 } else { 2 })
}

/// `loom ci report [run-id]`: the latest run by default. The run id is
/// positional because the global `--run` flag already exists (`doc/16` §7.5
/// spells it `--run <id>`; the flag name is taken).
fn report(
    project: &loom_store::Project,
    run_id: Option<&str>,
    json: bool,
    format: Format,
) -> Result<i32> {
    let ci_dir = project.loom_dir().join("ci");
    let dir = match run_id {
        Some(id) => ci_dir.join(id),
        None => latest_report_dir(&ci_dir)
            .ok_or_else(|| anyhow::anyhow!("no CI runs yet — `loom ci run` first"))?,
    };
    let path = dir.join("report.json");
    let text =
        std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    let value: serde_json::Value =
        serde_json::from_str(&text).with_context(|| format!("parse {}", path.display()))?;

    if json || matches!(format, Format::Json) {
        println!("{}", serde_json::to_string_pretty(&value)?);
        return Ok(0);
    }

    println!(
        "run:      {} profile={} ok={}",
        value["run_id"].as_str().unwrap_or("?"),
        value["profile"].as_str().unwrap_or("?"),
        value["ok"]
    );
    println!(
        "tree:     head={} dirty={}",
        value["tree"]["head"].as_str().unwrap_or("none"),
        value["tree"]["dirty"]
    );
    if let Some(tool) = value["missing_tool"].as_str() {
        println!("missing:  {tool}");
    }
    println!("jobs:");
    if let Some(jobs) = value["jobs"].as_array() {
        for job in jobs {
            println!(
                "  {:<12} exit={:<5} {:>6}ms  {}",
                job["id"].as_str().unwrap_or("?"),
                job["exit"]
                    .as_i64()
                    .map(|code| code.to_string())
                    .unwrap_or_else(|| "signal".to_string()),
                job["duration_ms"].as_u64().unwrap_or(0),
                job["log_path"].as_str().unwrap_or("")
            );
        }
    }
    println!(
        "report:   {}",
        path.strip_prefix(&project.root).unwrap_or(&path).display()
    );
    Ok(0)
}

/// `loom ci verify-attest <path>`: prove the file matches this tree —
/// nothing more (unsigned evidence, `doc/16` §7.8).
fn verify_attest(
    project: &loom_store::Project,
    path: &str,
    json: bool,
    format: Format,
) -> Result<i32> {
    let candidate = std::path::Path::new(path);
    let resolved = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        project.root.join(candidate)
    };
    let outcome = loom_ci::verify_attestation(&project.root, &resolved)
        .map_err(|error| anyhow::anyhow!(error))?;

    if json || matches!(format, Format::Json) {
        println!("{}", serde_json::to_string_pretty(&outcome)?);
    } else if outcome.ok {
        println!(
            "verified: {} matches this tree (unsigned — evidence, not a trust boundary)",
            resolved.display()
        );
    } else {
        println!("FAILED:   {}", resolved.display());
        for reason in &outcome.reasons {
            println!("  - {reason}");
        }
    }
    Ok(if outcome.ok { 0 } else { 1 })
}

fn latest_report_dir(ci_dir: &std::path::Path) -> Option<std::path::PathBuf> {
    let mut dirs: Vec<std::path::PathBuf> = std::fs::read_dir(ci_dir)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    // Run ids sort lexicographically in time order (CI-YYYYMMDD-...).
    dirs.sort();
    dirs.into_iter()
        .find(|dir| dir.join("report.json").exists())
}

fn latest_report(ci_dir: &std::path::Path) -> Option<serde_json::Value> {
    let dir = latest_report_dir(ci_dir)?;
    let text = std::fs::read_to_string(dir.join("report.json")).ok()?;
    serde_json::from_str(&text).ok()
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
