//! Bounded local tools for the native executor.
//!
//! Every result is projected to a short observation. Output that exceeds the
//! observation budget is written to an artifact and referenced by path.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{bail, Context as _, Result};
use ignore::WalkBuilder;
use loom_core::{parse_range, truncate_bytes, TaskSpec};
use regex::RegexBuilder;
use serde_json::Value;

const OBSERVATION_BYTES: usize = 8 * 1024;
const MAX_READ_LINES: usize = 400;
const MAX_READ_BYTES: usize = 48 * 1024;
const MAX_SEARCH_RESULTS: usize = 80;
const MAX_BASH_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone)]
pub struct ToolResult {
    pub observation: String,
    /// True when the tool changed the worktree (used by the no-progress guard).
    pub mutated: bool,
}

pub struct ToolBox {
    root: PathBuf,
    spec: TaskSpec,
    artifact_dir: PathBuf,
    step: u32,
}

impl ToolBox {
    pub fn new(root: impl Into<PathBuf>, spec: TaskSpec, artifact_dir: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            spec,
            artifact_dir: artifact_dir.into(),
            step: 0,
        }
    }

    pub fn set_step(&mut self, step: u32) {
        self.step = step;
    }

    pub fn execute(&self, name: &str, arguments: &str) -> Result<ToolResult> {
        let args: Value = if arguments.trim().is_empty() {
            Value::Object(Default::default())
        } else {
            serde_json::from_str(arguments)
                .with_context(|| format!("tool {name} arguments are not valid JSON"))?
        };
        match name {
            "read" => self.read(&args),
            "search" => self.search(&args),
            "edit" => self.edit(&args),
            "write" => self.write(&args),
            "bash" => self.bash(&args),
            other => bail!("unknown tool: {other}"),
        }
    }

    fn read(&self, args: &Value) -> Result<ToolResult> {
        let path = string_arg(args, "path")?;
        let resolved = self.resolve(&path)?;
        if !resolved.is_file() {
            bail!("not a file: {path}");
        }
        let bytes = fs::read(&resolved).with_context(|| format!("read {path}"))?;
        let text = String::from_utf8_lossy(&bytes);
        let lines: Vec<&str> = text.lines().collect();
        let total = lines.len();

        let (start, end) = match args.get("range").and_then(Value::as_str) {
            Some(range) => parse_range(range).map_err(|message| anyhow::anyhow!(message))?,
            None => (1, total.clamp(1, MAX_READ_LINES)),
        };
        if start > total {
            bail!("range starts after end of file ({total} lines)");
        }
        let end = end.min(total).min(start.saturating_add(MAX_READ_LINES - 1));

        let mut out = String::new();
        let mut used = 0usize;
        for (index, line) in lines.iter().enumerate() {
            let number = index + 1;
            if number < start || number > end {
                continue;
            }
            let rendered = format!("{number:>5}  {line}\n");
            if used + rendered.len() > MAX_READ_BYTES {
                out.push_str(&format!(
                    "... truncated at line {number} (file has {total} lines)\n"
                ));
                break;
            }
            used += rendered.len();
            out.push_str(&rendered);
        }
        if end < total && out.len() < MAX_READ_BYTES {
            out.push_str(&format!(
                "... {total} lines total; continue with range {}:{}\n",
                end + 1,
                (end + MAX_READ_LINES).min(total)
            ));
        }

        Ok(ToolResult {
            observation: format!("{path} (lines {start}-{end}):\n{out}"),
            mutated: false,
        })
    }

    fn search(&self, args: &Value) -> Result<ToolResult> {
        let query = string_arg(args, "query")?;
        let scope = args.get("scope").and_then(Value::as_str).unwrap_or(".");
        let scope_path = self.resolve(scope)?;
        let max_results = args
            .get("max_results")
            .and_then(Value::as_u64)
            .map(|value| value as usize)
            .unwrap_or(MAX_SEARCH_RESULTS)
            .min(MAX_SEARCH_RESULTS);
        let regex = RegexBuilder::new(&query)
            .case_insensitive(true)
            .build()
            .map_err(|error| anyhow::anyhow!("invalid regex: {error}"))?;

        let mut hits = Vec::new();
        let mut builder = WalkBuilder::new(&scope_path);
        builder.hidden(true).git_ignore(true).max_depth(Some(32));
        for entry in builder.build() {
            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => continue,
            };
            if !entry.file_type().is_some_and(|kind| kind.is_file()) {
                continue;
            }
            let path = entry.path();
            let metadata = match fs::metadata(path) {
                Ok(metadata) => metadata,
                Err(_) => continue,
            };
            if metadata.len() > 1_048_576 {
                continue;
            }
            let bytes = match fs::read(path) {
                Ok(bytes) => bytes,
                Err(_) => continue,
            };
            if bytes.contains(&0) {
                continue;
            }
            let text = String::from_utf8_lossy(&bytes);
            let relative = path
                .strip_prefix(&self.root)
                .unwrap_or(path)
                .to_string_lossy()
                .into_owned();
            for (index, line) in text.lines().enumerate() {
                if regex.is_match(line) {
                    let (line, _) = truncate_bytes(line, 200);
                    hits.push(format!("{relative}:{}:{}", index + 1, line.trim_end()));
                    if hits.len() >= max_results {
                        break;
                    }
                }
            }
            if hits.len() >= max_results {
                break;
            }
        }

        if hits.is_empty() {
            return Ok(ToolResult {
                observation: format!("no matches for /{query}/ in {scope}"),
                mutated: false,
            });
        }
        Ok(ToolResult {
            observation: format!(
                "{} match(es) for /{query}/ in {scope}:\n{}",
                hits.len(),
                hits.join("\n")
            ),
            mutated: false,
        })
    }

    fn edit(&self, args: &Value) -> Result<ToolResult> {
        let path = string_arg(args, "path")?;
        self.ensure_editable(&path)?;
        let old = string_arg(args, "old")?;
        let new = string_arg(args, "new")?;
        if old.is_empty() {
            bail!("old text must not be empty; use the write tool to create a file");
        }
        let replace_all = args
            .get("replace_all")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let resolved = self.resolve(&path)?;
        let text = fs::read_to_string(&resolved).with_context(|| format!("read {path}"))?;
        let count = text.matches(&old).count();
        if count == 0 {
            bail!("old text not found in {path}");
        }
        if count > 1 && !replace_all {
            bail!("old text appears {count} times in {path}; pass replace_all=true or add context");
        }
        let updated = if replace_all {
            text.replace(&old, &new)
        } else {
            text.replacen(&old, &new, 1)
        };
        fs::write(&resolved, updated).with_context(|| format!("write {path}"))?;
        Ok(ToolResult {
            observation: format!("edited {path} ({count} match(es) replaced)"),
            mutated: true,
        })
    }

    fn write(&self, args: &Value) -> Result<ToolResult> {
        let path = string_arg(args, "path")?;
        self.ensure_editable(&path)?;
        let content = string_arg(args, "content")?;
        let resolved = self.resolve(&path)?;
        if let Some(parent) = resolved.parent() {
            fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
        }
        fs::write(&resolved, content.as_bytes()).with_context(|| format!("write {path}"))?;
        Ok(ToolResult {
            observation: format!("wrote {path} ({} bytes)", content.len()),
            mutated: true,
        })
    }

    fn bash(&self, args: &Value) -> Result<ToolResult> {
        let cmd = string_arg(args, "cmd")?;
        let timeout_s = args
            .get("timeout_s")
            .and_then(Value::as_u64)
            .unwrap_or(120)
            .clamp(1, 600);

        let artifact = self.artifact_path("bash");
        if let Some(parent) = artifact.parent() {
            fs::create_dir_all(parent)?;
        }
        let before = git_status(&self.root);
        let stdout = fs::File::create(&artifact).with_context(|| "create bash log")?;
        let stderr = stdout.try_clone().with_context(|| "clone bash log")?;

        let mut child = Command::new("sh")
            .arg("-c")
            .arg(&cmd)
            .current_dir(&self.root)
            .env_remove("LOOM_API_KEY")
            .env_remove("OPENAI_API_KEY")
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr))
            .stdin(Stdio::null())
            .spawn()
            .with_context(|| "spawn bash")?;

        let started = Instant::now();
        let mut timed_out = false;
        let status = loop {
            if let Some(status) = child.try_wait()? {
                break Some(status);
            }
            if started.elapsed() >= Duration::from_secs(timeout_s) {
                timed_out = true;
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            std::thread::sleep(Duration::from_millis(25));
        };
        let after = git_status(&self.root);

        let mut bytes = Vec::new();
        fs::File::open(&artifact)?.read_to_end(&mut bytes)?;
        let text = String::from_utf8_lossy(&bytes);
        let (preview, cut) = truncate_bytes(&text, OBSERVATION_BYTES);
        let exit = if timed_out {
            124
        } else {
            status.and_then(|status| status.code()).unwrap_or(-1)
        };
        let mut observation = format!(
            "$ {cmd}\nexit={exit} timed_out={timed_out} duration={:.2}s\n",
            started.elapsed().as_secs_f64()
        );
        observation.push_str(preview);
        if cut {
            observation.push_str(&format!(
                "\n... output truncated; full log: {}",
                artifact.display()
            ));
        }
        if bytes.len() > MAX_BASH_BYTES {
            observation.push_str(&format!(
                "\n... log is {} bytes; inspect with read/bash if needed",
                bytes.len()
            ));
        }

        Ok(ToolResult {
            observation,
            mutated: before != after,
        })
    }

    fn ensure_editable(&self, path: &str) -> Result<()> {
        let relative = normalize(path)?;
        if !self.spec.workspace.allows(&relative) {
            bail!(
                "path {path} is outside the task's allowed_paths {}",
                self.spec.workspace.allowed_paths.join(", ")
            );
        }
        Ok(())
    }

    fn resolve(&self, path: &str) -> Result<PathBuf> {
        let relative = normalize(path)?;
        Ok(self.root.join(relative))
    }

    fn artifact_path(&self, name: &str) -> PathBuf {
        self.artifact_dir
            .join(format!("step-{:03}-{name}.log", self.step))
    }
}

