use std::fs;
use std::path::{Component, Path, PathBuf};

use anyhow::{bail, Context, Result};
use loom_core::Config;

#[derive(Debug, Clone)]
pub struct Project {
    pub root: PathBuf,
}

#[derive(Debug, Clone, Default)]
pub struct InitReport {
    pub created_dirs: Vec<PathBuf>,
    pub created_files: Vec<PathBuf>,
    pub skipped_files: Vec<PathBuf>,
    pub gitignore_updated: bool,
}

/// Orientation file for the tracked contract layer. Users are free to delete
/// it; `loom init` recreates it on the next run.
pub const LOOM_README: &str = "\
# .loom/ — Loom 的工作目录

两层结构（根 `.gitignore` 白名单：`/.loom/*` + 反选，新增状态默认忽略）：

| 层 | 路径 | git |
|---|---|---|
| 契约层 | `README.md`、`goal.md`、`constraints.md`、`specs/` | tracked |
| 状态层 | `tasks/`、`runs/`、`ledger/`、`snapshots/`、`locks/`、`config.toml`、`config.local.toml`、`events.jsonl` 等其余全部 | ignored |

- 契约层：人与控制面共同维护，走 git review；`loom goal` / `loom constraints` 起草，人审 diff；模块契约放 `specs/`。
- 状态层：orchestrator 唯一写者；事件日志 append-only，不要手改事实源。
- 本地实验配置写 `config.local.toml`（深合并覆盖 `config.toml`，`[[models]]` 按 alias 合并；永不提交，测试不碰主文件）。
- ⚠️ `rm -rf .loom/` 会连契约层一起删（tracked 可从 git 恢复），状态层不可恢复。
- 分层规则见仓库 `AGENTS.md` §6。
";

