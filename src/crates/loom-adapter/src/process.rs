use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result};
use loom_core::ChurnConfig;

use crate::preset::preset;
use crate::types::AdapterRequest;
use crate::watch::{ChurnStop, ChurnWatch};

#[derive(Debug, Clone)]
pub struct ProcessOutcome {
    pub exit_code: i32,
    pub timed_out: bool,
    pub duration_ms: u64,
    pub stdout_path: PathBuf,
    pub stderr_path: PathBuf,
    /// Set when the churn watchdog stopped a gross loop (`doc/15` §1.6).
    pub churn: Option<ChurnStop>,
}

/// How the wait loop ended.
#[derive(Debug)]
enum Supervision {
    Exited(std::process::ExitStatus),
    TimedOut,
    ChurnStopped(ChurnStop),
}

/// Wait for the child with a hard timeout and the coarse churn watchdog.
///
/// The watchdog only fires on a *gross* loop (the same file changed in
/// `changes` consecutive samples for at least `min_age`) — near-misses keep
/// running, because restarting an external agent may re-read the whole
/// repository (`doc/15` §1.6). Extracted from `execute` so tests can
/// supervise arbitrary children without an adapter preset.
fn supervise(
    child: &mut std::process::Child,
    worktree: &Path,
    churn: &ChurnConfig,
    timeout_s: u64,
) -> Result<Supervision> {
    let mut watch = churn.watchdog.then(|| ChurnWatch::new(worktree, churn));
    let started = Instant::now();
    loop {
        match child.try_wait().with_context(|| "wait for adapter")? {
            Some(status) => return Ok(Supervision::Exited(status)),
            None => {
                if started.elapsed().as_secs() >= timeout_s {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Ok(Supervision::TimedOut);
                }
                if let Some(watch) = &mut watch {
                    if let Some(stop) = watch.poll() {
                        let _ = child.kill();
                        let _ = child.wait();
                        return Ok(Supervision::ChurnStopped(stop));
                    }
                }
                thread::sleep(Duration::from_millis(25));
            }
        }
    }
}

