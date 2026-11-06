//! Native executor: a bounded, single-task agent loop.
//!
//! The executor is a normal adapter process. It reads a compiled packet, calls
//! an OpenAI-compatible model, runs local tools, and writes `final.md`,
//! `report.md`, and `usage.jsonl` into the outbox. Verification stays with
//! `loom verify`; this process only produces a claim plus evidence.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result};
use loom_core::TaskSpec;
use serde_json::{json, Value};

use crate::provider::{
    ChatMessage, ChatRequest, ChatResponse, OpenAiProvider, Provider, ScriptedProvider, ToolCall,
    Usage,
};
use crate::tools::ToolBox;

pub const NATIVE_VERSION: &str = "0.1.0";
const DEFAULT_MAX_IDLE_STEPS: u32 = 6;
const DEFAULT_MAX_TOKENS_PER_CALL: u64 = 8_192;

#[derive(Debug, Clone)]
pub struct NativeRequest {
    pub run_id: String,
    pub task_id: String,
    pub attempt: u32,
    pub packet_dir: PathBuf,
    pub worktree: PathBuf,
    pub output_dir: PathBuf,
    pub logs_dir: PathBuf,
    pub model: String,
    pub model_id: String,
    pub branch: String,
    pub timeout_s: u64,
}

impl NativeRequest {
    /// Read the contract passed by `loom-adapter` through environment variables.
    pub fn from_env() -> Result<Self> {
        let worktree = std::env::current_dir().context("current directory")?;
        let packet_dir = env_path("LOOM_PACKET_DIR").context("LOOM_PACKET_DIR is required")?;
        let output_dir = env_path("LOOM_OUTPUT_DIR").context("LOOM_OUTPUT_DIR is required")?;
        let logs_dir = env_path("LOOM_LOGS_DIR").unwrap_or_else(|| output_dir.join("logs"));
        let model = std::env::var("LOOM_MODEL").unwrap_or_else(|_| "worker".to_string());
        let model_id = std::env::var("LOOM_MODEL_ID")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| model.clone());
        Ok(Self {
            run_id: std::env::var("LOOM_RUN_ID").context("LOOM_RUN_ID is required")?,
            task_id: std::env::var("LOOM_TASK_ID").context("LOOM_TASK_ID is required")?,
            attempt: std::env::var("LOOM_ATTEMPT")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(1),
            packet_dir,
            worktree,
            output_dir,
            logs_dir,
            model,
            model_id,
            branch: std::env::var("LOOM_BRANCH").unwrap_or_else(|_| "agent/unknown".to_string()),
            timeout_s: std::env::var("LOOM_TIMEOUT_S")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(1_800),
        })
    }
}

#[derive(Debug, Clone)]
pub struct NativeConfig {
    pub max_steps: u32,
    pub max_input_tokens: u64,
    pub max_output_tokens: u64,
    pub max_wall_s: u64,
    pub max_cost_usd: f64,
    pub max_idle_steps: u32,
    pub max_tokens_per_call: u64,
    pub temperature: f64,
    pub run_acceptance: bool,
}

impl NativeConfig {
    fn from_spec(spec: &TaskSpec, timeout_s: u64) -> Self {
        let max_tokens_per_call = std::env::var("LOOM_NATIVE_MAX_TOKENS")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(DEFAULT_MAX_TOKENS_PER_CALL)
            .min(spec.budget.max_output_tokens);
        Self {
            max_steps: spec.budget.max_steps,
            max_input_tokens: spec.budget.max_input_tokens,
            max_output_tokens: spec.budget.max_output_tokens,
            max_wall_s: spec.budget.max_wall_s.min(timeout_s),
            max_cost_usd: spec.budget.max_cost_usd,
            max_idle_steps: std::env::var("LOOM_NATIVE_MAX_IDLE")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(DEFAULT_MAX_IDLE_STEPS),
            max_tokens_per_call,
            temperature: std::env::var("LOOM_NATIVE_TEMPERATURE")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(0.0),
            run_acceptance: std::env::var("LOOM_NATIVE_SKIP_ACCEPTANCE")
                .map(|value| value == "0" || value.eq_ignore_ascii_case("false"))
                .unwrap_or(true),
        }
    }
}

