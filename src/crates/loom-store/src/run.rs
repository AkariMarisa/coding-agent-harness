use std::fs;
use std::path::PathBuf;

use anyhow::{bail, Context, Result};

use crate::Project;

#[derive(Debug, Clone)]
pub struct Run {
    pub project: Project,
    pub id: String,
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
        fs::create_dir_all(self.artifacts_dir()).with_context(|| "create artifacts dir")?;
        fs::create_dir_all(self.logs_dir()).with_context(|| "create logs dir")?;
        Ok(())
    }

    pub fn dir(&self) -> PathBuf {
        self.project.loom_dir().join("runs").join(&self.id)
    }

    pub fn artifacts_dir(&self) -> PathBuf {
        self.dir().join("artifacts")
    }

    pub fn logs_dir(&self) -> PathBuf {
        self.dir().join("logs")
    }

    pub fn seen_path(&self) -> PathBuf {
        self.logs_dir().join("seen.jsonl")
    }

    pub fn telemetry_path(&self) -> PathBuf {
        self.logs_dir().join("tools.jsonl")
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
}