/// Run an external adapter as a child process with a hard timeout.
///
/// The packet travels via files, not a long prompt argument. Usage and final
/// message are read back from the outbox after the process exits.
pub fn execute(request: &AdapterRequest) -> Result<ProcessOutcome> {
    let preset = preset(request.adapter);
    let args = preset.expand(request);

    let stdout_path = request.logs_dir.join("adapter.stdout.log");
    let stderr_path = request.logs_dir.join("adapter.stderr.log");
    let stdout =
        File::create(&stdout_path).with_context(|| format!("create {}", stdout_path.display()))?;
    let stderr =
        File::create(&stderr_path).with_context(|| format!("create {}", stderr_path.display()))?;

    let mut command = Command::new(preset.resolve_executable());
    command
        .args(&args)
        .current_dir(&request.worktree)
        .env("LOOM_TASK_ID", &request.task_id)
        .env("LOOM_RUN_ID", &request.run_id)
        .env("LOOM_MODEL", &request.model)
        .env("LOOM_MODEL_ID", &request.model_id)
        .env("LOOM_PACKET_DIR", &request.packet_dir)
        .env("LOOM_OUTPUT_DIR", &request.output_dir)
        .env("LOOM_LOGS_DIR", &request.logs_dir)
        .env("LOOM_BRANCH", &request.branch)
        .env("LOOM_ATTEMPT", request.attempt.to_string())
        .env("LOOM_TIMEOUT_S", request.timeout_s.to_string())
        .stdout(stdout)
        .stderr(stderr);
    if let Some(capability) = &request.model_capability {
        let text = serde_json::to_string(capability).context("serialize model capability")?;
        command.env("LOOM_MODEL_CAPABILITY", text);
    }

    if let Some(max_tokens) = request.max_tokens {
        command.env("LOOM_MAX_TOKENS", max_tokens.to_string());
    }
    if preset.stdin_packet {
        command.stdin(Stdio::piped());
    } else {
        command.stdin(Stdio::null());
    }

    let mut child = command
        .spawn()
        .with_context(|| format!("spawn {} {}", preset.resolve_executable(), args.join(" ")))?;

    if preset.stdin_packet {
        if let Some(mut stdin) = child.stdin.take() {
            let packet = std::fs::read(preset.packet_path(request))
                .with_context(|| "read packet for stdin")?;
            stdin
                .write_all(&packet)
                .with_context(|| "write packet to adapter stdin")?;
            // Skill mentions ride the same stdin (`doc/15` §12.3): the packet
            // carries the bodies, opencode's loader needs the @-mention.
            if let Some(extra) = preset.stdin_extra(&request.skills) {
                stdin
                    .write_all(extra.as_bytes())
                    .with_context(|| "write skill mentions to adapter stdin")?;
            }
            drop(stdin);
        }
    }

    let started = Instant::now();
    let supervision = supervise(
        &mut child,
        &request.worktree,
        &request.churn,
        request.timeout_s,
    )?;
    let (exit_code, timed_out, churn) = match supervision {
        Supervision::Exited(status) => (status.code().unwrap_or(4), false, None),
        Supervision::TimedOut => (3, true, None),
        // A distinct code: 5 is neither an availability failure (no model
        // fallback — restarting may loop again) nor a plain adapter crash.
        Supervision::ChurnStopped(stop) => (5, false, Some(stop)),
    };

    Ok(ProcessOutcome {
        exit_code,
        timed_out,
        duration_ms: started.elapsed().as_millis() as u64,
        stdout_path,
        stderr_path,
        churn,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn churn(changes: u32, interval_s: u64, min_age_s: u64) -> ChurnConfig {
        ChurnConfig {
            watchdog: true,
            changes,
            interval_s,
            min_age_s,
        }
    }

    #[test]
    fn a_clean_child_exits_normally() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut child = Command::new("sh")
            .arg("-c")
            .arg("exit 7")
            .current_dir(dir.path())
            .spawn()
            .expect("spawn");
        let supervision =
            supervise(&mut child, dir.path(), &churn(3, 1, 60), 10).expect("supervise");
        match supervision {
            Supervision::Exited(status) => assert_eq!(status.code(), Some(7)),
            other => panic!("expected Exited, got {other:?}"),
        }
    }

    #[test]
    fn a_gross_loop_is_killed_with_evidence() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut child = Command::new("sh")
            .arg("-c")
            .arg("while :; do echo x >> loop.txt; sleep 0.005; done")
            .current_dir(dir.path())
            .spawn()
            .expect("spawn");
        // interval 0 → sample on every 25ms tick; three consecutive hits.
        let supervision =
            supervise(&mut child, dir.path(), &churn(3, 0, 0), 30).expect("supervise");
        match supervision {
            Supervision::ChurnStopped(stop) => {
                assert_eq!(stop.file, "loop.txt");
                assert!(stop.changes >= 3, "sustained: {}", stop.changes);
                assert!(stop.bytes_touched > 0);
            }
            other => panic!("expected ChurnStopped, got {other:?}"),
        }
        // The child is gone: no residue.
        assert!(child.try_wait().expect("wait").is_some());
    }

    #[test]
    fn timeout_survives_with_the_watchdog_enabled() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut child = Command::new("sh")
            .arg("-c")
            .arg("sleep 30")
            .current_dir(dir.path())
            .spawn()
            .expect("spawn");
        let started = Instant::now();
        let supervision =
            supervise(&mut child, dir.path(), &churn(2, 1, 3_600), 1).expect("supervise");
        assert!(matches!(supervision, Supervision::TimedOut));
        assert!(
            started.elapsed().as_secs() < 10,
            "timeout must not wait the sleep"
        );
    }

    #[test]
    fn watchdog_disabled_never_stops_a_looper() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut config = churn(2, 0, 0);
        config.watchdog = false;
        let mut child = Command::new("sh")
            .arg("-c")
            .arg("while :; do echo x >> loop.txt; sleep 0.005; done")
            .current_dir(dir.path())
            .spawn()
            .expect("spawn");
        // With the watchdog off, only the (long) timeout can end this — use a
        // tiny one to keep the test fast.
        let supervision = supervise(&mut child, dir.path(), &config, 1).expect("supervise");
        assert!(matches!(supervision, Supervision::TimedOut));
    }
}
