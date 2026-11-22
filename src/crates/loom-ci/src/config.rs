//! `.loom.ci.toml` — tracked CI policy at the project root.
//!
//! Implementation form is locked by `doc/16` §7.3: `[[jobs]] + argv +
//! [runner]`. The `[jobs.<id>] + cmd` example in `doc/15` §4.5 explains
//! field semantics only; supporting both is explicitly forbidden.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const CI_CONFIG_NAME: &str = ".loom.ci.toml";
pub const CI_VERSION: u32 = 1;
/// `doc/16` §7.3: unknown fields error, file size limit 256KB.
pub const CI_CONFIG_MAX_BYTES: u64 = 256 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CiFile {
    pub version: u32,
    /// argv prefix; final command = `runner.argv + job.argv`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runner: Option<Runner>,
    /// Profile name → job ids, executed in order.
    pub profiles: std::collections::BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub jobs: Vec<Job>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Runner {
    pub argv: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Job {
    pub id: String,
    /// Never passed through a shell: argv array only.
    pub argv: Vec<String>,
    /// Relative to the repo root; must not escape it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// Ordering dependencies only; v1 has no parallelism.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub needs: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fix_argv: Option<Vec<String>>,
    /// `--fix` allowed only when explicitly true; requires `fix_argv`.
    #[serde(default)]
    pub fix_safe: bool,
    pub timeout_s: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required_tools: Vec<String>,
    /// Variable names to pass through — never values, never secrets.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub env_pass: Vec<String>,
    /// Evidence marker only; v1 does not authorize server skipping.
    #[serde(default)]
    pub local_sufficient: bool,
}

pub fn ci_path(root: &Path) -> PathBuf {
    root.join(CI_CONFIG_NAME)
}

/// Read, size-check, parse and validate `.loom.ci.toml`.
///
/// `Ok(None)` means "no configuration yet"; parse/validation problems are
/// `Err` with a human-actionable message (`doc/16` §7.3 exit code 4).
pub fn load_ci_file(root: &Path) -> Result<Option<CiFile>, String> {
    let path = ci_path(root);
    let meta = match fs::metadata(&path) {
        Ok(meta) => meta,
        Err(_) => return Ok(None),
    };
    if meta.len() > CI_CONFIG_MAX_BYTES {
        return Err(format!(
            "{CI_CONFIG_NAME} is {} bytes (limit {CI_CONFIG_MAX_BYTES})",
            meta.len()
        ));
    }
    let text =
        fs::read_to_string(&path).map_err(|error| format!("read {CI_CONFIG_NAME}: {error}"))?;
    let file: CiFile =
        toml::from_str(&text).map_err(|error| format!("parse {CI_CONFIG_NAME}: {error}"))?;
    file.validate()?;
    Ok(Some(file))
}

impl CiFile {
    /// Every rule from `doc/16` §7.3, in order, so one message names one
    /// problem.
    pub fn validate(&self) -> Result<(), String> {
        if self.version != CI_VERSION {
            return Err(format!(
                "version must be {CI_VERSION}, got {}",
                self.version
            ));
        }

        let mut seen = std::collections::HashSet::new();
        for job in &self.jobs {
            if job.id.trim().is_empty() {
                return Err("job id must not be empty".to_string());
            }
            if !seen.insert(job.id.as_str()) {
                return Err(format!("duplicate job id: {}", job.id));
            }
        }

        let by_id: std::collections::HashMap<&str, &Job> =
            self.jobs.iter().map(|job| (job.id.as_str(), job)).collect();

        for job in &self.jobs {
            check_argv(&job.argv, &format!("job {}", job.id))?;
            if let Some(fix) = &job.fix_argv {
                check_argv(fix, &format!("job {} fix_argv", job.id))?;
            }
            if job.fix_safe && job.fix_argv.is_none() {
                return Err(format!("job {}: fix_safe requires fix_argv", job.id));
            }
            if !(1..=3600).contains(&job.timeout_s) {
                return Err(format!(
                    "job {}: timeout_s must be 1..=3600, got {}",
                    job.id, job.timeout_s
                ));
            }
            if let Some(cwd) = &job.cwd {
                check_cwd(cwd, &job.id)?;
            }
            for need in &job.needs {
                if !by_id.contains_key(need.as_str()) {
                    return Err(format!("job {} needs unknown job {need}", job.id));
                }
            }
            for name in &job.env_pass {
                check_env_pass(name, &job.id)?;
            }
        }

        for (profile, jobs) in &self.profiles {
            for job in jobs {
                if !by_id.contains_key(job.as_str()) {
                    return Err(format!("profile {profile} references unknown job {job}"));
                }
            }
        }

        self.check_needs_acyclic(&by_id)?;
        Ok(())
    }

    /// `needs` is ordering-only in v1, but a cycle would silently drop work.
    fn check_needs_acyclic(
        &self,
        by_id: &std::collections::HashMap<&str, &Job>,
    ) -> Result<(), String> {
        #[derive(PartialEq)]
        enum State {
            Open,
            Done,
        }
        let mut state: std::collections::HashMap<&str, State> = std::collections::HashMap::new();

        fn visit<'a>(
            id: &'a str,
            by_id: &std::collections::HashMap<&'a str, &'a Job>,
            state: &mut std::collections::HashMap<&'a str, State>,
        ) -> Result<(), String> {
            match state.get(id) {
                Some(State::Done) => return Ok(()),
                Some(State::Open) => return Err(format!("needs cycle through job {id}")),
                None => {}
            }
            state.insert(id, State::Open);
            if let Some(job) = by_id.get(id) {
                for need in &job.needs {
                    visit(need, by_id, state)?;
                }
            }
            state.insert(id, State::Done);
            Ok(())
        }

        for job in &self.jobs {
            visit(&job.id, by_id, &mut state)?;
        }
        Ok(())
    }
}

fn check_argv(argv: &[String], what: &str) -> Result<(), String> {
    if argv.is_empty() {
        return Err(format!("{what}: argv must not be empty"));
    }
    if argv[0].trim().is_empty() {
        return Err(format!("{what}: argv[0] must not be empty"));
    }
    Ok(())
}

/// `cwd` stays inside the repo root: relative only, no `..` hop.
fn check_cwd(cwd: &str, job_id: &str) -> Result<(), String> {
    let path = Path::new(cwd);
    if path.is_absolute() {
        return Err(format!(
            "job {job_id}: cwd must be relative to the repo root"
        ));
    }
    if path
        .components()
        .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err(format!("job {job_id}: cwd must not escape the repo root"));
    }
    Ok(())
}

