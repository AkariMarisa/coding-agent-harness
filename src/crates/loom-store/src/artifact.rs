use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};

use crate::Run;

pub fn write_artifact(
    run: &Run,
    tool: &str,
    content: &str,
    args_hash: &str,
    count: usize,
) -> Result<String> {
    fs::create_dir_all(run.artifacts_dir()).with_context(|| "create artifacts dir")?;

    let seq = next_seq(&run.artifacts_dir(), tool)?;
    let name = format!("{tool}-{seq:03}.txt");
    let path = run.artifacts_dir().join(&name);

    let header = serde_json::json!({
        "v": 1,
        "tool": tool,
        "args_hash": args_hash,
        "git": run.project.git_head(),
        "cwd": ".",
        "created_unix": now_unix(),
        "count": count,
        "sha256": crate::sha256_hex(content.as_bytes()),
    });

    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&path)
        .with_context(|| format!("create artifact {}", path.display()))?;

    writeln!(file, "{header}").with_context(|| "write artifact header")?;
    file.write_all(content.as_bytes())
        .with_context(|| "write artifact body")?;
    if !content.ends_with('\n') {
        file.write_all(b"\n").with_context(|| "finish artifact")?;
    }

    Ok(format!("artifacts/{name}"))
}

pub fn read_artifact(run: &Run, artifact_ref: &str) -> Result<String> {
    let path = artifact_path(run, artifact_ref)?;
    let text =
        fs::read_to_string(&path).with_context(|| format!("read artifact {}", path.display()))?;

    let body = text.split_once('\n').map(|(_, body)| body).unwrap_or("");
    Ok(body.to_string())
}

fn artifact_path(run: &Run, artifact_ref: &str) -> Result<PathBuf> {
    let relative = Path::new(artifact_ref);
    if relative.is_absolute()
        || relative
            .components()
            .any(|component| matches!(component, Component::ParentDir | Component::RootDir))
    {
        bail!("invalid artifact reference");
    }
    if relative
        .components()
        .next()
        .and_then(|component| match component {
            Component::Normal(name) => Some(name == "artifacts"),
            _ => None,
        })
        != Some(true)
    {
        bail!("artifact reference must start with artifacts/");
    }

    let path = run.dir().join(relative);
    if !path.exists() {
        bail!("artifact not found: {artifact_ref}");
    }
    Ok(path)
}

fn next_seq(dir: &Path, tool: &str) -> Result<u32> {
    let prefix = format!("{tool}-");
    let mut max = 0u32;

    for entry in fs::read_dir(dir).with_context(|| "scan artifacts")? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let Some(rest) = name.strip_prefix(&prefix) else {
            continue;
        };
        let Some(number) = rest.strip_suffix(".txt") else {
            continue;
        };
        if let Ok(value) = number.parse::<u32>() {
            max = max.max(value);
        }
    }

    Ok(max + 1)
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Project;

    #[test]
    fn writes_and_reads_artifact() {
        let dir = tempfile::tempdir().expect("tempdir");
        let run = Run::open(Project::at(dir.path()), Some("test")).expect("run");
        let artifact = write_artifact(&run, "search", "a\nb\n", "abc", 2).expect("write");
        assert_eq!(artifact, "artifacts/search-001.txt");
        assert_eq!(read_artifact(&run, &artifact).expect("read"), "a\nb\n");
    }

    #[test]
    fn rejects_escape() {
        let dir = tempfile::tempdir().expect("tempdir");
        let run = Run::open(Project::at(dir.path()), Some("test")).expect("run");
        assert!(read_artifact(&run, "../secret").is_err());
        assert!(read_artifact(&run, "artifacts/../../secret").is_err());
    }
}