#[derive(Debug, Clone)]
pub struct NativeOutcome {
    pub exit_code: i32,
    pub status: String,
    pub steps: u32,
    pub usage: Usage,
    pub summary: String,
    pub verify: Vec<VerifyLine>,
}

#[derive(Debug, Clone)]
pub struct VerifyLine {
    pub id: String,
    pub result: String,
    pub exit: i32,
}

/// Run from environment variables. Used by the `loom-native` binary.
pub fn run_from_env() -> Result<NativeOutcome> {
    let request = NativeRequest::from_env()?;
    if let Some(path) = std::env::var_os("LOOM_NATIVE_SCRIPT") {
        let mut provider = ScriptedProvider::from_file(Path::new(&path))?;
        run(&request, &mut provider, None)
    } else {
        let mut provider = OpenAiProvider::from_env()?;
        run(&request, &mut provider, None)
    }
}

/// Execute one task. `config` is optional so the binary can derive it from the
/// TaskSpec in the packet.
pub fn run(
    request: &NativeRequest,
    provider: &mut dyn Provider,
    config: Option<NativeConfig>,
) -> Result<NativeOutcome> {
    fs::create_dir_all(&request.output_dir).context("create outbox")?;
    fs::create_dir_all(&request.logs_dir).context("create logs")?;
    let spec = read_spec(&request.packet_dir)?;
    let config = config.unwrap_or_else(|| NativeConfig::from_spec(&spec, request.timeout_s));

    let system = read_optional(&request.packet_dir.join("system.md"))
        .unwrap_or_else(|| default_system(&spec));
    let packet = read_optional(&request.packet_dir.join("packet.md"))
        .or_else(|| read_optional(&request.packet_dir.join("task.json")))
        .unwrap_or_else(|| serde_json::to_string_pretty(&spec).unwrap_or_default());

    let mut messages = vec![
        ChatMessage::system(system),
        ChatMessage::user(format!(
            "{packet}\n\nWorktree: {}\nBranch: {}\n",
            request.worktree.display(),
            request.branch
        )),
    ];
    let tools = tool_schema();
    let mut toolbox = ToolBox::new(
        &request.worktree,
        spec.clone(),
        request.output_dir.join("artifacts"),
    );

    let started = Instant::now();
    let mut usage = Usage::default();
    let mut steps = 0u32;
    let mut idle = 0u32;
    let mut finished: Option<Finish> = None;

    while steps < config.max_steps {
        if let Some(reason) = budget_reason(&config, &usage, started.elapsed()) {
            finished = Some(Finish::budget(&reason));
            break;
        }

        steps += 1;
        toolbox.set_step(steps);
        let call_started = Instant::now();
        let response = provider.complete(&ChatRequest {
            model: request.model_id.clone(),
            messages: messages.clone(),
            tools: tools.clone(),
            max_tokens: config.max_tokens_per_call,
            temperature: config.temperature,
        })?;
        let latency_ms = call_started.elapsed().as_millis() as u64;
        usage.add(&response.usage);
        append_usage(request, &response, latency_ms)?;

        let ChatResponse { message, .. } = response;
        let calls = message.tool_calls.clone().unwrap_or_default();
        if calls.is_empty() {
            let summary = message.content.clone().unwrap_or_default();
            finished = Some(Finish::done(summary));
            messages.push(message);
            break;
        }

        messages.push(message);
        let mut mutated = false;
        for call in &calls {
            if call.name == "finish" {
                let finish = parse_finish(call);
                finished = Some(finish);
                continue;
            }
            let observation = match toolbox.execute(&call.name, &call.arguments) {
                Ok(result) => {
                    mutated |= result.mutated;
                    result.observation
                }
                Err(error) => format!("tool error: {error:#}"),
            };
            messages.push(ChatMessage::tool(
                call.id.clone(),
                call.name.clone(),
                observation,
            ));
        }

        if finished.is_some() {
            break;
        }
        if mutated {
            idle = 0;
        } else {
            idle += 1;
            if idle >= config.max_idle_steps {
                finished = Some(Finish::blocked(format!(
                    "no file changes for {idle} consecutive steps"
                )));
                break;
            }
        }
    }

    let mut finish = finished.unwrap_or_else(|| Finish::budget("max_steps exhausted"));
    if usage.input_tokens > config.max_input_tokens
        || usage.output_tokens > config.max_output_tokens
        || usage
            .cost_usd
            .is_some_and(|cost| cost > config.max_cost_usd)
    {
        finish = Finish::budget("token or cost budget exceeded");
    }
    if started.elapsed().as_secs() >= config.max_wall_s {
        finish = Finish::budget("wall-clock budget exceeded");
    }

    let verify = if finish.status == "done" && config.run_acceptance {
        run_acceptance(request, &spec, started.elapsed(), config.max_wall_s)
    } else {
        spec.acceptance
            .iter()
            .map(|item| VerifyLine {
                id: item.id.clone(),
                result: "skip".to_string(),
                exit: -1,
            })
            .collect()
    };
    let status = if finish.status == "done" && verify.iter().any(|item| item.result == "fail") {
        "failed"
    } else {
        finish.status.as_str()
    };
    let exit_code = match status {
        "done" | "failed" => 0,
        "budget_exceeded" => 3,
        "needs_input" | "blocked" => 2,
        _ => 4,
    };

    write_report(request, &spec, &finish, &usage, &verify, steps)?;
    write_final(request, &spec, status, &verify)?;

    Ok(NativeOutcome {
        exit_code,
        status: status.to_string(),
        steps,
        usage,
        summary: finish.summary,
        verify,
    })
}

