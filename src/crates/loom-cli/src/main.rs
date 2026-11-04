use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use loom_core::{truncate_chars, Budget, BudgetLevel, Envelope, ErrorCode, Limits, ToolError};
use loom_store::{append_event, now_ms, totals, Project, Run, ToolEvent, Totals};
use loom_tools::{
    read as read_file, read_artifact as read_artifact_file, search, ArtifactRequest, ReadRequest,
    SearchRequest, ToolOutcome,
};

#[derive(Debug, Parser)]
#[command(
    name = "loom",
    version,
    about = "Token-efficient agent harness control plane"
)]
struct Cli {
    /// Project root. Defaults to walking up from the current directory.
    #[arg(long, global = true)]
    root: Option<PathBuf>,

    /// Output format for tool envelopes.
    #[arg(long, global = true, value_enum, default_value = "json")]
    format: Format,

    /// Run id. Defaults to LOOM_RUN_ID or "local".
    #[arg(long, global = true)]
    run: Option<String>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum Format {
    Json,
    Pretty,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Create the .loom skeleton.
    Init {
        /// Overwrite config.toml (goal.md and constraints.md are never overwritten).
        #[arg(long)]
        force: bool,
    },
    /// Check the local workspace and runtime layout.
    Doctor,
    /// Show project and run summary.
    Status,
    /// Budgeted tool wrappers.
    Tool {
        #[command(subcommand)]
        command: ToolCommand,
    },
    /// Read a paged artifact produced by a previous tool call.
    Artifact {
        #[arg(long = "ref")]
        artifact_ref: String,
        #[arg(long)]
        range: Option<String>,
        #[arg(long)]
        max_lines: Option<usize>,
    },
}

#[derive(Debug, Subcommand)]
enum ToolCommand {
    /// Search the repository with ripgrep-like semantics.
    Search {
        #[arg(long, short = 'q')]
        query: String,
        #[arg(long)]
        scope: Option<String>,
        #[arg(long = "glob")]
        globs: Vec<String>,
        #[arg(long)]
        max_results: Option<usize>,
        #[arg(long)]
        case_sensitive: bool,
        #[arg(long)]
        cursor: Option<String>,
    },
    /// Read a bounded line range from a file.
    Read {
        #[arg(long)]
        path: String,
        #[arg(long)]
        range: Option<String>,
        #[arg(long)]
        max_lines: Option<usize>,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(code) => ExitCode::from(code.clamp(0, 255) as u8),
        Err(error) => {
            eprintln!("loom: {error:#}");
            ExitCode::from(4)
        }
    }
}

fn run(cli: Cli) -> Result<i32> {
    let root = match cli.root {
        Some(root) => root,
        None => std::env::current_dir().context("cannot read current directory")?,
    };
    let project = Project::at(root);

    match cli.command {
        Command::Init { force } => {
            let report = project.init(force)?;
            println!("initialized {}", project.loom_dir().display());
            for path in &report.created_files {
                println!("  created {}", path.display());
            }
            for path in &report.skipped_files {
                println!("  kept    {}", path.display());
            }
            if report.gitignore_updated {
                println!("  updated .gitignore (.loom/, .worktrees/)");
            }
            println!("next: edit .loom/goal.md and .loom/constraints.md");
            Ok(0)
        }
        Command::Doctor => doctor(&project),
        Command::Status => status(&project),
        Command::Tool { command } => {
            let config = project.load_config()?;
            let run = Run::open(project.clone(), cli.run.as_deref())?;
            let invocation = std::env::var("LOOM_INVOCATION_ID").ok();
            let totals_before = totals(&run, invocation.as_deref())?;
            let started = Instant::now();

            let outcome = match command {
                ToolCommand::Search {
                    query,
                    scope,
                    globs,
                    max_results,
                    case_sensitive,
                    cursor,
                } => search(
                    &project,
                    &run,
                    &SearchRequest {
                        query,
                        scope,
                        globs,
                        max_results,
                        case_sensitive,
                        cursor,
                    },
                    &config.tools.limits,
                )?,
                ToolCommand::Read {
                    path,
                    range,
                    max_lines,
                } => read_file(
                    &project,
                    &run,
                    &ReadRequest {
                        path,
                        range,
                        max_lines,
                    },
                    &config.tools.limits,
                )?,
            };

            finish(
                &run,
                &config.tools.limits,
                invocation.as_deref(),
                started,
                totals_before,
                outcome,
                cli.format,
            )
        }
        Command::Artifact {
            artifact_ref,
            range,
            max_lines,
        } => {
            let config = project.load_config()?;
            let run = Run::open(project.clone(), cli.run.as_deref())?;
            let invocation = std::env::var("LOOM_INVOCATION_ID").ok();
            let totals_before = totals(&run, invocation.as_deref())?;
            let started = Instant::now();

            let outcome = read_artifact_file(
                &run,
                &ArtifactRequest {
                    artifact_ref,
                    range,
                    max_lines,
                },
                &config.tools.limits,
            )?;

            finish(
                &run,
                &config.tools.limits,
                invocation.as_deref(),
                started,
                totals_before,
                outcome,
                cli.format,
            )
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn finish(
    run: &Run,
    limits: &Limits,
    invocation: Option<&str>,
    started: Instant,
    totals_before: Totals,
    outcome: ToolOutcome,
    format: Format,
) -> Result<i32> {
    let mut envelope = outcome.envelope;
    let degraded_at = apply_budget(&mut envelope, limits, totals_before, invocation);
    print_envelope(&envelope, format)?;

    let stats = envelope.stats.clone().unwrap_or_default();
    let exit = if envelope.ok {
        0
    } else {
        envelope
            .err
            .as_ref()
            .map(|error| error.code.exit_code())
            .unwrap_or(4)
    };

    let event = ToolEvent {
        ts_ms: now_ms(),
        run: run.id.clone(),
        invocation: invocation.map(ToOwned::to_owned),
        tool: envelope.tool.as_str().to_string(),
        args_hash: outcome.args_hash,
        scope: outcome.scope,
        items: envelope.items.as_ref().map(Vec::len).unwrap_or(0),
        bytes: stats.bytes.unwrap_or_else(|| envelope.bytes()),
        tok_est: stats.tok.unwrap_or(0),
        truncated: envelope.truncated.unwrap_or(false),
        spill: stats.spill.unwrap_or(false),
        dedupe: envelope.unchanged.unwrap_or(false) || stats.dedupe.unwrap_or(false),
        stale: outcome.stale,
        ms: started.elapsed().as_millis() as u64,
        exit,
        budget_at: degraded_at
            .or(envelope.budget.as_ref().and_then(|budget| budget.at))
            .map(BudgetLevel::as_str)
            .map(ToOwned::to_owned),
    };
    append_event(run, &event)?;

    Ok(exit)
}

fn apply_budget(
    envelope: &mut Envelope,
    limits: &Limits,
    totals: Totals,
    invocation: Option<&str>,
) -> Option<BudgetLevel> {
    envelope.finalize_stats();

    let mut bytes = envelope
        .stats
        .as_ref()
        .and_then(|stats| stats.bytes)
        .unwrap_or_else(|| envelope.bytes());
    let mut degraded_at = None;

    let run_over = totals.run_bytes.saturating_add(bytes) > limits.run_bytes;
    if run_over {
        envelope.degrade();
        envelope.finalize_stats();
        degraded_at = Some(BudgetLevel::Run);
        bytes = envelope
            .stats
            .as_ref()
            .and_then(|stats| stats.bytes)
            .unwrap_or_else(|| envelope.bytes());
        if totals.run_bytes.saturating_add(bytes) > limits.run_bytes
            && envelope.artifact_ref.is_none()
        {
            *envelope = Envelope::error(
                envelope.tool,
                "run 预算已用尽，请收窄 scope",
                ToolError::new(ErrorCode::Budget),
            );
            envelope.finalize_stats();
        }
    } else if invocation.is_some()
        && totals.invocation_bytes.saturating_add(bytes) > limits.invocation_bytes
    {
        envelope.degrade();
        envelope.finalize_stats();
        degraded_at = Some(BudgetLevel::Invocation);
        bytes = envelope
            .stats
            .as_ref()
            .and_then(|stats| stats.bytes)
            .unwrap_or_else(|| envelope.bytes());
        if totals.invocation_bytes.saturating_add(bytes) > limits.invocation_bytes
            && envelope.artifact_ref.is_none()
        {
            *envelope = Envelope::error(
                envelope.tool,
                "本次调用预算已用尽，请收窄 scope",
                ToolError::new(ErrorCode::Budget),
            );
            envelope.finalize_stats();
        }
    }

    bytes = envelope
        .stats
        .as_ref()
        .and_then(|stats| stats.bytes)
        .unwrap_or_else(|| envelope.bytes());
    let run_used = totals.run_bytes.saturating_add(bytes);
    let invocation_used = totals.invocation_bytes.saturating_add(bytes);

    let (used, limit, at) = if invocation.is_some() {
        let run_ratio = ratio(run_used, limits.run_bytes);
        let invocation_ratio = ratio(invocation_used, limits.invocation_bytes);
        if invocation_ratio > run_ratio {
            (
                invocation_used,
                limits.invocation_bytes,
                BudgetLevel::Invocation,
            )
        } else {
            (run_used, limits.run_bytes, BudgetLevel::Run)
        }
    } else {
        (run_used, limits.run_bytes, BudgetLevel::Run)
    };

    let percent = ((used.saturating_mul(100)) / limit).min(100) as u8;
    if percent >= 75 {
        envelope.budget = Some(Budget {
            u: used,
            l: limit,
            p: percent,
            at: Some(at),
        });
        envelope.finalize_stats();
    }

    degraded_at
}

fn ratio(used: usize, limit: usize) -> f64 {
    if limit == 0 {
        return 0.0;
    }
    used as f64 / limit as f64
}

fn print_envelope(envelope: &Envelope, format: Format) -> Result<()> {
    match format {
        Format::Json => println!("{}", envelope.to_json()),
        Format::Pretty => print_pretty(envelope),
    }
    Ok(())
}

fn print_pretty(envelope: &Envelope) {
    if let Some(summary) = &envelope.summary {
        println!("{summary}");
    }

    if !envelope.ok {
        if let Some(error) = &envelope.err {
            let hint = error.hint.as_deref().unwrap_or("");
            println!("error: {} {}", error.code.as_str(), hint);
        }
    }

    if let Some(text) = &envelope.text {
        println!("\n{text}");
    }

    if let Some(items) = &envelope.items {
        if !items.is_empty() {
            println!();
        }
        for item in items {
            let path = item.path.as_deref().unwrap_or_default();
            let path = truncate_chars(path, 56).0;
            let range = item
                .range
                .map(|range| format!("{}:{}", range[0], range[1]))
                .unwrap_or_default();
            let preview = item.preview.as_deref().unwrap_or_default();
            println!("{path:<56} {range:>10}  {preview}");
        }
    }

    if let Some(artifact_ref) = &envelope.artifact_ref {
        println!("\nref: {artifact_ref}");
    }
    if let Some(next) = &envelope.next {
        println!("next: {next}");
    }
    if envelope.unchanged == Some(true) {
        println!("unchanged: true");
    }
    if let Some(budget) = &envelope.budget {
        println!("budget: {}% ({}/{})", budget.p, budget.u, budget.l);
    }
    if let Some(stats) = &envelope.stats {
        println!(
            "stats: n={} bytes={} tok≈{}",
            stats.n.unwrap_or(0),
            stats.bytes.unwrap_or(0),
            stats.tok.unwrap_or(0)
        );
    }
}

fn doctor(project: &Project) -> Result<i32> {
    let mut healthy = true;
    let mut check = |name: &str, pass: bool, detail: &str| {
        println!("[{}] {name:<12} {detail}", if pass { "ok" } else { "FAIL" });
        if !pass {
            healthy = false;
        }
    };

    let loom = project.loom_dir();
    check(".loom", loom.is_dir(), &loom.display().to_string());
    check("git", project.root.join(".git").exists(), "repository");

    if loom.is_dir() {
        match project.load_config() {
            Ok(config) => check(
                "config",
                true,
                &format!(
                    "lang={} call={}B",
                    config.lang, config.tools.limits.call_bytes
                ),
            ),
            Err(error) => check("config", false, &error.to_string()),
        }

        let probe = loom.join(".doctor");
        match fs::write(&probe, b"ok").and_then(|()| fs::remove_file(&probe)) {
            Ok(()) => check("writable", true, ".loom/"),
            Err(error) => check("writable", false, &error.to_string()),
        }
    }

    Ok(if healthy { 0 } else { 1 })
}

fn status(project: &Project) -> Result<i32> {
    println!("project: {}", project.root.display());
    println!(
        "git:     {}",
        project.git_head().unwrap_or_else(|| "none".into())
    );

    let loom = project.loom_dir();
    if !loom.is_dir() {
        println!(".loom:   missing (run `loom init`)");
        return Ok(1);
    }

    let config = project.load_config()?;
    println!(".loom:   {} (lang={})", loom.display(), config.lang);
    println!(
        "limits:  call={}B invocation={}B run={}B",
        config.tools.limits.call_bytes,
        config.tools.limits.invocation_bytes,
        config.tools.limits.run_bytes
    );
    println!("runs:    {}", count_entries(&loom.join("runs"), true));
    println!("tasks:   {}", count_entries(&loom.join("tasks"), false));
    Ok(0)
}

fn count_entries(path: &std::path::Path, dirs_only: bool) -> usize {
    fs::read_dir(path)
        .map(|entries| {
            entries
                .flatten()
                .filter(|entry| {
                    entry
                        .file_type()
                        .map(|file_type| !dirs_only || file_type.is_dir())
                        .unwrap_or(false)
                })
                .count()
        })
        .unwrap_or(0)
}
