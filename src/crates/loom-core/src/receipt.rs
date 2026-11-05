use std::collections::HashSet;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

pub const RECEIPT_VERSION: &str = "1.0";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReceiptStatus {
    Done,
    Blocked,
    NeedsInput,
    #[default]
    Failed,
    BudgetExceeded,
}

impl ReceiptStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Done => "done",
            Self::Blocked => "blocked",
            Self::NeedsInput => "needs_input",
            Self::Failed => "failed",
            Self::BudgetExceeded => "budget_exceeded",
        }
    }
}

impl FromStr for ReceiptStatus {
    type Err = String;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        match input.trim() {
            "done" => Ok(Self::Done),
            "blocked" => Ok(Self::Blocked),
            "needs_input" => Ok(Self::NeedsInput),
            "failed" => Ok(Self::Failed),
            "budget_exceeded" => Ok(Self::BudgetExceeded),
            other => Err(format!("unknown receipt status: {other}")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VerifyResult {
    Pass,
    Fail,
    Skip,
    Flaky,
}

impl VerifyResult {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Fail => "fail",
            Self::Skip => "skip",
            Self::Flaky => "flaky",
        }
    }
}

impl FromStr for VerifyResult {
    type Err = String;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        match input.trim() {
            "pass" => Ok(Self::Pass),
            "fail" => Ok(Self::Fail),
            "skip" => Ok(Self::Skip),
            "flaky" => Ok(Self::Flaky),
            other => Err(format!("unknown verify result: {other}")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyClaim {
    pub id: String,
    pub result: VerifyResult,
}

/// Parsed four-line worker receipt from `doc/03` §5.1.
///
/// This is a *claim*. Only the independent verify runner can mark a task passed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FourLineReceipt {
    pub status: ReceiptStatus,
    pub branch: String,
    pub head_sha: String,
    pub verify: Vec<VerifyClaim>,
    pub log_path: String,
    pub report_path: String,
}

/// Parse strictly: exactly four lines, fixed order, no prose or blank lines.
pub fn parse_four_line(text: &str) -> Result<FourLineReceipt, String> {
    let mut lines: Vec<&str> = text.split('\n').collect();
    if matches!(lines.last(), Some(line) if line.is_empty()) {
        lines.pop();
    }
    if lines.len() != 4 {
        return Err(format!(
            "receipt must contain exactly 4 lines (found {})",
            lines.len()
        ));
    }

    let mut parsed = Vec::with_capacity(4);
    for line in &lines {
        if line.trim().is_empty() {
            return Err("receipt must not contain blank lines".to_string());
        }
        parsed.push(line.strip_suffix('\r').unwrap_or(line));
    }

    let status = expect_field(parsed[0], "status")?.parse::<ReceiptStatus>()?;

    let branch_value = expect_field(parsed[1], "branch")?;
    let (branch, head_sha) = branch_value
        .rsplit_once('@')
        .ok_or_else(|| "branch must be <branch>@<sha>".to_string())?;
    if branch.trim().is_empty() {
        return Err("branch must not be empty".to_string());
    }
    if head_sha.trim().is_empty()
        || !head_sha
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-')
    {
        return Err(format!("invalid head sha: {head_sha}"));
    }

    let verify_value = expect_field(parsed[2], "verify")?;
    let (results, log_path) = parse_verify(verify_value)?;

    let report_path = expect_field(parsed[3], "report")?;
    if report_path.trim().is_empty() {
        return Err("report path must not be empty".to_string());
    }

    Ok(FourLineReceipt {
        status,
        branch: branch.to_string(),
        head_sha: head_sha.to_string(),
        verify: results,
        log_path,
        report_path: report_path.to_string(),
    })
}

fn expect_field<'a>(line: &'a str, name: &str) -> Result<&'a str, String> {
    let prefix = format!("{name}: ");
    line.strip_prefix(&prefix)
        .ok_or_else(|| format!("expected line \"{name}: ...\""))
}

fn parse_verify(value: &str) -> Result<(Vec<VerifyClaim>, String), String> {
    let (results_part, log_part) = value
        .split_once(';')
        .ok_or_else(|| "verify line must contain \"; log=<path>\"".to_string())?;
    let log_path = log_part
        .trim()
        .strip_prefix("log=")
        .ok_or_else(|| "verify log must use log=<path>".to_string())?;
    if log_path.trim().is_empty() {
        return Err("verify log path must not be empty".to_string());
    }

    let mut claims = Vec::new();
    let mut seen = HashSet::new();
    for item in results_part.split(',') {
        let (id, result) = item
            .split_once('=')
            .ok_or_else(|| format!("invalid verify item: {item}"))?;
        let id = id.trim();
        if id.is_empty() {
            return Err("verify id must not be empty".to_string());
        }
        if !seen.insert(id.to_string()) {
            return Err(format!("duplicate verify id: {id}"));
        }
        claims.push(VerifyClaim {
            id: id.to_string(),
            result: result.trim().parse()?,
        });
    }
    if claims.is_empty() {
        return Err("verify must contain at least one acceptance id".to_string());
    }
    Ok((claims, log_path.trim().to_string()))
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Receipt {
    #[serde(default = "default_version")]
    pub schema_version: String,
    pub task_id: String,
    pub attempt: u32,
    pub status: ReceiptStatus,
    pub base_sha: String,
    pub head_sha: String,
    pub branch: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diffstat: Option<Diffstat>,
    pub verify: Vec<VerifyEvidence>,
    pub artifacts: Vec<ArtifactEvidence>,
    pub tokens: TokenTotals,
    pub decisions: Vec<Decision>,
    pub blockers: Vec<String>,
    pub next_hint: String,
    pub report_path: String,
}

fn default_version() -> String {
    RECEIPT_VERSION.to_string()
}

impl Receipt {
    pub fn new(task_id: impl Into<String>, attempt: u32) -> Self {
        Self {
            schema_version: default_version(),
            task_id: task_id.into(),
            attempt,
            status: ReceiptStatus::Failed,
            base_sha: String::new(),
            head_sha: String::new(),
            branch: String::new(),
            diffstat: None,
            verify: Vec::new(),
            artifacts: Vec::new(),
            tokens: TokenTotals::default(),
            decisions: Vec::new(),
            blockers: Vec::new(),
            next_hint: String::new(),
            report_path: String::new(),
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != RECEIPT_VERSION {
            return Err(format!(
                "unsupported receipt schema_version: {}",
                self.schema_version
            ));
        }
        if self.task_id.trim().is_empty() {
            return Err("receipt task_id must not be empty".to_string());
        }
        if self.attempt == 0 {
            return Err("receipt attempt must be >= 1".to_string());
        }
        if self.report_path.trim().is_empty() {
            return Err("receipt report_path must not be empty".to_string());
        }
        self.tokens.validate()?;

        let mut ids = HashSet::new();
        for item in &self.verify {
            if item.id.trim().is_empty() {
                return Err("verify id must not be empty".to_string());
            }
            if !ids.insert(item.id.as_str()) {
                return Err(format!("duplicate verify id: {}", item.id));
            }
            if item.log_path.trim().is_empty() {
                return Err(format!("verify {} log_path must not be empty", item.id));
            }
        }
        for artifact in &self.artifacts {
            if artifact.path.trim().is_empty() {
                return Err("artifact path must not be empty".to_string());
            }
            if artifact.sha256.len() != 64
                || !artifact.sha256.chars().all(|ch| ch.is_ascii_hexdigit())
            {
                return Err(format!(
                    "artifact sha256 must be 64 hex characters: {}",
                    artifact.path
                ));
            }
        }
        Ok(())
    }

    /// Runner-derived status: a worker's `done` claim does not survive a failed
    /// acceptance command.
    pub fn derived_status(&self) -> ReceiptStatus {
        if self.status == ReceiptStatus::Done {
            if !self.verify.is_empty()
                && self
                    .verify
                    .iter()
                    .all(|item| item.result == VerifyResult::Pass)
            {
                ReceiptStatus::Done
            } else {
                ReceiptStatus::Failed
            }
        } else {
            self.status
        }
    }

    pub fn all_verified(&self) -> bool {
        !self.verify.is_empty()
            && self
                .verify
                .iter()
                .all(|item| item.result == VerifyResult::Pass)
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Diffstat {
    pub files: u64,
    pub insertions: u64,
    pub deletions: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct VerifyEvidence {
    pub id: String,
    pub cmd: String,
    pub exit: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signal: Option<String>,
    pub duration_s: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_s: Option<u64>,
    pub log_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log_sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub git_sha: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub env_hash: Option<String>,
    pub result: VerifyResult,
}

impl Default for VerifyEvidence {
    fn default() -> Self {
        Self {
            id: String::new(),
            cmd: String::new(),
            exit: -1,
            signal: None,
            duration_s: 0.0,
            timeout_s: None,
            log_path: String::new(),
            log_sha256: None,
            git_sha: None,
            env_hash: None,
            result: VerifyResult::Fail,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ArtifactEvidence {
    pub path: String,
    pub sha256: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TokenTotals {
    pub input: u64,
    pub cached_input: u64,
    pub output: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
}

impl TokenTotals {
    fn validate(&self) -> Result<(), String> {
        if self.cached_input > self.input {
            return Err(format!(
                "cached_input ({}) exceeds input ({})",
                self.cached_input, self.input
            ));
        }
        if let Some(cost) = self.cost_usd {
            if !cost.is_finite() || cost < 0.0 {
                return Err("token cost_usd must be finite and >= 0".to_string());
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Decision {
    pub id: String,
    pub summary: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "status: done\nbranch: agent/T-20261104-002@7f3c2a1\nverify: A1=pass,A2=pass; log=.loom/reports/T-20261104-002.verify.log\nreport: .loom/reports/T-20261104-002.md\n";

    fn evidence() -> VerifyEvidence {
        VerifyEvidence {
            id: "A1".into(),
            cmd: "true".into(),
            exit: 0,
            duration_s: 1.0,
            log_path: ".loom/reports/test.log".into(),
            result: VerifyResult::Pass,
            ..VerifyEvidence::default()
        }
    }

    fn receipt() -> Receipt {
        Receipt {
            schema_version: RECEIPT_VERSION.into(),
            task_id: "T-20261104-002".into(),
            attempt: 1,
            status: ReceiptStatus::Done,
            branch: "agent/T-20261104-002".into(),
            verify: vec![evidence()],
            tokens: TokenTotals {
                input: 10,
                cached_input: 4,
                output: 2,
                ..TokenTotals::default()
            },
            report_path: ".loom/reports/T-20261104-002.md".into(),
            ..Receipt::new("T-20261104-002", 1)
        }
    }

    #[test]
    fn parses_four_line_receipt() {
        let parsed = parse_four_line(SAMPLE).expect("parse");
        assert_eq!(parsed.status, ReceiptStatus::Done);
        assert_eq!(parsed.branch, "agent/T-20261104-002");
        assert_eq!(parsed.head_sha, "7f3c2a1");
        assert_eq!(parsed.verify.len(), 2);
        assert_eq!(parsed.verify[1].result, VerifyResult::Pass);
        assert_eq!(parsed.log_path, ".loom/reports/T-20261104-002.verify.log");
    }

    #[test]
    fn rejects_malformed_four_line_receipts() {
        assert!(parse_four_line("status: done\n").is_err());

        let extra = format!("{SAMPLE}\n");
        assert!(parse_four_line(&extra).is_err());

        let prose = format!("hello\n{SAMPLE}");
        assert!(parse_four_line(&prose).is_err());

        let wrong_order = "branch: b@sha\nstatus: done\nverify: A1=pass; log=x\nreport: r\n";
        assert!(parse_four_line(wrong_order).is_err());

        let no_log = "status: done\nbranch: b@sha\nverify: A1=pass\nreport: r\n";
        assert!(parse_four_line(no_log).is_err());

        let dup = "status: done\nbranch: b@sha\nverify: A1=pass,A1=fail; log=x\nreport: r\n";
        assert!(parse_four_line(dup).is_err());

        let bad_sha = "status: done\nbranch: b@\nverify: A1=pass; log=x\nreport: r\n";
        assert!(parse_four_line(bad_sha).is_err());
    }

    #[test]
    fn validates_structured_receipt() {
        assert!(receipt().validate().is_ok());

        let mut bad = receipt();
        bad.attempt = 0;
        assert!(bad.validate().is_err());

        let mut bad = receipt();
        bad.tokens.cached_input = bad.tokens.input + 1;
        assert!(bad.validate().is_err());

        let mut bad = receipt();
        bad.verify.push(bad.verify[0].clone());
        assert!(bad.validate().is_err());

        let mut bad = receipt();
        bad.artifacts.push(ArtifactEvidence {
            path: "a".into(),
            sha256: "abc".into(),
            bytes: 1,
        });
        assert!(bad.validate().is_err());
    }

    #[test]
    fn done_claim_needs_passing_verify() {
        let mut claim = receipt();
        assert_eq!(claim.derived_status(), ReceiptStatus::Done);

        claim.verify[0].result = VerifyResult::Fail;
        assert_eq!(claim.derived_status(), ReceiptStatus::Failed);

        claim.status = ReceiptStatus::Blocked;
        assert_eq!(claim.derived_status(), ReceiptStatus::Blocked);
    }
}