struct Finish {
    status: String,
    summary: String,
    report: Option<String>,
}

impl Finish {
    fn done(summary: String) -> Self {
        Self {
            status: "done".into(),
            summary,
            report: None,
        }
    }

    fn blocked(summary: String) -> Self {
        Self {
            status: "blocked".into(),
            summary,
            report: None,
        }
    }

    fn budget(summary: &str) -> Self {
        Self {
            status: "budget_exceeded".into(),
            summary: summary.to_string(),
            report: None,
        }
    }
}

fn parse_finish(call: &ToolCall) -> Finish {
    let value: Value = serde_json::from_str(&call.arguments).unwrap_or_default();
    let status = value
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("done")
        .to_string();
    let status = match status.as_str() {
        "done" | "blocked" | "needs_input" | "failed" => status,
        _ => "blocked".to_string(),
    };
    Finish {
        status,
        summary: value
            .get("summary")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        report: value
            .get("report")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
    }
}

fn budget_reason(config: &NativeConfig, usage: &Usage, elapsed: Duration) -> Option<String> {
    if usage.input_tokens >= config.max_input_tokens {
        return Some("input token budget exceeded".to_string());
    }
    if usage.output_tokens >= config.max_output_tokens {
        return Some("output token budget exceeded".to_string());
    }
    if usage
        .cost_usd
        .is_some_and(|cost| cost >= config.max_cost_usd)
    {
        return Some("cost budget exceeded".to_string());
    }
    if elapsed.as_secs() >= config.max_wall_s {
        return Some("wall-clock budget exceeded".to_string());
    }
    None
}

fn read_spec(packet_dir: &Path) -> Result<TaskSpec> {
    let path = packet_dir.join("task.json");
    let text = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("parse {}", path.display()))
}

fn read_optional(path: &Path) -> Option<String> {
    fs::read_to_string(path)
        .ok()
        .filter(|text| !text.trim().is_empty())
}

fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name).map(PathBuf::from)
}

fn default_system(spec: &TaskSpec) -> String {
    format!(
        "You are a Loom worker implementing one bounded task ({}).\n\
         Use the tools to inspect and edit the worktree. Keep changes inside allowed_paths: {}.\n\
         Run focused checks, then call finish with status done and a short summary.\n\
         Do not modify .loom or files outside the task workspace.",
        spec.task_id,
        spec.workspace.allowed_paths.join(", ")
    )
}

