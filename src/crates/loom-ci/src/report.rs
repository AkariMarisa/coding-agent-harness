//! CI run report and attestation (`doc/16` §7.8).
//!
//! The report is evidence of "these commands ran in this tree", not a trust
//! boundary: v1 does not sign (`signature = "none"`) and never skips server
//! jobs. The tree fingerprint covers HEAD, the tracked diff, and untracked
//! files — verification must see new files too.

use std::path::Path;
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::config::ci_path;
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TreeFingerprint {
    pub head: Option<String>,
    pub dirty_sha256: Option<String>,
    pub untracked_sha256: Option<String>,
    pub dirty: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobReport {
    pub id: String,
    pub argv: Vec<String>,
    pub cwd: String,
    pub exit: Option<i32>,
    pub signal: Option<i32>,
    pub timed_out: bool,
    pub duration_ms: u64,
    pub log_path: String,
    pub log_sha256: String,
    /// True when `--fix` ran this job's `fix_argv` first.
    #[serde(default)]
    pub fixed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunReport {
    pub v: u32,
    pub run_id: String,
    pub profile: String,
    pub ci_toml_sha256: Option<String>,
    pub tree: TreeFingerprint,
    pub started_at_ms: u64,
    pub duration_ms: u64,
    pub jobs: Vec<JobReport>,
    pub ok: bool,
    /// Evidence marker aggregated over the executed jobs; v1 never skips
    /// server CI on this basis.
    pub local_sufficient: bool,
    pub advisory: bool,
    /// First tool/runner that could not be found (exit 2); absent when all
    /// probes passed. Extra beyond `doc/16` §7.8's minimum field list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub missing_tool: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attestation {
    pub v: u32,
    pub run_id: String,
    pub report_sha256: String,
    pub tree: TreeFingerprint,
    pub ci_toml_sha256: Option<String>,
    /// v1 never signs; unsigned does not mean trustworthy.
    pub signature: String,
    pub created_at_ms: u64,
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// Hash of `HEAD` + `git diff --binary` + untracked files (path/size/content,
/// sorted). Never runs `git write-tree` — that would touch the index. When
/// git is unavailable the fingerprint degrades to all-`None` instead of
/// failing the run: a non-repo temp project can still execute jobs.
pub fn tree_fingerprint(root: &Path) -> TreeFingerprint {
    let head = git_stdout(root, &["rev-parse", "HEAD"]);
    let diff = git_stdout(root, &["diff", "--binary"]);
    let untracked =
        git_stdout(root, &["ls-files", "--others", "--exclude-standard"]).unwrap_or_default();

    let dirty_sha256 = diff.as_ref().map(|diff| sha256_hex(diff.as_bytes()));
    let dirty = diff.as_deref().is_some_and(|diff| !diff.is_empty());

    let mut untracked_hashes = Vec::new();
    for path in untracked.lines().filter(|line| !line.trim().is_empty()) {
        let full = root.join(path);
        let content = std::fs::read(&full).unwrap_or_default();
        untracked_hashes.push(format!(
            "{path}\0{}\0{}\n",
            content.len(),
            sha256_hex(&content)
        ));
    }
    untracked_hashes.sort();
    let untracked_sha256 = head
        .as_ref()
        .map(|_| sha256_hex(untracked_hashes.join("").as_bytes()));

    TreeFingerprint {
        head,
        dirty_sha256,
        untracked_sha256,
        dirty,
    }
}

fn git_stdout(root: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Stable identifier for attention dedupe keys: a short hash of the whole
/// fingerprint, so "same tree" means the same bytes including untracked.
pub fn tree_id(fingerprint: &TreeFingerprint) -> String {
    let encoded = serde_json::to_string(fingerprint).unwrap_or_default();
    let digest = sha256_hex(encoded.as_bytes());
    digest[..12].to_string()
}

/// Result of `loom ci verify-attest`: what the file can and cannot prove.
#[derive(Debug, Clone, serde::Serialize)]
pub struct VerifyOutcome {
    pub ok: bool,
    /// Everything that failed to line up; empty when the file matches.
    pub reasons: Vec<String>,
    /// Always true in v1: unsigned is expected, never trustworthy-by-default.
    pub unsigned: bool,
}

/// Verify an attestation against the report it pins and the tree on disk
/// (`doc/16` §7.8). Proves "this file matches this tree" — nothing more:
/// v1 has no signature, and no server job may skip because of this.
pub fn verify_attestation(root: &Path, path: &Path) -> Result<VerifyOutcome, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| format!("read {}: {error}", path.display()))?;
    let attestation: Attestation =
        serde_json::from_str(&text).map_err(|error| format!("parse attestation: {error}"))?;

    let mut reasons = Vec::new();

    let report_path = path.parent().unwrap_or(root).join("report.json");
    match std::fs::read(&report_path) {
        Ok(bytes) => {
            if sha256_hex(&bytes) != attestation.report_sha256 {
                reasons.push("report.json does not match the pinned digest".to_string());
            }
        }
        Err(error) => reasons.push(format!("cannot read {}: {error}", report_path.display())),
    }

    let current = tree_fingerprint(root);
    if tree_id(&current) != tree_id(&attestation.tree) {
        reasons.push("working tree differs from the attested tree".to_string());
    }

    let ci_hash = std::fs::read(ci_path(root))
        .ok()
        .map(|bytes| sha256_hex(&bytes));
    if ci_hash != attestation.ci_toml_sha256 {
        reasons.push(".loom.ci.toml changed since the run".to_string());
    }

    Ok(VerifyOutcome {
        ok: reasons.is_empty(),
        reasons,
        unsigned: attestation.signature == "none",
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn fingerprint_degrades_outside_a_repo() {
        let dir = tempfile::tempdir().expect("tempdir");
        // Not a git repo: the fingerprint degrades instead of failing.
        let first = tree_fingerprint(dir.path());
        assert!(first.head.is_none());
        assert!(!first.dirty);
        assert_eq!(tree_id(&first), tree_id(&first), "stable for one tree");
    }

    #[test]
    fn an_untracked_file_changes_the_tree_id() {
        let dir = tempfile::tempdir().expect("tempdir");
        let git = |args: &[&str]| {
            let status = std::process::Command::new("git")
                .arg("-C")
                .arg(dir.path())
                .args(args)
                .status()
                .expect("git");
            assert!(status.success(), "git {args:?}");
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "ci@example.com"]);
        git(&["config", "user.name", "ci"]);
        std::fs::write(dir.path().join("tracked.txt"), "one").expect("write");
        git(&["add", "."]);
        git(&["commit", "-qm", "init"]);

        let before = tree_fingerprint(dir.path());
        std::fs::write(dir.path().join("new.txt"), "hello").expect("write");
        let after = tree_fingerprint(dir.path());
        assert_ne!(
            tree_id(&before),
            tree_id(&after),
            "verification must see untracked files (doc/16 §7.8)"
        );
    }

    #[test]
    fn fingerprint_sees_git_tracked_changes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let git = |args: &[&str]| {
            let status = std::process::Command::new("git")
                .arg("-C")
                .arg(dir.path())
                .args(args)
                .status()
                .expect("git");
            assert!(status.success(), "git {args:?}");
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "ci@example.com"]);
        git(&["config", "user.name", "ci"]);
        std::fs::write(dir.path().join("a.txt"), "one").expect("write");
        git(&["add", "."]);
        git(&["commit", "-qm", "init"]);

        let clean = tree_fingerprint(dir.path());
        assert!(clean.head.is_some());
        assert!(!clean.dirty);

        std::fs::write(dir.path().join("a.txt"), "two").expect("write");
        let dirty = tree_fingerprint(dir.path());
        assert!(dirty.dirty);
        assert_ne!(clean.dirty_sha256, dirty.dirty_sha256);
        assert_eq!(clean.head, dirty.head, "HEAD unchanged, diff changed");
    }

    #[test]
    fn report_and_attestation_roundtrip() {
        let report = RunReport {
            v: 1,
            run_id: "CI-1".into(),
            profile: "fast".into(),
            ci_toml_sha256: Some("abc".into()),
            tree: TreeFingerprint {
                head: None,
                dirty_sha256: None,
                untracked_sha256: None,
                dirty: false,
            },
            started_at_ms: 1,
            duration_ms: 2,
            jobs: Vec::new(),
            ok: true,
            local_sufficient: true,
            advisory: false,
            missing_tool: None,
        };
        let text = serde_json::to_string_pretty(&report).expect("serialize");
        let back: RunReport = serde_json::from_str(&text).expect("parse");
        assert_eq!(back.run_id, "CI-1");
        assert!(back.ok);

        let attestation = Attestation {
            v: 1,
            run_id: "CI-1".into(),
            report_sha256: sha256_hex(text.as_bytes()),
            tree: report.tree.clone(),
            ci_toml_sha256: report.ci_toml_sha256.clone(),
            signature: "none".into(),
            created_at_ms: 3,
        };
        assert_eq!(attestation.signature, "none");
        assert_eq!(attestation.report_sha256.len(), 64);
    }

    fn git_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        let git = |args: &[&str]| {
            let status = std::process::Command::new("git")
                .arg("-C")
                .arg(dir.path())
                .args(args)
                .status()
                .expect("git");
            assert!(status.success(), "git {args:?}");
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "ci@example.com"]);
        git(&["config", "user.name", "ci"]);
        // Mirror a real project: the state layer is ignored, so CI artifacts
        // never move the tree the attestation pins.
        std::fs::write(dir.path().join(".gitignore"), "/.loom/*\n").expect("write");
        std::fs::write(dir.path().join("a.txt"), "one").expect("write");
        git(&["add", "."]);
        git(&["commit", "-qm", "init"]);
        dir
    }

    fn attest(dir: &Path) -> PathBuf {
        let tree = tree_fingerprint(dir);
        let run_dir = dir.join(".loom").join("ci").join("CI-TEST");
        std::fs::create_dir_all(&run_dir).expect("mkdir");
        let report = RunReport {
            v: 1,
            run_id: "CI-TEST".into(),
            profile: "fast".into(),
            ci_toml_sha256: None,
            tree: tree.clone(),
            started_at_ms: 1,
            duration_ms: 2,
            jobs: Vec::new(),
            ok: true,
            local_sufficient: true,
            advisory: false,
            missing_tool: None,
        };
        let report_path = run_dir.join("report.json");
        let text = serde_json::to_string_pretty(&report).expect("serialize");
        let bytes = format!("{text}\n");
        std::fs::write(&report_path, &bytes).expect("write");
        let attestation = Attestation {
            v: 1,
            run_id: "CI-TEST".into(),
            // Pin the bytes on disk, newline included — same as `run` does.
            report_sha256: sha256_hex(bytes.as_bytes()),
            tree,
            ci_toml_sha256: None,
            signature: "none".into(),
            created_at_ms: 3,
        };
        let attestation_path = run_dir.join("attestation.json");
        let text = serde_json::to_string_pretty(&attestation).expect("serialize");
        std::fs::write(&attestation_path, format!("{text}\n")).expect("write");
        attestation_path
    }

    #[test]
    fn attestation_verifies_on_the_same_tree_and_fails_on_another() {
        let dir = git_repo();
        let path = attest(dir.path());

        let outcome = verify_attestation(dir.path(), &path).expect("verify");
        assert!(outcome.ok, "same tree must verify: {:?}", outcome.reasons);
        assert!(outcome.unsigned, "v1 is unsigned by design");

        // A tracked edit moves the tree.
        std::fs::write(dir.path().join("a.txt"), "changed").expect("write");
        let outcome = verify_attestation(dir.path(), &path).expect("verify");
        assert!(!outcome.ok);
        assert!(outcome
            .reasons
            .iter()
            .any(|reason| reason.contains("tree differs")));

        // An untracked file moves it too (verification must see new files).
        std::fs::write(dir.path().join("a.txt"), "one").expect("restore");
        std::fs::write(dir.path().join("brand-new.txt"), "x").expect("write");
        let outcome = verify_attestation(dir.path(), &path).expect("verify");
        assert!(!outcome.ok, "untracked files are part of the tree");
    }

    #[test]
    fn tampered_report_fails_the_digest_check() {
        let dir = git_repo();
        let path = attest(dir.path());
        let report_path = path.parent().expect("dir").join("report.json");
        std::fs::write(&report_path, "{\"v\":1}").expect("tamper");

        let outcome = verify_attestation(dir.path(), &path).expect("verify");
        assert!(!outcome.ok);
        assert!(outcome
            .reasons
            .iter()
            .any(|reason| reason.contains("digest")));
    }
}
