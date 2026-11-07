use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::Write;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::Run;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Entry {
    k: String,
    #[serde(rename = "ref")]
    artifact_ref: String,
}

/// Per-run content dedupe ledger.
pub struct Seen {
    path: std::path::PathBuf,
}

impl Seen {
    pub fn new(run: &Run) -> Self {
        Self {
            path: run.seen_path(),
        }
    }

    pub fn get(&self, key: &str) -> Result<Option<String>> {
        Ok(self.load()?.remove(key))
    }

    pub fn mark(&self, key: &str, artifact_ref: &str) -> Result<()> {
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .with_context(|| "open seen.jsonl")?;
        let mut line = serde_json::to_string(&Entry {
            k: key.to_string(),
            artifact_ref: artifact_ref.to_string(),
        })?;
        line.push('\n');
        file.write_all(line.as_bytes())
            .with_context(|| "append seen.jsonl")?;
        Ok(())
    }

    fn load(&self) -> Result<HashMap<String, String>> {
        let mut map = HashMap::new();
        if !self.path.exists() {
            return Ok(map);
        }

        let text = fs::read_to_string(&self.path).with_context(|| "read seen.jsonl")?;
        for line in text.lines() {
            let Ok(entry) = serde_json::from_str::<Entry>(line) else {
                continue;
            };
            map.insert(entry.k, entry.artifact_ref);
        }
        Ok(map)
    }
}