fn tool_schema() -> Value {
    json!([
        {
            "type": "function",
            "function": {
                "name": "read",
                "description": "Read a file with line numbers. Prefer a 1-based range for large files.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {"type": "string"},
                        "range": {"type": "string", "description": "start:end, 1-based inclusive"}
                    },
                    "required": ["path"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "search",
                "description": "Regex search inside the worktree. Returns path:line:text matches.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "query": {"type": "string"},
                        "scope": {"type": "string"},
                        "max_results": {"type": "integer"}
                    },
                    "required": ["query"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "edit",
                "description": "Replace exact existing text in a file. The old text must be unique unless replace_all is true.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {"type": "string"},
                        "old": {"type": "string"},
                        "new": {"type": "string"},
                        "replace_all": {"type": "boolean"}
                    },
                    "required": ["path", "old", "new"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "write",
                "description": "Create or overwrite a file inside allowed_paths.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {"type": "string"},
                        "content": {"type": "string"}
                    },
                    "required": ["path", "content"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "bash",
                "description": "Run a shell command in the worktree. Long output is truncated to an artifact path.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "cmd": {"type": "string"},
                        "timeout_s": {"type": "integer"}
                    },
                    "required": ["cmd"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "finish",
                "description": "Stop the task. status: done, blocked, needs_input, or failed.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "status": {"type": "string", "enum": ["done", "blocked", "needs_input", "failed"]},
                        "summary": {"type": "string"},
                        "report": {"type": "string"}
                    },
                    "required": ["status", "summary"]
                }
            }
        }
    ])
}

fn append_usage(request: &NativeRequest, response: &ChatResponse, latency_ms: u64) -> Result<()> {
    let usage = &response.usage;
    let mut row = json!({
        "v": 1,
        "run_id": request.run_id,
        "task_id": request.task_id,
        "model": request.model_id,
        "input_tokens": usage.input_tokens,
        "cached_input_tokens": usage.cached_input_tokens,
        "output_tokens": usage.output_tokens,
        "reasoning_tokens": usage.reasoning_tokens,
        "latency_ms": latency_ms,
        "status": 200,
        "source": "runtime",
    });
    if let Some(cost) = usage.cost_usd {
        row["cost_usd"] = json!(cost);
    }
    let path = request.output_dir.join("usage.jsonl");
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("open {}", path.display()))?;
    writeln!(file, "{}", serde_json::to_string(&row)?)?;
    Ok(())
}

fn run_acceptance(
    request: &NativeRequest,
    spec: &TaskSpec,
    already_elapsed: Duration,
    max_wall_s: u64,
) -> Vec<VerifyLine> {
    let log_path = request.logs_dir.join("verify.log");
    let mut log = match OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&log_path)
    {
        Ok(file) => file,
        Err(_error) => {
            return spec
                .acceptance
                .iter()
                .map(|item| VerifyLine {
                    id: item.id.clone(),
                    result: "fail".to_string(),
                    exit: -1,
                })
                .collect();
        }
    };

    let mut results = Vec::new();
    for item in &spec.acceptance {
        let remaining = max_wall_s.saturating_sub(already_elapsed.as_secs());
        if remaining == 0 {
            results.push(VerifyLine {
                id: item.id.clone(),
                result: "fail".to_string(),
                exit: -1,
            });
            continue;
        }
        let timeout = item.timeout_s.min(remaining).max(1);
        let _ = writeln!(log, "== {}: {}", item.id, item.cmd);
        let started = Instant::now();
        let output = run_with_timeout(&item.cmd, &request.worktree, timeout);
        let (exit, text) = match output {
            Ok(result) => result,
            Err(error) => (-1, format!("failed to run: {error:#}")),
        };
        let _ = writeln!(
            log,
            "exit={exit} duration={:.2}s\n{}",
            started.elapsed().as_secs_f64(),
            text
        );
        results.push(VerifyLine {
            id: item.id.clone(),
            result: if exit == item.expect_exit {
                "pass".to_string()
            } else {
                "fail".to_string()
            },
            exit,
        });
    }
    results
}