fn normalize(path: &str) -> Result<String> {
    let path = path.trim();
    if path.is_empty() {
        bail!("path must not be empty");
    }
    let path = path.strip_prefix("./").unwrap_or(path);
    if path.starts_with('/') || path.contains('\\') {
        bail!("path must be relative and use forward slashes: {path}");
    }
    if Path::new(path)
        .components()
        .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        bail!("path must not contain ..: {path}");
    }
    Ok(path.to_string())
}

fn git_status(root: &Path) -> String {
    Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["status", "--porcelain"])
        .output()
        .map(|output| String::from_utf8_lossy(&output.stdout).into_owned())
        .unwrap_or_default()
}

fn string_arg(args: &Value, name: &str) -> Result<String> {
    args.get(name)
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .with_context(|| format!("missing string argument: {name}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn toolbox(dir: &Path) -> ToolBox {
        let spec = TaskSpec {
            workspace: loom_core::TaskWorkspace {
                allowed_paths: vec!["src/**".into(), "README.md".into()],
                ..loom_core::TaskWorkspace::default()
            },
            ..TaskSpec::default()
        };
        ToolBox::new(dir, spec, dir.join(".artifacts"))
    }

    #[test]
    fn writes_reads_edits_and_rejects_escape() {
        let dir = tempfile::tempdir().expect("tempdir");
        let tools = toolbox(dir.path());
        tools
            .execute(
                "write",
                &json!({"path": "src/a.txt", "content": "hello\nworld\n"}).to_string(),
            )
            .expect("write");
        let read = tools
            .execute(
                "read",
                &json!({"path": "src/a.txt", "range": "1:1"}).to_string(),
            )
            .expect("read");
        assert!(read.observation.contains("hello"));
        assert!(!read.observation.contains("world"));

        tools
            .execute(
                "edit",
                &json!({"path": "src/a.txt", "old": "world", "new": "there"}).to_string(),
            )
            .expect("edit");
        let text = fs::read_to_string(dir.path().join("src/a.txt")).expect("read");
        assert!(text.contains("there"));

        assert!(tools
            .execute(
                "write",
                &json!({"path": "../escape", "content": "x"}).to_string()
            )
            .is_err());
        assert!(tools
            .execute(
                "write",
                &json!({"path": "src/../escape", "content": "x"}).to_string()
            )
            .is_err());
        assert!(tools
            .execute(
                "write",
                &json!({"path": "other/x", "content": "x"}).to_string()
            )
            .is_err());
    }

    #[test]
    fn searches_with_limits() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::create_dir_all(dir.path().join("src")).expect("mkdir");
        fs::write(dir.path().join("src/a.rs"), "fn refresh() {}\n").expect("write");
        let tools = toolbox(dir.path());
        let result = tools
            .execute(
                "search",
                &json!({"query": "refresh", "scope": "src"}).to_string(),
            )
            .expect("search");
        assert!(result.observation.contains("src/a.rs:1"));
    }

    #[test]
    fn bash_spills_long_output_to_artifact() {
        let dir = tempfile::tempdir().expect("tempdir");
        Command::new("git")
            .args(["init", "-q"])
            .current_dir(dir.path())
            .output()
            .expect("git init");
        let tools = toolbox(dir.path());
        let result = tools
            .execute(
                "bash",
                &json!({"cmd": "mkdir -p src && seq 1 4000 > src/generated.txt && cat src/generated.txt", "timeout_s": 10}).to_string(),
            )
            .expect("bash");
        assert!(result.observation.contains("full log:"));
        assert!(result.mutated);
    }
}
