//! Step-level context projection for the native loop (`doc/15` §1.3A,
//! `doc/16` §8.2).
//!
//! The native loop resends the whole history every step, so old tool payloads
//! compound: one big `bash`/`read` result or a full-file `write` argument
//! gets paid again on every following request. Projection rewrites **old**
//! interactions into bounded summaries while keeping the stable prefix
//! (system + packet) and the most recent turns verbatim, so the model can
//! still see fresh context and the provider prefix cache stays intact.
//!
//! Properties the rules below must keep (see doc/16 §8.2):
//! - deterministic: output depends only on the input message list;
//! - idempotent: projecting an already-projected list is a no-op, so a
//!   message's bytes change at most once as it crosses the recency boundary;
//! - never drops messages (only replaces content), so `tool_call_id`
//!   pairing can never be orphaned;
//! - the thinking echo (`reasoning_content`) is never stripped: official
//!   DeepSeek rejects tool history without it.

use loom_core::{estimate_tokens, truncate_chars};

use crate::provider::ChatMessage;

/// Old tool-call arguments shorter than this stay untouched (most calls are
/// small; keeping them byte-stable keeps more of the prefix cache warm).
const ARG_KEEP_BYTES: usize = 512;
/// Old tool results are reduced to their first line plus an elision marker.
const RESULT_KEEP_CHARS: usize = 600;
const ELIDED: &str = "[elided";

/// How much of the history a request actually carried. `full_*` is what an
/// unprojected replay would have sent; `sent_*` is what went on the wire.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct ProjectionStats {
    pub full_bytes: usize,
    pub sent_bytes: usize,
    pub full_tokens_est: u64,
    pub sent_tokens_est: u64,
    /// True when at least one message was rewritten for this request.
    pub projected: bool,
}

/// Project a run's message history for sending, plus the byte/token accounting
/// needed for step-level churn attribution (`doc/16` §8.4).
///
/// The estimates use the serialized message JSON, so they stay deterministic and
/// comparable between the projected and unprojected request.
///
/// Returns the input unchanged (cloned) when the dynamic tail is under
/// `max_history_bytes` or when the projection would save fewer than
/// `min_clear_tokens` tokens. `max_history_bytes == 0` disables projection.
pub(crate) fn project_messages_with_stats(
    messages: &[ChatMessage],
    keep_recent_turns: usize,
    max_history_bytes: usize,
    min_clear_tokens: usize,
) -> (Vec<ChatMessage>, ProjectionStats) {
    let full = serialized(messages);
    let full_bytes = full.len();
    let full_tokens_est = estimate_tokens(&full) as u64;
    let unchanged = |projected: bool| {
        (
            messages.to_vec(),
            ProjectionStats {
                full_bytes,
                sent_bytes: full_bytes,
                full_tokens_est,
                sent_tokens_est: full_tokens_est,
                projected,
            },
        )
    };

    if max_history_bytes == 0 || messages.len() <= 3 {
        return unchanged(false);
    }

    // The dynamic tail excludes the stable prefix (system + packet user).
    let dynamic = &messages[2..];
    let dynamic_bytes = serialized(dynamic).len();
    if dynamic_bytes <= max_history_bytes {
        return unchanged(false);
    }

    // Keep the last K assistant turns that carried tool calls (plus
    // everything after the first of them) verbatim; project before it.
    let call_positions: Vec<usize> = messages
        .iter()
        .enumerate()
        .filter(|(_, message)| message.role == "assistant" && message.tool_calls.is_some())
        .map(|(index, _)| index)
        .collect();
    if call_positions.len() <= keep_recent_turns.max(1) {
        return unchanged(false);
    }
    let tail_start = call_positions[call_positions.len() - keep_recent_turns.max(1)];

    let mut projected: Vec<ChatMessage> = messages.to_vec();
    for message in projected.iter_mut().take(tail_start) {
        if message.role == "system" {
            continue; // stable prefix
        }
        if message.role == "assistant" {
            project_tool_arguments(message);
        } else if message.role == "tool" {
            project_tool_result(message);
        }
        // user messages (packet, failure packets) stay verbatim: they are the
        // instructions the model must not lose.
    }

    // Only ship the projection when it actually pays for itself.
    let sent = serialized(&projected);
    let sent_bytes = sent.len();
    let sent_tokens_est = estimate_tokens(&sent) as u64;
    if full_tokens_est.saturating_sub(sent_tokens_est) < min_clear_tokens as u64 {
        return unchanged(false);
    }
    (
        projected,
        ProjectionStats {
            full_bytes,
            sent_bytes,
            full_tokens_est,
            sent_tokens_est,
            projected: true,
        },
    )
}