fn check_env_pass(name: &str, job_id: &str) -> Result<(), String> {
    let pattern = regex::Regex::new(r"^[A-Za-z_][A-Za-z0-9_]*$").expect("static env_pass pattern");
    if !pattern.is_match(name) {
        return Err(format!(
            "job {job_id}: env_pass name {name:?} must match ^[A-Za-z_][A-Za-z0-9_]*$"
        ));
    }
    for denied in ["_API_KEY", "_TOKEN", "_SECRET", "_PASSWORD"] {
        if name.ends_with(denied) {
            return Err(format!(
                "job {job_id}: env_pass name {name:?} matches a denied secret pattern ({denied})"
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn good() -> CiFile {
        CiFile {
            version: 1,
            runner: Some(Runner {
                argv: vec!["./scripts/dev.sh".into()],
            }),
            profiles: BTreeMap::from([("fast".to_string(), vec!["clippy".to_string()])]),
            jobs: vec![
                Job {
                    id: "clippy".into(),
                    argv: vec!["cargo".into(), "clippy".into()],
                    cwd: None,
                    needs: Vec::new(),
                    fix_argv: None,
                    fix_safe: false,
                    timeout_s: 600,
                    required_tools: Vec::new(),
                    env_pass: Vec::new(),
                    local_sufficient: true,
                },
                Job {
                    id: "fmt".into(),
                    argv: vec!["cargo".into(), "fmt".into()],
                    cwd: None,
                    needs: vec!["clippy".into()],
                    fix_argv: Some(vec!["cargo".into(), "fmt".into(), "--all".into()]),
                    fix_safe: true,
                    timeout_s: 120,
                    required_tools: Vec::new(),
                    env_pass: Vec::new(),
                    local_sufficient: false,
                },
            ],
        }
    }

    fn root() -> tempfile::TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    #[test]
    fn accepts_the_locked_shape() {
        good().validate().expect("valid config");
    }

    #[test]
    fn rejects_wrong_version_and_duplicates() {
        let mut file = good();
        file.version = 2;
        assert!(file.validate().unwrap_err().contains("version"));

        let mut file = good();
        file.jobs[1].id = "clippy".into();
        assert!(file.validate().unwrap_err().contains("duplicate job id"));
    }

    #[test]
    fn rejects_unknown_refs_and_cycles() {
        let mut file = good();
        file.profiles.insert("fast".into(), vec!["missing".into()]);
        assert!(file.validate().unwrap_err().contains("unknown job missing"));

        let mut file = good();
        file.jobs[0].needs = vec!["fmt".into()]; // clippy needs fmt, fmt needs clippy
        assert!(file.validate().unwrap_err().contains("cycle"));
    }

    #[test]
    fn rejects_escaping_cwd_and_bad_timeouts() {
        let mut file = good();
        file.jobs[0].cwd = Some("../outside".into());
        assert!(file.validate().unwrap_err().contains("escape"));

        let mut file = good();
        file.jobs[0].cwd = Some("/etc".into());
        assert!(file.validate().unwrap_err().contains("relative"));

        let mut file = good();
        file.jobs[0].timeout_s = 0;
        assert!(file.validate().unwrap_err().contains("timeout_s"));
        let mut file = good();
        file.jobs[0].timeout_s = 3601;
        assert!(file.validate().is_err());
    }

    #[test]
    fn rejects_fix_safe_without_fix_argv() {
        let mut file = good();
        file.jobs[1].fix_argv = None; // fix_safe stays true
        assert!(file
            .validate()
            .unwrap_err()
            .contains("fix_safe requires fix_argv"));
    }

    #[test]
    fn rejects_bad_env_pass_names() {
        let mut file = good();
        file.jobs[0].env_pass = vec!["MY_API_KEY".into()];
        assert!(file
            .validate()
            .unwrap_err()
            .contains("denied secret pattern"));

        let mut file = good();
        file.jobs[0].env_pass = vec!["9BAD".into()];
        assert!(file.validate().is_err());
    }

    #[test]
    fn unknown_fields_error_directly() {
        let text = r#"
version = 1
[profiles]
fast = ["a"]
kind = "mystery"
"#;
        assert!(toml::from_str::<CiFile>(text).is_err());
        let text = r#"
version = 1
[profiles]
fast = ["a"]
[[jobs]]
id = "a"
argv = ["true"]
timeout_s = 10
network = true
"#;
        assert!(toml::from_str::<CiFile>(text).is_err());
    }

    #[test]
    fn load_enforces_the_size_limit_and_absence() {
        let dir = root();
        assert!(load_ci_file(dir.path()).expect("absent").is_none());

        fs::write(ci_path(dir.path()), "version = 1\n[profiles]\n").expect("write");
        // Empty profiles parses; validation is per-field, not per-shape here.
        let parsed = load_ci_file(dir.path()).expect("parse").expect("present");
        assert_eq!(parsed.version, 1);

        let big = "x".repeat((CI_CONFIG_MAX_BYTES + 1) as usize);
        fs::write(ci_path(dir.path()), &big).expect("write");
        assert!(load_ci_file(dir.path()).unwrap_err().contains("limit"));
    }

    #[test]
    fn roundtrips_through_toml() {
        let file = good();
        let text = toml::to_string_pretty(&file).expect("serialize");
        assert!(text.contains("[[jobs]]"), "jobs serialize as tables");
        assert!(text.contains("[profiles]"), "profiles as a table");
        let back: CiFile = toml::from_str(&text).expect("parse back");
        back.validate().expect("valid after roundtrip");
        assert_eq!(back.jobs.len(), 2);
        assert!(back.jobs[1].fix_safe);
    }
}
