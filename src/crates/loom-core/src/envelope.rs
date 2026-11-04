use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u8 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolName {
    Search,
    Read,
    Tree,
    Glob,
    Diff,
    Verify,
    Log,
    Git,
    Edit,
    Write,
    Bash,
    Artifact,
}

impl ToolName {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Search => "search",
            Self::Read => "read",
            Self::Tree => "tree",
            Self::Glob => "glob",
            Self::Diff => "diff",
            Self::Verify => "verify",
            Self::Log => "log",
            Self::Git => "git",
            Self::Edit => "edit",
            Self::Write => "write",
            Self::Bash => "bash",
            Self::Artifact => "artifact",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ItemType {
    #[default]
    Match,
    File,
    Dir,
    Path,
    Hunk,
    Log,
    Symbol,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ItemKind {
    Def,
    Ref,
    Call,
    Type,
    Test,
    Doc,
    Config,
    Other,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Item {
    #[serde(rename = "t")]
    pub item_type: ItemType,
    #[serde(rename = "p", skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(rename = "b", skip_serializing_if = "Option::is_none")]
    pub blob: Option<String>,
    #[serde(rename = "r", skip_serializing_if = "Option::is_none")]
    pub range: Option<[usize; 2]>,
    #[serde(rename = "s", skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
    #[serde(rename = "k", skip_serializing_if = "Option::is_none")]
    pub kind: Option<ItemKind>,
    #[serde(rename = "x", skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
    #[serde(rename = "sc", skip_serializing_if = "Option::is_none")]
    pub score: Option<f32>,
    #[serde(rename = "n", skip_serializing_if = "Option::is_none")]
    pub count: Option<usize>,
    #[serde(rename = "a", skip_serializing_if = "Option::is_none")]
    pub added: Option<usize>,
    #[serde(rename = "d", skip_serializing_if = "Option::is_none")]
    pub deleted: Option<usize>,
    #[serde(rename = "l", skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
}

impl Item {
    pub fn new(item_type: ItemType) -> Self {
        Self {
            item_type,
            ..Self::default()
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Stats {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub n: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tok: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spill: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dedupe: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BudgetLevel {
    Call,
    Invocation,
    Run,
}

impl BudgetLevel {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Call => "call",
            Self::Invocation => "invocation",
            Self::Run => "run",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Budget {
    /// Used bytes after this envelope.
    pub u: usize,
    /// Limit in bytes.
    pub l: usize,
    /// Percentage used (0-100).
    pub p: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub at: Option<BudgetLevel>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    InvalidArgs,
    NotFound,
    TooLarge,
    Binary,
    Timeout,
    Permission,
    Stale,
    StaleCursor,
    Budget,
    Internal,
}

impl ErrorCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidArgs => "invalid_args",
            Self::NotFound => "not_found",
            Self::TooLarge => "too_large",
            Self::Binary => "binary",
            Self::Timeout => "timeout",
            Self::Permission => "permission",
            Self::Stale => "stale",
            Self::StaleCursor => "stale_cursor",
            Self::Budget => "budget",
            Self::Internal => "internal",
        }
    }

    pub const fn exit_code(self) -> i32 {
        match self {
            Self::InvalidArgs
            | Self::NotFound
            | Self::TooLarge
            | Self::Binary
            | Self::Permission => 2,
            Self::Timeout | Self::Budget => 3,
            Self::Stale | Self::StaleCursor => 5,
            Self::Internal => 4,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolError {
    pub code: ErrorCode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl ToolError {
    pub fn new(code: ErrorCode) -> Self {
        Self {
            code,
            hint: None,
            detail: None,
        }
    }

    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Envelope {
    pub v: u8,
    pub ok: bool,
    pub tool: ToolName,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub items: Option<Vec<Item>>,
    /// Bounded content for read/log-like tools.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(rename = "ref", skip_serializing_if = "Option::is_none")]
    pub artifact_ref: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub truncated: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unchanged: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stats: Option<Stats>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub budget: Option<Budget>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub err: Option<ToolError>,
}

impl Envelope {
    pub fn new(tool: ToolName, summary: impl Into<String>) -> Self {
        Self {
            v: PROTOCOL_VERSION,
            ok: true,
            tool,
            summary: Some(summary.into()),
            items: None,
            text: None,
            artifact_ref: None,
            truncated: None,
            next: None,
            unchanged: None,
            stats: None,
            budget: None,
            err: None,
        }
    }

    pub fn error(tool: ToolName, summary: impl Into<String>, err: ToolError) -> Self {
        Self {
            v: PROTOCOL_VERSION,
            ok: false,
            tool,
            summary: Some(summary.into()),
            items: None,
            text: None,
            artifact_ref: None,
            truncated: None,
            next: None,
            unchanged: None,
            stats: None,
            budget: None,
            err: Some(err),
        }
    }

    pub fn with_items(mut self, items: Vec<Item>) -> Self {
        self.items = Some(items);
        self
    }

    pub fn with_text(mut self, text: impl Into<String>) -> Self {
        self.text = Some(text.into());
        self
    }

    pub fn with_ref(mut self, artifact_ref: impl Into<String>) -> Self {
        self.artifact_ref = Some(artifact_ref.into());
        self
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| {
            r#"{"v":1,"ok":false,"tool":"search","err":{"code":"internal"}}"#.to_string()
        })
    }

    pub fn bytes(&self) -> usize {
        self.to_json().len()
    }

    /// Fill `stats.bytes` / `stats.tok` from the serialized envelope.
    ///
    /// Two passes are enough to converge because the size of the numeric fields
    /// changes by only a few bytes.
    pub fn finalize_stats(&mut self) {
        let mut stats = self.stats.take().unwrap_or_default();
        stats.bytes = None;
        stats.tok = None;
        self.stats = Some(stats);

        for _ in 0..2 {
            let json = self.to_json();
            let bytes = json.len();
            let tok = crate::estimate_tokens(&json);
            let stats = self.stats.as_mut().expect("stats was set");
            stats.bytes = Some(bytes);
            stats.tok = Some(tok);
        }
    }

    /// Drop bulky payload but keep the summary and artifact reference.
    pub fn degrade(&mut self) {
        self.items = None;
        self.text = None;
        if let Some(summary) = self.summary.as_mut() {
            if !summary.contains("已降级") {
                summary.push_str("（预算不足，已降级为摘要 + 引用）");
            }
        }
    }

    pub fn has_payload(&self) -> bool {
        self.items.as_ref().is_some_and(|items| !items.is_empty())
            || self.text.as_ref().is_some_and(|text| !text.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_serializes_short_keys() {
        let env = Envelope::new(ToolName::Search, "ok").with_items(vec![Item {
            path: Some("src/lib.rs".into()),
            range: Some([1, 2]),
            preview: Some("pub fn main()".into()),
            ..Item::new(ItemType::Match)
        }]);
        let json = env.to_json();
        assert!(json.contains(r#""p":"src/lib.rs""#));
        assert!(json.contains(r#""r":[1,2]"#));
        assert!(json.contains(r#""x":"pub fn main()""#));
        assert!(!json.contains("path"));
    }

    #[test]
    fn error_has_err() {
        let env = Envelope::error(
            ToolName::Read,
            "missing",
            ToolError::new(ErrorCode::NotFound),
        );
        assert!(!env.ok);
        assert_eq!(env.err.expect("err").code, ErrorCode::NotFound);
    }
}
