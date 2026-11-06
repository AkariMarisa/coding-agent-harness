//! Native executor crate. The binary is a process-level adapter; the library is
//! also usable from tests with a scripted provider.

mod provider;
mod run;
mod tools;

pub use provider::{
    ChatMessage, ChatRequest, ChatResponse, OpenAiProvider, Provider, ScriptedProvider, ToolCall,
    Usage,
};
pub use run::{
    run, run_from_env, NativeConfig, NativeOutcome, NativeRequest, VerifyLine, NATIVE_VERSION,
};