/// Root `.gitignore` block. It must live at the root and must exclude
/// *children* (`/.loom/*`), not the directory itself: once `.loom/` is
/// excluded, git never descends and every `!` negation inside becomes inert.
const GITIGNORE_WHITELIST: &str = "\
# loom: contract layer tracked, state layer ignored (whitelist)
/.loom/*
!/.loom/README.md
!/.loom/goal.md
!/.loom/constraints.md
!/.loom/specs/";

impl Project {
    pub fn at(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        let root = root.canonicalize().unwrap_or(root);
        Self { root }
    }

    /// Walk up from `start` looking for `.loom/` or `.git/`.
    pub fn discover(start: &Path) -> Result<Self> {
        let start = start.canonicalize().unwrap_or_else(|_| start.to_path_buf());
        let mut current = start.as_path();

        loop {
            if current.join(".loom").is_dir() || current.join(".git").exists() {
                return Ok(Self {
                    root: current.to_path_buf(),
                });
            }
            match current.parent() {
                Some(parent) => current = parent,
                None => break,
            }
        }

        Ok(Self { root: start })
    }

    pub fn loom_dir(&self) -> PathBuf {
        self.root.join(".loom")
    }

    pub fn config_path(&self) -> PathBuf {
        self.loom_dir().join("config.toml")
    }

    /// Local experiment overlay: deep-merged over `config.toml`, never
    /// committed (it stays in the ignored state layer).
    pub fn config_local_path(&self) -> PathBuf {
        self.loom_dir().join("config.local.toml")
    }

    pub fn load_config(&self) -> Result<Config> {
        Config::load_with_local(&self.config_path(), &self.config_local_path()).map_err(Into::into)
    }

    pub fn ensure_layout(&self) -> Result<()> {
        for dir in ["tasks", "runs", "ledger", "snapshots", "locks"] {
            fs::create_dir_all(self.loom_dir().join(dir))
                .with_context(|| format!("create .loom/{dir}"))?;
        }
        Ok(())
    }

    pub fn init(&self, force: bool) -> Result<InitReport> {
        let mut report = InitReport::default();
        self.ensure_layout()?;

        for dir in ["tasks", "runs", "ledger", "snapshots", "locks"] {
            report.created_dirs.push(self.loom_dir().join(dir));
        }

        let config = Config::default();
        let config_path = self.config_path();
        if force || !config_path.exists() {
            fs::write(&config_path, config.to_toml()?).with_context(|| "write config.toml")?;
            report.created_files.push(config_path);
        } else {
            report.skipped_files.push(config_path);
        }

        // No fill-in placeholders: `loom goal` / `loom constraints` draft these
        // with a model and the human reviews the diff (AGENTS.md §10).
        let readme = self.loom_dir().join("README.md");
        if !readme.exists() {
            fs::write(&readme, LOOM_README).with_context(|| "write .loom/README.md")?;
            report.created_files.push(readme);
        } else {
            report.skipped_files.push(readme);
        }

        report.gitignore_updated = self.ensure_gitignore()?;
        Ok(report)
    }

    /// Migrate legacy whole-dir `.loom/` rules to the whitelist block and
    /// keep `.worktrees/` ignored. Idempotent: a second run reports no change.
    fn ensure_gitignore(&self) -> Result<bool> {
        if !self.root.join(".git").exists() {
            return Ok(false);
        }

        let path = self.root.join(".gitignore");
        let text = if path.exists() {
            fs::read_to_string(&path).with_context(|| "read .gitignore")?
        } else {
            String::new()
        };

        let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
        // A whole-dir rule (with or without leading slash) makes git skip
        // `.loom/` entirely, so every `!` negation below it is inert: remove
        // it first, whether or not the whitelist block is already present.
        lines.retain(|line| !matches!(line.trim(), ".loom/" | ".loom" | "/.loom/" | "/.loom"));
        if !lines.iter().any(|line| line.trim() == "/.loom/*") {
            if lines.iter().any(|line| !line.trim().is_empty()) {
                lines.push(String::new());
            }
            lines.extend(GITIGNORE_WHITELIST.lines().map(String::from));
        }
        if !lines.iter().any(|line| line.trim() == ".worktrees/") {
            lines.push(".worktrees/".to_string());
        }

        let new_text = lines.join("\n") + "\n";
        if new_text == text {
            return Ok(false);
        }
        fs::write(&path, new_text).with_context(|| "write .gitignore")?;
        Ok(true)
    }

    /// Resolve a repository-relative path, rejecting traversal and symlink escapes.
    pub fn resolve_path(&self, relative: &str) -> Result<PathBuf> {
        if relative.trim().is_empty() {
            bail!("path is empty");
        }

        let path = Path::new(relative);
        if path.is_absolute() {
            bail!("absolute paths are not allowed");
        }
        if path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        }) {
            bail!("path traversal is not allowed");
        }

        let root = self
            .root
            .canonicalize()
            .unwrap_or_else(|_| self.root.clone());
        let joined = root.join(path);
        let resolved = joined
            .canonicalize()
            .with_context(|| format!("resolve {relative}"))?;

        if !resolved.starts_with(&root) {
            bail!("path escapes project root");
        }
        Ok(resolved)
    }

    pub fn relative(&self, path: &Path) -> String {
        path.strip_prefix(&self.root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/")
    }

    pub fn git_head(&self) -> Option<String> {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(&self.root)
            .args(["rev-parse", "--short", "HEAD"])
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let head = String::from_utf8_lossy(&output.stdout).trim().to_string();
        (!head.is_empty()).then_some(head)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_traversal() {
        let dir = tempfile::tempdir().expect("tempdir");
        let project = Project::at(dir.path());
        assert!(project.resolve_path("../etc/passwd").is_err());
        assert!(project.resolve_path("/etc/passwd").is_err());
    }

    #[test]
    fn init_creates_layout() {
        let dir = tempfile::tempdir().expect("tempdir");
        let project = Project::at(dir.path());
        project.init(false).expect("init");

        assert!(project.loom_dir().join("config.toml").exists());
        assert!(project.loom_dir().join("README.md").exists());
        assert!(project.loom_dir().join("runs").is_dir());
        // No fill-in placeholders: `loom goal` drafts these on demand.
        assert!(!project.loom_dir().join("goal.md").exists());
        assert!(!project.loom_dir().join("constraints.md").exists());
    }

    #[test]
    fn init_writes_gitignore_whitelist() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::create_dir(dir.path().join(".git")).expect("fake .git");
        let project = Project::at(dir.path());
        project.init(false).expect("init");

        let text = fs::read_to_string(dir.path().join(".gitignore")).expect("gitignore");
        assert!(text.lines().any(|line| line.trim() == "/.loom/*"));
        assert!(text.lines().any(|line| line.trim() == "!/.loom/specs/"));
        assert!(text.lines().any(|line| line.trim() == ".worktrees/"));
        assert!(!text.lines().any(|line| line.trim() == ".loom/"));

        // Second run is a no-op.
        let again = project.init(false).expect("init");
        assert!(!again.gitignore_updated);
    }

    #[test]
    fn init_migrates_legacy_gitignore() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::create_dir(dir.path().join(".git")).expect("fake .git");
        fs::write(
            dir.path().join(".gitignore"),
            "target/\n/.loom/\n.worktrees/\n",
        )
        .expect("write");

        let project = Project::at(dir.path());
        let report = project.init(false).expect("init");
        assert!(report.gitignore_updated);

        let text = fs::read_to_string(dir.path().join(".gitignore")).expect("gitignore");
        assert!(text.lines().any(|line| line.trim() == "target/"));
        assert!(!text.lines().any(|line| line.trim() == "/.loom/"));
        assert!(text.lines().any(|line| line.trim() == "/.loom/*"));
        // `.worktrees/` must not be duplicated.
        assert_eq!(
            text.lines()
                .filter(|line| line.trim() == ".worktrees/")
                .count(),
            1
        );
    }

    #[test]
    fn load_config_applies_local_overlay() {
        let dir = tempfile::tempdir().expect("tempdir");
        let project = Project::at(dir.path());
        project.init(false).expect("init");
        fs::write(
            project.config_path(),
            "[[models]]\nalias = \"base\"\nmodel_id = \"m1\"\ntier = \"worker\"\n",
        )
        .expect("write base");
        fs::write(
            project.config_local_path(),
            "[[models]]\nalias = \"base\"\nmodel_id = \"m2\"\ntier = \"worker\"\n",
        )
        .expect("write local");

        let config = project.load_config().expect("load");
        assert_eq!(config.models.len(), 1);
        assert_eq!(config.models[0].model_id.as_deref(), Some("m2"));
        // The overlay lives in the ignored state layer.
        assert!(project.config_local_path().is_file());
    }

    #[test]
    fn init_removes_residual_legacy_rule() {
        // A previous migration appended the block but left the shadowing
        // whole-dir rule in place; the negations were inert until this runs.
        let dir = tempfile::tempdir().expect("tempdir");
        fs::create_dir(dir.path().join(".git")).expect("fake .git");
        fs::write(
            dir.path().join(".gitignore"),
            "/.loom/\n# loom: contract layer tracked, state layer ignored (whitelist)\n/.loom/*\n!/.loom/README.md\n",
        )
        .expect("write");

        let project = Project::at(dir.path());
        let report = project.init(false).expect("init");
        assert!(report.gitignore_updated);

        let text = fs::read_to_string(dir.path().join(".gitignore")).expect("gitignore");
        assert!(!text.lines().any(|line| line.trim() == "/.loom/"));
        assert!(text.lines().any(|line| line.trim() == "/.loom/*"));
    }
}
