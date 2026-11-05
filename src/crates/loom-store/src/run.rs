use std::fs;
use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::Project;

pub const RUN_META_VERSION: u8 = 1;

#[derive(Debug, Clone)]
pub struct Run {
    pub project: Project,
    pub id: String,
}

/// Dispatch-time metadata. The workspace path is what `verify` reuses; it is
/// never inferred from the current shell.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RunMeta {
    pub v: u8,
    pub run_id: String,
    pub task_id: String,
    pub adapter: String,
    pub model: String,
    pub model_id: String,
    pub workspace: String,
    pub base_sha: String,
    pub dispatched_at_ms: u64,
    pub timeout_s: u64,
    pub packet_dir: String,
}

impl RunMeta {
    pub fn validate(&self) -> Result<()> {
        if self.v != RUN_META_VERSION {
            bail!("unsupported run meta version: {}", self.v);
        }
        if self.run_id.trim().is_empty() || self.task_id.trim().is_empty() {
            bail!("run meta requires run_id and task_id");
        }
        if self.workspace.trim().is_empty() {
            bail!("run meta requires workspace");
        }
        Ok(())
    }
}

impl Run {
    pub fn open(project: Project, id: Option<&str>) -> Result<Self> {
        let id = id
            .map(ToOwned::to_owned)
            .or_else(|| std::env::var("LOOM_RUN_ID").ok())
            .unwrap_or_else(|| "local".to_string());

        if !valid_id(&id) {
            bail!("invalid run id: {id}");
        }

        let run = Self { project, id };
        run.ensure()?;
        Ok(run)
    }

    pub fn ensure(&self) -> Result<()> {
        for dir in [
            self.dir(),
            self.context_dir(),
            self.outbox_dir(),
            self.logs_dir(),
        ] {
            fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
        }
        Ok(())
    }

    pub fn dir(&self) -> PathBuf {
        self.project.loom_dir().join("runs").join(&self.id)
    }

    pub fn context_dir(&self) -> PathBuf {
        self.dir().join("context")
    }

    pub fn outbox_dir(&self) -> PathBuf {
        self.dir().join("outbox")
    }

    pub fn artifacts_dir(&self) -> PathBuf {
        self.dir().join("artifacts")
    }

    pub fn logs_dir(&self) -> PathBuf {
        self.dir().join("logs")
    }

    pub fn meta_path(&self) -> PathBuf {
        self.dir().join("run.json")
    }

    pub fn task_snapshot_path(&self) -> PathBuf {
        self.dir().join("task.json")
    }

    pub fn packet_path(&self) -> PathBuf {
        self.context_dir().join("packet.md")
    }

    pub fn manifest_path(&self) -> PathBuf {
        self.context_dir().join("manifest.json")
    }

    pub fn receipt_path(&self) -> PathBuf {
        self.outbox_dir().join("receipt.json")
    }

    pub fn final_path(&self) -> PathBuf {
        self.outbox_dir().join("final.md")
    }

    pub fn report_path(&self) -> PathBuf {
        self.outbox_dir().join("report.md")
    }

    pub fn usage_path(&self) -> PathBuf {
        self.outbox_dir().join("usage.jsonl")
    }

    pub fn verify_log_path(&self) -> PathBuf {
        self.logs_dir().join("verify.log")
    }

    pub fn agent_log_path(&self) -> PathBuf {
        self.logs_dir().join("agent.jsonl")
    }

    pub fn seen_path(&self) -> PathBuf {
        self.logs_dir().join("seen.jsonl")
    }

    pub fn telemetry_path(&self) -> PathBuf {
        self.logs_dir().join("tools.jsonl")
    }

    pub fn write_meta(&self, meta: &RunMeta) -> Result<()> {
        meta.validate()?;
        let text = serde_json::to_string_pretty(meta)?;
        fs::write(self.meta_path(), format!("{text}\n"))
            .with_context(|| format!("write {}", self.meta_path().display()))
    }

    pub fn read_meta(&self) -> Result<RunMeta> {
        let path = self.meta_path();
        let text = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
        let meta = serde_json::from_str::<RunMeta>(&text)
            .with_context(|| format!("parse {}", path.display()))?;
        meta.validate()?;
        Ok(meta)
    }
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id != "."
        && id != ".."
        && id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_bad_ids() {
        let dir = tempfile::tempdir().expect("tempdir");
        let project = Project::at(dir.path());
        assert!(Run::open(project.clone(), Some("../x")).is_err());
        assert!(Run::open(project.clone(), Some("")).is_err());
        assert!(Run::open(project, Some("R-001")).is_ok());
    }

    #[test]
    fn creates_and_reads_meta() {
        let dir = tempfile::tempdir().expect("tempdir");
        let project = Project::at(dir.path());
        let run = Run::open(project, Some("R-001")).expect("run");
        let meta = RunMeta {
            v: RUN_META_VERSION,
            run_id: run.id.clone(),
            task_id: "T-20261104-001".into(),
            adapter: "mock".into(),
            workspace: ".".into(),
            dispatched_at_ms: 1,
            ..RunMeta::default()
        };
        run.write_meta(&meta).expect("write");
        assert_eq!(run.read_meta().expect("read").task_id, meta.task_id);
        assert!(run.context_dir().is_dir());
        assert!(run.outbox_dir().is_dir());
    }

    #[test]
    fn rejects_bad_meta() {
        let dir = tempfile::tempdir().expect("tempdir");
        let project = Project::at(dir.path());
        let run = Run::open(project, Some("R-001")).expect("run");
        assert!(run.write_meta(&RunMeta::default()).is_err());
    }
}
