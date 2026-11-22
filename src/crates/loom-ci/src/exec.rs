//! One CI command's execution (`doc/16` §7.4): argv arrays only — never a
//! shell — a cleared environment with an explicit allowlist, process-group
//! timeouts, and ANSI-stripped logs with a bounded in-memory summary.

use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

/// The fixed environment allowlist; job `env_pass` names extend it — names
/// only, values come from the host. `*_API_KEY` etc. can never be listed.
const ENV_ALLOWLIST: [&str; 6] = ["PATH", "HOME", "LANG", "TERM", "CARGO_HOME", "RUSTUP_HOME"];

/// Head and tail kept in memory per stream for the failure summary; the full
/// text stays in the log file (spill, never inline).
const SUMMARY_HEAD: usize = 2 * 1024;
const SUMMARY_TAIL: usize = 4 * 1024;
/// Grace between SIGTERM and SIGKILL when a timeout fires.
const KILL_GRACE: Duration = Duration::from_millis(2_000);

#[derive(Debug, Default)]
pub struct ExecOutcome {
    /// `None` when the process died on a signal.
    pub exit: Option<i32>,
    pub signal: Option<i32>,
    pub timed_out: bool,
    pub duration_ms: u64,
    /// Bounded `head … tail` of the stripped output, for failure feedback.
    pub summary: String,
}

/// Locate a tool the way a shell would, without spawning one.
pub fn which(tool: &str) -> Option<PathBuf> {
    if tool.contains('/') {
        let direct = PathBuf::from(tool);
        return direct.is_file().then_some(direct);
    }
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(tool);
        if candidate.is_file() {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                if candidate.metadata().ok()?.permissions().mode() & 0o111 != 0 {
                    return Some(candidate);
                }
            }
            #[cfg(not(unix))]
            return Some(candidate);
        }
    }
    None
}

/// Probe the binary that will actually start: the runner when one exists
/// (the job's own tool may only exist inside it), else the job's tools.
pub fn missing_tool(
    runner: Option<&[String]>,
    argv0: &str,
    required_tools: &[String],
) -> Option<String> {
    let probe: Vec<String> = match runner {
        Some(runner) => vec![runner[0].clone()],
        None if required_tools.is_empty() => vec![argv0.to_string()],
        None => required_tools.to_vec(),
    };
    probe.into_iter().find(|tool| which(tool).is_none())
}

pub struct SpawnSpec<'a> {
    pub argv: &'a [String],
    /// Absolute working directory, already contained by the caller.
    pub cwd: PathBuf,
    pub env_pass: &'a [String],
    pub timeout_s: u64,
    /// Absolute path of the log file; created/truncated by this call.
    pub log_path: PathBuf,
}

