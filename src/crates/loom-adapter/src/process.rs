use std::fs::File;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result};

use crate::preset::preset;
use crate::types::AdapterRequest;

#[derive(Debug, Clone)]
pub struct ProcessOutcome {
    pub exit_code: i32,
    pub timed_out: bool,
    pub duration_ms: u64,
    pub stdout_path: PathBuf,
    pub stderr_path: PathBuf,
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
            drop(stdin);
        }
    }

    let started = Instant::now();
    let mut timed_out = false;
    let mut status = None;
    loop {
        match child.try_wait().with_context(|| "wait for adapter")? {
            Some(waited) => {
                status = Some(waited);
                break;
            }
            None => {
                if started.elapsed().as_secs() >= request.timeout_s {
                    timed_out = true;
                    let _ = child.kill();
                    let _ = child.wait();
                    break;
                }
                thread::sleep(Duration::from_millis(25));
            }
        }
    }

    let exit_code = if timed_out {
        3
    } else {
        status
            .as_ref()
            .and_then(|status| status.code())
            .unwrap_or(4)
    };

    Ok(ProcessOutcome {
        exit_code,
        timed_out,
        duration_ms: started.elapsed().as_millis() as u64,
        stdout_path,
        stderr_path,
    })
}
