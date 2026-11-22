//! Stack detection and `.loom.ci.toml` generation (`doc/16` §7.6).
//!
//! v1 is single-stack, root-level detection only: it reports evidence, never
//! guesses commands, and with insufficient evidence it writes nothing. The
//! Loom repository's own `./scripts/dev.sh` runner is one example of a
//! runner, not a product default.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::config::{ci_path, CiFile, Job, Runner, CI_CONFIG_NAME, CI_VERSION};

#[derive(Debug, Clone, serde::Serialize)]
pub struct Evidence {
    pub path: String,
    pub detail: String,
}

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct Detection {
    pub sufficient: bool,
    pub evidence: Vec<Evidence>,
    /// Human hints for stacks v1 does not generate for (no guessing).
    pub notes: Vec<String>,
    /// The configuration that detection suggests; `None` when insufficient.
    pub file: Option<CiFile>,
}

#[derive(Debug, Default)]
pub struct BootstrapReport {
    pub detection: Detection,
    pub dry_run: bool,
    /// Set when a configuration was written this call.
    pub written: Option<PathBuf>,
    /// Set when `--force` moved an existing file aside first.
    pub backup: Option<PathBuf>,
    pub kept_existing: bool,
    pub messages: Vec<String>,
}

/// Detect the stack at `root` and, per `doc/16` §7.6, either generate
/// `.loom.ci.toml` or explain why nothing was written.
///
/// Policy: existing file kept by default (hint only); `--force` backs up to
/// `.loom.ci.toml.bak.<ts>` before replacing; `--dry-run` writes nothing;
/// insufficient evidence never writes; `.github/` is never touched.
pub fn bootstrap(root: &Path, dry_run: bool, force: bool) -> Result<BootstrapReport> {
    let detection = detect(root);
    let mut report = BootstrapReport {
        detection,
        dry_run,
        ..Default::default()
    };

    let path = ci_path(root);
    let existed = path.exists();

    if existed && !force {
        report.kept_existing = true;
        report.messages.push(format!(
            "{CI_CONFIG_NAME} already exists — kept; re-run with --force to regenerate (backs up first)"
        ));
        return Ok(report);
    }

    let Some(file) = report.detection.file.clone() else {
        report
            .messages
            .push("insufficient evidence for a single-stack root configuration; suggestions only, nothing written".to_string());
        for note in &report.detection.notes {
            report.messages.push(format!("note: {note}"));
        }
        return Ok(report);
    };

    if existed {
        // Never leave the project without a configuration: the backup happens
        // only when a replacement is about to land.
        let stamp_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_millis() as u64)
            .unwrap_or(0);
        let stamp = loom_core::compact_utc(stamp_ms);
        let backup = path.with_extension(format!("toml.bak.{stamp}"));
        fs::copy(&path, &backup).with_context(|| format!("backup {}", path.display()))?;
        report.backup = Some(backup.clone());
        report
            .messages
            .push(format!("backed up existing file to {}", backup.display()));
    }

    if dry_run {
        report.messages.push(format!(
            "would write {} (dry run, nothing written)",
            path.display()
        ));
        return Ok(report);
    }

    let text = toml::to_string_pretty(&file).context("serialize .loom.ci.toml")?;
    fs::write(&path, format!("{text}\n")).with_context(|| format!("write {}", path.display()))?;
    report.written = Some(path.clone());
    report.messages.push(format!("wrote {}", path.display()));
    Ok(report)
}

/// Single-stack, root-level detection (`doc/16` §7.6): Cargo first, an
/// adoptable `Makefile` target second, everything else is a note.
pub fn detect(root: &Path) -> Detection {
    let mut evidence = Vec::new();
    let mut notes = Vec::new();

    let cargo = root.join("Cargo.toml").exists() || root.join("src/Cargo.toml").exists();
    let dev_sh = root.join("scripts/dev.sh").exists();
    let makefile = read_makefile_targets(root);
    let other_stack = ["package.json", "pyproject.toml", "go.mod"]
        .iter()
        .find(|name| root.join(name).exists())
        .map(|name| name.to_string());

    if cargo {
        evidence.push(Evidence {
            path: "Cargo.toml".to_string(),
            detail: "Rust package/workspace → cargo fmt/clippy/test".to_string(),
        });
    }
    if let Some((path, targets)) = &makefile {
        evidence.push(Evidence {
            path: path.clone(),
            detail: format!("targets: {}", targets.join(", ")),
        });
    }
    if dev_sh {
        evidence.push(Evidence {
            path: "scripts/dev.sh".to_string(),
            detail: "container script → runner.argv".to_string(),
        });
    }
    if let Some(name) = &other_stack {
        notes.push(format!(
            "{name} found; other stacks are deferred in v1 (no guessing)"
        ));
    }

    let runner = dev_sh.then(|| Runner {
        argv: vec!["./scripts/dev.sh".to_string()],
    });

    let jobs = if cargo {
        cargo_jobs()
    } else if let Some((_, targets)) = &makefile {
        if targets.iter().any(|target| target == "check") {
            make_jobs()
        } else {
            Vec::new()
        }
    } else {
        Vec::new()
    };

    let sufficient = !jobs.is_empty();
    if !sufficient && evidence.is_empty() {
        notes.push("no recognizable stack (Cargo.toml, or a Makefile with a check target)".into());
    }
    if !sufficient && !evidence.is_empty() && jobs.is_empty() {
        notes.push("evidence found but no adoptable check command; not writing a guess".into());
    }

    let file = sufficient.then(|| CiFile {
        version: CI_VERSION,
        runner,
        profiles: profiles_for(&jobs),
        jobs,
    });

    Detection {
        sufficient,
        evidence,
        notes,
        file,
    }
}

