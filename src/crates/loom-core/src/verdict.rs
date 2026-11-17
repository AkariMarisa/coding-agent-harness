//! Verdicts that review and judgment tasks produce in their reports.
//!
//! Verify parses them from the report at `output.report_path` instead of
//! grepping for a keyword, records the result on the receipt, and the merge
//! queue consumes it (`task.blocked(code=review_changes)`). See `doc/03`.

use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// What a review or judgment task concluded.
///
/// Review prompts emit `verdict: pass|request_changes|human`; judgment prompts
/// emit a JSON object whose `decision` field uses `fix_plan|split|reject`
/// alongside `human`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Pass,
    RequestChanges,
    Human,
    FixPlan,
    Split,
    Reject,
}

impl Verdict {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::RequestChanges => "request_changes",
            Self::Human => "human",
            Self::FixPlan => "fix_plan",
            Self::Split => "split",
            Self::Reject => "reject",
        }
    }

    /// Only `pass` lets a reviewed task merge.
    pub const fn is_pass(self) -> bool {
        matches!(self, Self::Pass)
    }
}

impl FromStr for Verdict {
    type Err = String;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        match input.trim().to_ascii_lowercase().as_str() {
            "pass" => Ok(Self::Pass),
            "request_changes" => Ok(Self::RequestChanges),
            "human" => Ok(Self::Human),
            "fix_plan" => Ok(Self::FixPlan),
            "split" => Ok(Self::Split),
            "reject" => Ok(Self::Reject),
            other => Err(format!("unknown verdict: {other}")),
        }
    }
}

/// Parse `verdict: <value>` from a review report. Leading bullets (`-`, `*`,
/// `#`) and trailing prose ("verdict: request_changes (see reasons)") are
/// tolerated; `None` means no line carries a parseable verdict.
pub fn parse_review_verdict(report: &str) -> Option<Verdict> {
    report.lines().find_map(|line| {
        let line = line
            .trim()
            .trim_start_matches(&['-', '*', '#'][..])
            .trim()
            .to_ascii_lowercase();
        let rest = line.strip_prefix("verdict")?;
        let value = rest.trim_start().trim_start_matches(':').trim();
        value.split_whitespace().next()?.parse().ok()
    })
}

/// Parse the `decision` field of a judgment report. The report may be fenced
/// or wrapped in prose; the outermost JSON object is extracted.
pub fn parse_judgment_decision(report: &str) -> Option<Verdict> {
    let start = report.find('{')?;
    let end = report.rfind('}')?;
    if end < start {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(&report[start..=end]).ok()?;
    value.get("decision")?.as_str()?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_review_verdict_lines() {
        assert_eq!(
            parse_review_verdict("verdict: request_changes"),
            Some(Verdict::RequestChanges)
        );
        assert_eq!(
            parse_review_verdict("- verdict: pass\n\nreasons below"),
            Some(Verdict::Pass)
        );
        assert_eq!(
            parse_review_verdict("## Verdict: HUMAN"),
            Some(Verdict::Human)
        );
        assert_eq!(
            parse_review_verdict("verdict: request_changes (see reasons)"),
            Some(Verdict::RequestChanges)
        );
        assert_eq!(parse_review_verdict("no verdict here"), None);
        assert_eq!(parse_review_verdict("verdict: maybe"), None);
        assert_eq!(parse_review_verdict(""), None);
    }

    #[test]
    fn parses_judgment_decision_json() {
        let pure = r#"{"decision": "fix_plan", "plan": ["a"], "refs": []}"#;
        assert_eq!(parse_judgment_decision(pure), Some(Verdict::FixPlan));
        let fenced = "```json\n{\"decision\": \"human\"}\n```";
        assert_eq!(parse_judgment_decision(fenced), Some(Verdict::Human));
        let wrapped = "Here is my judgment:\n```\n{\"decision\": \"reject\"}\n```\nDone.";
        assert_eq!(parse_judgment_decision(wrapped), Some(Verdict::Reject));
        assert_eq!(parse_judgment_decision("{\"plan\": []}"), None);
        assert_eq!(parse_judgment_decision("not json at all"), None);
    }

    #[test]
    fn verdict_serializes_snake_case() {
        assert_eq!(
            serde_json::to_string(&Verdict::RequestChanges).expect("json"),
            "\"request_changes\""
        );
        assert!(Verdict::Pass.is_pass());
        assert!(!Verdict::Human.is_pass());
        assert_eq!("split".parse::<Verdict>().expect("parse"), Verdict::Split);
        assert!("nope".parse::<Verdict>().is_err());
    }
}
