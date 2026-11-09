use std::path::PathBuf;

use anyhow::{Context as _, Result};
use loom_adapter::{AdapterKind, AdapterRequest};
use loom_context::{compile, CompileRequest};
use loom_core::{TaskSpec, TaskStatus};
use loom_store::{
    mark_ready_tasks, missing_dependencies, read_task, read_tasks, sync_index, task_status,
    task_statuses, Run,
};
use serde_json::json;

use super::context_cmd::{default_kind, default_prompt_role, load_failure_pack};
use super::route_cmd::plan_with_config;
use super::run_util::{
    append_blocked_event, find_run_for_task, git_head, new_run_id, prepare_workspace,
    write_run_meta,
};
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
    pub route: loom_core::RouteDecision,
}

/// Gate a dispatch: pending/ready tasks, plus failed/escalated ones being
/// retried. Failover attempts skip this gate because the task is already
/// mid-dispatch.
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
    let status = task_status(project, task_id)?;
    if !matches!(
        status,
        TaskStatus::Pending
            | TaskStatus::Ready
            | TaskStatus::Blocked
            | TaskStatus::NeedsInput
            | TaskStatus::Failed
            | TaskStatus::Escalated
    ) {
        anyhow::bail!(
            "task {task_id} is {}; dispatch only accepts pending/ready/blocked/failed/escalated tasks",
            status.as_str()
        );
    }
    prepare_attempt(
        project,
        task_id,
        adapter_name,
        model,
        timeout_s,
        in_place,
        run_id,
        attempt,
    )
}

/// Resolve the model, prepare the workspace, compile the packet, and write run
/// metadata. One call = one attempt = one run directory, so a failover gets
/// its own receipt and usage rows instead of overwriting the failed attempt.
#[allow(clippy::too_many_arguments)]
fn prepare_attempt(
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
    let route = plan_with_config(project, &config, &spec, model)?;
    if !route.is_dispatchable() {
        anyhow::bail!("route for {task_id} rejected: {}", route.issues.join("; "));
    }
    let model_alias = route.alias.clone();
    let model_id = route.model_id.clone();
    let model_capability = config.model(&model_alias).cloned();
    // Fail fast when the model names its own key variable: better a config
    // error before any run state exists than an adapter exit 4 later.
    if let Some(capability) = &model_capability {
        if let Some(name) = &capability.key_env {
            let missing = std::env::var(name)
                .map(|value| value.trim().is_empty())
                .unwrap_or(true);
            if missing {
                anyhow::bail!(
                    "model {} needs the {name} environment variable; set it in .env.local (see .env.example)",
                    capability.alias
                );
            }
        }
    }
    let previous_run = find_run_for_task(project, task_id, None)?;
    let failure_pack = load_failure_pack(project, task_id, &previous_run)?;

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
        failure_pack: failure_pack.as_deref(),
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
        model_id: model_id.clone(),
        model_capability,
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
        route,
    })
}

