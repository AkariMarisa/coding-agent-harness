//! Local skills catalog (`doc/15` §3.3): the hand-maintained registry of
//! what exists, where it came from, and whether it is trusted.
//!
//! v1 refresh is a reviewed edit of the tracked file — no network, no
//! embeddings (`doc/15` §3.1). Integrity (sha256) lives in the lock, not
//! here; the catalog owns identity and trust, the lock owns bytes.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

pub const CATALOG_REL: &str = ".opencode/skills.catalog.json";
const CATALOG_MAX_BYTES: u64 = 256 * 1024;

/// Third-party skills must sit under one of these (`doc/15` §3.8). Anything
/// else needs human review, which in v1 means `source = "internal"` — this
/// repository's own license travels with the tracked lock, reviewed in git.
pub const LICENSE_ALLOWLIST: [&str; 4] = ["MIT", "Apache-2.0", "BSD-2-Clause", "BSD-3-Clause"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillStatus {
    Core,
    Project,
    Experimental,
    Blocked,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Triggers {
    #[serde(default)]
    pub lang: Vec<String>,
    #[serde(default)]
    pub globs: Vec<String>,
    #[serde(default)]
    pub roles: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogSkill {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// `internal` (this repository) or `third-party` (provenance detail is
    /// recorded in `.opencode/skill-pilot/THIRD_PARTY_NOTICES.md`).
    pub source: String,
    pub license: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub triggers: Triggers,
    pub status: SkillStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Catalog {
    pub v: u32,
    pub skills: Vec<CatalogSkill>,
}

pub fn catalog_path(root: &Path) -> PathBuf {
    root.join(CATALOG_REL)
}

/// Read and validate the catalog. `Ok(None)` means "no catalog yet";
/// parse/validation problems are actionable errors (exit 4).
pub fn load_catalog(root: &Path) -> Result<Option<Catalog>> {
    let path = catalog_path(root);
    let meta = match fs::metadata(&path) {
        Ok(meta) => meta,
        Err(_) => return Ok(None),
    };
    if meta.len() > CATALOG_MAX_BYTES {
        return Err(anyhow::anyhow!(
            "{CATALOG_REL} is {} bytes (limit {CATALOG_MAX_BYTES})",
            meta.len()
        ));
    }
    let text = fs::read_to_string(&path).with_context(|| format!("read {CATALOG_REL}"))?;
    let catalog: Catalog =
        serde_json::from_str(&text).with_context(|| format!("parse {CATALOG_REL}"))?;
    catalog
        .validate()
        .map_err(|message| anyhow::anyhow!(message))?;
    Ok(Some(catalog))
}

impl Catalog {
    pub fn validate(&self) -> Result<(), String> {
        if self.v != 1 {
            return Err(format!("version must be 1, got {}", self.v));
        }
        let mut seen = HashSet::new();
        for skill in &self.skills {
            if skill.id.trim().is_empty() {
                return Err("catalog skill id must not be empty".to_string());
            }
            if !seen.insert(skill.id.as_str()) {
                return Err(format!("duplicate catalog skill id: {}", skill.id));
            }
            if skill.license.trim().is_empty() {
                return Err(format!("skill {}: license must not be empty", skill.id));
            }
            if !matches!(skill.source.as_str(), "internal" | "third-party") {
                return Err(format!(
                    "skill {}: source must be internal or third-party, got {:?}",
                    skill.id, skill.source
                ));
            }
        }
        Ok(())
    }

    pub fn get(&self, id: &str) -> Option<&CatalogSkill> {
        self.skills.iter().find(|skill| skill.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str) -> serde_json::Value {
        serde_json::json!({
            "id": id,
            "source": "third-party",
            "license": "MIT",
            "tags": ["rust"],
            "triggers": {"lang": ["rust"], "globs": ["**/*.rs"], "roles": ["review"]},
            "status": "project"
        })
    }

    fn write_catalog(dir: &Path, value: serde_json::Value) {
        let path = catalog_path(dir);
        fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        fs::write(&path, serde_json::to_string_pretty(&value).expect("ser")).expect("write");
    }

    #[test]
    fn loads_a_valid_catalog() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_catalog(
            dir.path(),
            serde_json::json!({"v": 1, "skills": [entry("a"), entry("b")]}),
        );
        let catalog = load_catalog(dir.path()).expect("load").expect("present");
        assert_eq!(catalog.skills.len(), 2);
        assert_eq!(catalog.get("a").expect("a").status, SkillStatus::Project);
        assert!(catalog.get("missing").is_none());
        assert_eq!(catalog.get("a").expect("a").triggers.globs, vec!["**/*.rs"]);
    }

    #[test]
    fn absent_catalog_is_none_not_an_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(load_catalog(dir.path()).expect("load").is_none());
    }

    #[test]
    fn rejects_duplicates_bad_source_and_unknown_fields() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_catalog(
            dir.path(),
            serde_json::json!({"v": 1, "skills": [entry("a"), entry("a")]}),
        );
        assert!(load_catalog(dir.path())
            .expect_err("dup")
            .to_string()
            .contains("duplicate"));

        let mut bad = entry("a");
        bad["source"] = serde_json::json!("somewhere");
        write_catalog(dir.path(), serde_json::json!({"v": 1, "skills": [bad]}));
        assert!(load_catalog(dir.path())
            .expect_err("source")
            .to_string()
            .contains("source"));

        write_catalog(
            dir.path(),
            serde_json::json!({"v": 1, "skills": [entry("a")], "mystery": true}),
        );
        assert!(load_catalog(dir.path()).is_err(), "unknown fields error");
    }

    #[test]
    fn license_allowlist_covers_the_documented_licenses() {
        for license in ["MIT", "Apache-2.0", "BSD-2-Clause", "BSD-3-Clause"] {
            assert!(LICENSE_ALLOWLIST.contains(&license));
        }
        assert!(!LICENSE_ALLOWLIST.contains(&"PolyForm-Noncommercial-1.0.0"));
    }
}
