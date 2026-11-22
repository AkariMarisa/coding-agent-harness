//! git hooks for local CI (`doc/16` §7.7).
//!
//! Installed only explicitly, marked with `# loom-managed`, and removed only
//! when marked: a foreign hook is refused by default and `--force` backs it
//! up first. Generated hooks call the public CLI only — never inline
//! business commands — and fail open when `loom` is missing (a fresh clone
//! must still be able to commit), unless `hooks_fail_open = false`.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

/// Marker every generated hook carries; also the remove/status key.
pub const MANAGED_MARKER: &str = "# loom-managed";
/// The hooks v1 knows how to install (`hook name == profile name`).
pub const INSTALLABLE: [&str; 2] = ["pre-commit", "pre-push"];

#[derive(Debug, Default)]
pub struct InstallReport {
    pub installed: Vec<String>,
    /// Managed hooks rewritten in place (idempotent refresh).
    pub refreshed: Vec<String>,
    pub backed_up: Vec<(String, PathBuf)>,
    /// Foreign hooks left untouched because `--force` was not given.
    pub refused: Vec<String>,
    pub messages: Vec<String>,
}

#[derive(Debug, Default)]
pub struct RemoveReport {
    pub removed: Vec<String>,
    pub kept_foreign: Vec<String>,
    pub messages: Vec<String>,
}

/// Install (or refresh) one hook per named profile under the repo's hooks
/// directory. Unknown names are a usage error; foreign hooks are refused
/// unless `force`, which backs up `<hook>.bak.<ts>` first.
pub fn install(
    root: &Path,
    profiles: &[String],
    force: bool,
    fail_open: bool,
) -> Result<InstallReport> {
    for profile in profiles {
        if !INSTALLABLE.contains(&profile.as_str()) {
            bail!(
                "unknown hook {profile:?} — v1 installs only {}",
                INSTALLABLE.join(", ")
            );
        }
    }
    let hooks_dir = hooks_dir(root)?;
    let mut report = InstallReport::default();

    for profile in profiles {
        let path = hooks_dir.join(profile);
        if path.exists() {
            let existing =
                fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
            if !existing.contains(MANAGED_MARKER) {
                if !force {
                    report.refused.push(profile.clone());
                    report.messages.push(format!(
                        "refused: {} exists and is not Loom-managed; rerun with --force to back it up and replace",
                        path.display()
                    ));
                    continue;
                }
                let stamp = now_stamp();
                let backup = path.with_extension(format!("{stamp}.bak"));
                fs::copy(&path, &backup).with_context(|| format!("backup {}", path.display()))?;
                report.backed_up.push((profile.clone(), backup.clone()));
                report
                    .messages
                    .push(format!("backed up foreign hook to {}", backup.display()));
            } else {
                report.refreshed.push(profile.clone());
            }
        }

        let script = hook_script(profile, fail_open);
        fs::write(&path, script).with_context(|| format!("write {}", path.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mut permissions = fs::metadata(&path)
                .with_context(|| format!("stat {}", path.display()))?
                .permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&path, permissions)
                .with_context(|| format!("chmod {}", path.display()))?;
        }
        report.installed.push(profile.clone());
        report
            .messages
            .push(format!("installed {} ({profile} profile)", path.display()));
    }
    Ok(report)
}

/// Remove every managed hook. A foreign hook is never deleted — it is
/// reported and kept (recoverable misuse lives with the user, AGENTS §10).
pub fn remove(root: &Path) -> Result<RemoveReport> {
    let hooks_dir = hooks_dir(root)?;
    let mut report = RemoveReport::default();
    for name in INSTALLABLE {
        let path = hooks_dir.join(name);
        let Ok(existing) = fs::read_to_string(&path) else {
            continue;
        };
        if existing.contains(MANAGED_MARKER) {
            fs::remove_file(&path).with_context(|| format!("remove {}", path.display()))?;
            report.removed.push(name.to_string());
            report.messages.push(format!("removed {}", path.display()));
        } else {
            report.kept_foreign.push(name.to_string());
            report
                .messages
                .push(format!("kept {} (not Loom-managed)", path.display()));
        }
    }
    Ok(report)
}

