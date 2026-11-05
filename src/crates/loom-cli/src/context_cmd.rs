use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::Result;
use loom_context::{
    compile, get, CompileRequest, ContextKind, ContextManifest, FailurePack, PromptRole,
};
use loom_core::{TaskRole, TaskStatus};
use loom_store::{read_task, read_tasks, task_statuses, Run};

use super::run_util::find_run_for_task;
use super::{ContextCommand, Format};

pub fn context(
    project: &loom_store::Project,
    command: ContextCommand,
    run_id: Option<&str>,
    format: Format,
) -> Result<i32> {
    match command {
        ContextCommand::Compile {
            task_id,
            kind,
            budget_tokens,
            out,
        } => {
            let spec = read_task(project, &task_id)?;
            let statuses: BTreeMap<String, TaskStatus> =
                task_statuses(project)?.into_iter().collect();
            let all_tasks = read_tasks(project)?;
            let dependencies: Vec<_> = all_tasks
                .iter()
                .filter(|task| spec.depends_on.contains(&task.task_id))
                .cloned()
                .collect();

            let kind = match kind.as_deref() {
                Some("main") => ContextKind::Main,
                Some("worker") => ContextKind::Worker,
                Some("judgment") => ContextKind::Judgment,
                Some(other) => anyhow::bail!("unknown context kind: {other}"),
                None => default_kind(&spec),
            };
            let mut budget = kind.budget();
            if let Some(budget_tokens) = budget_tokens {
                if budget_tokens == 0 {
                    anyhow::bail!("--budget-tokens must be > 0");
                }
                budget.total_tokens = budget_tokens;
            }

            let role = default_prompt_role(&spec);
            let prompt = get(role)?;
            let resolved_run = find_run_for_task(project, &task_id, run_id)?;
            let failure_pack = load_failure_pack(project, &task_id, &resolved_run)?;
            let request = CompileRequest {
                project,
                task: &spec,
                dependencies: &dependencies,
                statuses,
                failure_pack: failure_pack.as_deref(),
                kind,
                budget: Some(budget),
                prompt,
            };
            let compiled = compile(&request)?;

            let out_dir: PathBuf = match out {
                Some(out) => out,
                None => Run::open(project.clone(), Some(&resolved_run))?.context_dir(),
            };
            compiled.write(&out_dir)?;

            match format {
                Format::Json => {
                    println!("{}", serde_json::to_string_pretty(&compiled.manifest)?);
                }
                Format::Pretty => {
                    print_manifest(&compiled.manifest, &out_dir);
                }
            }
            Ok(0)
        }
    }
}

pub(crate) fn default_kind(spec: &loom_core::TaskSpec) -> ContextKind {
    if spec.role() == TaskRole::Judgment {
        ContextKind::Judgment
    } else if spec.tier == loom_core::Tier::Worker {
        ContextKind::Worker
    } else {
        ContextKind::Main
    }
}

pub(crate) fn default_prompt_role(spec: &loom_core::TaskSpec) -> PromptRole {
    match spec.role() {
        TaskRole::Judgment => PromptRole::Judgment,
        TaskRole::Review => PromptRole::Review,
        TaskRole::Implement | TaskRole::SpecTests | TaskRole::Survey | TaskRole::Docs => {
            if spec.tier == loom_core::Tier::Worker {
                PromptRole::Worker
            } else {
                PromptRole::Orchestrator
            }
        }
    }
}

fn load_failure_pack(
    project: &loom_store::Project,
    task_id: &str,
    run_id: &str,
) -> Result<Option<Box<FailurePack>>> {
    let run_dir = project.loom_dir().join("runs").join(run_id);
    let receipt_path = run_dir.join("outbox").join("receipt.json");
    if !receipt_path.exists() {
        return Ok(None);
    }
    let text = match std::fs::read_to_string(&receipt_path) {
        Ok(text) => text,
        Err(_) => return Ok(None),
    };
    let receipt: loom_core::Receipt = match serde_json::from_str(&text) {
        Ok(receipt) => receipt,
        Err(_) => return Ok(None),
    };
    if receipt.task_id != task_id || receipt.derived_status() == loom_core::ReceiptStatus::Done {
        return Ok(None);
    }

    let failures = receipt
        .verify
        .iter()
        .filter(|item| item.result == loom_core::VerifyResult::Fail)
        .map(|item| loom_context::FailedAcceptance {
            id: item.id.clone(),
            cmd: item.cmd.clone(),
            exit: item.exit,
            log_path: item.log_path.clone(),
        })
        .collect();
    let log_excerpt =
        std::fs::read_to_string(run_dir.join("logs").join("verify.log")).unwrap_or_default();

    Ok(Some(Box::new(FailurePack {
        attempt: receipt.attempt,
        failures,
        log_excerpt,
        diff_ref: None,
        evidence_paths: vec![receipt.report_path.clone()],
    })))
}

fn print_manifest(manifest: &ContextManifest, out_dir: &std::path::Path) {
    println!(
        "{} {} kind={} prompt={}@{}",
        manifest.task_id,
        manifest.goal_id,
        manifest.kind,
        manifest.prompt_role,
        manifest.prompt_version
    );
    println!(
        "tokens:   {} / {} (stable prefix {})",
        manifest.total_tokens, manifest.total_budget_tokens, manifest.stable_prefix_tokens
    );
    println!("degraded: {}", if manifest.degraded { "yes" } else { "no" });
    for section in &manifest.sections {
        println!(
            "  {:<14} {:>6} tok (cap {:>6}{})",
            section.name,
            section.tokens,
            section.cap_tokens,
            if section.truncated { ", truncated" } else { "" }
        );
    }
    if !manifest.notes.is_empty() {
        println!("notes:");
        for note in &manifest.notes {
            println!("  - {note}");
        }
    }
    println!("out: {}", out_dir.display());
    println!("manifest: {}", out_dir.join("manifest.json").display());
}