fn serialized(messages: &[ChatMessage]) -> String {
    serde_json::to_string(messages).unwrap_or_default()
}

/// Replace oversized payloads inside a history assistant message: long
/// `write`/`edit` arguments become a small reference, everything else (id,
/// name, reasoning echo, small arguments) stays byte-identical.
fn project_tool_arguments(message: &mut ChatMessage) {
    let Some(calls) = message.tool_calls.as_mut() else {
        return;
    };
    for call in calls.iter_mut() {
        if call.arguments.len() <= ARG_KEEP_BYTES || call.arguments.contains(ELIDED) {
            continue;
        }
        let original_bytes = call.arguments.len();
        let stub = match serde_json::from_str::<serde_json::Value>(&call.arguments) {
            Ok(serde_json::Value::Object(mut map)) => {
                let path = map
                    .get("path")
                    .and_then(serde_json::Value::as_str)
                    .map(ToOwned::to_owned);
                for key in ["content", "old", "new"] {
                    if let Some(serde_json::Value::String(value)) = map.get_mut(key) {
                        if value.len() > ARG_KEEP_BYTES {
                            let kept = value.chars().take(64).collect::<String>();
                            *value = format!("{kept}…[{ELIDED} {key} {} bytes]", value.len());
                        }
                    }
                }
                map.insert(
                    "__elided__".to_string(),
                    serde_json::json!({
                        "from_bytes": original_bytes,
                        "path": path.unwrap_or_default(),
                    }),
                );
                serde_json::Value::Object(map).to_string()
            }
            _ => format!("\"[{ELIDED} {original_bytes} bytes]\""),
        };
        call.arguments = stub;
    }
}

