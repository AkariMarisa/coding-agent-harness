//! Core protocol types and deterministic helpers for Loom.
//!
//! This crate intentionally stays free of filesystem and process I/O.

mod config;
mod envelope;
mod limits;
mod text;

pub use config::{Config, ConfigError, ToolsConfig};
pub use envelope::{
    Budget, BudgetLevel, Envelope, ErrorCode, Item, ItemKind, ItemType, Stats, ToolError, ToolName,
    PROTOCOL_VERSION,
};
pub use limits::Limits;
pub use text::{estimate_tokens, parse_range, truncate_bytes, truncate_chars};
