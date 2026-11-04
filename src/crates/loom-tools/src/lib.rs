//! Tool implementations. Each function returns a bounded envelope and never
//! prints to stdout.

mod artifact;
mod read;
mod search;
mod util;

pub use artifact::{read_artifact, ArtifactRequest};
pub use read::{read, ReadRequest};
pub use search::{search, SearchRequest};

use loom_core::{Envelope, ErrorCode, ToolError, ToolName};

#[derive(Debug, Clone)]
pub struct ToolOutcome {
    pub envelope: Envelope,
    pub args_hash: String,
    pub scope: Option<String>,
    pub stale: usize,
}

impl ToolOutcome {
    pub fn new(envelope: Envelope, args_hash: String, scope: Option<String>) -> Self {
        Self {
            envelope,
            args_hash,
            scope,
            stale: 0,
        }
    }
}

pub(crate) fn error_outcome(
    tool: ToolName,
    summary: impl Into<String>,
    code: ErrorCode,
    hint: Option<String>,
    args_hash: String,
    scope: Option<String>,
) -> ToolOutcome {
    let mut error = ToolError::new(code);
    error.hint = hint;
    ToolOutcome::new(Envelope::error(tool, summary, error), args_hash, scope)
}
