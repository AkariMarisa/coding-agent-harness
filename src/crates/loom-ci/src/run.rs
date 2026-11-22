//! Sequential profile execution (`doc/16` §7.4/§7.5).
//!
//! v1 runs one job at a time, stops at the first failure unless
//! `--keep-going`, and writes `report.json` + `attestation.json` under
//! `.loom/ci/<run_id>/`. No parallelism, no affected-only, no shell.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result};

use crate::config::{ci_path, CiFile, Job};
use crate::exec::{self, SpawnSpec};
use crate::report::{sha256_hex, tree_fingerprint, tree_id, Attestation, JobReport, RunReport};
use loom_core::compact_utc;

/// Exit codes from `doc/16` §7.5.
pub const EXIT_OK: i32 = 0;
pub const EXIT_FAILED: i32 = 1;
pub const EXIT_NEEDS_HUMAN: i32 = 2;
pub const EXIT_TIMEOUT: i32 = 3;

pub struct RunRequest<'a> {
    pub root: &'a Path,
    pub file: &'a CiFile,
    pub profile: &'a str,
    pub keep_going: bool,
    pub fix: bool,
    /// Run the profile's `fix_argv`s first, then the checks, and fail when
    /// the tree moved (`hooks_fix_safe`, `doc/16` §7.7). Never combined
    /// with `fix` — that mode is the explicit `--fix` path.
    pub pre_fix: bool,
    pub now_ms: u64,
}

#[derive(Debug)]
pub struct JobExecution {
    pub report: JobReport,
    /// Bounded head/tail of the output, kept for CLI feedback only — the
    /// report on disk carries the log path, not the text.
    pub summary: String,
    pub passed: bool,
}

#[derive(Debug)]
pub struct RunOutcome {
    /// Project root, so callers can render repo-relative paths.
    pub root: PathBuf,
    pub run_id: String,
    pub log_dir: PathBuf,
    pub report_path: PathBuf,
    pub attestation_path: PathBuf,
    pub report: RunReport,
    pub executions: Vec<JobExecution>,
    /// First missing tool/runner, if any (`exit 2`).
    pub missing_tool: Option<String>,
    /// `git diff --stat` after `--fix`, when git could produce one.
    pub diff_stat: Option<String>,
    /// Safe fixes moved the working tree after the run fingerprint: the
    /// hook must stop and ask for a fresh `git add` (`doc/16` §7.7).
    pub fixes_changed: bool,
    pub exit_code: i32,
}

