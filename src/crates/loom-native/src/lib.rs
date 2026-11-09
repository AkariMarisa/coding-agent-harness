//! Native executor crate. The binary is a process-level adapter; the library is
//! also usable from tests with a scripted provider.

mod churn;
mod project;
mod provider;
mod run;
mod tools;

/// Strip credentials from a child environment: anything that looks like a
/// secret (`*_API_KEY`, `*_TOKEN`, `*_SECRET`) never reaches acceptance
/// commands or the `bash` tool, whatever relay the key belongs to.
pub(crate) fn scrub_child_env(command: &mut std::process::Command) {
    for (key, _) in std::env::vars() {
        if key.contains("API_KEY") || key.ends_with("_TOKEN") || key.ends_with("_SECRET") {
            command.env_remove(key);
        }
    }
}

pub use provider::{
    build_chat_body, ChatMessage, ChatRequest, ChatResponse, OpenAiProvider, Provider,
    ScriptedProvider, ToolCall, Usage,
};
pub use run::{
    run, run_from_env, NativeConfig, NativeOutcome, NativeRequest, VerifyLine, NATIVE_VERSION,
};
