//! Provider boundary for the native executor.
//!
//! The executor speaks an OpenAI-compatible chat-completions protocol. Tests
//! use a scripted provider so the loop can run without a network or API key.

use std::fs;
use std::path::Path;
use std::time::Duration;

use anyhow::{bail, Context as _, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    /// Raw JSON arguments exactly as the model produced them.
    pub arguments: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl ChatMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: "system".into(),
            content: Some(content.into()),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: "user".into(),
            content: Some(content.into()),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }
    }

    pub fn assistant(content: Option<String>, tool_calls: Vec<ToolCall>) -> Self {
        Self {
            role: "assistant".into(),
            content,
            tool_calls: (!tool_calls.is_empty()).then_some(tool_calls),
            tool_call_id: None,
            name: None,
        }
    }

    pub fn tool(
        call_id: impl Into<String>,
        name: impl Into<String>,
        content: impl Into<String>,
    ) -> Self {
        Self {
            role: "tool".into(),
            content: Some(content.into()),
            tool_calls: None,
            tool_call_id: Some(call_id.into()),
            name: Some(name.into()),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Usage {
    pub input_tokens: u64,
    pub cached_input_tokens: u64,
    pub output_tokens: u64,
    pub reasoning_tokens: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
}

impl Usage {
    pub fn total_tokens(&self) -> u64 {
        self.input_tokens.saturating_add(self.output_tokens)
    }

    pub fn add(&mut self, other: &Usage) {
        self.input_tokens = self.input_tokens.saturating_add(other.input_tokens);
        self.cached_input_tokens = self
            .cached_input_tokens
            .saturating_add(other.cached_input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(other.output_tokens);
        self.reasoning_tokens = self.reasoning_tokens.saturating_add(other.reasoning_tokens);
        self.cost_usd = match (self.cost_usd, other.cost_usd) {
            (Some(a), Some(b)) => Some(a + b),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        };
    }
}

#[derive(Debug, Clone)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    pub tools: Value,
    pub max_tokens: u64,
    pub temperature: f64,
}

#[derive(Debug, Clone)]
pub struct ChatResponse {
    pub message: ChatMessage,
    pub usage: Usage,
}

pub trait Provider {
    fn complete(&mut self, request: &ChatRequest) -> Result<ChatResponse>;
}

/// OpenAI-compatible provider (OpenAI, DeepSeek, LiteLLM, OpenRouter, ...).
pub struct OpenAiProvider {
    base_url: String,
    api_key: String,
    timeout: Duration,
}

impl OpenAiProvider {
    pub fn from_env() -> Result<Self> {
        let api_key = std::env::var("LOOM_API_KEY")
            .or_else(|_| std::env::var("OPENAI_API_KEY"))
            .context("LOOM_API_KEY (or OPENAI_API_KEY) is required for the native executor")?;
        let base_url = std::env::var("LOOM_API_BASE")
            .unwrap_or_else(|_| "https://api.openai.com/v1".to_string());
        let timeout_s = std::env::var("LOOM_API_TIMEOUT_S")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(180);
        Ok(Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key,
            timeout: Duration::from_secs(timeout_s),
        })
    }

    fn url(&self) -> String {
        format!("{}/chat/completions", self.base_url)
    }
}

impl Provider for OpenAiProvider {
    fn complete(&mut self, request: &ChatRequest) -> Result<ChatResponse> {
        let body = json!({
            "model": request.model,
            "messages": request.messages,
            "tools": request.tools,
            "tool_choice": "auto",
            "max_tokens": request.max_tokens,
            "temperature": request.temperature,
        });
        // Bodies carry no credentials (the key travels in the header), so a
        // debug dump is safe to keep next to the run logs.
        let body_text = serde_json::to_string(&body).context("encode chat-completions request")?;

        // Relays answer403/429/5xx for transient reasons ("retry later"), and
        // killing a whole run over one blip is expensive. Three attempts with
        // 1s/2s backoff; everything else fails fast.
        let mut last_retry: Option<anyhow::Error> = None;
        for attempt in 0u32..3 {
            if attempt > 0 {
                std::thread::sleep(Duration::from_secs(u64::from(attempt)));
            }
            let response = ureq::post(&self.url())
                .set("Authorization", &format!("Bearer {}", self.api_key))
                .set("Content-Type", "application/json")
                .timeout(self.timeout)
                .send_json(&body);

            match response {
                Ok(response) => {
                    let raw = response
                        .into_string()
                        .context("read chat-completions response")?;
                    debug_dump(&body_text, &raw);
                    let value: Value =
                        serde_json::from_str(&raw).context("parse chat-completions response")?;
                    return parse_response(&value);
                }
                Err(ureq::Error::Status(code, response)) => {
                    let text = response.into_string().unwrap_or_default();
                    debug_dump(&body_text, &format!("HTTP {code}: {text}"));
                    let retryable = matches!(code, 403 | 408 | 429) || (500..600).contains(&code);
                    if retryable {
                        last_retry = Some(anyhow::anyhow!(
                            "provider returned HTTP {code}: {}",
                            truncate(&text, 800)
                        ));
                        continue;
                    }
                    bail!("provider returned HTTP {code}: {}", truncate(&text, 800));
                }
                Err(error) => {
                    debug_dump(&body_text, "(transport error before any response)");
                    last_retry = Some(anyhow::Error::new(error).context("call provider"));
                    continue;
                }
            }
        }
        Err(last_retry.unwrap_or_else(|| anyhow::anyhow!("provider request failed")))
    }
}

/// Append request/response to `<logs>/provider.debug.log` when
/// `LOOM_NATIVE_DEBUG` is set. Used to diagnose relay compatibility.
fn debug_dump(request_json: &str, response_text: &str) {
    let enabled = std::env::var("LOOM_NATIVE_DEBUG")
        .map(|value| !value.is_empty() && value != "0")
        .unwrap_or(false);
    if !enabled {
        return;
    }
    let Ok(dir) = std::env::var("LOOM_LOGS_DIR") else {
        return;
    };
    if fs::create_dir_all(&dir).is_err() {
        return;
    }
    use std::io::Write as _;
    let mut file = match fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(Path::new(&dir).join("provider.debug.log"))
    {
        Ok(file) => file,
        Err(_) => return,
    };
    let _ = writeln!(file, "=== request ===\n{request_json}");
    let _ = writeln!(file, "=== response ===\n{response_text}\n");
}

fn parse_response(value: &Value) -> Result<ChatResponse> {
    let choice = value
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .context("provider response has no choices")?;
    let message = choice
        .get("message")
        .context("provider response has no message")?;

    let content = message
        .get("content")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .filter(|text| !text.is_empty());

    let mut tool_calls = Vec::new();
    if let Some(calls) = message.get("tool_calls").and_then(Value::as_array) {
        for (index, call) in calls.iter().enumerate() {
            let function = call.get("function").context("tool call has no function")?;
            let name = function
                .get("name")
                .and_then(Value::as_str)
                .context("tool call has no name")?
                .to_string();
            let arguments = match function.get("arguments") {
                Some(Value::String(text)) => text.clone(),
                Some(other) => other.to_string(),
                None => "{}".to_string(),
            };
            let id = call
                .get("id")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
                .unwrap_or_else(|| format!("call-{index}"));
            tool_calls.push(ToolCall {
                id,
                name,
                arguments,
            });
        }
    }

    let usage = value.get("usage").map(parse_usage).unwrap_or_default();

    Ok(ChatResponse {
        message: ChatMessage::assistant(content, tool_calls),
        usage,
    })
}

fn parse_usage(value: &Value) -> Usage {
    let input = value
        .get("prompt_tokens")
        .or_else(|| value.get("input_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let cached = value
        .get("prompt_tokens_details")
        .and_then(|details| details.get("cached_tokens"))
        .or_else(|| value.get("cached_input_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0)
        .min(input);
    Usage {
        input_tokens: input,
        cached_input_tokens: cached,
        output_tokens: value
            .get("completion_tokens")
            .or_else(|| value.get("output_tokens"))
            .and_then(Value::as_u64)
            .unwrap_or(0),
        reasoning_tokens: value
            .get("completion_tokens_details")
            .and_then(|details| details.get("reasoning_tokens"))
            .and_then(Value::as_u64)
            .unwrap_or(0),
        cost_usd: value
            .get("cost_usd")
            .or_else(|| value.get("cost"))
            .and_then(Value::as_f64),
    }
}

/// Deterministic provider used by tests and offline canaries.
///
/// The script is a JSON array (or `{"responses": [...]}`) of:
/// `{"content": "...", "tool_calls": [{"name": "...", "arguments": {...}}], "usage": {...}}`.
pub struct ScriptedProvider {
    responses: Vec<ScriptResponse>,
    cursor: usize,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct ScriptResponse {
    content: Option<String>,
    tool_calls: Vec<ScriptToolCall>,
    usage: Usage,
}

#[derive(Debug, Clone, Deserialize)]
struct ScriptToolCall {
    #[serde(default)]
    id: Option<String>,
    name: String,
    #[serde(default)]
    arguments: Value,
}

impl ScriptedProvider {
    pub fn from_file(path: &Path) -> Result<Self> {
        let text = fs::read_to_string(path)
            .with_context(|| format!("read native script {}", path.display()))?;
        Self::parse(&text).with_context(|| format!("parse native script {}", path.display()))
    }

    pub fn parse(text: &str) -> Result<Self> {
        let value: Value = serde_json::from_str(text)?;
        let array = if let Some(array) = value.as_array() {
            array.clone()
        } else {
            value
                .get("responses")
                .and_then(Value::as_array)
                .cloned()
                .context("native script must be an array or {\"responses\": [...]}")?
        };
        let responses = array
            .into_iter()
            .map(serde_json::from_value::<ScriptResponse>)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            responses,
            cursor: 0,
        })
    }
}

impl Provider for ScriptedProvider {
    fn complete(&mut self, _request: &ChatRequest) -> Result<ChatResponse> {
        let response = self
            .responses
            .get(self.cursor)
            .with_context(|| format!("native script exhausted at step {}", self.cursor + 1))?;
        self.cursor += 1;

        let tool_calls = response
            .tool_calls
            .iter()
            .enumerate()
            .map(|(index, call)| ToolCall {
                id: call
                    .id
                    .clone()
                    .unwrap_or_else(|| format!("call-{}", self.cursor * 100 + index)),
                name: call.name.clone(),
                arguments: if call.arguments.is_null() {
                    "{}".to_string()
                } else {
                    call.arguments.to_string()
                },
            })
            .collect();

        Ok(ChatResponse {
            message: ChatMessage::assistant(response.content.clone(), tool_calls),
            usage: response.usage.clone(),
        })
    }
}

fn truncate(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let mut end = max;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_openai_response() {
        let value = json!({
            "choices": [{
                "message": {
                    "content": null,
                    "tool_calls": [{
                        "id": "call_1",
                        "type": "function",
                        "function": {"name": "read", "arguments": "{\"path\":\"a.rs\"}"}
                    }]
                }
            }],
            "usage": {
                "prompt_tokens": 120,
                "completion_tokens": 30,
                "prompt_tokens_details": {"cached_tokens": 80}
            }
        });
        let response = parse_response(&value).expect("parse");
        assert_eq!(response.usage.input_tokens, 120);
        assert_eq!(response.usage.cached_input_tokens, 80);
        assert_eq!(response.usage.output_tokens, 30);
        assert_eq!(
            response.message.tool_calls.as_ref().unwrap()[0].name,
            "read"
        );
    }

    #[test]
    fn scripted_provider_replays_responses() {
        let mut provider = ScriptedProvider::parse(
            r#"[{"content":"hello"},{"tool_calls":[{"name":"finish","arguments":{"status":"done"}}]}]"#,
        )
        .expect("script");
        let request = ChatRequest {
            model: "test".into(),
            messages: vec![],
            tools: json!([]),
            max_tokens: 10,
            temperature: 0.0,
        };
        let first = provider.complete(&request).expect("first");
        assert_eq!(first.message.content.as_deref(), Some("hello"));
        let second = provider.complete(&request).expect("second");
        assert_eq!(
            second.message.tool_calls.as_ref().unwrap()[0].name,
            "finish"
        );
        assert!(provider.complete(&request).is_err());
    }
}