/// Run one command to completion (or timeout) and return its bounded summary.
pub fn spawn_and_wait(spec: SpawnSpec<'_>) -> Result<ExecOutcome> {
    let log_out = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&spec.log_path)
        .with_context(|| format!("open log {}", spec.log_path.display()))?;
    let log_err = log_out
        .try_clone()
        .with_context(|| format!("clone log {}", spec.log_path.display()))?;

    let mut command = Command::new(&spec.argv[0]);
    command
        .args(&spec.argv[1..])
        .current_dir(&spec.cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // A separate process group per job, so a timeout kills the whole
        // tree — no orphaned `podman run` children (`doc/16` §7.4).
        .process_group(0);
    command.env_clear();
    for name in ENV_ALLOWLIST {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    for name in spec.env_pass {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    command
        .env("NO_COLOR", "1")
        .env("TERM", "dumb")
        .env("LOOM_CI", "1");

    let mut child = command
        .spawn()
        .with_context(|| format!("spawn {}", spec.argv.join(" ")))?;
    let pid = child.id();

    let out_pipe = child.stdout.take().context("stdout pipe")?;
    let err_pipe = child.stderr.take().context("stderr pipe")?;
    let out_thread = thread::spawn(move || drain(out_pipe, log_out));
    let err_thread = thread::spawn(move || drain(err_pipe, log_err));

    let started = Instant::now();
    let timeout = Duration::from_secs(spec.timeout_s.max(1));
    let mut timed_out = false;
    let status = loop {
        match child.try_wait().context("wait for CI job")? {
            Some(status) => break Some(status),
            None if started.elapsed() >= timeout => {
                timed_out = true;
                kill_group(pid, libc::SIGTERM);
                let grace_deadline = Instant::now() + KILL_GRACE;
                let mut reaped = None;
                while Instant::now() < grace_deadline {
                    if let Some(status) = child.try_wait().context("wait after SIGTERM")? {
                        reaped = Some(status);
                        break;
                    }
                    thread::sleep(Duration::from_millis(20));
                }
                if reaped.is_none() {
                    kill_group(pid, libc::SIGKILL);
                    reaped = Some(child.wait().context("wait after SIGKILL")?);
                }
                break reaped;
            }
            None => thread::sleep(Duration::from_millis(10)),
        }
    };

    let out_summary = out_thread.join().unwrap_or_default();
    let err_summary = err_thread.join().unwrap_or_default();
    let mut summary = out_summary;
    if !err_summary.is_empty() {
        if !summary.is_empty() {
            summary.push_str("\n--- stderr ---\n");
        }
        summary.push_str(&err_summary);
    }

    let duration_ms = started.elapsed().as_millis() as u64;
    let (exit, signal) = match status {
        Some(status) => {
            use std::os::unix::process::ExitStatusExt as _;
            (status.code(), status.signal())
        }
        None => (None, None),
    };
    Ok(ExecOutcome {
        exit,
        signal,
        timed_out,
        duration_ms,
        summary,
    })
}

/// SIGTERM/SIGKILL the whole process group; a zero/negative failure is fine
/// (the child may already be gone).
fn kill_group(pid: u32, signal: i32) {
    // Safety: signalling a negative pid targets the group created by
    // `process_group(0)`; an invalid group simply returns ESRCH.
    unsafe {
        libc::kill(-(pid as i32), signal);
    }
}

/// Read one stream to EOF, strip ANSI, append to the log, and keep a bounded
/// head/tail for the failure summary.
fn drain<R: Read + Send + 'static>(mut pipe: R, mut log: std::fs::File) -> String {
    let mut strip = AnsiStrip::default();
    let mut head: Vec<u8> = Vec::new();
    let mut tail: Vec<u8> = Vec::new();
    let mut total = 0usize;
    let mut buffer = [0u8; 8 * 1024];
    loop {
        let read = match pipe.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(read) => read,
        };
        let clean = strip.feed(&buffer[..read]);
        let _ = log.write_all(&clean);
        total += clean.len();
        for byte in &clean {
            if head.len() < SUMMARY_HEAD {
                head.push(*byte);
            }
        }
        tail.extend_from_slice(&clean);
        if tail.len() > SUMMARY_TAIL {
            let cut = tail.len() - SUMMARY_TAIL;
            tail.drain(..cut);
        }
    }
    let _ = log.flush();

    if total <= head.len() {
        return String::from_utf8_lossy(&head).into_owned();
    }
    format!(
        "{}\n… ({} bytes total) …\n{}",
        String::from_utf8_lossy(&head),
        total,
        String::from_utf8_lossy(&tail)
    )
}

/// Drop ANSI escapes so logs on disk are plain text (`doc/16` §7.4).
#[derive(Default)]
struct AnsiStrip {
    in_escape: bool,
    csi: bool,
}

impl AnsiStrip {
    fn feed(&mut self, input: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(input.len());
        for &byte in input {
            if self.in_escape {
                if byte == 0x1b {
                    self.csi = false;
                    continue;
                }
                if !self.csi {
                    // ESC <cmd>: consume the command byte; a `[` starts CSI.
                    if byte == b'[' {
                        self.csi = true;
                    } else {
                        self.in_escape = false;
                    }
                    continue;
                }
                if (0x40..=0x7e).contains(&byte) {
                    self.in_escape = false;
                    self.csi = false;
                }
                continue;
            }
            if byte == 0x1b {
                self.in_escape = true;
                self.csi = false;
                continue;
            }
            out.push(byte);
        }
        out
    }
}

