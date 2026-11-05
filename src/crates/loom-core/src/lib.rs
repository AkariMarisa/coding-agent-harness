//! Core protocol types and deterministic helpers for Loom.
//!
//! This crate intentionally stays free of filesystem and process I/O.

mod capability;
mod config;
mod envelope;
mod event;
mod limits;
mod receipt;
mod task;
mod text;
mod time;
mod usage;

pub use capability::{find_model, ModelCapability, ReasoningPassthrough, ThinkingMode, Tier, Wire};
pub use config::{Config, ConfigError, ToolsConfig};
pub use envelope::{
    Budget, BudgetLevel, Envelope, ErrorCode, Item, ItemKind, ItemType, Stats, ToolError, ToolName,
    PROTOCOL_VERSION,
};
pub use event::{Event, EventFilter, EventType, EVENT_VERSION};
pub use limits::Limits;
pub use receipt::{
    parse_four_line, ArtifactEvidence, Decision, Diffstat, FourLineReceipt, Receipt, ReceiptStatus,
    TokenTotals, VerifyClaim, VerifyEvidence, VerifyResult, RECEIPT_VERSION,
};
pub use task::{
    status_after_event, validate_graph, Acceptance, ContextPolicy, TaskBudget, TaskConstraints,
    TaskIndex, TaskIndexEntry, TaskInput, TaskOutput, TaskRole, TaskSpec, TaskStatus,
    TaskWorkspace, WorkspaceMode, TASK_VERSION,
};
pub use text::{estimate_tokens, parse_range, truncate_bytes, truncate_chars};
pub use time::{compact_utc, rfc3339_ms};
pub use usage::{
    aggregate_usage, CostSource, UsageAggregate, UsageFilter, UsageRecord, UsageSource,
    USAGE_VERSION,
};
