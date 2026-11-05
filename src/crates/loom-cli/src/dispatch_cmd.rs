use std::path::PathBuf;

use anyhow::{Context as _, Result};
use loom_adapter::{AdapterKind, AdapterRequest};
use loom_context::{compile, CompileRequest};
use loom_core::{TaskSpec, TaskStatus, Tier};
use loom_store::{
    mark_ready_tasks, missing_dependencies, read_task, read_tasks, sync_index, task_status,
    task_statuses, Run,
};
use serde_json::json;

use super::context_cmd::{default_kind, default_prompt_role};
use super::run_util::{git_head, new_run_id, prepare_workspace, write_run_meta};
use super::Format;

#[derive(Debug, Clone)]
pub(crate) struct DispatchArgs {
    pub task_id: String,
    pub adapter: String,
    pub model: Option<String>,
    pub timeout_s: Option<u64>,
    pub attempt: u32,
    pub in_place: bool,
}

pub(crate) struct Prepared {
    pub spec: TaskSpec,
    pub run: Run,
    pub workspace: PathBuf,
    pub request: AdapterRequest,
    pub model_alias: String,
    pub model_id: String,
}

/// Resolve the model, prepare the workspace, compile the packet, and write run
/// metadata. Shared by `dispatch` and adapter live canaries.
#[allow(clippy::too_many_arguments)]
pub(crate) fn prepare(
    project: &loom_store::Project,
    task_id: &str,
    adapter_name: &str,
    model: Option<&str>,
    timeout_s: Option<u64>,
    in_place: bool,
    run_id: Option<&str>,
    attempt: u32,
) -> Result<Prepared> {
    let spec = read_task(project, task_id)?;
    let status = task_status(project, task_id)?;
    if !matches!(status, TaskStatus::Pending | TaskStatus::Ready) {
        anyhow::bail!(
            "task {task_id} is {}; dispatch only accepts pending/ready tasks",
            status.as_str()
        );
    }
    let statuses: std::collections::BTreeMap<String, TaskStatus> =
        task_statuses(project)?.into_iter().collect();
    let missing = missing_dependencies(&spec, &statuses);
    if !missing.is_empty() {
        anyhow::bail!(
            "task {task_id} still waits for dependencies: {}",
            missing.join(", ")
        );
    }

    let config = project.load_config()?;
    let (model_alias, model_id) = resolve_model(&config, &spec, model)?;

    let run_id = match run_id {
        Some(run_id) => run_id.to_string(),
        None => new_run_id(project, task_id),
    };
    let workspace = prepare_workspace(project, &spec, in_place)?;
    let run = Run::open(project.clone(), Some(&run_id))?;

    let all_tasks = read_tasks(project)?;
    let dependencies: Vec<_> = all_tasks
        .iter()
        .filter(|task| spec.depends_on.contains(&task.task_id))
        .cloned()
        .collect();
    let kind = default_kind(&spec);
    let prompt = loom_context::get(default_prompt_role(&spec))?;
    let request_context = CompileRequest {
        project,
        task: &spec,
        dependencies: &dependencies,
        statuses,
        failure_pack: None,
        kind,
        budget: None,
        prompt,
    };
    let compiled = compile(&request_context)?;
    compiled.write(&run.context_dir())?;

    // Snapshot the task exactly as dispatched.
    let text = serde_json::to_string_pretty(&spec)?;
    std::fs::write(run.task_snapshot_path(), format!("{text}\n"))
        .with_context(|| "write run task snapshot")?;

    let adapter: AdapterKind = adapter_name
        .parse()
        .map_err(|message: String| anyhow::anyhow!(message))?;
    let timeout = timeout_s.unwrap_or(spec.budget.max_wall_s);
    let request = AdapterRequest {
        adapter,
        run_id: run.id.clone(),
        task_id: spec.task_id.clone(),
        attempt,
        packet_dir: run.context_dir(),
        worktree: workspace.clone(),
        output_dir: run.outbox_dir(),
        logs_dir: run.logs_dir(),
        model: model_alias.clone(),
        branch: spec.workspace.branch.clone(),
        timeout_s: timeout,
        max_tokens: Some(spec.budget.max_output_tokens),
    };

    let base_sha = git_head(&workspace).unwrap_or_default();
    write_run_meta(
        project,
        &run.id,
        &spec.task_id,
        adapter.as_str(),
        &model_alias,
        &model_id,
        &workspace,
        &base_sha,
        timeout,
        &run.context_dir(),
    )?;

    Ok(Prepared {
        spec,
        run,
        workspace,
        request,
        model_alias,
        model_id,
    })
}

fn resolve_model(
    config: &loom_core::Config,
    spec: &TaskSpec,
    model: Option<&str>,
) -> Result<(String, String)> {
    if let Some(alias) = model {
        let capability = config
            .model(alias)
            .ok_or_else(|| super::unknown_alias(config, alias))?;
        return Ok((
            capability.alias.clone(),
            capability.model_id.clone().unwrap_or_default(),
        ));
    }

    if let Some(capability) = config
        .models
        .iter()
        .find(|model| model.tier == spec.tier)
        .or_else(|| config.models.first())
    {
        return Ok((
            capability.alias.clone(),
            capability.model_id.clone().unwrap_or_default(),
        ));
    }

    // No model configured: keep the tier as the alias so events still record it.
    let alias = match spec.tier {
        Tier::Frontier => "frontier",
        Tier::Worker => "worker",
    };
    Ok((alias.to_string(), String::new()))
}