pub fn run(request: RunRequest<'_>) -> Result<RunOutcome> {
    let RunRequest {
        root,
        file,
        profile,
        keep_going,
        fix,
        pre_fix,
        now_ms,
    } = request;

    let profile_jobs = file
        .profiles
        .get(profile)
        .ok_or_else(|| anyhow::anyhow!("unknown profile: {profile}"))?;
    let ordered = order_by_needs(profile_jobs, file)?;

    let run_id = format!("CI-{}-{}", compact_utc(now_ms), std::process::id());
    let log_dir = root.join(".loom").join("ci").join(&run_id);
    fs::create_dir_all(&log_dir).with_context(|| format!("create {}", log_dir.display()))?;

    let start_tree = tree_fingerprint(root);
    let ci_toml_sha256 = fs::read(ci_path(root)).ok().as_deref().map(sha256_hex);

    let started = Instant::now();
    let started_at_ms = now_ms;

    let selected: Vec<&Job> = if fix {
        // `--fix` touches only jobs that declared a safe fix; checks without
        // one are unrelated to fixing and stay untouched.
        ordered
            .iter()
            .filter(|job| job.fix_safe && job.fix_argv.is_some())
            .copied()
            .collect()
    } else {
        ordered.clone()
    };

    let local_sufficient = selected.iter().all(|job| job.local_sufficient);
    let mut executions: Vec<JobExecution> = Vec::new();
    let mut missing_tool: Option<String> = None;
    let mut stop = false;
    let mut fixes_changed = false;

    // `hooks_fix_safe`: apply safe fixes before the checks. The report pins
    // the tree the checks actually ran on, and a tree that moved under the
    // run fails it — staged content would differ from checked content.
    if pre_fix && !fix {
        for job in selected
            .iter()
            .filter(|job| job.fix_safe && job.fix_argv.is_some())
        {
            if stop {
                break;
            }
            let runner = file.runner.as_ref().map(|runner| runner.argv.as_slice());
            let fix_argv = job.fix_argv.as_ref().expect("filtered");
            if let Some(tool) = exec::missing_tool(runner, &fix_argv[0], &job.required_tools) {
                missing_tool = Some(tool);
                break;
            }
            let fix_log = log_dir.join(format!("{}.fix.log", job.id));
            let outcome = execute(root, job, runner, fix_argv, &fix_log)?;
            let passed = !outcome.timed_out && outcome.exit == Some(0);
            executions.push(JobExecution {
                report: JobReport {
                    id: format!("{}.fix", job.id),
                    argv: fix_argv.clone(),
                    cwd: job.cwd.clone().unwrap_or_else(|| ".".to_string()),
                    exit: outcome.exit,
                    signal: outcome.signal,
                    timed_out: outcome.timed_out,
                    duration_ms: outcome.duration_ms,
                    log_path: rel_log(root, &fix_log),
                    log_sha256: hash_file(&fix_log),
                    fixed: false,
                },
                summary: outcome.summary,
                passed,
            });
            if !passed {
                stop = true;
            }
        }
        let after_tree = tree_fingerprint(root);
        fixes_changed = tree_id(&start_tree) != tree_id(&after_tree);
    }
    let tree = if pre_fix && !fix {
        tree_fingerprint(root)
    } else {
        start_tree
    };

    for job in selected {
        if stop {
            break;
        }
        let runner = file.runner.as_ref().map(|runner| runner.argv.as_slice());

        if let Some(tool) = exec::missing_tool(runner, &job.argv[0], &job.required_tools) {
            missing_tool = Some(tool);
            // A missing tool needs a human, not more attempts.
            break;
        }

        if fix {
            let fix_argv = job
                .fix_argv
                .as_ref()
                .expect("filtered to jobs with fix_argv");
            let fix_log = log_dir.join(format!("{}.fix.log", job.id));
            let outcome = execute(root, job, runner, fix_argv, &fix_log)?;
            let fix_report = JobReport {
                id: format!("{}.fix", job.id),
                argv: fix_argv.clone(),
                cwd: job.cwd.clone().unwrap_or_else(|| ".".to_string()),
                exit: outcome.exit,
                signal: outcome.signal,
                timed_out: outcome.timed_out,
                duration_ms: outcome.duration_ms,
                log_path: rel_log(root, &fix_log),
                log_sha256: hash_file(&fix_log),
                fixed: false,
            };
            let fix_passed = !outcome.timed_out && outcome.exit == Some(0);
            executions.push(JobExecution {
                report: fix_report,
                summary: outcome.summary,
                passed: fix_passed,
            });
            if !fix_passed {
                stop = true;
                continue;
            }
        }

        let log_path = log_dir.join(format!("{}.log", job.id));
        let outcome = execute(root, job, runner, &job.argv, &log_path)?;
        let passed = !outcome.timed_out && outcome.exit == Some(0);
        executions.push(JobExecution {
            report: JobReport {
                id: job.id.clone(),
                argv: job.argv.clone(),
                cwd: job.cwd.clone().unwrap_or_else(|| ".".to_string()),
                exit: outcome.exit,
                signal: outcome.signal,
                timed_out: outcome.timed_out,
                duration_ms: outcome.duration_ms,
                log_path: rel_log(root, &log_path),
                log_sha256: hash_file(&log_path),
                fixed: fix,
            },
            summary: outcome.summary,
            passed,
        });
        if !passed && !keep_going {
            stop = true;
        }
    }

    let diff_stat = fix.then(|| git_diff_stat(root)).flatten();

    let failed: Vec<&JobExecution> = executions
        .iter()
        .filter(|execution| !execution.passed)
        .collect();
    let any_timeout = executions
        .iter()
        .any(|execution| execution.report.timed_out);
    let exit_code = if missing_tool.is_some() {
        EXIT_NEEDS_HUMAN
    } else if any_timeout {
        EXIT_TIMEOUT
    } else if !failed.is_empty() || fixes_changed {
        // A moved tree blocks with exit 1: `git add` first (§7.7).
        EXIT_FAILED
    } else {
        EXIT_OK
    };

    let report = RunReport {
        v: 1,
        run_id: run_id.clone(),
        profile: profile.to_string(),
        ci_toml_sha256,
        tree,
        started_at_ms,
        duration_ms: started.elapsed().as_millis() as u64,
        jobs: executions
            .iter()
            .map(|execution| execution.report.clone())
            .collect(),
        ok: exit_code == EXIT_OK,
        local_sufficient,
        advisory: false,
        missing_tool: missing_tool.clone(),
        tree_changed_by_fix: fixes_changed,
    };

    let report_path = log_dir.join("report.json");
    let attestation_path = log_dir.join("attestation.json");
    write_json(&report_path, &report)?;
    let report_sha256 = sha256_hex(&fs::read(&report_path).unwrap_or_default());
    let attestation = Attestation {
        v: 1,
        run_id: run_id.clone(),
        report_sha256,
        tree: report.tree.clone(),
        ci_toml_sha256: report.ci_toml_sha256.clone(),
        signature: "none".to_string(),
        created_at_ms: now_ms,
    };
    write_json(&attestation_path, &attestation)?;

    Ok(RunOutcome {
        root: root.to_path_buf(),
        run_id,
        log_dir,
        report_path,
        attestation_path,
        report,
        executions,
        missing_tool,
        diff_stat,
        fixes_changed,
        exit_code,
    })
}