fn run_with_timeout(cmd: &str, cwd: &Path, timeout_s: u64) -> Result<(i32, String)> {
    let dir = tempfile::tempdir().context("tempdir for command output")?;
    let log_path = dir.path().join("output.log");
    let log = fs::File::create(&log_path)?;
    let err = log.try_clone()?;
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .current_dir(cwd)
        .env_remove("LOOM_API_KEY")
        .env_remove("OPENAI_API_KEY")
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(err))
        .stdin(Stdio::null())
        .spawn()
        .with_context(|| format!("spawn: {cmd}"))?;
    let started = Instant::now();
    let mut timed_out = false;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break Some(status);
        }
        if started.elapsed() >= Duration::from_secs(timeout_s) {
            timed_out = true;
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(25));
    };
    let text = fs::read_to_string(&log_path).unwrap_or_default();
    let exit = if timed_out {
        124
    } else {
        status.and_then(|status| status.code()).unwrap_or(-1)
    };
    Ok((exit, text))
}

fn write_final(
    request: &NativeRequest,
    spec: &TaskSpec,
    status: &str,
    verify: &[VerifyLine],
) -> Result<()> {
    let head = git_head(&request.worktree).unwrap_or_else(|| "unknown".to_string());
    let results = if verify.is_empty() {
        spec.acceptance
            .iter()
            .map(|item| format!("{}=skip", item.id))
            .collect::<Vec<_>>()
            .join(",")
    } else {
        verify
            .iter()
            .map(|item| format!("{}={}", item.id, item.result))
            .collect::<Vec<_>>()
            .join(",")
    };
    let log = format!(".loom/runs/{}/logs/verify.log", request.run_id);
    let report = format!(".loom/runs/{}/outbox/report.md", request.run_id);
    let text = format!(
        "status: {status}\nbranch: {}@{head}\nverify: {results}; log={log}\nreport: {report}\n",
        request.branch
    );
    fs::write(request.output_dir.join("final.md"), text).context("write final.md")?;
    Ok(())
}

fn write_report(
    request: &NativeRequest,
    spec: &TaskSpec,
    finish: &Finish,
    usage: &Usage,
    verify: &[VerifyLine],
    steps: u32,
) -> Result<()> {
    let mut text = String::new();
    text.push_str(&format!("# {}\n\n", spec.title));
    text.push_str(&format!("- task: `{}`\n", spec.task_id));
    text.push_str(&format!("- run: `{}`\n", request.run_id));
    text.push_str(&format!("- model: `{}`\n", request.model_id));
    text.push_str(&format!("- status: `{}`\n", finish.status));
    text.push_str(&format!("- steps: {steps}\n"));
    text.push_str(&format!(
        "- tokens: input={} cached={} output={}\n",
        usage.input_tokens, usage.cached_input_tokens, usage.output_tokens
    ));
    if let Some(cost) = usage.cost_usd {
        text.push_str(&format!("- cost: ${cost:.6}\n"));
    }
    text.push_str("\n## Summary\n\n");
    text.push_str(if finish.summary.trim().is_empty() {
        "(no summary)"
    } else {
        &finish.summary
    });
    text.push_str("\n\n## Verify\n\n");
    if verify.is_empty() {
        text.push_str("not run\n");
    } else {
        for item in verify {
            text.push_str(&format!(
                "- {}: {} (exit {})\n",
                item.id, item.result, item.exit
            ));
        }
    }
    if let Some(report) = &finish.report {
        text.push_str("\n## Worker report\n\n");
        text.push_str(report);
        text.push('\n');
    }
    fs::write(request.output_dir.join("report.md"), text).context("write report.md")?;
    Ok(())
}

