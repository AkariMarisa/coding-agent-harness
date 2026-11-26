//! Repo profile scan (`doc/15` §3.2): deterministic, zero-LLM, read-only
//! with respect to the repo — the only write is the `.loom/` view itself.
//!
//! Rules: every tag carries at least one evidence file (no evidence, no
//! tag); the cache key is `git_head` + the mtimes of the candidate files,
//! so an unchanged tree never rescans; nothing is guessed — stacks without
//! a detection rule simply do not appear.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

pub const STACK_PROFILE_NAME: &str = "stack-profile.json";

/// Files whose presence/content decides a tag; the same list feeds the
/// cache key, so a change here invalidates the profile.
const CANDIDATES: [&str; 12] = [
    "Cargo.toml",
    "src/Cargo.toml",
    "package.json",
    "pyproject.toml",
    "requirements.txt",
    "go.mod",
    "Makefile",
    "Containerfile",
    "compose.yaml",
    "scripts/dev.sh",
    "rust-toolchain.toml",
    ".nvmrc",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StackProfile {
    pub v: u32,
    /// Absolute project root (runtime view, never committed).
    pub root: String,
    pub git_head: Option<String>,
    pub lang: Vec<String>,
    /// No detection rules in v1 — an empty list is the honest answer,
    /// never a guessed framework.
    pub frameworks: Vec<String>,
    pub infra: Vec<String>,
    pub tests: Vec<String>,
    /// Version requirements read from pin files (`.nvmrc` and friends);
    /// live probing belongs to `loom ci doctor`, not a deterministic scan.
    pub toolchain: BTreeMap<String, String>,
    /// tag → the files that prove it.
    pub evidence: BTreeMap<String, Vec<String>>,
    /// `sha256:` over `git_head` + candidate mtimes — content identity and
    /// cache key in one (`doc/15` §3.2).
    pub hash: String,
}

#[derive(Debug)]
pub struct ScanOutcome {
    pub profile: StackProfile,
    /// False when an unchanged profile was reused (cache hit).
    pub written: bool,
}

pub fn profile_path(root: &Path) -> PathBuf {
    root.join(".loom").join(STACK_PROFILE_NAME)
}

/// Scan `root` and refresh `.loom/stack-profile.json` only when the cache
/// key moved.
pub fn scan(root: &Path) -> Result<ScanOutcome> {
    let git_head = git_head(root);
    let mut material = String::new();
    material.push_str(git_head.as_deref().unwrap_or(""));
    let mut existing: Vec<&str> = Vec::new();
    for candidate in CANDIDATES {
        let path = root.join(candidate);
        if let Ok(meta) = fs::metadata(&path) {
            let mtime = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            material.push('\n');
            material.push_str(&format!("{candidate}:{mtime}"));
            existing.push(candidate);
        }
    }
    let hash = format!("sha256:{}", sha256_hex(material.as_bytes()));

    let path = profile_path(root);
    if let Ok(text) = fs::read_to_string(&path) {
        if let Ok(cached) = serde_json::from_str::<StackProfile>(&text) {
            if cached.hash == hash {
                return Ok(ScanOutcome {
                    profile: cached,
                    written: false,
                });
            }
        }
    }

    let profile = detect(root, git_head, &existing, hash)?;
    write_profile(&path, &profile)?;
    Ok(ScanOutcome {
        profile,
        written: true,
    })
}

fn add_evidence(evidence: &mut BTreeMap<String, Vec<String>>, tag: &str, file: &str) {
    let entry = evidence.entry(tag.to_string()).or_default();
    let path = file.to_string();
    if !entry.contains(&path) {
        entry.push(path);
    }
}

fn detect(
    root: &Path,
    git_head: Option<String>,
    existing: &[&str],
    hash: String,
) -> Result<StackProfile> {
    let has = |name: &str| existing.contains(&name);
    let read = |name: &str| fs::read_to_string(root.join(name)).unwrap_or_default();

    let mut lang = Vec::new();
    let mut infra = Vec::new();
    let mut tests = Vec::new();
    let mut toolchain = BTreeMap::new();
    let mut evidence: BTreeMap<String, Vec<String>> = BTreeMap::new();

    // Language stacks: presence only, never content guessing.
    if has("Cargo.toml") || has("src/Cargo.toml") {
        lang.push("rust".to_string());
        add_evidence(&mut evidence, "rust", "Cargo.toml");
        if has("src/Cargo.toml") {
            add_evidence(&mut evidence, "rust", "src/Cargo.toml");
        }
        tests.push("cargo-test".to_string());
        add_evidence(&mut evidence, "cargo-test", "Cargo.toml");
    }
    if has("package.json") {
        lang.push("node".to_string());
        add_evidence(&mut evidence, "node", "package.json");
        if read("package.json").contains("\"test\"") {
            tests.push("node-test".to_string());
            add_evidence(&mut evidence, "node-test", "package.json");
        }
    }
    if has("pyproject.toml") || has("requirements.txt") {
        lang.push("python".to_string());
        if has("pyproject.toml") {
            add_evidence(&mut evidence, "python", "pyproject.toml");
            if read("pyproject.toml").contains("pytest") {
                tests.push("pytest".to_string());
                add_evidence(&mut evidence, "pytest", "pyproject.toml");
            }
        }
        if has("requirements.txt") {
            add_evidence(&mut evidence, "python", "requirements.txt");
        }
    }
    if has("go.mod") {
        lang.push("go".to_string());
        add_evidence(&mut evidence, "go", "go.mod");
        tests.push("go-test".to_string());
        add_evidence(&mut evidence, "go-test", "go.mod");
    }

    // Infrastructure evidence.
    if has("Containerfile") || has("compose.yaml") || has("scripts/dev.sh") {
        infra.push("container".to_string());
        for file in ["Containerfile", "compose.yaml", "scripts/dev.sh"] {
            if has(file) {
                add_evidence(&mut evidence, "container", file);
            }
        }
    }
    if has("Makefile") {
        infra.push("make".to_string());
        add_evidence(&mut evidence, "make", "Makefile");
        if read("Makefile")
            .lines()
            .any(|line| line.starts_with("check:"))
        {
            tests.push("make-check".to_string());
            add_evidence(&mut evidence, "make-check", "Makefile");
        }
    }

    // Toolchain pins: file contents only (deterministic, no execution).
    if has("rust-toolchain.toml") {
        let text = read("rust-toolchain.toml");
        if let Some(line) = text.lines().find(|line| line.contains("channel")) {
            let value = line
                .split("channel")
                .nth(1)
                .unwrap_or("")
                .trim_start_matches(['=', ' '])
                .trim()
                .trim_matches('"')
                .trim();
            if !value.is_empty() {
                toolchain.insert("rust".to_string(), value.to_string());
            }
        }
    }
    if has(".nvmrc") {
        let value = read(".nvmrc").trim().to_string();
        if !value.is_empty() {
            toolchain.insert("node".to_string(), value);
        }
    }
    if has(".python-version") {
        let value = read(".python-version").trim().to_string();
        if !value.is_empty() {
            toolchain.insert("python".to_string(), value);
        }
    }

    lang.sort();
    infra.sort();
    tests.sort();

    // Invariant: no evidence, no tag (`doc/15` §3.2, checklist).
    for tag in lang.iter().chain(&infra).chain(&tests) {
        anyhow::ensure!(
            evidence.contains_key(tag),
            "tag {tag} was produced without an evidence file"
        );
    }

    Ok(StackProfile {
        v: 1,
        root: root.display().to_string(),
        git_head,
        lang,
        frameworks: Vec::new(),
        infra,
        tests,
        toolchain,
        evidence,
        hash,
    })
}

fn write_profile(path: &Path, profile: &StackProfile) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    let text = serde_json::to_string_pretty(profile)?;
    let temp = path.with_extension(format!("json.tmp-{}", std::process::id()));
    fs::write(&temp, format!("{text}\n")).with_context(|| format!("write {}", temp.display()))?;
    fs::rename(&temp, path).with_context(|| format!("replace {}", path.display()))?;
    Ok(())
}

