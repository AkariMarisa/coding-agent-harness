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

    pub fn load_config(&self) -> Result<Config> {
        Config::load(&self.config_path()).map_err(Into::into)
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

        let goal = self.loom_dir().join("goal.md");
        if !goal.exists() {
            fs::write(
                &goal,
                "# Goal\n\n<!-- 人写：项目目标、范围、非目标。orchestrator 只读。 -->\n",
            )
            .with_context(|| "write goal.md")?;
            report.created_files.push(goal);
        } else {
            report.skipped_files.push(goal);
        }

        let constraints = self.loom_dir().join("constraints.md");
        if !constraints.exists() {
            fs::write(
                &constraints,
                "# Constraints\n\n<!-- 人写：技术约束、禁止事项、验收偏好。 -->\n",
            )
            .with_context(|| "write constraints.md")?;
            report.created_files.push(constraints);
        } else {
            report.skipped_files.push(constraints);
        }

        report.gitignore_updated = self.ensure_gitignore()?;
        Ok(report)
    }

    fn ensure_gitignore(&self) -> Result<bool> {
        if !self.root.join(".git").exists() {
            return Ok(false);
        }

        let path = self.root.join(".gitignore");
        let mut text = if path.exists() {
            fs::read_to_string(&path).with_context(|| "read .gitignore")?
        } else {
            String::new()
        };

        let mut changed = false;
        for entry in [".loom/", ".worktrees/"] {
            if !text.lines().any(|line| line.trim() == entry) {
                if !text.is_empty() && !text.ends_with('\n') {
                    text.push('\n');
                }
                text.push_str(entry);
                text.push('\n');
                changed = true;
            }
        }

        if changed {
            fs::write(&path, text).with_context(|| "write .gitignore")?;
        }
        Ok(changed)
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
        assert!(project.loom_dir().join("goal.md").exists());
        assert!(project.loom_dir().join("runs").is_dir());
    }
}