/// `(hook, state)` for `loom ci doctor`: `managed` / `foreign` / `absent`,
/// or `unknown` when the directory cannot be located (not a git repo).
pub fn hook_status(root: &Path) -> Vec<(String, &'static str)> {
    let Ok(hooks_dir) = hooks_dir(root) else {
        return INSTALLABLE
            .iter()
            .map(|name| (name.to_string(), "unknown"))
            .collect();
    };
    INSTALLABLE
        .iter()
        .map(|name| {
            let state = match fs::read_to_string(hooks_dir.join(name)) {
                Err(_) => "absent",
                Ok(content) if content.contains(MANAGED_MARKER) => "managed",
                Ok(_) => "foreign",
            };
            (name.to_string(), state)
        })
        .collect()
}

/// `git rev-parse --git-path hooks`, resolved against the repo root so
/// linked worktrees install into the shared hooks directory.
fn hooks_dir(root: &Path) -> Result<PathBuf> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rev-parse", "--git-path", "hooks"])
        .output()
        .context("git rev-parse --git-path hooks")?;
    if !output.status.success() {
        bail!(
            "not a git repository (or git failed): {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let printed = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let path = PathBuf::from(&printed);
    Ok(if path.is_absolute() {
        path
    } else {
        root.join(path)
    })
}

fn now_stamp() -> String {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0);
    loom_core::compact_utc(millis)
}