/// Stable order: the profile's own order, with each job's `needs` emitted
/// first (depth-first). Cycles are impossible — validation rejects them.
fn order_by_needs<'a>(profile_jobs: &'a [String], file: &'a CiFile) -> Result<Vec<&'a Job>> {
    let by_id: std::collections::HashMap<&str, &Job> =
        file.jobs.iter().map(|job| (job.id.as_str(), job)).collect();
    let mut emitted: Vec<&Job> = Vec::new();
    let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();

    fn emit<'a>(
        id: &'a str,
        by_id: &std::collections::HashMap<&'a str, &'a Job>,
        seen: &mut std::collections::HashSet<&'a str>,
        out: &mut Vec<&'a Job>,
    ) -> Result<()> {
        if !seen.insert(id) {
            return Ok(());
        }
        let job = by_id
            .get(id)
            .ok_or_else(|| anyhow::anyhow!("unknown job {id}"))?;
        for need in &job.needs {
            emit(need, by_id, seen, out)?;
        }
        out.push(job);
        Ok(())
    }

    for id in profile_jobs {
        emit(id, &by_id, &mut seen, &mut emitted)?;
    }
    Ok(emitted)
}

fn execute(
    root: &Path,
    job: &Job,
    runner: Option<&[String]>,
    argv: &[String],
    log_path: &Path,
) -> Result<exec::ExecOutcome> {
    let cwd = exec::contained_cwd(root, job.cwd.as_deref())?;
    let final_argv: Vec<String> = match runner {
        Some(runner) => runner.iter().chain(argv.iter()).cloned().collect(),
        None => argv.to_vec(),
    };
    exec::spawn_and_wait(SpawnSpec {
        argv: &final_argv,
        cwd,
        env_pass: &job.env_pass,
        timeout_s: job.timeout_s,
        log_path: log_path.to_path_buf(),
    })
}

fn rel_log(root: &Path, log: &Path) -> String {
    log.strip_prefix(root).unwrap_or(log).display().to_string()
}

fn hash_file(path: &Path) -> String {
    fs::read(path)
        .map(|bytes| sha256_hex(&bytes))
        .unwrap_or_default()
}