/// `doc/16` §7.6: generated jobs are `cargo fmt/clippy/test` — the generator
/// never hard-codes these as defaults for non-Cargo projects.
fn cargo_jobs() -> Vec<Job> {
    vec![
        Job {
            id: "fmt".to_string(),
            argv: vec![
                "cargo".into(),
                "fmt".into(),
                "--all".into(),
                "--".into(),
                "--check".into(),
            ],
            cwd: None,
            needs: Vec::new(),
            fix_argv: Some(vec!["cargo".into(), "fmt".into(), "--all".into()]),
            fix_safe: true,
            timeout_s: 120,
            required_tools: vec!["cargo".into()],
            env_pass: Vec::new(),
            local_sufficient: true,
        },
        Job {
            id: "clippy".to_string(),
            argv: vec![
                "cargo".into(),
                "clippy".into(),
                "--workspace".into(),
                "--all-targets".into(),
                "--".into(),
                "-D".into(),
                "warnings".into(),
            ],
            cwd: None,
            needs: Vec::new(),
            fix_argv: None,
            fix_safe: false,
            timeout_s: 600,
            required_tools: vec!["cargo".into()],
            env_pass: Vec::new(),
            local_sufficient: true,
        },
        Job {
            id: "test".to_string(),
            argv: vec!["cargo".into(), "test".into(), "--workspace".into()],
            cwd: None,
            needs: vec!["clippy".into()],
            fix_argv: None,
            fix_safe: false,
            timeout_s: 900,
            required_tools: vec!["cargo".into()],
            env_pass: Vec::new(),
            local_sufficient: true,
        },
    ]
}

/// An existing `Makefile` `check` target is adopted, not reinvented.
fn make_jobs() -> Vec<Job> {
    vec![Job {
        id: "check".to_string(),
        argv: vec!["make".into(), "check".into()],
        cwd: None,
        needs: Vec::new(),
        fix_argv: None,
        fix_safe: false,
        timeout_s: 900,
        required_tools: vec!["make".into()],
        env_pass: Vec::new(),
        local_sufficient: true,
    }]
}

fn profiles_for(jobs: &[Job]) -> std::collections::BTreeMap<String, Vec<String>> {
    let has = |id: &str| jobs.iter().any(|job| job.id == id);
    let mut profiles = std::collections::BTreeMap::new();
    if has("fmt") {
        profiles.insert("pre-commit".to_string(), vec!["fmt".to_string()]);
    }
    if has("check") {
        profiles.insert("fast".to_string(), vec!["check".to_string()]);
        profiles.insert("pre-push".to_string(), vec!["check".to_string()]);
        profiles.insert("full".to_string(), vec!["check".to_string()]);
        return profiles;
    }
    let mut fast: Vec<String> = Vec::new();
    let mut full: Vec<String> = Vec::new();
    for id in ["fmt", "clippy", "test"] {
        if has(id) {
            fast.push(id.to_string());
            full.push(id.to_string());
        }
    }
    // `fast` is fmt+clippy by definition; `pre-push`/`full` add test.
    fast.retain(|id| id != "test");
    profiles.insert("fast".to_string(), fast.clone());
    profiles.insert("pre-push".to_string(), full.clone());
    profiles.insert("full".to_string(), full);
    profiles
}

