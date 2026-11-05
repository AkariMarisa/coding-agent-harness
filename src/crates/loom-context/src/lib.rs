//! Context compilation and versioned prompt library.
//!
//! This crate is the deterministic middle layer between the state store and an
//! executor adapter: it turns a TaskSpec, stable project state, and references
//! into a bounded, cache-friendly packet.

mod compiler;
mod prompt;

pub use compiler::{
    compile, CompileRequest, CompiledContext, ContextBudget, ContextKind, ContextManifest,
    ContextSection, FailedAcceptance, FailurePack, InputHash, SectionContent, CONTEXT_VERSION,
};
pub use prompt::{get, list, PromptRole, PromptTemplate, PROMPT_VERSION};