fn git_head(root: &Path) -> Option<String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let head = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!head.is_empty()).then_some(head)
}

fn sha256_hex(bytes: &[u8]) -> String {
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

    fn write(path: &Path, content: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("mkdir");
        }
        fs::write(path, content).expect("write");
    }

    #[test]
    fn rust_workspace_gets_lang_and_tests_with_evidence() {
        let dir = tempfile::tempdir().expect("tempdir");
        write(&dir.path().join("Cargo.toml"), "[workspace]\n");
        write(
            &dir.path().join("src/Cargo.toml"),
            "[package]\nname = \"x\"\n",
        );
        write(&dir.path().join("Makefile"), "check:\n\t./run.sh\n");
        write(&dir.path().join("Containerfile"), "FROM scratch\n");

        let outcome = scan(dir.path()).expect("scan");
        assert!(outcome.written);
        let profile = &outcome.profile;
        assert_eq!(profile.lang, vec!["rust"]);
        assert_eq!(profile.infra, vec!["container", "make"]);
        assert_eq!(profile.tests, vec!["cargo-test", "make-check"]);
        assert_eq!(
            profile.evidence.get("rust").expect("rust evidence").len(),
            2,
            "both Cargo.toml paths are evidence"
        );
        // No evidence, no tag: every declared tag has an evidence entry.
        for tag in profile
            .lang
            .iter()
            .chain(&profile.infra)
            .chain(&profile.tests)
        {
            assert!(
                profile.evidence.contains_key(tag),
                "missing evidence for {tag}"
            );
        }
        assert!(profile.frameworks.is_empty(), "v1 never guesses frameworks");
        assert!(profile.hash.starts_with("sha256:"));
        assert!(profile_path(dir.path()).exists());
    }

    #[test]
    fn virgin_directory_still_produces_a_valid_profile() {
        let dir = tempfile::tempdir().expect("tempdir");
        let outcome = scan(dir.path()).expect("scan");
        assert!(outcome.written);
        assert!(outcome.profile.lang.is_empty());
        assert!(outcome.profile.evidence.is_empty());
        assert!(outcome.profile.hash.starts_with("sha256:"));
    }

    #[test]
    fn unchanged_tree_hits_the_cache_and_a_change_invalidates_it() {
        let dir = tempfile::tempdir().expect("tempdir");
        write(&dir.path().join("Cargo.toml"), "[workspace]\n");
        let first = scan(dir.path()).expect("scan");
        assert!(first.written);

        let second = scan(dir.path()).expect("scan");
        assert!(!second.written, "same git_head + mtimes must not rewrite");
        assert_eq!(first.profile.hash, second.profile.hash);

        // mtime granularity is nanoseconds on the test filesystem; a short
        // sleep keeps the assertion honest on coarse clocks too.
        std::thread::sleep(std::time::Duration::from_millis(15));
        write(
            &dir.path().join("Cargo.toml"),
            "[workspace]\nresolver = \"2\"\n",
        );
        let third = scan(dir.path()).expect("scan");
        assert!(third.written, "a touched candidate must invalidate");
        assert_ne!(first.profile.hash, third.profile.hash);
    }

    #[test]
    fn toolchain_pins_are_read_from_files_not_probed() {
        let dir = tempfile::tempdir().expect("tempdir");
        write(
            &dir.path().join("rust-toolchain.toml"),
            "[toolchain]\nchannel = \"1.99.1\"\n",
        );
        write(&dir.path().join(".nvmrc"), "22.11.0\n");
        let outcome = scan(dir.path()).expect("scan");
        assert_eq!(
            outcome.profile.toolchain.get("rust").map(String::as_str),
            Some("1.99.1")
        );
        assert_eq!(
            outcome.profile.toolchain.get("node").map(String::as_str),
            Some("22.11.0")
        );
    }

    #[test]
    fn node_and_python_detection_never_guess() {
        let dir = tempfile::tempdir().expect("tempdir");
        write(
            &dir.path().join("package.json"),
            "{\"scripts\":{\"test\":\"vitest\"}}",
        );
        write(
            &dir.path().join("pyproject.toml"),
            "[tool.pytest.ini_options]\n",
        );
        let outcome = scan(dir.path()).expect("scan");
        assert_eq!(outcome.profile.lang, vec!["node", "python"]);
        assert_eq!(outcome.profile.tests, vec!["node-test", "pytest"]);

        // A package.json without a test script earns no test tag.
        let dir2 = tempfile::tempdir().expect("tempdir");
        write(
            &dir2.path().join("package.json"),
            "{\"scripts\":{\"build\":\"tsc\"}}",
        );
        let outcome = scan(dir2.path()).expect("scan");
        assert!(outcome.profile.tests.is_empty(), "no script, no test tag");
    }

    #[test]
    fn profile_roundtrips_through_json() {
        let dir = tempfile::tempdir().expect("tempdir");
        write(&dir.path().join("Cargo.toml"), "[workspace]\n");
        scan(dir.path()).expect("scan");
        let text = fs::read_to_string(profile_path(dir.path())).expect("read");
        let back: StackProfile = serde_json::from_str(&text).expect("parse");
        assert_eq!(back.v, 1);
        assert_eq!(back.lang, vec!["rust"]);
    }
}