fn git_diff_stat(root: &Path) -> Option<String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["diff", "--stat"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout).into_owned();
    (!text.trim().is_empty()).then_some(text)
}

fn write_json<T: serde::Serialize>(path: &Path, value: &T) -> Result<()> {
    let text = serde_json::to_string_pretty(value).context("serialize CI report")?;
    fs::write(path, format!("{text}\n")).with_context(|| format!("write {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Job;
    use crate::report::TreeFingerprint;
    use std::collections::BTreeMap;

    fn job(id: &str, argv: Vec<&str>, needs: Vec<&str>) -> Job {
        Job {
            id: id.to_string(),
            argv: argv.into_iter().map(String::from).collect(),
            cwd: None,
            needs: needs.into_iter().map(String::from).collect(),
            fix_argv: None,
            fix_safe: false,
            timeout_s: 10,
            required_tools: Vec::new(),
            env_pass: Vec::new(),
            local_sufficient: true,
        }
    }

    fn file_with(jobs: Vec<Job>, profile: Vec<&str>) -> CiFile {
        CiFile {
            version: 1,
            runner: None,
            profiles: BTreeMap::from([(
                "fast".to_string(),
                profile.into_iter().map(String::from).collect(),
            )]),
            jobs,
        }
    }

    #[test]
    fn orders_needs_before_dependents() {
        let file = file_with(
            vec![
                job("test", vec!["true"], vec!["clippy"]),
                job("clippy", vec!["true"], vec!["fmt"]),
                job("fmt", vec!["true"], vec![]),
            ],
            vec!["test"],
        );
        let profile = ["test".to_string()];
        let ordered = order_by_needs(&profile, &file).expect("order");
        let ids: Vec<&str> = ordered.iter().map(|job| job.id.as_str()).collect();
        assert_eq!(ids, vec!["fmt", "clippy", "test"]);
    }

    #[test]
    fn full_run_writes_report_and_attestation() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = file_with(
            vec![job("ok", vec!["sh", "-c", "echo fine"], vec![])],
            vec!["ok"],
        );
        let outcome = run(RunRequest {
            root: dir.path(),
            file: &file,
            profile: "fast",
            keep_going: false,
            fix: false,
            pre_fix: false,
            now_ms: 1_795_180_800_000,
        })
        .expect("run");
        assert_eq!(outcome.exit_code, EXIT_OK);
        assert!(outcome.report.ok);
        assert!(outcome.report_path.exists());
        assert!(outcome.attestation_path.exists());

        let text = fs::read_to_string(&outcome.report_path).expect("read");
        let parsed: RunReport = serde_json::from_str(&text).expect("parse report");
        assert_eq!(parsed.jobs.len(), 1);
        assert_eq!(parsed.jobs[0].exit, Some(0));
        assert!(
            !parsed.jobs[0].log_path.contains(".."),
            "log path is repo-relative"
        );
        assert!(outcome.log_dir.join("ok.log").exists());

        let attestation: Attestation =
            serde_json::from_str(&fs::read_to_string(&outcome.attestation_path).expect("read"))
                .expect("parse attestation");
        assert_eq!(attestation.signature, "none");
        assert_eq!(
            attestation.report_sha256,
            sha256_hex(fs::read_to_string(&outcome.report_path).unwrap().as_bytes()),
            "attestation pins the report bytes"
        );
    }

    #[test]
    fn failure_stops_by_default_and_keep_going_runs_the_rest() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = file_with(
            vec![
                job("bad", vec!["sh", "-c", "exit 1"], vec![]),
                job("after", vec!["true"], vec![]),
            ],
            vec!["bad", "after"],
        );
        let outcome = run(RunRequest {
            root: dir.path(),
            file: &file,
            profile: "fast",
            keep_going: false,
            fix: false,
            pre_fix: false,
            now_ms: 0,
        })
        .expect("run");
        assert_eq!(outcome.exit_code, EXIT_FAILED);
        assert_eq!(outcome.report.jobs.len(), 1, "stopped at the first failure");

        let outcome = run(RunRequest {
            root: dir.path(),
            file: &file,
            profile: "fast",
            keep_going: true,
            fix: false,
            pre_fix: false,
            now_ms: 1,
        })
        .expect("run");
        assert_eq!(outcome.report.jobs.len(), 2, "--keep-going runs everything");
        assert_eq!(outcome.exit_code, EXIT_FAILED);
    }

    #[test]
    fn timeout_maps_to_exit_three() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut slow = job("slow", vec!["sleep", "30"], vec![]);
        slow.timeout_s = 1;
        let file = file_with(vec![slow], vec!["slow"]);
        let outcome = run(RunRequest {
            root: dir.path(),
            file: &file,
            profile: "fast",
            keep_going: false,
            fix: false,
            pre_fix: false,
            now_ms: 0,
        })
        .expect("run");
        assert_eq!(outcome.exit_code, EXIT_TIMEOUT);
        assert!(outcome.report.jobs[0].timed_out);
    }

    #[test]
    fn missing_tool_maps_to_exit_two() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = file_with(
            vec![job("nope", vec!["definitely-not-a-real-tool-xyz"], vec![])],
            vec!["nope"],
        );
        let outcome = run(RunRequest {
            root: dir.path(),
            file: &file,
            profile: "fast",
            keep_going: false,
            fix: false,
            pre_fix: false,
            now_ms: 0,
        })
        .expect("run");
        assert_eq!(outcome.exit_code, EXIT_NEEDS_HUMAN);
        assert_eq!(
            outcome.missing_tool.as_deref(),
            Some("definitely-not-a-real-tool-xyz")
        );
        assert!(outcome.report.jobs.is_empty(), "nothing ran");
    }

    #[test]
    fn fix_mode_runs_fix_then_verify_and_changes_nothing_else() {
        let dir = tempfile::tempdir().expect("tempdir");
        let target = dir.path().join("file.txt");
        fs::write(&target, "dirty").expect("write");
        let mut fixing = job("fmt", vec!["sh", "-c", "grep -q fixed file.txt"], vec![]);
        fixing.fix_argv = Some(vec![
            "sh".to_string(),
            "-c".to_string(),
            "echo fixed > file.txt".to_string(),
        ]);
        fixing.fix_safe = true;
        let untouched = job("other", vec!["sh", "-c", "exit 9"], vec![]);
        let file = file_with(vec![fixing, untouched], vec!["fmt", "other"]);

        let outcome = run(RunRequest {
            root: dir.path(),
            file: &file,
            profile: "fast",
            keep_going: false,
            fix: true,
            pre_fix: false,
            now_ms: 0,
        })
        .expect("run");
        assert_eq!(
            outcome.exit_code, EXIT_OK,
            "--fix touches only fix_safe jobs; `other` is skipped"
        );
        assert_eq!(outcome.report.jobs.len(), 2, "fix pass + verify pass");
        assert_eq!(outcome.report.jobs[0].id, "fmt.fix");
        assert!(
            outcome
                .report
                .jobs
                .iter()
                .any(|job| job.id == "fmt" && job.fixed),
            "verify leg marked as fixed"
        );
        assert_eq!(
            fs::read_to_string(&target).expect("read"),
            "fixed\n",
            "the fix landed"
        );
        assert!(
            !dir.path().join(".git").exists(),
            "nothing is staged: this test never creates an index"
        );
    }

    #[test]
    fn unknown_profile_is_a_configuration_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = file_with(vec![job("a", vec!["true"], vec![])], vec!["a"]);
        let error = run(RunRequest {
            root: dir.path(),
            file: &file,
            profile: "nope",
            keep_going: false,
            fix: false,
            pre_fix: false,
            now_ms: 0,
        })
        .expect_err("unknown profile");
        assert!(error.to_string().contains("unknown profile"));
    }

    #[test]
    fn tree_id_is_reported_consistently() {
        let fingerprint = TreeFingerprint {
            head: None,
            dirty_sha256: None,
            untracked_sha256: None,
            dirty: false,
        };
        assert_eq!(tree_id(&fingerprint).len(), 12);
    }
}