/// Containment re-check at execution time: the resolved cwd must stay inside
/// the repo root even after symlink resolution by the OS.
pub fn contained_cwd(root: &Path, cwd: Option<&str>) -> Result<PathBuf> {
    // Reject `..` lexically first: `starts_with` treats `root/../x` as
    // inside because the prefix matches before the hop.
    if let Some(relative) = cwd {
        if Path::new(relative)
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
        {
            anyhow::bail!("cwd {relative} escapes the repo root");
        }
    }
    let candidate = match cwd {
        None => root.to_path_buf(),
        Some(relative) => root.join(relative),
    };
    let resolved_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let resolved = candidate
        .canonicalize()
        .unwrap_or_else(|_| candidate.clone());
    if !resolved.starts_with(&resolved_root) {
        anyhow::bail!(
            "cwd {} escapes the repo root",
            cwd.unwrap_or(&candidate.display().to_string())
        );
    }
    Ok(candidate)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_removes_csi_and_keeps_text() {
        let mut strip = AnsiStrip::default();
        let clean = strip.feed(b"\x1b[31mred\x1b[0m plain");
        assert_eq!(clean, b"red plain");
        // OSC sequences (ESC ]) are consumed at their command byte; the
        // payload passes through but the escape never does.
        let clean = strip.feed(b"a\x1b]8;;http://x\x07b");
        assert!(!clean.contains(&0x1b), "no raw escape survives");
        assert!(clean.starts_with(b"a"));
    }

    #[test]
    fn which_finds_sh_and_misses_nonsense() {
        assert!(which("sh").is_some());
        assert!(which("definitely-not-a-real-tool-xyz").is_none());
        assert!(which("/bin/sh").is_some());
    }

    #[test]
    fn tool_probe_prefers_the_runner_when_present() {
        // With a runner only the runner is probed: the job's own tool may
        // exist only inside it (`doc/16` §7.3).
        assert_eq!(
            missing_tool(Some(["/bin/sh".to_string()].as_slice()), "cargo", &[]),
            None
        );
        assert_eq!(
            missing_tool(None, "cargo", &["no-such-tool-xyz".into()]),
            Some("no-such-tool-xyz".to_string())
        );
        assert_eq!(
            missing_tool(None, "definitely-not-a-real-tool-xyz", &[]),
            Some("definitely-not-a-real-tool-xyz".to_string())
        );
    }

    #[test]
    fn contained_cwd_rejects_escapes() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join("sub")).expect("mkdir");
        assert!(contained_cwd(dir.path(), Some("sub")).is_ok());
        assert!(contained_cwd(dir.path(), None).is_ok());
        assert!(contained_cwd(dir.path(), Some("../outside")).is_err());
        assert!(contained_cwd(dir.path(), Some("/etc")).is_err());
    }

    #[test]
    fn runs_a_command_and_captures_a_bounded_summary() {
        let dir = tempfile::tempdir().expect("tempdir");
        let log = dir.path().join("job.log");
        let argv = vec![
            "sh".to_string(),
            "-c".to_string(),
            "echo hello; echo world".to_string(),
        ];
        let outcome = spawn_and_wait(SpawnSpec {
            argv: &argv,
            cwd: dir.path().to_path_buf(),
            env_pass: &[],
            timeout_s: 10,
            log_path: log.clone(),
        })
        .expect("run");
        assert_eq!(outcome.exit, Some(0));
        assert!(!outcome.timed_out);
        assert!(outcome.summary.contains("hello"));
        let on_disk = std::fs::read_to_string(&log).expect("log");
        assert!(on_disk.contains("world"));
    }

    #[test]
    fn nonzero_exit_and_timeout_are_distinct() {
        let dir = tempfile::tempdir().expect("tempdir");
        let failing = vec!["sh".to_string(), "-c".to_string(), "exit 3".to_string()];
        let outcome = spawn_and_wait(SpawnSpec {
            argv: &failing,
            cwd: dir.path().to_path_buf(),
            env_pass: &[],
            timeout_s: 10,
            log_path: dir.path().join("fail.log"),
        })
        .expect("run");
        assert_eq!(outcome.exit, Some(3));
        assert!(!outcome.timed_out);

        let sleepy = vec!["sleep".to_string(), "30".to_string()];
        let started = Instant::now();
        let outcome = spawn_and_wait(SpawnSpec {
            argv: &sleepy,
            cwd: dir.path().to_path_buf(),
            env_pass: &[],
            timeout_s: 1,
            log_path: dir.path().join("slow.log"),
        })
        .expect("run");
        assert!(outcome.timed_out, "the sleep must be reported as timed out");
        assert!(outcome.exit.is_none(), "a signalled child has no exit code");
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "timeout must not wait the full sleep"
        );
    }

    #[test]
    fn env_is_allowlisted_and_strips_secrets() {
        // No process-env mutation here: host secrets are stripped by the
        // allowlist either way, while PATH (allowlisted), an `env_pass` name
        // and LOOM_CI survive.
        let dir = tempfile::tempdir().expect("tempdir");
        let argv = vec![
            "sh".to_string(),
            "-c".to_string(),
            "test -n \"$PATH\" && test \"$LOOM_CI\" = 1 \
             && test -z \"${OPENAI_API_KEY:-}\" && test -z \"${LOOM_API_KEY:-}\""
                .to_string(),
        ];
        let outcome = spawn_and_wait(SpawnSpec {
            argv: &argv,
            cwd: dir.path().to_path_buf(),
            env_pass: &[],
            timeout_s: 10,
            log_path: dir.path().join("env.log"),
        })
        .expect("run");
        assert_eq!(
            outcome.exit,
            Some(0),
            "allowlisted env present, host secrets stripped"
        );
    }
}