fn git_head(worktree: &Path) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(worktree)
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!text.is_empty()).then_some(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::ScriptedProvider;

    fn request(dir: &Path) -> NativeRequest {
        let packet = dir.join("context");
        let worktree = dir.join("worktree");
        let output = dir.join("outbox");
        let logs = dir.join("logs");
        fs::create_dir_all(&packet).expect("packet");
        fs::create_dir_all(&worktree).expect("worktree");
        fs::create_dir_all(&output).expect("outbox");
        fs::create_dir_all(&logs).expect("logs");
        let spec = TaskSpec {
            task_id: "T-20261106-001".into(),
            title: "native test".into(),
            workspace: loom_core::TaskWorkspace {
                allowed_paths: vec!["src/**".into()],
                ..loom_core::TaskWorkspace::default()
            },
            acceptance: vec![loom_core::Acceptance {
                id: "A1".into(),
                cmd: "test -f src/out.txt".into(),
                expect_exit: 0,
                timeout_s: 10,
            }],
            budget: loom_core::TaskBudget {
                max_steps: 5,
                max_input_tokens: 10_000,
                max_output_tokens: 2_000,
                max_wall_s: 30,
                max_cost_usd: 1.0,
            },
            ..TaskSpec::default()
        };
        fs::write(
            packet.join("task.json"),
            serde_json::to_string_pretty(&spec).unwrap(),
        )
        .expect("task");
        fs::write(packet.join("system.md"), "system").expect("system");
        fs::write(packet.join("packet.md"), "packet").expect("packet");
        NativeRequest {
            run_id: "R-20261106-001".into(),
            task_id: spec.task_id,
            attempt: 1,
            packet_dir: packet,
            worktree,
            output_dir: output,
            logs_dir: logs,
            model: "worker".into(),
            model_id: "test-model".into(),
            branch: "agent/T-20261106-001".into(),
            timeout_s: 30,
        }
    }

    #[test]
    fn writes_file_and_runs_acceptance() {
        let dir = tempfile::tempdir().expect("tempdir");
        let request = request(dir.path());
        let script = json!([
            {
                "tool_calls": [{
                    "name": "write",
                    "arguments": {"path": "src/out.txt", "content": "ok\n"}
                }],
                "usage": {"input_tokens": 100, "output_tokens": 10}
            },
            {
                "tool_calls": [{
                    "name": "finish",
                    "arguments": {"status": "done", "summary": "created out.txt"}
                }],
                "usage": {"input_tokens": 100, "output_tokens": 10}
            }
        ]);
        let mut provider = ScriptedProvider::parse(&script.to_string()).expect("script");
        let outcome = run(&request, &mut provider, None).expect("run");
        assert_eq!(outcome.status, "done", "{outcome:?}");
        assert_eq!(outcome.exit_code, 0);
        assert_eq!(outcome.verify[0].result, "pass");
        assert!(request.worktree.join("src/out.txt").exists());
        let final_text = fs::read_to_string(request.output_dir.join("final.md")).expect("final");
        assert!(final_text.starts_with("status: done\n"));
        let usage = fs::read_to_string(request.output_dir.join("usage.jsonl")).expect("usage");
        assert_eq!(usage.lines().count(), 2);
        let row: Value =
            serde_json::from_str(usage.lines().next().expect("row")).expect("parse usage row");
        assert_eq!(row["v"], 1);
        assert!(row.get("cost_usd").is_none(), "absent cost must be omitted");
    }

    #[test]
    fn stops_on_budget() {
        let dir = tempfile::tempdir().expect("tempdir");
        let request = request(dir.path());
        let mut config = NativeConfig::from_spec(
            &read_spec(&request.packet_dir).expect("spec"),
            request.timeout_s,
        );
        config.max_steps = 1;
        config.max_input_tokens = 1;
        let script = json!([{
            "tool_calls": [{"name": "read", "arguments": {"path": "src/missing"}}],
            "usage": {"input_tokens": 10, "output_tokens": 1}
        }]);
        let mut provider = ScriptedProvider::parse(&script.to_string()).expect("script");
        let outcome = run(&request, &mut provider, Some(config)).expect("run");
        assert_eq!(outcome.status, "budget_exceeded");
        assert_eq!(outcome.exit_code, 3);
    }
}