/// Reduce an old tool result to its first line plus an elision marker. The
/// marker makes the projection idempotent (a second pass is a no-op).
fn project_tool_result(message: &mut ChatMessage) {
    let Some(content) = message.content.as_mut() else {
        return;
    };
    // Already projected, or small enough that summarizing would only bloat it.
    if content.contains(ELIDED) || content.len() <= RESULT_KEEP_CHARS {
        return;
    }
    let original_bytes = content.len();
    let first_line = content.lines().next().unwrap_or_default();
    let (head, _) = truncate_chars(first_line, 200);
    let artifact = content
        .find(".loom/runs/")
        .map(|start| {
            let rest = &content[start..];
            rest.split_whitespace().next().unwrap_or(rest).trim()
        })
        .unwrap_or_default();
    let artifact_note = if artifact.is_empty() {
        String::new()
    } else {
        format!(" full={artifact}")
    };
    *content = format!("{head}\n[{ELIDED} {original_bytes} bytes, shown 1 line.{artifact_note}]");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::{ChatMessage, ToolCall};

    fn call(id: &str, arguments: String) -> ToolCall {
        ToolCall {
            id: id.to_string(),
            name: "write".to_string(),
            arguments,
        }
    }

    fn assistant(id: &str, arguments: String, reasoning: Option<&str>) -> ChatMessage {
        let mut message = ChatMessage::assistant(None, vec![call(id, arguments)]);
        message.reasoning_content = reasoning.map(str::to_string);
        message
    }

    fn tool(id: &str, content: &str) -> ChatMessage {
        ChatMessage::tool(id, "write", content)
    }

    /// system + packet + N turns of (assistant with big write, big tool result).
    fn history(turns: usize, arg_pad: usize, result_pad: usize) -> Vec<ChatMessage> {
        let mut messages = vec![
            ChatMessage::system("stable system prompt"),
            ChatMessage::user("stable packet"),
        ];
        for index in 0..turns {
            let arguments = format!(
                r#"{{"path":"src/file{index}.kt","content":"{}"}}"#,
                "x".repeat(arg_pad)
            );
            messages.push(assistant(
                &format!("call-{index}"),
                arguments,
                Some("because"),
            ));
            messages.push(tool(
                &format!("call-{index}"),
                &format!("line one of result {index}\n{}", "y".repeat(result_pad)),
            ));
        }
        messages
    }

    fn serialized(messages: &[ChatMessage]) -> String {
        serde_json::to_string(messages).expect("serialize")
    }

    #[test]
    fn under_threshold_returns_history_untouched() {
        let messages = history(6, 100, 100);
        let out = project_messages_with_stats(&messages, 4, 24 * 1024, 2000).0;
        assert_eq!(serialized(&out), serialized(&messages));
    }

    #[test]
    fn disabled_with_zero_budget() {
        let messages = history(6, 4000, 4000);
        let out = project_messages_with_stats(&messages, 1, 0, 0).0;
        assert_eq!(serialized(&out), serialized(&messages));
    }

    #[test]
    fn projects_old_payloads_but_keeps_recent_turns_verbatim() {
        // Threshold low enough to trigger, savings huge, keep only the last turn.
        let messages = history(5, 3000, 3000);
        let out = project_messages_with_stats(&messages, 1, 1024, 100).0;

        assert_eq!(out.len(), messages.len(), "never drops messages");
        // Stable prefix untouched.
        assert_eq!(out[0].content, messages[0].content);
        assert_eq!(out[1].content, messages[1].content);
        // The most recent assistant turn keeps its arguments byte-identical.
        let last_assistant = out.len() - 2;
        assert_eq!(
            out[last_assistant].tool_calls,
            messages[last_assistant].tool_calls
        );
        // An old write argument is stubbed but keeps its path and the pairing id.
        let old = &out[2];
        let old_call = &old.tool_calls.as_ref().unwrap()[0];
        assert!(old_call.arguments.contains("[elided"), "old args elided");
        assert!(old_call.arguments.contains("src/file0.kt"), "path kept");
        assert_eq!(old_call.id, "call-0", "id preserved for pairing");
        assert_eq!(
            old.reasoning_content, messages[2].reasoning_content,
            "thinking echo must survive"
        );
        // An old tool result keeps its first line plus an elision marker.
        let old_tool = &out[3];
        let content = old_tool.content.as_deref().unwrap();
        assert!(content.starts_with("line one of result 0"));
        assert!(content.contains("[elided"));
        // And it is actually smaller than the original.
        assert!(content.len() < messages[3].content.as_ref().unwrap().len());
        assert!(
            serialized(&out).len() < serialized(&messages).len(),
            "projection must shrink the request"
        );
    }

    #[test]
    fn stats_report_full_and_sent_bytes() {
        let messages = history(5, 3000, 3000);
        let (out, stats) = project_messages_with_stats(&messages, 1, 1024, 100);
        assert!(stats.projected);
        assert!(stats.sent_bytes < stats.full_bytes);
        assert!(stats.sent_tokens_est < stats.full_tokens_est);
        assert_eq!(stats.sent_bytes, serialized(&out).len());

        let (_, untouched) = project_messages_with_stats(&messages, 4, 24 * 1024, 2000);
        assert!(!untouched.projected);
        assert_eq!(untouched.full_bytes, untouched.sent_bytes);
        assert_eq!(untouched.full_tokens_est, untouched.sent_tokens_est);
    }

    #[test]
    fn projection_is_idempotent() {
        let messages = history(5, 3000, 3000);
        let once = project_messages_with_stats(&messages, 1, 1024, 100).0;
        let twice = project_messages_with_stats(&once, 1, 1024, 100).0;
        assert_eq!(serialized(&once), serialized(&twice));
    }

    #[test]
    fn skipped_when_savings_below_min_clear() {
        // Over the byte threshold but nothing big inside: with a huge
        // min_clear the projection must not ship (it would not pay for itself).
        let messages = history(5, 100, 100);
        let out = project_messages_with_stats(&messages, 1, 100, 50_000).0;
        assert_eq!(serialized(&out), serialized(&messages));
    }

    #[test]
    fn user_messages_and_small_payloads_stay_verbatim() {
        let messages = history(5, 100, 4000);
        let out = project_messages_with_stats(&messages, 1, 1024, 100).0;
        // Packet (user) untouched.
        assert_eq!(out[1].content, messages[1].content);
        // Small write args untouched (under ARG_KEEP_BYTES).
        assert_eq!(
            out[2].tool_calls, messages[2].tool_calls,
            "small args stay byte-stable for the prefix cache"
        );
        // But the big tool result still gets elided.
        assert!(out[3].content.as_deref().unwrap().contains("[elided"));
    }
}
