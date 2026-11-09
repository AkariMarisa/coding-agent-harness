use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use anyhow::{anyhow, Context, Result};
use clap::{Args, Parser, Subcommand, ValueEnum};
use loom_core::{
    truncate_chars, Budget, BudgetLevel, Config, CostSource, Envelope, ErrorCode, Limits,
    ModelCapability, Tier, ToolError, UsageAggregate, UsageFilter, UsageRecord, UsageSource, Wire,
};
use loom_store::{
    aggregate_usage, append_tool_event, append_usage, ledger_path, now_ms, read_usage, totals,
    Project, Run, ToolEvent, Totals,
};
use loom_tools::{
    read as read_file, read_artifact as read_artifact_file, search, ArtifactRequest, ReadRequest,
    SearchRequest, ToolOutcome,
};

mod adapter_cmd;
mod board_cmd;
mod churn_cmd;
mod context_cmd;
mod dispatch_cmd;
mod event_cmd;
mod merge_cmd;
mod prompt_cmd;
mod resume_cmd;
mod route_cmd;
mod run_util;
mod task_cmd;
mod usage_cmd;
mod verify_cmd;

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

    /// Output format for envelopes, capability checks, and ledger reports.
    #[arg(long, global = true, value_enum, default_value = "json")]
    format: Format,

    /// Run id for tools/usage. Ledger only filters when this is provided.
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
    /// Show tasks grouped by status, plus running/lock counts.
    Board,
    /// Budgeted tool wrappers.
    Tool {
        #[command(subcommand)]
        command: ToolCommand,
    },
    /// Inspect configured model aliases and capability bits.
    Capability {
        #[command(subcommand)]
        command: CapabilityCommand,
    },
    /// Explain the deterministic route for a task, or summarize dispatch history.
    #[command(args_conflicts_with_subcommands = true)]
    Route {
        #[command(subcommand)]
        command: Option<RouteCommand>,
        /// Task id to explain; omit when using `route stats`.
        task_id: Option<String>,
        /// Override with a specific model alias from .loom/config.toml.
        #[arg(long)]
        model: Option<String>,
    },
    /// Manage TaskSpecs and the task index.
    Task {
        #[command(subcommand)]
        command: TaskCommand,
    },
    /// Inspect the append-only event log.
    Event {
        #[command(subcommand)]
        command: EventCommand,
    },
    /// Compile a bounded context packet for a task.
    Context {
        #[command(subcommand)]
        command: ContextCommand,
    },
    /// Inspect the versioned prompt library.
    Prompt {
        #[command(subcommand)]
        command: PromptCommand,
    },
    /// Inspect executor adapters and run contract canaries.
    Adapter {
        #[command(subcommand)]
        command: AdapterCommand,
    },
    /// Compile a packet and dispatch a task through an adapter.
    Dispatch {
        task_id: String,
        /// Executor adapter: mock, goose, or opencode.
        #[arg(long, default_value = "mock")]
        adapter: String,
        /// Model alias from .loom/config.toml; defaults to the task tier.
        #[arg(long)]
        model: Option<String>,
        /// Override the adapter timeout in seconds.
        #[arg(long = "timeout-s")]
        timeout_s: Option<u64>,
        /// Attempt number for the receipt and events.
        #[arg(long, default_value_t = 1)]
        attempt: u32,
        /// Force in-place execution instead of a git worktree.
        #[arg(long)]
        in_place: bool,
    },
    /// Re-run acceptance commands independently and write the receipt.
    Verify {
        task_id: String,
        /// Attempt number for the receipt.
        #[arg(long, default_value_t = 1)]
        attempt: u32,
    },
    /// Merge a passed task branch into the base branch (serial merge queue).
    Merge {
        task_id: String,
        /// Attempt number recorded in the verify and merge events.
        #[arg(long, default_value_t = 1)]
        attempt: u32,
    },
    /// Rebuild state from the event log and optionally dispatch ready tasks.
    Resume {
        /// Dispatch every ready task after syncing the index.
        #[arg(long)]
        dispatch: bool,
        /// Adapter used with --dispatch.
        #[arg(long, default_value = "mock")]
        adapter: String,
    },
    /// Record request usage into the append-only ledger.
    Usage {
        #[command(subcommand)]
        command: UsageCommand,
    },
    /// Aggregate the request ledger into CPVT/TPVT/WPT.
    Ledger(LedgerArgs),
    /// Show step-level churn attribution from native run logs.
    Churn {
        #[command(subcommand)]
        command: ChurnCommand,
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
enum ChurnCommand {
    /// Aggregate logs/churn.json across native runs.
    Stats {
        /// Group by model alias/provider id.
        #[arg(long)]
        by: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
enum RouteCommand {
    /// Summarize dispatch tiers from the event log (worker share, escalations).
    Stats,
}

#[derive(Debug, Subcommand)]
enum CapabilityCommand {
    /// List all configured model aliases.
    List,
    /// Show one alias with its capability bits.
    Show {
        /// Model alias from .loom/config.toml.
        alias: String,
    },
    /// Validate configured capability bits and pricing coverage.
    Check {
        /// Check one alias; omit to check all.
        alias: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
enum UsageCommand {
    /// Append one request row. Provider cost wins over local estimation.
    Record(UsageRecordArgs),
    /// Import adapter/LiteLLM usage rows from a JSONL file.
    Ingest(UsageIngestArgs),
}

#[derive(Debug, Args)]
struct UsageIngestArgs {
    /// JSONL file with one usage row per line.
    file: PathBuf,
    /// Default run id when a row does not carry one.
    #[arg(long)]
    run: Option<String>,
    /// Default task id when a row does not carry one.
    #[arg(long)]
    task: Option<String>,
    /// Default usage source: provider, litellm, runtime, headroom, manual.
    #[arg(long, default_value = "litellm")]
    source: String,
    /// Default cost source when a row carries cost without a marker.
    #[arg(long = "cost-source")]
    cost_source: Option<String>,
}

#[derive(Debug, Subcommand)]
enum TaskCommand {
    /// Validate and register a TaskSpec from JSON.
    Add {
        /// Read the TaskSpec from this file instead of --json.
        #[arg(long)]
        file: Option<PathBuf>,
        /// Inline TaskSpec JSON.
        #[arg(long)]
        json: Option<String>,
        /// Replace an existing spec with the same task_id.
        #[arg(long)]
        force: bool,
    },
    /// List every task with its replayed status.
    List,
    /// Show one TaskSpec with its current status.
    Show { task_id: String },
    /// Sync tasks.json and list tasks whose dependencies are verified.
    Ready,
    /// Validate the task graph and print edges.
    Graph,
}

#[derive(Debug, Subcommand)]
enum EventCommand {
    /// List events, optionally filtered by run/task/type.
    List {
        /// Only events for this task id.
        #[arg(long)]
        task: Option<String>,
        /// Event type, e.g. task.dispatched.
        #[arg(long = "type")]
        event_type: Option<String>,
        /// Keep only the last N events.
        #[arg(long)]
        limit: Option<usize>,
    },
}

#[derive(Debug, Subcommand)]
enum ContextCommand {
    /// Compile a bounded packet for a task.
    Compile {
        task_id: String,
        /// Context kind: main, worker, or judgment. Defaults from the task.
        #[arg(long)]
        kind: Option<String>,
        /// Override the hard total budget in tokens.
        #[arg(long = "budget-tokens")]
        budget_tokens: Option<usize>,
        /// Output directory. Defaults to the run's context/ directory.
        #[arg(long)]
        out: Option<PathBuf>,
    },
}

#[derive(Debug, Subcommand)]
enum PromptCommand {
    /// List versioned prompt templates.
    List,
    /// Show one prompt template.
    Show {
        /// Prompt role: orchestrator, worker, judgment, or review.
        role: String,
        /// Print only the body without the header.
        #[arg(long)]
        raw: bool,
    },
}

#[derive(Debug, Subcommand)]
enum AdapterCommand {
    /// List adapter presets and their availability.
    List,
    /// Run adapter contract canaries.
    Canary {
        /// Adapter: mock, goose, or opencode.
        adapter: String,
        /// Also execute a real adapter run (needs --task).
        #[arg(long)]
        live: bool,
        /// Task used for the live canary.
        #[arg(long)]
        task: Option<String>,
    },
}

#[derive(Debug, Args)]
struct UsageRecordArgs {
    /// Task id for attribution; missing means unattributed.
    #[arg(long)]
    task: Option<String>,
    #[arg(long)]
    attempt: Option<u32>,
    /// Model tier: frontier or worker.
    #[arg(long)]
    tier: Option<String>,
    /// Model alias from .loom/config.toml.
    #[arg(long)]
    model: Option<String>,
    /// Real provider model id; defaults to the alias capability.
    #[arg(long = "model-id")]
    model_id: Option<String>,
    /// Provider name; defaults to the alias capability.
    #[arg(long)]
    provider: Option<String>,
    /// Wire protocol: chat, responses, or messages.
    #[arg(long)]
    wire: Option<String>,
    #[arg(long = "input-tokens")]
    input_tokens: Option<u64>,
    #[arg(long = "cached-input-tokens")]
    cached_input_tokens: Option<u64>,
    #[arg(long = "output-tokens")]
    output_tokens: Option<u64>,
    #[arg(long = "reasoning-tokens")]
    reasoning_tokens: Option<u64>,
    /// Actual provider cost. Do not mix estimates here without cost-source.
    #[arg(long = "cost-usd")]
    cost_usd: Option<f64>,
    /// provider, pricing_table, manual, or unknown. Defaults to manual when
    /// --cost-usd is set, otherwise unknown.
    #[arg(long = "cost-source")]
    cost_source: Option<String>,
    /// provider, litellm, runtime, headroom, manual, or unknown.
    #[arg(long, default_value = "unknown")]
    source: String,
    #[arg(long = "latency-ms")]
    latency_ms: Option<u64>,
    /// HTTP/provider status; >= 400 counts as failed.
    #[arg(long)]
    status: Option<u16>,
}

#[derive(Debug, Args)]
struct LedgerArgs {
    /// Filter by task id.
    #[arg(long)]
    task: Option<String>,
    /// Filter by tier: frontier or worker.
    #[arg(long)]
    tier: Option<String>,
    /// Filter by model alias or real model id.
    #[arg(long)]
    model: Option<String>,
    /// Count of verified tasks for CPVT/TPVT/WPT.
    #[arg(long = "passed-tasks", default_value_t = 0)]
    passed_tasks: u64,
    /// Wall-clock seconds for the filtered run/workload; required for WPT.
    #[arg(long = "wall-seconds")]
    wall_seconds: Option<f64>,
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
        Command::Board => board_cmd::board(&project, cli.format),
        Command::Capability { command } => capability(&project, command, cli.format),
        Command::Route {
            command,
            task_id,
            model,
        } => match command {
            Some(RouteCommand::Stats) => route_cmd::stats(&project, cli.format),
            None => {
                let task_id = task_id.ok_or_else(|| {
                    anyhow!("route requires a task id, or use `loom route stats`")
                })?;
                route_cmd::route(&project, &task_id, model.as_deref(), cli.format)
            }
        },
        Command::Task { command } => {
            task_cmd::task(&project, command, cli.run.as_deref(), cli.format)
        }
        Command::Event { command } => {
            event_cmd::events(&project, command, cli.run.as_deref(), cli.format)
        }
        Command::Context { command } => {
            context_cmd::context(&project, command, cli.run.as_deref(), cli.format)
        }
        Command::Prompt { command } => prompt_cmd::prompt(&project, command, cli.format),
        Command::Adapter { command } => {
            adapter_cmd::adapter(&project, command, cli.run.as_deref(), cli.format)
        }
        Command::Dispatch {
            task_id,
            adapter,
            model,
            timeout_s,
            attempt,
            in_place,
        } => dispatch_cmd::dispatch(
            &project,
            &dispatch_cmd::DispatchArgs {
                task_id,
                adapter,
                model,
                timeout_s,
                attempt,
                in_place,
            },
            cli.run.as_deref(),
            cli.format,
        ),
        Command::Verify { task_id, attempt } => {
            verify_cmd::verify(&project, &task_id, attempt, cli.run.as_deref(), cli.format)
        }
        Command::Merge { task_id, attempt } => {
            merge_cmd::merge(&project, &task_id, attempt, cli.format)
        }
        Command::Resume { dispatch, adapter } => {
            resume_cmd::resume(&project, dispatch, &adapter, cli.run.as_deref(), cli.format)
        }
        Command::Usage { command } => match command {
            UsageCommand::Record(args) => {
                record_usage(&project, args, cli.run.as_deref(), cli.format)
            }
            UsageCommand::Ingest(args) => {
                usage_cmd::ingest(&project, args, cli.run.as_deref(), cli.format)
            }
        },
        Command::Ledger(args) => ledger(&project, args, cli.run.as_deref(), cli.format),
        Command::Churn { command } => match command {
            ChurnCommand::Stats { by } => churn_cmd::stats(&project, by.as_deref(), cli.format),
        },
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

fn capability(project: &Project, command: CapabilityCommand, format: Format) -> Result<i32> {
    let config = project.load_config()?;
    match command {
        CapabilityCommand::List => {
            match format {
                Format::Json => println!("{}", serde_json::to_string_pretty(&config.models)?),
                Format::Pretty => {
                    if config.models.is_empty() {
                        println!("no models configured");
                        println!(
                            "add [[models]] blocks to {}",
                            project.config_path().display()
                        );
                        return Ok(0);
                    }
                    for model in &config.models {
                        print_model(model);
                    }
                }
            }
            Ok(0)
        }
        CapabilityCommand::Show { alias } => {
            let model = config
                .model(&alias)
                .ok_or_else(|| unknown_alias(&config, &alias))?;
            match format {
                Format::Json => println!("{}", serde_json::to_string_pretty(model)?),
                Format::Pretty => print_model(model),
            }
            Ok(0)
        }
        CapabilityCommand::Check { alias } => check_capabilities(&config, alias.as_deref(), format),
    }
}

fn check_capabilities(config: &Config, alias: Option<&str>, format: Format) -> Result<i32> {
    let models: Vec<&ModelCapability> = match alias {
        Some(alias) => vec![config
            .model(alias)
            .ok_or_else(|| unknown_alias(config, alias))?],
        None => config.models.iter().collect(),
    };

    if models.is_empty() {
        println!("no models configured");
        return Ok(1);
    }

    let mut healthy = true;
    let mut results = Vec::new();
    for model in models {
        let mut errors = Vec::new();
        let mut warnings = Vec::new();
        if let Err(message) = model.validate() {
            errors.push(message);
        }
        if !model.tool_loop_safe() {
            errors.push(
                "forced thinking requires reasoning_passthrough=required for multi-turn tools"
                    .to_string(),
            );
        }
        if model.input_usd_per_mtok.is_none() || model.output_usd_per_mtok.is_none() {
            warnings.push("missing input/output price; cost will stay unknown".to_string());
        }
        if model.cached_input_usd_per_mtok.is_none() {
            warnings.push("missing cached input price; cache cost cannot be estimated".to_string());
        }

        let ok = errors.is_empty();
        healthy &= ok;

        if matches!(format, Format::Pretty) {
            let status = if ok { "ok" } else { "FAIL" };
            println!("[{status}] {:<16} {}", model.alias, model.display_model());
            for error in &errors {
                println!("  error: {error}");
            }
            for warning in &warnings {
                println!("  warn:  {warning}");
            }
        }

        results.push(serde_json::json!({
            "alias": model.alias,
            "model_id": model.model_id,
            "ok": ok,
            "tool_loop_safe": model.tool_loop_safe(),
            "pricing_complete": model.input_usd_per_mtok.is_some()
                && model.output_usd_per_mtok.is_some(),
            "errors": errors,
            "warnings": warnings,
        }));
    }

    if matches!(format, Format::Json) {
        let payload = serde_json::json!({"ok": healthy, "models": results});
        println!("{}", serde_json::to_string_pretty(&payload)?);
    }

    Ok(if healthy { 0 } else { 1 })
}

fn record_usage(
    project: &Project,
    args: UsageRecordArgs,
    run_id: Option<&str>,
    format: Format,
) -> Result<i32> {
    let run = Run::open(project.clone(), run_id)?;
    let config = project.load_config()?;
    let mut record = UsageRecord::new(run.id);
    record.ts_ms = now_ms();
    record.task_id = args.task;
    record.attempt = args.attempt;
    record.tier = args.tier.as_deref().map(parse_tier).transpose()?;
    record.model_alias = args.model.clone();
    record.model_id = args.model_id;
    record.provider = args.provider;
    record.wire = args.wire.as_deref().map(parse_wire).transpose()?;
    record.input_tokens = args.input_tokens;
    record.cached_input_tokens = args.cached_input_tokens;
    record.output_tokens = args.output_tokens;
    record.reasoning_tokens = args.reasoning_tokens;
    record.cost_usd = args.cost_usd;
    record.cost_source = match args.cost_source {
        Some(value) => value
            .parse::<CostSource>()
            .map_err(|message| anyhow!(message))?,
        None if record.cost_usd.is_some() => CostSource::Manual,
        None => CostSource::Unknown,
    };
    record.source = args
        .source
        .parse::<UsageSource>()
        .map_err(|message| anyhow!(message))?;
    record.latency_ms = args.latency_ms;
    record.status = args.status;

    let capability = args.model.as_deref().and_then(|alias| config.model(alias));
    if let Some(alias) = record.model_alias.as_deref() {
        if capability.is_none() {
            eprintln!(
                "warning: model alias {alias} is not configured; capability bits and cost stay unknown"
            );
        }
    }
    if let Some(model) = capability {
        if record.tier.is_none() {
            record.tier = Some(model.tier);
        }
        if record.wire.is_none() {
            record.wire = Some(model.wire);
        }
        if record.model_id.is_none() {
            record.model_id = model.model_id.clone();
        }
        if record.provider.is_none() {
            record.provider = model.provider.clone();
        }
    }
    record.fill_estimated_cost(capability);

    if !record.is_attributed() {
        eprintln!("warning: request is unattributed (missing task_id)");
    }
    append_usage(project, &record)?;

    match format {
        Format::Json => println!("{}", serde_json::to_string_pretty(&record)?),
        Format::Pretty => {
            println!("recorded usage {}", record.run_id);
            if let Some(task) = &record.task_id {
                println!("task:    {task}");
            }
            println!(
                "model:   {} ({}) tier={} wire={}",
                record.model_alias.as_deref().unwrap_or("-"),
                record.model_id.as_deref().unwrap_or("-"),
                record.tier.map(Tier::as_str).unwrap_or("-"),
                record.wire.map(Wire::as_str).unwrap_or("-"),
            );
            println!(
                "tokens:  input={} cached={} output={} reasoning={}",
                opt_u64(record.input_tokens),
                opt_u64(record.cached_input_tokens),
                opt_u64(record.output_tokens),
                opt_u64(record.reasoning_tokens),
            );
            println!(
                "cost:    {} ({})",
                record
                    .cost_usd
                    .map(|cost| format!("${cost:.6}"))
                    .unwrap_or_else(|| "unknown".to_string()),
                record.cost_source.as_str(),
            );
            println!(
                "source:  {} status={} latency={}ms",
                record.source.as_str(),
                opt_u16(record.status),
                opt_u64(record.latency_ms),
            );
            println!("ledger:  {}", ledger_path(project).display());
        }
    }
    Ok(0)
}

fn ledger(
    project: &Project,
    args: LedgerArgs,
    run_id: Option<&str>,
    format: Format,
) -> Result<i32> {
    let filter = UsageFilter {
        run_id: run_id.map(ToOwned::to_owned),
        task_id: args.task,
        tier: args.tier.as_deref().map(parse_tier).transpose()?,
        model_alias: args.model,
    };
    let aggregate = aggregate_usage(project, &filter)?;
    let cpvt = aggregate.cpvt(args.passed_tasks);
    let tpvt = aggregate.tpvt(args.passed_tasks);
    let wpt = args
        .wall_seconds
        .and_then(|wall| UsageAggregate::wpt(wall, args.passed_tasks));

    match format {
        Format::Json => {
            let payload = serde_json::json!({
                "v": 1,
                "filter": {
                    "run_id": filter.run_id,
                    "task_id": filter.task_id,
                    "tier": filter.tier.map(Tier::as_str),
                    "model": filter.model_alias,
                },
                "requests": aggregate.requests,
                "failed_requests": aggregate.failed_requests,
                "unattributed_requests": aggregate.unattributed_requests,
                "unknown_cost_requests": aggregate.unknown_cost_requests,
                "unknown_token_requests": aggregate.unknown_token_requests,
                "input_tokens": aggregate.input_tokens,
                "cached_input_tokens": aggregate.cached_input_tokens,
                "output_tokens": aggregate.output_tokens,
                "reasoning_tokens": aggregate.reasoning_tokens,
                "billed_tokens": aggregate.billed_tokens(),
                "cache_hit_ratio": aggregate.cache_hit_ratio(),
                "cost_usd": aggregate.cost_usd,
                "latency_ms": aggregate.latency_ms,
                "passed_tasks": args.passed_tasks,
                "cpvt_usd": cpvt,
                "tpvt_tokens": tpvt,
                "wpt_s": wpt,
            });
            println!("{}", serde_json::to_string_pretty(&payload)?);
        }
        Format::Pretty => {
            println!("scope:    {}", format_scope(&filter));
            println!(
                "requests: {} (failed {}, unattributed {}, unknown cost {}, unknown tokens {})",
                aggregate.requests,
                aggregate.failed_requests,
                aggregate.unattributed_requests,
                aggregate.unknown_cost_requests,
                aggregate.unknown_token_requests,
            );
            println!(
                "tokens:   input={} cached={} ({:.1}%) output={} reasoning={} billed={}",
                aggregate.input_tokens,
                aggregate.cached_input_tokens,
                aggregate.cache_hit_ratio() * 100.0,
                aggregate.output_tokens,
                aggregate.reasoning_tokens,
                aggregate.billed_tokens(),
            );
            println!("cost:     ${:.6}", aggregate.cost_usd);
            println!(
                "cpvt:     {}",
                cpvt.map(|value| format!("${value:.6} / passed task"))
                    .unwrap_or_else(|| "n/a".to_string())
            );
            println!(
                "tpvt:     {}",
                tpvt.map(|value| format!("{value:.0} tokens / passed task"))
                    .unwrap_or_else(|| "n/a".to_string())
            );
            println!(
                "wpt:      {}",
                wpt.map(|value| format!("{value:.1} s / passed task"))
                    .unwrap_or_else(|| "n/a (needs --passed-tasks and --wall-seconds)".to_string())
            );
        }
    }
    Ok(0)
}

fn unknown_alias(config: &Config, alias: &str) -> anyhow::Error {
    let configured = if config.models.is_empty() {
        "none".to_string()
    } else {
        config
            .models
            .iter()
            .map(|model| model.alias.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    };
    anyhow!("unknown model alias: {alias} (configured: {configured})")
}

fn parse_tier(value: &str) -> Result<Tier> {
    value.parse().map_err(|message: String| anyhow!(message))
}

fn parse_wire(value: &str) -> Result<Wire> {
    value.parse().map_err(|message: String| anyhow!(message))
}

fn print_model(model: &ModelCapability) {
    println!(
        "{:<16} tier={} wire={} thinking={} reasoning={}",
        model.alias,
        model.tier.as_str(),
        model.wire.as_str(),
        model.thinking_mode.as_str(),
        model.reasoning_passthrough.as_str(),
    );
    println!(
        "  model={} provider={}",
        model.display_model(),
        model.provider.as_deref().unwrap_or("-"),
    );
    println!(
        "  context={} max_out={} tool_calls={} streaming={}",
        opt_usize(model.context_window),
        opt_usize(model.max_output),
        opt_bool(model.tool_calls),
        opt_bool(model.streaming),
    );
    println!(
        "  usd/mtok in={} cached={} cache_write={} out={}",
        fmt_price(model.input_usd_per_mtok),
        fmt_price(model.cached_input_usd_per_mtok),
        fmt_price(model.cache_write_usd_per_mtok),
        fmt_price(model.output_usd_per_mtok),
    );
    if let Some(version) = &model.pricing_version {
        println!("  pricing_version={version}");
    }
    if !model.tool_loop_safe() {
        println!("  WARN: forced thinking without reasoning passthrough");
    }
}

fn format_scope(filter: &UsageFilter) -> String {
    let mut parts = Vec::new();
    if let Some(run) = &filter.run_id {
        parts.push(format!("run={run}"));
    }
    if let Some(task) = &filter.task_id {
        parts.push(format!("task={task}"));
    }
    if let Some(tier) = filter.tier {
        parts.push(format!("tier={}", tier.as_str()));
    }
    if let Some(model) = &filter.model_alias {
        parts.push(format!("model={model}"));
    }
    if parts.is_empty() {
        "all".to_string()
    } else {
        parts.join(" ")
    }
}

fn opt_u64(value: Option<u64>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "-".to_string())
}

fn opt_u16(value: Option<u16>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "-".to_string())
}

fn opt_usize(value: Option<usize>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "-".to_string())
}

fn opt_bool(value: Option<bool>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "-".to_string())
}

fn fmt_price(value: Option<f64>) -> String {
    value
        .map(|value| format!("{value}"))
        .unwrap_or_else(|| "?".to_string())
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
    append_tool_event(run, &event)?;

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
                    "lang={} call={}B models={}",
                    config.lang,
                    config.tools.limits.call_bytes,
                    config.models.len()
                ),
            ),
            Err(error) => check("config", false, &error.to_string()),
        }

        let probe = loom.join(".doctor");
        match fs::write(&probe, b"ok").and_then(|()| fs::remove_file(&probe)) {
            Ok(()) => check("writable", true, ".loom/"),
            Err(error) => check("writable", false, &error.to_string()),
        }

        // `doc/03` §13: leftover locks are cleaned here; a lock whose lease
        // expired while the owner is still alive needs a human.
        match loom_store::reclaim_stale_locks(project) {
            Ok(reclaimed) => {
                let held = loom_store::read_locks(project).unwrap_or_default();
                let stuck = held.iter().filter(|lock| lock.is_stuck(now_ms())).count();
                check(
                    "locks",
                    stuck == 0,
                    &format!(
                        "{} held, {} reclaimed, {} stuck",
                        held.len(),
                        reclaimed.len(),
                        stuck
                    ),
                );
            }
            Err(error) => check("locks", false, &error.to_string()),
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
    println!("models:  {}", config.models.len());
    match read_usage(project) {
        Ok(records) => println!("ledger:  {} requests", records.len()),
        Err(error) => println!("ledger:  unreadable ({error:#})"),
    }
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
