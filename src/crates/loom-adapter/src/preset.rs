use std::path::Path;

use crate::types::{AdapterKind, AdapterRequest};

/// A named adapter preset. Commands are examples; pin versions in production.
#[derive(Debug, Clone, Copy)]
pub struct AdapterPreset {
    pub kind: AdapterKind,
    /// Executable the preset runs.
    pub executable: &'static str,
    /// Executable that must exist for availability checks. It may differ from
    /// `executable` when the preset wraps another tool with `sh -c`.
    pub check_executable: &'static str,
    pub description: &'static str,
    pub args: &'static [&'static str],
    /// Feed the packet on stdin instead of an argument.
    pub stdin_packet: bool,
}

pub fn preset(kind: AdapterKind) -> AdapterPreset {
    match kind {
        AdapterKind::Mock => AdapterPreset {
            kind: AdapterKind::Mock,
            executable: "loom-adapter-mock",
            check_executable: "loom-adapter-mock",
            description: "内置 mock adapter，用于契约测试与 canary",
            args: &[],
            stdin_packet: false,
        },
        AdapterKind::Native => AdapterPreset {
            kind: AdapterKind::Native,
            executable: "loom-native",
            check_executable: "loom-native",
            description: "内置 native executor（OpenAI-compatible loop，可脚本化测试）",
            args: &[],
            stdin_packet: false,
        },
        AdapterKind::Goose => AdapterPreset {
            kind: AdapterKind::Goose,
            executable: "goose",
            check_executable: "goose",
            description: "goose run --no-session -i <packet> --output-format json",
            args: &[
                "run",
                "--no-session",
                "-i",
                "{packet}",
                "--output-format",
                "json",
            ],
            stdin_packet: false,
        },
        AdapterKind::Opencode => AdapterPreset {
            kind: AdapterKind::Opencode,
            executable: "sh",
            check_executable: "opencode",
            description: "opencode run --format json < packet",
            args: &["-c", "opencode run --format json"],
            stdin_packet: true,
        },
    }
}

pub fn list() -> Vec<AdapterPreset> {
    vec![
        preset(AdapterKind::Mock),
        preset(AdapterKind::Native),
        preset(AdapterKind::Goose),
        preset(AdapterKind::Opencode),
    ]
}

impl AdapterPreset {
    pub fn packet_path(&self, request: &AdapterRequest) -> String {
        request
            .packet_dir
            .join("packet.md")
            .to_string_lossy()
            .into_owned()
    }

    pub fn expand(&self, request: &AdapterRequest) -> Vec<String> {
        let packet = self.packet_path(request);
        let worktree = request.worktree.to_string_lossy().into_owned();
        self.args
            .iter()
            .map(|arg| {
                arg.replace("{packet}", &packet)
                    .replace("{worktree}", &worktree)
            })
            .collect()
    }

    /// Extra stdin content appended after the packet, for executors whose
    /// skill loader needs an explicit mention instead of (or in addition to)
    /// the injected bodies — opencode's `@skill-id` is its load entrypoint
    /// (`doc/15` §12.3). None for every other preset.
    pub fn stdin_extra(&self, skills: &[String]) -> Option<String> {
        if self.kind != AdapterKind::Opencode || skills.is_empty() {
            return None;
        }
        let mentions: Vec<String> = skills.iter().map(|id| format!("@{id}")).collect();
        // Unverified against a live binary until the opencode canary runs
        // (doc/14 blocker); the mention form follows doc/15 §12.3.
        Some(format!(
            "\n\nLoad the declared skills: {}\n",
            mentions.join(" ")
        ))
    }

    pub fn is_available(&self) -> bool {
        if self.kind == AdapterKind::Mock {
            return true;
        }
        let executable = self.probe_executable();
        if executable.contains('/') {
            return Path::new(&executable).is_file();
        }
        which(&executable).is_some()
    }

    /// Resolve the executable, allowing the native executor to live beside the
    /// `loom` binary during development.
    pub fn resolve_executable(&self) -> String {
        if self.kind != AdapterKind::Native {
            return self.executable.to_string();
        }
        if let Ok(path) = std::env::var("LOOM_NATIVE_BIN") {
            if !path.trim().is_empty() {
                return path;
            }
        }
        if let Ok(current) = std::env::current_exe() {
            if let Some(parent) = current.parent() {
                let candidate = parent.join("loom-native");
                if candidate.is_file() {
                    return candidate.to_string_lossy().into_owned();
                }
            }
        }
        self.executable.to_string()
    }

    /// What to probe for availability and `--version`: a custom resolved
    /// path wins (native's `LOOM_NATIVE_BIN`); otherwise the real tool, not
    /// a wrapper — the opencode preset execs through `sh -c`, but its
    /// version comes from `opencode` itself.
    pub fn probe_executable(&self) -> String {
        let resolved = self.resolve_executable();
        if resolved.contains('/') {
            resolved
        } else {
            self.check_executable.to_string()
        }
    }

    pub fn version_command(&self) -> (String, Vec<String>) {
        (self.probe_executable(), vec!["--version".to_string()])
    }
}

pub fn which(executable: &str) -> Option<std::path::PathBuf> {
    if executable.contains('/') {
        let path = Path::new(executable);
        return path.is_file().then(|| path.to_path_buf());
    }
    let paths = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&paths) {
        let candidate = dir.join(executable);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::AdapterKind;

    #[test]
    fn skill_mentions_only_for_opencode_and_only_with_skills() {
        let skills = vec!["docs-writing".to_string(), "format-code".to_string()];

        let opencode = preset(AdapterKind::Opencode);
        let extra = opencode
            .stdin_extra(&skills)
            .expect("opencode gets mentions");
        assert!(extra.contains("@docs-writing"), "{extra}");
        assert!(extra.contains("@format-code"), "{extra}");
        // Mentions come after the packet, never inside it.
        assert!(extra.starts_with("\n\n"));

        // No declared skills → nothing appended (no trailing noise).
        assert!(opencode.stdin_extra(&[]).is_none());

        // Every other preset consumes the injected bodies from the packet;
        // @-mentions are opencode's entrypoint, not theirs.
        for kind in [AdapterKind::Mock, AdapterKind::Native, AdapterKind::Goose] {
            assert!(
                preset(kind).stdin_extra(&skills).is_none(),
                "{kind:?} must not append mentions"
            );
        }
    }
}
