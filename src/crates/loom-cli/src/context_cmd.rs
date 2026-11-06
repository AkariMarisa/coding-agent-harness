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

pub(crate) fn load_failure_pack(
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

#[cfg(test)]
mod tests {
    use std::fs;

    use loom_core::{Receipt, ReceiptStatus, VerifyEvidence, VerifyResult};
    use loom_store::Project;

    use super::load_failure_pack;

    fn write_receipt(root: &std::path::Path, receipt: &Receipt) {
        let outbox = root.join(".loom/runs/R-1/outbox");
        fs::create_dir_all(&outbox).expect("create outbox");
        fs::write(
            outbox.join("receipt.json"),
            serde_json::to_string_pretty(receipt).expect("serialize receipt"),
        )
        .expect("write receipt");
    }

    fn failed_receipt(task_id: &str) -> Receipt {
        let mut receipt = Receipt::new(task_id, 2);
        receipt.status = ReceiptStatus::Failed;
        receipt.report_path = ".loom/runs/R-1/outbox/report.md".to_string();
        receipt.verify = vec![
            VerifyEvidence {
                id: "A1".to_string(),
                cmd: "make test".to_string(),
                exit: 1,
                result: VerifyResult::Fail,
                log_path: ".loom/runs/R-1/logs/verify.log".to_string(),
                ..VerifyEvidence::default()
            },
            VerifyEvidence {
                id: "A2".to_string(),
                cmd: "make lint".to_string(),
                exit: 0,
                result: VerifyResult::Pass,
                log_path: ".loom/runs/R-1/logs/verify.log".to_string(),
                ..VerifyEvidence::default()
            },
        ];
        receipt
    }

    #[test]
    fn loads_failure_pack_from_previous_failed_run() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let project = Project::at(root);
        write_receipt(root, &failed_receipt("T-1"));
        let logs = root.join(".loom/runs/R-1/logs");
        fs::create_dir_all(&logs).expect("create logs");
        fs::write(logs.join("verify.log"), "A1 failed: assertion mismatch\n").expect("write log");

        let pack = load_failure_pack(&project, "T-1", "R-1")
            .expect("load")
            .expect("some pack");
        assert_eq!(pack.attempt, 2);
        assert_eq!(pack.failures.len(), 1);
        assert_eq!(pack.failures[0].id, "A1");
        assert_eq!(pack.failures[0].cmd, "make test");
        assert_eq!(pack.failures[0].exit, 1);
        assert_eq!(pack.failures[0].log_path, ".loom/runs/R-1/logs/verify.log");
        assert!(pack.log_excerpt.contains("assertion mismatch"));
        assert_eq!(
            pack.evidence_paths,
            vec![".loom/runs/R-1/outbox/report.md".to_string()]
        );
    }

    #[test]
    fn ignores_missing_done_and_foreign_receipts() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let project = Project::at(root);

        assert!(load_failure_pack(&project, "T-1", "R-1")
            .expect("missing")
            .is_none());

        let mut done = failed_receipt("T-1");
        done.status = ReceiptStatus::Done;
        done.verify[0].result = VerifyResult::Pass;
        write_receipt(root, &done);
        assert!(load_failure_pack(&project, "T-1", "R-1")
            .expect("done")
            .is_none());

        write_receipt(root, &failed_receipt("T-2"));
        assert!(load_failure_pack(&project, "T-1", "R-1")
            .expect("foreign")
            .is_none());
    }
}