/// The hook body: skip switches first, fail-open on a missing `loom`, then
/// one public CLI call — the profile lives in `.loom.ci.toml`, never here.
pub fn hook_script(profile: &str, fail_open: bool) -> String {
    let missing_branch = if fail_open {
        format!(
            "echo \"loom: warning: loom not on PATH; {profile} skipped (fail-open, not evidence)\" >&2\n    exit 0"
        )
    } else {
        format!(
            "echo \"loom: error: loom not on PATH; {profile} blocked (hooks_fail_open = false)\" >&2\n    exit 1"
        )
    };
    format!(
        r#"#!/bin/sh
{MANAGED_MARKER}
# Installed by `loom ci install-hooks` for the `{profile}` profile.
# Do not edit: change `.loom.ci.toml`, then reinstall.

set -u

if [ "${{LOOM_CI_SKIP:-}}" = "1" ]; then
    echo "loom: LOOM_CI_SKIP=1 — {profile} checks skipped (local diagnostics only, not evidence)" >&2
    exit 0
fi

if ! command -v loom >/dev/null 2>&1; then
    {missing_branch}
fi

exec loom ci run --profile {profile} --from-hook
"#,
        MANAGED_MARKER = MANAGED_MARKER,
        profile = profile,
        missing_branch = missing_branch,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("tempdir");
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(dir.path())
            .arg("init")
            .arg("-q")
            .status()
            .expect("git init");
        assert!(status.success());
        let hooks = hooks_dir(dir.path()).expect("hooks dir");
        (dir, hooks)
    }

    #[test]
    fn installs_a_marked_script_that_only_calls_the_cli() {
        let (dir, hooks) = repo();
        let report =
            install(dir.path(), &["pre-commit".to_string()], false, true).expect("install");
        assert_eq!(report.installed, vec!["pre-commit"]);
        assert!(report.refused.is_empty());

        let script = fs::read_to_string(hooks.join("pre-commit")).expect("read");
        assert!(script.contains(MANAGED_MARKER));
        assert!(
            script.contains("exec loom ci run --profile pre-commit --from-hook"),
            "a hook only invokes the public CLI"
        );
        assert!(!script.contains("cargo"), "no inlined business commands");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = fs::metadata(hooks.join("pre-commit"))
                .expect("stat")
                .permissions()
                .mode();
            assert_ne!(mode & 0o111, 0, "hook must be executable");
        }
    }

    #[test]
    fn reinstall_is_idempotent_and_unknown_hooks_are_usage_errors() {
        let (dir, hooks) = repo();
        let profiles = vec!["pre-commit".to_string()];
        install(dir.path(), &profiles, false, true).expect("first");
        let report = install(dir.path(), &profiles, false, true).expect("second");
        assert_eq!(
            report.refreshed,
            vec!["pre-commit"],
            "managed refresh, not refusal"
        );
        assert!(report.refused.is_empty());
        assert!(hooks.join("pre-commit").exists());

        assert!(
            install(dir.path(), &["commit-msg".to_string()], false, true).is_err(),
            "v1 installs only the two known hooks"
        );
    }

    #[test]
    fn foreign_hooks_are_refused_then_backed_up_with_force() {
        let (dir, hooks) = repo();
        fs::write(hooks.join("pre-push"), "#!/bin/sh\nexit 0\n").expect("foreign");

        let report = install(dir.path(), &["pre-push".to_string()], false, true).expect("refuse");
        assert_eq!(report.refused, vec!["pre-push".to_string()]);
        assert!(
            !fs::read_to_string(hooks.join("pre-push"))
                .expect("read")
                .contains(MANAGED_MARKER),
            "foreign content untouched"
        );

        let report = install(dir.path(), &["pre-push".to_string()], true, true).expect("force");
        assert!(report.backed_up.len() == 1, "backup made");
        let (_, backup) = &report.backed_up[0];
        assert_eq!(
            fs::read_to_string(backup).expect("backup content"),
            "#!/bin/sh\nexit 0\n",
            "backup preserves the original"
        );
        assert!(fs::read_to_string(hooks.join("pre-push"))
            .expect("read")
            .contains(MANAGED_MARKER));
    }

    #[test]
    fn remove_takes_managed_hooks_only() {
        let (dir, hooks) = repo();
        install(dir.path(), &["pre-commit".to_string()], false, true).expect("install");
        fs::write(hooks.join("pre-push"), "#!/bin/sh\nexit 0\n").expect("foreign");

        let report = remove(dir.path()).expect("remove");
        assert_eq!(report.removed, vec!["pre-commit"]);
        assert_eq!(report.kept_foreign, vec!["pre-push"]);
        assert!(!hooks.join("pre-commit").exists());
        assert!(
            hooks.join("pre-push").exists(),
            "never delete a foreign hook"
        );

        let report = remove(dir.path()).expect("idempotent");
        assert!(report.removed.is_empty() && report.kept_foreign.len() == 1);
    }

    #[test]
    fn status_distinguishes_managed_foreign_and_absent() {
        let (dir, hooks) = repo();
        install(dir.path(), &["pre-commit".to_string()], false, true).expect("install");
        fs::write(hooks.join("pre-push"), "#!/bin/sh\n").expect("foreign");
        let status = hook_status(dir.path());
        assert_eq!(
            status,
            vec![
                ("pre-commit".to_string(), "managed"),
                ("pre-push".to_string(), "foreign"),
            ]
        );
    }

    #[test]
    fn fail_open_switches_the_missing_loom_branch() {
        let open = hook_script("pre-commit", true);
        assert!(open.contains("fail-open"));
        assert!(open.contains("exit 0"));
        let closed = hook_script("pre-commit", false);
        assert!(closed.contains("hooks_fail_open = false"));
        assert!(closed.contains("exit 1"));
        for script in [open, closed] {
            assert!(script.contains("LOOM_CI_SKIP"));
        }
    }

    #[test]
    fn hooks_dir_is_rejected_outside_a_repo() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(hooks_dir(dir.path()).is_err());
        let status = hook_status(dir.path());
        assert!(status.iter().all(|(_, state)| *state == "unknown"));
    }
}