fn read_makefile_targets(root: &Path) -> Option<(String, Vec<String>)> {
    for name in ["Makefile", "makefile"] {
        let path = root.join(name);
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        let targets: Vec<String> = text
            .lines()
            .filter_map(|line| {
                let head = line.split(':').next().unwrap_or("");
                let target = head.trim();
                if target.is_empty() || line.starts_with('\t') || line.starts_with('#') {
                    return None;
                }
                if matches!(target, "check" | "test" | "fmt" | "lint") {
                    Some(target.to_string())
                } else {
                    None
                }
            })
            .collect();
        if !targets.is_empty() {
            return Some((name.to_string(), targets));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::load_ci_file;

    #[test]
    fn detects_a_rust_workspace_with_container_runner() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(dir.path().join("Cargo.toml"), "[workspace]\n").expect("write");
        fs::create_dir_all(dir.path().join("scripts")).expect("mkdir");
        fs::write(dir.path().join("scripts/dev.sh"), "#!/bin/sh\n").expect("write");

        let detection = detect(dir.path());
        assert!(detection.sufficient);
        let file = detection.file.expect("generated");
        assert_eq!(file.version, 1);
        assert_eq!(
            file.runner.as_ref().map(|runner| runner.argv.clone()),
            Some(vec!["./scripts/dev.sh".to_string()])
        );
        let ids: Vec<&str> = file.jobs.iter().map(|job| job.id.as_str()).collect();
        assert_eq!(ids, vec!["fmt", "clippy", "test"]);
        assert!(file.profiles.contains_key("fast"));
        assert!(file.profiles.contains_key("pre-push"));
        file.validate().expect("generated config validates");
    }

    #[test]
    fn detects_workspace_under_src_without_guessing_other_stacks() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::create_dir_all(dir.path().join("src")).expect("mkdir");
        fs::write(dir.path().join("src/Cargo.toml"), "[package]\n").expect("write");
        fs::write(dir.path().join("package.json"), "{}").expect("write");

        let detection = detect(dir.path());
        assert!(detection.sufficient);
        assert!(detection
            .notes
            .iter()
            .any(|note| note.contains("package.json") && note.contains("deferred")));
    }

    #[test]
    fn makefile_check_is_adopted_when_there_is_no_cargo() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(dir.path().join("Makefile"), "check:\n\t./run.sh\n").expect("write");

        let detection = detect(dir.path());
        assert!(detection.sufficient);
        let file = detection.file.expect("generated");
        assert_eq!(
            file.jobs[0].argv,
            vec!["make".to_string(), "check".to_string()]
        );
        assert!(file.runner.is_none(), "no invented runner");
    }

    #[test]
    fn nothing_recognizable_is_insufficient_and_writes_nothing() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(dir.path().join("package.json"), "{}").expect("write");

        let detection = detect(dir.path());
        assert!(!detection.sufficient);
        assert!(detection.file.is_none());

        let report = bootstrap(dir.path(), false, false).expect("bootstrap");
        assert!(report.written.is_none());
        assert!(report
            .messages
            .iter()
            .any(|message| message.contains("insufficient evidence")));
        assert!(!dir.path().join(CI_CONFIG_NAME).exists());
    }

    #[test]
    fn existing_config_is_kept_then_force_backs_up() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(dir.path().join("Cargo.toml"), "[workspace]\n").expect("write");
        let original = "version = 1\n[profiles]\nfast = []\n";
        fs::write(dir.path().join(CI_CONFIG_NAME), original).expect("write");

        let report = bootstrap(dir.path(), false, false).expect("bootstrap");
        assert!(report.kept_existing);
        assert!(report.written.is_none());
        assert_eq!(
            fs::read_to_string(dir.path().join(CI_CONFIG_NAME)).expect("read"),
            original,
            "kept byte-for-byte"
        );

        let report = bootstrap(dir.path(), false, true).expect("force");
        assert!(report.written.is_some());
        let backup = report.backup.expect("backup made");
        assert_eq!(fs::read_to_string(&backup).expect("read"), original);
        assert_ne!(
            fs::read_to_string(dir.path().join(CI_CONFIG_NAME)).expect("read"),
            original,
            "regenerated"
        );
    }

    #[test]
    fn dry_run_writes_nothing() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(dir.path().join("Cargo.toml"), "[workspace]\n").expect("write");

        let report = bootstrap(dir.path(), true, false).expect("dry run");
        assert!(report.dry_run);
        assert!(report.written.is_none());
        assert!(!dir.path().join(CI_CONFIG_NAME).exists());
        assert!(report
            .messages
            .iter()
            .any(|message| message.contains("would write")));
    }

    #[test]
    fn force_with_insufficient_evidence_never_destroys_the_existing_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let original = "version = 1\n[profiles]\nfast = []\n";
        fs::write(dir.path().join(CI_CONFIG_NAME), original).expect("write");

        let report = bootstrap(dir.path(), false, true).expect("force");
        assert!(report.backup.is_none(), "no replacement means no backup");
        assert!(report.written.is_none());
        assert_eq!(
            fs::read_to_string(dir.path().join(CI_CONFIG_NAME)).expect("read"),
            original
        );
    }

    #[test]
    fn generated_file_loads_back_through_the_loader() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(dir.path().join("Cargo.toml"), "[workspace]\n").expect("write");
        bootstrap(dir.path(), false, false).expect("write");
        let file = load_ci_file(dir.path()).expect("load").expect("present");
        assert_eq!(file.jobs.len(), 3);
        assert!(file.jobs[0].fix_safe, "fmt keeps its safe fix");
    }
}
