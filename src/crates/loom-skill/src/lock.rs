//! `skills.lock` — the tracked integrity snapshot (`doc/16` §9.2).
//!
//! Lives under `.opencode/` next to the skills it pins and is **never**
//! under `.loom/`: teammates, worktrees and CI must all see the same bytes.
//! Identity and trust live in the catalog; the lock records what is on disk
//! so `loom skill verify` can prove nothing drifted.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

pub const LOCK_REL: &str = ".opencode/skills.lock";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LockEntry {
    pub id: String,
    /// Repo-relative path with forward slashes, always `…/<id>/SKILL.md`.
    pub path: String,
    pub sha256: String,
    pub license: String,
    pub source: String,
    pub description_chars: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkillsLock {
    pub v: u32,
    pub skills: Vec<LockEntry>,
}

pub fn lock_path(root: &Path) -> PathBuf {
    root.join(LOCK_REL)
}

pub fn load_lock(root: &Path) -> Result<Option<SkillsLock>> {
    let path = lock_path(root);
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(_) => return Ok(None),
    };
    let lock: SkillsLock =
        serde_json::from_str(&text).with_context(|| format!("parse {LOCK_REL}"))?;
    if lock.v != 1 {
        return Err(anyhow::anyhow!(
            "{LOCK_REL}: version must be 1, got {}",
            lock.v
        ));
    }
    let mut seen = std::collections::HashSet::new();
    for entry in &lock.skills {
        if !seen.insert(entry.id.as_str()) {
            return Err(anyhow::anyhow!("{LOCK_REL}: duplicate id {}", entry.id));
        }
        if entry.sha256.len() != 64 || !entry.sha256.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(anyhow::anyhow!(
                "{LOCK_REL}: {} has a malformed sha256",
                entry.id
            ));
        }
    }
    Ok(Some(lock))
}

/// Atomic replace so a reviewed lock never lands half-written.
pub fn write_lock(root: &Path, lock: &SkillsLock) -> Result<PathBuf> {
    let path = lock_path(root);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    let text = serde_json::to_string_pretty(lock)?;
    let temp = path.with_extension(format!("json.tmp-{}", std::process::id()));
    fs::write(&temp, format!("{text}\n")).with_context(|| format!("write {}", temp.display()))?;
    fs::rename(&temp, &path).with_context(|| format!("replace {}", path.display()))?;
    Ok(path)
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_validation() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(load_lock(dir.path()).expect("load").is_none());

        let lock = SkillsLock {
            v: 1,
            skills: vec![LockEntry {
                id: "a".into(),
                path: ".opencode/skills/a/SKILL.md".into(),
                sha256: "0".repeat(64),
                license: "MIT".into(),
                source: "internal".into(),
                description_chars: 10,
            }],
        };
        write_lock(dir.path(), &lock).expect("write");
        let back = load_lock(dir.path()).expect("load").expect("present");
        assert_eq!(back.skills[0].id, "a");

        // Duplicate ids and malformed hashes are rejected on read.
        let dup = SkillsLock {
            v: 1,
            skills: vec![lock.skills[0].clone(), lock.skills[0].clone()],
        };
        write_lock(dir.path(), &dup).expect("write");
        assert!(load_lock(dir.path())
            .expect_err("dup")
            .to_string()
            .contains("duplicate"));

        let mut bad = lock.clone();
        bad.skills[0].sha256 = "abc".into();
        write_lock(dir.path(), &bad).expect("write");
        assert!(load_lock(dir.path())
            .expect_err("sha")
            .to_string()
            .contains("sha256"));
    }
}
