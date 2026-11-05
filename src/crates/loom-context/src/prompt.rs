//! Versioned prompt library. Templates are compiled into the binary, so a run
//! never depends on files copied into the project.

use std::collections::BTreeMap;
use std::str::FromStr;

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

pub const PROMPT_VERSION: &str = "1.0";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PromptRole {
    Orchestrator,
    Worker,
    Judgment,
    Review,
}

impl PromptRole {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Orchestrator => "orchestrator",
            Self::Worker => "worker",
            Self::Judgment => "judgment",
            Self::Review => "review",
        }
    }
}

impl FromStr for PromptRole {
    type Err = String;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        match input.trim().to_ascii_lowercase().as_str() {
            "orchestrator" => Ok(Self::Orchestrator),
            "worker" => Ok(Self::Worker),
            "judgment" | "judge" => Ok(Self::Judgment),
            "review" | "reviewer" => Ok(Self::Review),
            other => Err(format!("unknown prompt role: {other}")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PromptTemplate {
    pub role: PromptRole,
    pub version: &'static str,
    pub description: &'static str,
    pub body: &'static str,
}

impl PromptTemplate {
    /// Substitute `{{key}}` placeholders. Unknown keys fail loudly instead of
    /// leaking a template artifact into the model context.
    pub fn render(&self, vars: &BTreeMap<&str, String>) -> Result<String> {
        let mut out = String::with_capacity(self.body.len());
        let mut rest = self.body;
        while let Some(start) = rest.find("{{") {
            out.push_str(&rest[..start]);
            let after = &rest[start + 2..];
            let Some(end) = after.find("}}") else {
                bail!(
                    "{}@{}: unterminated placeholder",
                    self.role.as_str(),
                    self.version
                );
            };
            let key = &after[..end];
            let Some(value) = vars.get(key) else {
                bail!(
                    "{}@{}: missing placeholder {key}",
                    self.role.as_str(),
                    self.version
                );
            };
            out.push_str(value);
            rest = &after[end + 2..];
        }
        out.push_str(rest);
        Ok(out)
    }

    pub fn id(&self) -> String {
        format!("{}@{}", self.role.as_str(), self.version)
    }
}

const TEMPLATES: &[PromptTemplate] = &[
    PromptTemplate {
        role: PromptRole::Orchestrator,
        version: "1",
        description: "维护 GOAL、任务图、回执与升级决策",
        body: include_str!("../prompts/orchestrator@1.md"),
    },
    PromptTemplate {
        role: PromptRole::Worker,
        version: "1",
        description: "执行一个切片并输出四行回执",
        body: include_str!("../prompts/worker@1.md"),
    },
    PromptTemplate {
        role: PromptRole::Judgment,
        version: "1",
        description: "只读证据包，输出 fix_plan/split/reject/human",
        body: include_str!("../prompts/judgment@1.md"),
    },
    PromptTemplate {
        role: PromptRole::Review,
        version: "1",
        description: "只审 diff 与证据，输出 verdict 与理由",
        body: include_str!("../prompts/review@1.md"),
    },
];

pub fn list() -> &'static [PromptTemplate] {
    TEMPLATES
}

pub fn get(role: PromptRole) -> Result<&'static PromptTemplate> {
    TEMPLATES
        .iter()
        .find(|template| template.role == role)
        .ok_or_else(|| anyhow::anyhow!("no prompt template for {}", role.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars() -> BTreeMap<&'static str, String> {
        BTreeMap::from([
            ("task_id", "T-20261104-001".to_string()),
            ("goal_id", "G-20261104-auth".to_string()),
            ("role", "worker".to_string()),
            ("tier", "worker".to_string()),
            ("report_path", ".loom/outbox/report.md".to_string()),
        ])
    }

    #[test]
    fn lists_four_versioned_templates() {
        let templates = list();
        assert_eq!(templates.len(), 4);
        for template in templates {
            assert_eq!(template.version, "1");
            assert!(!template.body.trim().is_empty());
            assert!(get(template.role).is_ok());
        }
    }

    #[test]
    fn renders_placeholders() {
        let worker = get(PromptRole::Worker).expect("worker");
        let rendered = worker.render(&vars()).expect("render");
        assert!(rendered.contains("T-20261104-001"));
        assert!(!rendered.contains("{{"));
    }

    #[test]
    fn missing_placeholder_fails() {
        let worker = get(PromptRole::Worker).expect("worker");
        let empty = BTreeMap::new();
        assert!(worker.render(&empty).is_err());
    }
}