pub(crate) fn dispatch(
    project: &loom_store::Project,
    args: &DispatchArgs,
    run_id: Option<&str>,
    format: Format,
) -> Result<i32> {
    let prepared = prepare(
        project,
        &args.task_id,
        &args.adapter,
        args.model.as_deref(),
        args.timeout_s,
        args.in_place,
        run_id,
        args.attempt,
    )?;
    let Prepared {
        spec,
        run,
        workspace,
        request,
        model_alias,
        model_id,
    } = &prepared;

    if run_id.is_none() {
        mark_ready_tasks(project, &run.id)?;
    }

    let statuses = task_statuses(project)?;
    let status = statuses
        .get(&spec.task_id)
        .copied()
        .unwrap_or(TaskStatus::Pending);
    if status == TaskStatus::Pending {
        // Dependencies are verified but the ready event was not written yet.
        loom_store::append_event(
            project,
            &loom_core::Event::new(
                &run.id,
                loom_store::now_ms(),
                loom_core::EventType::TaskReady,
            )
            .task(&spec.task_id)
            .data([("deps_done".to_string(), json!(spec.depends_on))]),
        )?;
    }

    loom_store::append_event(
        project,
        &loom_core::Event::new(
            &run.id,
            loom_store::now_ms(),
            loom_core::EventType::TaskDispatched,
        )
        .task(&spec.task_id)
        .attempt(args.attempt)
        .data([
            ("tier".to_string(), json!(spec.tier.as_str())),
            ("model".to_string(), json!(model_alias)),
            ("worktree".to_string(), json!(workspace.to_string_lossy())),
            (
                "budget".to_string(),
                json!({
                    "max_input_tokens": spec.budget.max_input_tokens,
                    "max_output_tokens": spec.budget.max_output_tokens,
                    "max_steps": spec.budget.max_steps,
                    "max_wall_s": spec.budget.max_wall_s,
                    "max_cost_usd": spec.budget.max_cost_usd,
                }),
            ),
            (
                "packet".to_string(),
                json!(run.packet_path().to_string_lossy()),
            ),
        ]),
    )?;
    loom_store::append_event(
        project,
        &loom_core::Event::new(
            &run.id,
            loom_store::now_ms(),
            loom_core::EventType::TaskStarted,
        )
        .task(&spec.task_id)
        .attempt(args.attempt)
        .data([
            ("adapter".to_string(), json!(request.adapter.as_str())),
            ("model".to_string(), json!(model_alias)),
        ]),
    )?;

    let adapter_run = loom_adapter::run(request)?;

    loom_store::append_event(
        project,
        &loom_core::Event::new(
            &run.id,
            loom_store::now_ms(),
            loom_core::EventType::TaskReceipt,
        )
        .task(&spec.task_id)
        .attempt(args.attempt)
        .data([
            (
                "status".to_string(),
                json!(adapter_run.receipt.status.as_str()),
            ),
            ("branch".to_string(), json!(adapter_run.receipt.branch)),
            ("head_sha".to_string(), json!(adapter_run.receipt.head_sha)),
            ("exit".to_string(), json!(adapter_run.exit_code)),
            ("timed_out".to_string(), json!(adapter_run.timed_out)),
        ]),
    )?;
    sync_index(project)?;

    match format {
        Format::Json => {
            let payload = json!({
                "run_id": run.id,
                "task_id": spec.task_id,
                "adapter": request.adapter.as_str(),
                "model": model_alias,
                "model_id": model_id,
                "exit_code": adapter_run.exit_code,
                "timed_out": adapter_run.timed_out,
                "status": adapter_run.receipt.status.as_str(),
                "receipt": run.receipt_path().to_string_lossy(),
                "usage_rows": adapter_run.usage.len(),
                "tokens": {
                    "input": adapter_run.receipt.tokens.input,
                    "cached_input": adapter_run.receipt.tokens.cached_input,
                    "output": adapter_run.receipt.tokens.output,
                    "cost_usd": adapter_run.receipt.tokens.cost_usd,
                },
            });
            println!("{}", serde_json::to_string_pretty(&payload)?);
        }
        Format::Pretty => {
            println!(
                "dispatched {} via {} in {}",
                spec.task_id,
                request.adapter.as_str(),
                workspace.display()
            );
            println!("run:      {}", run.id);
            println!(
                "adapter:  exit={} timed_out={}",
                adapter_run.exit_code, adapter_run.timed_out
            );
            println!(
                "receipt:  {} ({})",
                adapter_run.receipt.status.as_str(),
                run.receipt_path().display()
            );
            println!(
                "usage:    {} rows, input={} cached={} output={} cost={}",
                adapter_run.usage.len(),
                adapter_run.receipt.tokens.input,
                adapter_run.receipt.tokens.cached_input,
                adapter_run.receipt.tokens.output,
                adapter_run
                    .receipt
                    .tokens
                    .cost_usd
                    .map(|cost| format!("${cost:.6}"))
                    .unwrap_or_else(|| "unknown".to_string()),
            );
        }
    }

    Ok(adapter_run.exit_code)
}