pub(crate) fn dispatch(
    project: &loom_store::Project,
    args: &DispatchArgs,
    run_id: Option<&str>,
    format: Format,
) -> Result<i32> {
    let mut attempt = prepare(
        project,
        &args.task_id,
        &args.adapter,
        args.model.as_deref(),
        args.timeout_s,
        args.in_place,
        run_id,
        args.attempt,
    )?;

    // M4: hold the task's write set before any dispatch event. A conflicting
    // lock or a full parallel budget records `task.blocked` and stops, so a
    // second writer never starts.
    let config = project.load_config()?;
    let lease_s = attempt.spec.budget.max_wall_s.saturating_add(600);
    let locks = match loom_store::LockGuard::acquire(
        project,
        &args.task_id,
        &attempt.run.id,
        &attempt.spec.workspace.allowed_paths,
        lease_s,
    ) {
        Ok(guard) => guard,
        Err(loom_store::LockError::Conflict(conflict)) => {
            append_blocked_event(
                project,
                &attempt.run.id,
                &args.task_id,
                args.attempt,
                vec![
                    ("code", json!("lock_owner")),
                    ("lock_owner", json!(conflict.held.owner_task)),
                    ("lock_run", json!(conflict.held.owner_run)),
                    ("lock_path", json!(conflict.held.path)),
                    ("detail", json!(conflict.to_string())),
                ],
            )?;
            anyhow::bail!("task {} blocked by path lock: {conflict}", args.task_id);
        }
        Err(loom_store::LockError::Io(error)) => return Err(error),
    };

    let owners = loom_store::live_lock_owners(project)?;
    if owners.len() > config.parallel.max_running {
        // `locks` drops on return and releases the write set again.
        append_blocked_event(
            project,
            &attempt.run.id,
            &args.task_id,
            args.attempt,
            vec![
                ("code", json!("parallel_cap")),
                ("running", json!(owners)),
                ("max_running", json!(config.parallel.max_running)),
            ],
        )?;
        anyhow::bail!(
            "task {} blocked: {} other task(s) running, over parallel.max_running = {}",
            args.task_id,
            owners.len().saturating_sub(1),
            config.parallel.max_running
        );
    }

    // Parallel budget: worst-case cost of everything in flight, including
    // this task (`doc/05` §6). 0 disables the cap.
    let inflight_limit = config.parallel.max_inflight_cost_usd;
    if inflight_limit > 0.0 {
        let mut inflight = attempt.spec.budget.max_cost_usd;
        for owner in &owners {
            if owner == &args.task_id {
                continue;
            }
            if let Ok(other) = loom_store::read_task(project, owner) {
                inflight += other.budget.max_cost_usd;
            }
        }
        if inflight > inflight_limit {
            append_blocked_event(
                project,
                &attempt.run.id,
                &args.task_id,
                args.attempt,
                vec![
                    ("code", json!("inflight_budget")),
                    ("inflight_cost_usd", json!(inflight)),
                    ("max_inflight_cost_usd", json!(inflight_limit)),
                ],
            )?;
            anyhow::bail!(
                "task {} blocked: inflight worst-case ${inflight:.4} exceeds parallel.max_inflight_cost_usd ${inflight_limit:.4}",
                args.task_id
            );
        }
    }
    let _locks = locks;

    // Routing is decided once, from the first plan. Failover stays inside that
    // tier, keeps the same attempt number, and never re-runs escalation.
    let route = attempt.route.clone();
    let chain: Vec<String> = if route.fallback_enabled {
        route.fallbacks.clone()
    } else {
        Vec::new()
    };
    let mut chain_index = 0usize;
    let mut switches: Vec<serde_json::Value> = Vec::new();

    if run_id.is_none() {
        mark_ready_tasks(project, &attempt.run.id)?;
    }

    {
        let statuses = task_statuses(project)?;
        let status = statuses
            .get(&attempt.spec.task_id)
            .copied()
            .unwrap_or(TaskStatus::Pending);
        if status == TaskStatus::Pending {
            // Dependencies are verified but the ready event was not written yet.
            loom_store::append_event(
                project,
                &loom_core::Event::new(
                    &attempt.run.id,
                    loom_store::now_ms(),
                    loom_core::EventType::TaskReady,
                )
                .task(&attempt.spec.task_id)
                .data([("deps_done".to_string(), json!(attempt.spec.depends_on))]),
            )?;
        }

        if route.escalated {
            loom_store::append_event(
                project,
                &loom_core::Event::new(
                    &attempt.run.id,
                    loom_store::now_ms(),
                    loom_core::EventType::TaskEscalated,
                )
                .task(&attempt.spec.task_id)
                .attempt(args.attempt)
                .data([
                    ("from_tier".to_string(), json!(attempt.spec.tier.as_str())),
                    ("to_tier".to_string(), json!(route.tier.as_str())),
                    ("model".to_string(), json!(route.alias)),
                    ("reason".to_string(), json!(route.reason.as_str())),
                ]),
            )?;
        }

        loom_store::append_event(
            project,
            &loom_core::Event::new(
                &attempt.run.id,
                loom_store::now_ms(),
                loom_core::EventType::TaskDispatched,
            )
            .task(&attempt.spec.task_id)
            .attempt(args.attempt)
            .data([
                ("tier".to_string(), json!(attempt.spec.tier.as_str())),
                ("model".to_string(), json!(attempt.model_alias)),
                (
                    "route".to_string(),
                    json!({
                        "tier": route.tier.as_str(),
                        "requested_tier": route.requested_tier.as_str(),
                        "reason": route.reason.as_str(),
                        "escalated": route.escalated,
                        "fallbacks": route.fallbacks,
                        "fallback_enabled": route.fallback_enabled,
                    }),
                ),
                (
                    "worktree".to_string(),
                    json!(attempt.workspace.to_string_lossy()),
                ),
                (
                    "budget".to_string(),
                    json!({
                        "max_input_tokens": attempt.spec.budget.max_input_tokens,
                        "max_output_tokens": attempt.spec.budget.max_output_tokens,
                        "max_steps": attempt.spec.budget.max_steps,
                        "max_wall_s": attempt.spec.budget.max_wall_s,
                        "max_cost_usd": attempt.spec.budget.max_cost_usd,
                    }),
                ),
                (
                    "packet".to_string(),
                    json!(attempt.run.packet_path().to_string_lossy()),
                ),
            ]),
        )?;
    }

    let adapter_run = loop {
        loom_store::append_event(
            project,
            &loom_core::Event::new(
                &attempt.run.id,
                loom_store::now_ms(),
                loom_core::EventType::TaskStarted,
            )
            .task(&attempt.spec.task_id)
            .attempt(args.attempt)
            .data([
                (
                    "adapter".to_string(),
                    json!(attempt.request.adapter.as_str()),
                ),
                ("model".to_string(), json!(attempt.model_alias)),
            ]),
        )?;

        let run_result = loom_adapter::run(&attempt.request)?;

        loom_store::append_event(
            project,
            &loom_core::Event::new(
                &attempt.run.id,
                loom_store::now_ms(),
                loom_core::EventType::TaskReceipt,
            )
            .task(&attempt.spec.task_id)
            .attempt(args.attempt)
            .data([
                (
                    "status".to_string(),
                    json!(run_result.receipt.status.as_str()),
                ),
                ("branch".to_string(), json!(run_result.receipt.branch)),
                ("head_sha".to_string(), json!(run_result.receipt.head_sha)),
                ("exit".to_string(), json!(run_result.exit_code)),
                ("timed_out".to_string(), json!(run_result.timed_out)),
            ]),
        )?;
        sync_index(project)?;

        // Only executor failures are worth another model: a task that ran and
        // failed is a verify problem, and a different alias would not fix it.
        if !run_result.is_availability_failure() {
            break run_result;
        }
        let Some(next_alias) = chain.get(chain_index).cloned() else {
            break run_result;
        };
        chain_index += 1;

        let reason = if run_result.timed_out {
            "timeout"
        } else {
            "adapter_error"
        };
        let from_alias = attempt.model_alias.clone();
        let from_run = attempt.run.id.clone();
        let exit = run_result.exit_code;
        let timed_out = run_result.timed_out;

        let next = prepare_attempt(
            project,
            &args.task_id,
            &args.adapter,
            Some(&next_alias),
            args.timeout_s,
            args.in_place,
            None,
            args.attempt,
        )?;
        loom_store::append_event(
            project,
            &loom_core::Event::new(
                &next.run.id,
                loom_store::now_ms(),
                loom_core::EventType::TaskFallback,
            )
            .task(&args.task_id)
            .attempt(args.attempt)
            .parent_run_id(&from_run)
            .data([
                ("from".to_string(), json!(from_alias)),
                ("to".to_string(), json!(next_alias)),
                ("tier".to_string(), json!(route.tier.as_str())),
                ("reason".to_string(), json!(reason)),
                ("exit".to_string(), json!(exit)),
                ("timed_out".to_string(), json!(timed_out)),
                ("switch".to_string(), json!(switches.len() + 1)),
            ]),
        )?;
        switches.push(json!({
            "from": from_alias,
            "to": next_alias,
            "reason": reason,
            "run_id": next.run.id,
            "parent_run_id": from_run,
            "exit": exit,
            "timed_out": timed_out,
        }));
        attempt = next;
    };

    let final_run_id = attempt.run.id.clone();
    let final_alias = attempt.model_alias.clone();
    let final_model_id = attempt.model_id.clone();
    let receipt_path = attempt.run.receipt_path();
    let workspace = attempt.workspace.display().to_string();
    let adapter_name = attempt.request.adapter.as_str();

    match format {
        Format::Json => {
            let payload = json!({
                "run_id": final_run_id,
                "task_id": args.task_id,
                "adapter": adapter_name,
                "model": final_alias,
                "model_id": final_model_id,
                "route": route,
                "fallbacks": switches,
                "exit_code": adapter_run.exit_code,
                "timed_out": adapter_run.timed_out,
                "status": adapter_run.receipt.status.as_str(),
                "receipt": receipt_path.to_string_lossy(),
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
                args.task_id, adapter_name, workspace
            );
            println!("run:      {final_run_id}");
            println!(
                "route:    {} via {} ({}){}",
                route.tier.as_str(),
                route.alias,
                route.reason.as_str(),
                if route.escalated { " [escalated]" } else { "" },
            );
            if !switches.is_empty() {
                for switch in &switches {
                    println!(
                        "fallback: {} -> {} ({}) run={}",
                        switch["from"], switch["to"], switch["reason"], switch["run_id"]
                    );
                }
                println!("model:    {final_alias} (after {} switch)", switches.len());
            }
            println!(
                "adapter:  exit={} timed_out={}",
                adapter_run.exit_code, adapter_run.timed_out
            );
            println!(
                "receipt:  {} ({})",
                adapter_run.receipt.status.as_str(),
                receipt_path.display()
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
