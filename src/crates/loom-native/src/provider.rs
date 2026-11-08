//! Provider boundary for the native executor.
//!
//! The executor speaks an OpenAI-compatible chat-completions protocol. Tests
//! use a scripted provider so the loop can run without a network or API key.

use std::fs;
use std::path::Path;
use std::time::Duration;

use anyhow::{bail, Context as _, Result};
use loom_core::{ModelCapability, Wire};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
    /// Thinking echo (DeepSeek-style): when a provider returns
    /// `reasoning_content`, official endpoints reject tool history that does
    /// not pass it back, so we carry it verbatim.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
}

impl ChatMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: "system".into(),
            content: Some(content.into()),
            tool_calls: None,
            tool_call_id: None,
            name: None,
            reasoning_content: None,
        }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: "user".into(),
            content: Some(content.into()),
            tool_calls: None,
            tool_call_id: None,
            name: None,
            reasoning_content: None,
        }
    }

    pub fn assistant(content: Option<String>, tool_calls: Vec<ToolCall>) -> Self {
        Self {
            role: "assistant".into(),
            content,
            tool_calls: (!tool_calls.is_empty()).then_some(tool_calls),
            tool_call_id: None,
            name: None,
            reasoning_content: None,
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
            reasoning_content: None,
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

/// LLM wire spoken by the native executor. `chat` is OpenAI
/// chat-completions; `messages` is the Anthropic Messages shape, needed for
/// relay-aggregated models whose upstream speaks Anthropic natively (their
/// chat translation of tool history is what403/400s on such relays).
pub struct OpenAiProvider {
    base_url: String,
    api_key: String,
    timeout: Duration,
    wire: Wire,
}

impl OpenAiProvider {
    pub fn from_env() -> Result<Self> {
        Self::from_env_with_capability(None)
    }

    /// Build from the selected model's capability: the wire, the endpoint,
    /// and which env var holds the key come from `[[models]]` config; key
    /// *values* still only come from the environment.
    pub fn from_env_with_capability(capability: Option<&ModelCapability>) -> Result<Self> {
        let wire = capability
            .map(|capability| capability.wire)
            .unwrap_or_default();
        if matches!(wire, Wire::Responses) {
            bail!("wire=responses is not implemented in the native executor; use chat or messages");
        }
        let (base_url, api_key) = resolve_endpoint(
            capability,
            std::env::var("LOOM_API_BASE").ok(),
            std::env::var("LOOM_API_KEY")
                .ok()
                .or_else(|| std::env::var("OPENAI_API_KEY").ok()),
            &|name| std::env::var(name).ok(),
        )?;
        Ok(Self {
            base_url,
            api_key,
            timeout: Duration::from_secs(timeout_from_env()),
            wire,
        })
    }

    /// Force a wire without touching model config (tests and probes).
    pub fn from_env_with_wire(wire: Wire) -> Result<Self> {
        if matches!(wire, Wire::Responses) {
            bail!("wire=responses is not implemented in the native executor; use chat or messages");
        }
        let mut provider = Self::from_env_with_capability(None)?;
        provider.wire = wire;
        Ok(provider)
    }

    fn url(&self) -> String {
        match self.wire {
            Wire::Messages => format!("{}/messages", self.base_url),
            _ => format!("{}/chat/completions", self.base_url),
        }
    }

    /// Auth headers per wire. Messages sends both key styles plus the
    /// version header: the relay probe needed all three to pass.
    fn headers(&self) -> Vec<(&'static str, String)> {
        let bearer = format!("Bearer {}", self.api_key);
        match self.wire {
            Wire::Messages => vec![
                ("x-api-key", self.api_key.clone()),
                ("authorization", bearer),
                ("anthropic-version", "2023-06-01".to_string()),
            ],
            _ => vec![("authorization", bearer)],
        }
    }
}

fn timeout_from_env() -> u64 {
    std::env::var("LOOM_API_TIMEOUT_S")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(180)
}

/// Endpoint resolution order: per-model config first, then environment, then
/// the OpenAI default. `key_lookup` reads the variable named by `key_env`;
/// the value itself never comes from config.
fn resolve_endpoint(
    capability: Option<&ModelCapability>,
    base_env: Option<String>,
    global_key: Option<String>,
    key_lookup: &dyn Fn(&str) -> Option<String>,
) -> Result<(String, String)> {
    let base_url = capability
        .and_then(|capability| capability.base_url.clone())
        .or(base_env)
        .unwrap_or_else(|| "https://api.openai.com/v1".to_string());
    let api_key = match capability.and_then(|capability| capability.key_env.as_deref()) {
        Some(name) => key_lookup(name)
            .filter(|value| !value.trim().is_empty())
            .with_context(|| {
                format!("key_env = {name} is not set for this model; export it (see .env.example)")
            })?,
        None => global_key
            .context("LOOM_API_KEY (or OPENAI_API_KEY) is required for the native executor")?,
    };
    Ok((base_url.trim_end_matches('/').to_string(), api_key))
}

impl Provider for OpenAiProvider {
    fn complete(&mut self, request: &ChatRequest) -> Result<ChatResponse> {
        let body = match self.wire {
            Wire::Messages => build_messages_body(request),
            _ => build_chat_body(request),
        };
        // Bodies carry no credentials (the key travels in the header), so a
        // debug dump is safe to keep next to the run logs.
        let body_text = serde_json::to_string(&body).context("encode provider request")?;
        let headers = self.headers();

        // Relays answer403/429/5xx for transient reasons ("retry later"), and
        // killing a whole run over one blip is expensive. Three attempts with
        // 1s/2s backoff; everything else fails fast.
        let mut last_retry: Option<anyhow::Error> = None;
        for attempt in 0u32..3 {
            if attempt > 0 {
                std::thread::sleep(Duration::from_secs(u64::from(attempt)));
            }
            let mut req = ureq::post(&self.url())
                .set("Content-Type", "application/json")
                .timeout(self.timeout);
            for (name, value) in &headers {
                req = req.set(name, value);
            }
            let response = req.send_json(&body);

            match response {
                Ok(response) => {
                    let raw = response.into_string().context("read provider response")?;
                    debug_dump(&body_text, &raw);
                    let value: Value =
                        serde_json::from_str(&raw).context("parse provider response")?;
                    return match self.wire {
                        Wire::Messages => parse_messages_response(&value),
                        _ => parse_response(&value),
                    };
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

/// Build the chat-wire request body. Public so the relay probe sends exactly
/// the bytes the native executor would.
pub fn build_chat_body(request: &ChatRequest) -> Value {
    json!({
        "model": request.model,
        "messages": request
            .messages
            .iter()
            .map(chat_message_json)
            .collect::<Vec<Value>>(),
        "tools": request.tools,
        "tool_choice": "auto",
        "max_tokens": request.max_tokens,
        "temperature": request.temperature,
    })
}

/// Chat-wire message shape. `ChatMessage` is wire-neutral, so tool calls are
/// serialized explicitly: the OpenAI format nests them as
/// `{id, type: "function", function: {name, arguments}}`. Sending the flat
/// internal shape is what official endpoints reject with
/// `messages[N]: missing field 'type'`.
fn chat_message_json(message: &ChatMessage) -> Value {
    match message.role.as_str() {
        "assistant" => {
            let mut out = json!({ "role": "assistant" });
            out["content"] = match &message.content {
                Some(text) => json!(text),
                None => Value::Null,
            };
            if let Some(reasoning) = &message.reasoning_content {
                out["reasoning_content"] = json!(reasoning);
            }
            if let Some(calls) = &message.tool_calls {
                out["tool_calls"] = Value::Array(
                    calls
                        .iter()
                        .map(|call| {
                            json!({
                                "id": call.id,
                                "type": "function",
                                "function": {
                                    "name": call.name,
                                    "arguments": call.arguments,
                                },
                            })
                        })
                        .collect(),
                );
            }
            out
        }
        _ => json!(message),
    }
}

/// Anthropic Messages body from our wire-neutral history.
///
/// Mapping: `system` messages become the top-level `system` field; `tool`
/// messages become `tool_result` blocks inside the user message that follows
/// the assistant `tool_use` turn (consecutive results are batched, as the
/// API requires); `temperature` is omitted because thinking models reject
/// non-default values and the relay default is fine.
fn build_messages_body(request: &ChatRequest) -> Value {
    let mut system: Vec<String> = Vec::new();
    let mut messages: Vec<Value> = Vec::new();
    let mut tool_results: Vec<Value> = Vec::new();

    for message in &request.messages {
        match message.role.as_str() {
            "system" => {
                if let Some(text) = &message.content {
                    system.push(text.clone());
                }
            }
            "user" => {
                flush_tool_results(&mut messages, &mut tool_results);
                messages.push(json!({
                    "role": "user",
                    "content": message.content.clone().unwrap_or_default(),
                }));
            }
            "assistant" => {
                flush_tool_results(&mut messages, &mut tool_results);
                let mut blocks: Vec<Value> = Vec::new();
                if let Some(text) = message.content.as_deref().filter(|text| !text.is_empty()) {
                    blocks.push(json!({"type": "text", "text": text}));
                }
                let mut has_calls = false;
                if let Some(calls) = &message.tool_calls {
                    for call in calls {
                        has_calls = true;
                        let input: Value = serde_json::from_str(&call.arguments)
                            .unwrap_or_else(|_| json!({ "raw": call.arguments }));
                        blocks.push(json!({
                            "type": "tool_use",
                            "id": call.id,
                            "name": call.name,
                            "input": input,
                        }));
                    }
                }
                if blocks.is_empty() {
                    continue;
                }
                if has_calls {
                    messages.push(json!({ "role": "assistant", "content": blocks }));
                } else {
                    messages.push(json!({
                        "role": "assistant",
                        "content": message.content.clone().unwrap_or_default(),
                    }));
                }
            }
            "tool" => {
                tool_results.push(json!({
                    "type": "tool_result",
                    "tool_use_id": message.tool_call_id.clone().unwrap_or_default(),
                    "content": message.content.clone().unwrap_or_default(),
                }));
            }
            _ => {}
        }
    }
    flush_tool_results(&mut messages, &mut tool_results);

    let tools: Vec<Value> = request
        .tools
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| {
                    let function = entry.get("function")?;
                    let name = function.get("name")?.clone();
                    let input_schema = function.get("parameters")?.clone();
                    let description = function
                        .get("description")
                        .cloned()
                        .unwrap_or_else(|| Value::String(String::new()));
                    Some(json!({
                        "name": name,
                        "description": description,
                        "input_schema": input_schema,
                    }))
                })
                .collect()
        })
        .unwrap_or_default();

    let mut body = json!({
        "model": request.model,
        "max_tokens": request.max_tokens,
        "messages": messages,
    });
    if !system.is_empty() {
        body["system"] = json!(system.join("\n\n"));
    }
    if !tools.is_empty() {
        body["tools"] = Value::Array(tools);
    }
    body
}

fn flush_tool_results(messages: &mut Vec<Value>, tool_results: &mut Vec<Value>) {
    if !tool_results.is_empty() {
        let blocks = std::mem::take(tool_results);
        messages.push(json!({ "role": "user", "content": blocks }));
    }
}

/// Parse an Anthropic Messages response into our wire-neutral shape.
/// `thinking` / `redacted_thinking` blocks are dropped: the relay accepts a
/// history without them (probe-verified) and we do not keep signatures.
fn parse_messages_response(value: &Value) -> Result<ChatResponse> {
    let content = value
        .get("content")
        .and_then(Value::as_array)
        .context("messages response has no content")?;
    let mut text = String::new();
    let mut tool_calls = Vec::new();
    for block in content {
        match block.get("type").and_then(Value::as_str) {
            Some("text") => {
                if let Some(part) = block.get("text").and_then(Value::as_str) {
                    text.push_str(part);
                }
            }
            Some("tool_use") => {
                let id = block
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let name = block
                    .get("name")
                    .and_then(Value::as_str)
                    .context("tool_use block has no name")?
                    .to_string();
                let input = block.get("input").cloned().unwrap_or_else(|| json!({}));
                tool_calls.push(ToolCall {
                    id,
                    name,
                    arguments: input.to_string(),
                });
            }
            _ => {}
        }
    }
    let usage = value
        .get("usage")
        .map(parse_messages_usage)
        .unwrap_or_default();
    Ok(ChatResponse {
        message: ChatMessage::assistant((!text.is_empty()).then_some(text), tool_calls),
        usage,
    })
}

/// Anthropic usage: `input_tokens` counts only non-cached input, so the
/// wire-neutral `input_tokens` (total, incl. cached, matching the chat wire
/// semantics that `estimate_cost` assumes) is the sum of the three buckets.
fn parse_messages_usage(value: &Value) -> Usage {
    let field = |name: &str| value.get(name).and_then(Value::as_u64).unwrap_or(0);
    let cached = field("cache_read_input_tokens");
    let creation = field("cache_creation_input_tokens");
    Usage {
        input_tokens: field("input_tokens") + cached + creation,
        cached_input_tokens: cached,
        output_tokens: field("output_tokens"),
        reasoning_tokens: 0,
        cost_usd: None,
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
    let mut chat_message = ChatMessage::assistant(content, tool_calls);
    // Keep the thinking echo: official DeepSeek rejects tool history that
    // drops `reasoning_content` ("must be passed back to the API").
    chat_message.reasoning_content = message
        .get("reasoning_content")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);

    Ok(ChatResponse {
        message: chat_message,
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
    fn complete(&mut self, request: &ChatRequest) -> Result<ChatResponse> {
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

        let chat_response = ChatResponse {
            message: ChatMessage::assistant(response.content.clone(), tool_calls),
            usage: response.usage.clone(),
        };
        // Mirror the live provider: with LOOM_NATIVE_DEBUG set, dump the exact
        // (projected) request body so offline runs can assert on it too.
        if let Ok(body) = serde_json::to_string(&build_chat_body(request)) {
            let text = json!({
                "scripted": true,
                "message": chat_response.message,
                "usage": chat_response.usage,
            })
            .to_string();
            debug_dump(&body, &text);
        }
        Ok(chat_response)
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
                    "reasoning_content": "because",
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
        assert_eq!(
            response.message.reasoning_content.as_deref(),
            Some("because"),
            "the thinking echo must survive parsing so it can be sent back"
        );
    }

    fn sample_request(messages: Vec<ChatMessage>) -> ChatRequest {
        ChatRequest {
            model: "mimo-v2.6-pro".into(),
            messages,
            tools: json!([{
                "type": "function",
                "function": {
                    "name": "bash",
                    "description": "Run a shell command.",
                    "parameters": {
                        "type": "object",
                        "properties": {"cmd": {"type": "string"}},
                        "required": ["cmd"]
                    }
                }
            }]),
            max_tokens: 512,
            temperature: 0.0,
        }
    }

    fn call(id: &str) -> ToolCall {
        ToolCall {
            id: id.to_string(),
            name: "bash".to_string(),
            arguments: r#"{"cmd":"pwd"}"#.to_string(),
        }
    }

    #[test]
    fn builds_messages_body_from_chat_history() {
        let request = sample_request(vec![
            ChatMessage::system("sys"),
            ChatMessage::user("hello"),
            ChatMessage::assistant(None, vec![call("call-1")]),
            ChatMessage::tool("call-1", "bash", "exit=0"),
        ]);
        let body = build_messages_body(&request);
        assert_eq!(body["system"], json!("sys"));
        assert_eq!(body["max_tokens"], json!(512));
        // No temperature: thinking models reject non-default values and the
        // relay default is fine (probe-verified).
        assert!(body.get("temperature").is_none());

        let messages = body["messages"].as_array().expect("messages");
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[1]["role"], json!("assistant"));
        assert_eq!(messages[1]["content"][0]["type"], json!("tool_use"));
        assert_eq!(messages[1]["content"][0]["input"], json!({"cmd":"pwd"}));
        assert_eq!(messages[2]["role"], json!("user"));
        assert_eq!(messages[2]["content"][0]["type"], json!("tool_result"));
        assert_eq!(messages[2]["content"][0]["tool_use_id"], json!("call-1"));

        // OpenAI function tools convert to Anthropic input_schema.
        assert_eq!(body["tools"][0]["name"], json!("bash"));
        assert!(body["tools"][0].get("function").is_none());
        assert_eq!(body["tools"][0]["input_schema"]["type"], json!("object"));
    }

    #[test]
    fn batches_consecutive_tool_results_into_one_user_message() {
        let request = sample_request(vec![
            ChatMessage::user("go"),
            ChatMessage::assistant(None, vec![call("call-1"), call("call-2")]),
            ChatMessage::tool("call-1", "bash", "one"),
            ChatMessage::tool("call-2", "bash", "two"),
        ]);
        let body = build_messages_body(&request);
        let messages = body["messages"].as_array().expect("messages");
        assert_eq!(messages.len(), 3, "system is hoisted out of messages");
        let results = messages[2]["content"].as_array().expect("results");
        assert_eq!(results.len(), 2, "parallel results batch into one message");
        assert_eq!(results[1]["tool_use_id"], json!("call-2"));
    }

    #[test]
    fn chat_body_nests_tool_calls_in_openai_format() {
        let mut assistant =
            ChatMessage::assistant(Some("running".to_string()), vec![call("call-1")]);
        assistant.reasoning_content = Some("because".to_string());
        let request = sample_request(vec![
            ChatMessage::system("sys"),
            ChatMessage::user("go"),
            assistant,
            ChatMessage::tool("call-1", "bash", "exit=0"),
        ]);
        let body = build_chat_body(&request);
        let messages = body["messages"].as_array().expect("messages");
        let tool_call = &messages[2]["tool_calls"][0];
        assert_eq!(tool_call["type"], json!("function"));
        assert_eq!(tool_call["function"]["name"], json!("bash"));
        assert!(
            tool_call["function"]["arguments"].is_string(),
            "arguments stay a raw JSON string on the wire"
        );
        assert!(
            tool_call.get("name").is_none() && tool_call.get("arguments").is_none(),
            "flat internal fields must not leak onto the wire"
        );
        assert_eq!(
            messages[2]["reasoning_content"],
            json!("because"),
            "the thinking echo must be sent back verbatim"
        );
        assert_eq!(messages[3]["role"], json!("tool"));
        assert_eq!(messages[3]["tool_call_id"], json!("call-1"));
    }

    #[test]
    fn parses_messages_response_with_thinking_and_tools() {
        let value = json!({
            "content": [
                {"type": "thinking", "text": null},
                {"type": "text", "text": "running "},
                {"type": "text", "text": "now"},
                {"type": "tool_use", "id": "call-9", "name": "bash",
                 "input": {"cmd": "ls"}}
            ],
            "stop_reason": "tool_use",
            "usage": {
                "input_tokens": 100,
                "cache_read_input_tokens": 400,
                "cache_creation_input_tokens": 20,
                "output_tokens": 50
            }
        });
        let response = parse_messages_response(&value).expect("parse");
        assert_eq!(response.message.content.as_deref(), Some("running now"));
        let calls = response.message.tool_calls.as_ref().expect("calls");
        assert_eq!(calls[0].name, "bash");
        assert_eq!(
            serde_json::from_str::<Value>(&calls[0].arguments).expect("args"),
            json!({"cmd": "ls"})
        );
        // Anthropic input_tokens excludes cache buckets; the wire-neutral
        // total (what estimate_cost assumes) is their sum.
        assert_eq!(response.usage.input_tokens, 520);
        assert_eq!(response.usage.cached_input_tokens, 400);
        assert_eq!(response.usage.output_tokens, 50);
    }

    #[test]
    fn responses_wire_is_rejected_early() {
        assert!(
            OpenAiProvider::from_env_with_wire(loom_core::Wire::Responses).is_err(),
            "responses wire must fail fast with a clear message"
        );
    }

    #[test]
    fn endpoint_prefers_model_config_over_environment() {
        let capability = ModelCapability {
            alias: "deepseek-official".into(),
            base_url: Some("https://api.deepseek.com/v1/".into()),
            key_env: Some("DEEPSEEK_OFFICIAL_API_KEY".into()),
            ..ModelCapability::default()
        };
        let lookup =
            |name: &str| (name == "DEEPSEEK_OFFICIAL_API_KEY").then(|| "sk-official".to_string());
        let (base, key) = resolve_endpoint(
            Some(&capability),
            Some("https://relay.example.com/v1".into()),
            Some("sk-relay".into()),
            &lookup,
        )
        .expect("resolve");
        assert_eq!(
            base, "https://api.deepseek.com/v1",
            "trailing slash trimmed"
        );
        assert_eq!(key, "sk-official", "key_env wins over the global key");
    }

    #[test]
    fn endpoint_falls_back_to_environment_without_capability() {
        let (base, key) = resolve_endpoint(
            None,
            Some("https://relay.example.com/v1".into()),
            Some("sk-relay".into()),
            &|_| None,
        )
        .expect("resolve");
        assert_eq!(base, "https://relay.example.com/v1");
        assert_eq!(key, "sk-relay");
    }

    #[test]
    fn missing_key_env_is_a_clear_error() {
        let capability = ModelCapability {
            alias: "deepseek-official".into(),
            key_env: Some("DEEPSEEK_OFFICIAL_API_KEY".into()),
            ..ModelCapability::default()
        };
        let error = resolve_endpoint(Some(&capability), None, Some("sk-relay".into()), &|_| None)
            .expect_err("missing key must fail");
        assert!(
            error.to_string().contains("DEEPSEEK_OFFICIAL_API_KEY"),
            "error must name the missing variable: {error:#}"
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
