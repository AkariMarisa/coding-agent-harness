//! Live relay probe (ignored by default): find at least one model whose
//! tool history the relay accepts, across several fresh sessions, and print
//! a per-shape diagnosis for the ones that fail. Run through
//! `scripts/relay-probe.sh`, which maps `.env.local` credentials onto
//! `LOOM_API_BASE`/`LOOM_API_KEY`.
//!
//! Model list: `LOOM_PROBE_MODELS` (comma separated), default
//! `deepseek-flash,mimo-v2.6-pro`.
//!
//! Hard token cap: every request asks for at most 512 completion tokens, each
//! conversation is a fixed ~300-token payload, and the final assert charges
//! worst case for every request (including raw ones) against 100k.

use std::time::Duration;

use loom_native::{ChatMessage, ChatRequest, OpenAiProvider, Provider, ToolCall};
use serde_json::{json, Value};

const SYSTEM: &str = "You are a probe. Reply with one short word.";
const USER: &str = "Reply with the single word: ok";
/// Completion cap per request: 512 tokens x ~25 requests <= 13k output.
const MAX_TOKENS: u64 = 512;
/// Fresh sessions per model for the tool-history shape: one poisoned session
/// must not decide the result.
const ATTEMPTS: usize = 3;

fn bash_tool() -> Value {
    json!([{
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
    }])
}

fn tool_message(call_id: &str, with_name: bool) -> ChatMessage {
    ChatMessage {
        role: "tool".into(),
        content: Some("exit=0\n/work".into()),
        tool_calls: None,
        tool_call_id: Some(call_id.into()),
        name: with_name.then(|| "bash".to_string()),
    }
}

fn one_call() -> Vec<ToolCall> {
    vec![ToolCall {
        id: "call-1".into(),
        name: "bash".into(),
        arguments: r#"{"cmd":"pwd"}"#.into(),
    }]
}

fn two_calls() -> Vec<ToolCall> {
    vec![
        ToolCall {
            id: "call-1".into(),
            name: "bash".into(),
            arguments: r#"{"cmd":"pwd"}"#.into(),
        },
        ToolCall {
            id: "call-2".into(),
            name: "bash".into(),
            arguments: r#"{"cmd":"ls"}"#.into(),
        },
    ]
}

fn base_messages() -> Vec<ChatMessage> {
    vec![ChatMessage::system(SYSTEM), ChatMessage::user(USER)]
}

/// Single-shot POST on a brand-new agent: no connection reuse, so every
/// attempt is a fresh session (one poisoned upstream session must not stick).
fn post(url: &str, headers: &[(&str, String)], body: &Value) -> (u16, String) {
    let agent = ureq::Agent::new();
    let mut req = agent
        .post(url)
        .set("Content-Type", "application/json")
        .timeout(Duration::from_secs(60));
    for (name, value) in headers {
        req = req.set(name, value);
    }
    match req.send_json(body) {
        Ok(response) => (
            response.status(),
            response.into_string().unwrap_or_default(),
        ),
        Err(ureq::Error::Status(code, response)) => {
            (code, response.into_string().unwrap_or_default())
        }
        Err(error) => (0, format!("transport: {error}")),
    }
}

fn brief(body: &str) -> String {
    body.chars()
        .take(200)
        .collect::<String>()
        .replace('\n', " ")
}

#[test]
#[ignore = "live relay probe; run scripts/relay-probe.sh"]
fn probes_relay_message_shapes() {
    let models: Vec<String> = std::env::var("LOOM_PROBE_MODELS")
        .unwrap_or_else(|_| "deepseek-flash,mimo-v2.6-pro".to_string())
        .split(',')
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
        .collect();
    assert!(!models.is_empty(), "LOOM_PROBE_MODELS must not be empty");

    let base = std::env::var("LOOM_API_BASE").expect("LOOM_API_BASE");
    let base = base.trim_end_matches('/').to_string();
    let key = std::env::var("LOOM_API_KEY").expect("LOOM_API_KEY");
    let auth: Vec<(&str, String)> = vec![("authorization", format!("Bearer {key}"))];

    let mut requests = 0u32;
    let mut provider_input = 0u64;
    let mut provider_output = 0u64;
    let mut working: Vec<&str> = Vec::new();
    let mut failures: Vec<String> = Vec::new();

    let chat_raw = |label: &str,
                    model: &str,
                    messages: Vec<ChatMessage>,
                    requests: &mut u32,
                    failures: &mut Vec<String>|
     -> bool {
        let body = json!({
            "model": model,
            "messages": serde_json::to_value(&messages).expect("messages"),
            "tools": bash_tool(),
            "tool_choice": "auto",
            "max_tokens": MAX_TOKENS,
            "temperature": 0.0,
        });
        *requests += 1;
        let (status, text) = post(&format!("{base}/chat/completions"), &auth, &body);
        let ok = (200..300).contains(&status);
        println!(
            "{} {label}: HTTP {status} {}",
            if ok { "OK  " } else { "FAIL" },
            brief(&text)
        );
        if !ok {
            failures.push(format!("{label} [{model}]"));
        }
        ok
    };

    for (index, model) in models.iter().enumerate() {
        println!("== model {model} ==");

        // Sanity: plain conversation through our real client path.
        let mut provider =
            OpenAiProvider::from_env().expect("LOOM_API_BASE / LOOM_API_KEY must be set");
        let request = ChatRequest {
            model: model.clone(),
            messages: base_messages(),
            tools: bash_tool(),
            max_tokens: MAX_TOKENS,
            temperature: 0.0,
        };
        match provider.complete(&request) {
            Ok(response) => {
                provider_input += response.usage.input_tokens;
                provider_output += response.usage.output_tokens;
                println!(
                    "OK   provider_baseline [{model}] (input={} cached={} output={})",
                    response.usage.input_tokens,
                    response.usage.cached_input_tokens,
                    response.usage.output_tokens
                );
            }
            Err(error) => {
                println!("FAIL provider_baseline [{model}]: {error:#}");
                failures.push(format!("provider_baseline [{model}]"));
            }
        }

        // The decisive question: does ANY fresh session accept tool history?
        let mut tool_history_ok = false;
        for attempt in 1..=ATTEMPTS {
            if attempt > 1 {
                std::thread::sleep(Duration::from_secs(1));
            }
            let mut messages = base_messages();
            messages.push(ChatMessage::assistant(None, one_call()));
            messages.push(tool_message("call-1", true));
            if chat_raw(
                &format!("tool_history attempt {attempt}"),
                model,
                messages,
                &mut requests,
                &mut failures,
            ) {
                tool_history_ok = true;
                break;
            }
        }
        if tool_history_ok {
            working.push(model.as_str());
            continue;
        }

        // Detailed shape sweep for the first model only; the others just get
        // the decisive question plus the other wires when they fail.
        if index == 0 {
            let mut no_name = base_messages();
            no_name.push(ChatMessage::assistant(None, one_call()));
            no_name.push(tool_message("call-1", false));
            chat_raw(
                "chat_tool_history_no_name",
                model,
                no_name,
                &mut requests,
                &mut failures,
            );

            let mut with_content = base_messages();
            with_content.push(ChatMessage::assistant(
                Some("running".to_string()),
                one_call(),
            ));
            with_content.push(tool_message("call-1", true));
            chat_raw(
                "chat_assistant_content_tool",
                model,
                with_content,
                &mut requests,
                &mut failures,
            );

            let mut parallel = base_messages();
            parallel.push(ChatMessage::assistant(None, two_calls()));
            parallel.push(tool_message("call-1", true));
            parallel.push(tool_message("call-2", true));
            chat_raw(
                "chat_parallel_tool_results",
                model,
                parallel,
                &mut requests,
                &mut failures,
            );

            let mut only_calls = base_messages();
            only_calls.push(ChatMessage::assistant(None, one_call()));
            chat_raw(
                "chat_assistant_toolcalls_only",
                model,
                only_calls,
                &mut requests,
                &mut failures,
            );

            // Control: plain multi-turn with ordinary assistant text and no
            // tool payloads. A pass here isolates the rejection to tool data.
            let mut text_history = base_messages();
            text_history.push(ChatMessage::assistant(
                Some("hello".to_string()),
                Vec::new(),
            ));
            text_history.push(ChatMessage::user("Say ok".to_string()));
            chat_raw(
                "chat_assistant_text_history",
                model,
                text_history,
                &mut requests,
                &mut failures,
            );
        }

        // Other protocol surfaces for a model that rejects chat tool history.
        // A pass on either wire means the model is usable (via that wire).
        let mut wire_ok = false;
        let messages_body = json!({
            "model": model,
            "max_tokens": MAX_TOKENS,
            "system": SYSTEM,
            "messages": [
                {"role": "user", "content": USER},
                {"role": "assistant", "content": [
                    {"type": "tool_use", "id": "call-1", "name": "bash", "input": {"cmd": "pwd"}}
                ]},
                {"role": "user", "content": [
                    {"type": "tool_result", "tool_use_id": "call-1", "content": "exit=0"}
                ]}
            ],
            "tools": [{
                "name": "bash",
                "description": "Run a shell command.",
                "input_schema": {
                    "type": "object",
                    "properties": {"cmd": {"type": "string"}},
                    "required": ["cmd"]
                }
            }]
        });
        let messages_headers: Vec<(&str, String)> = vec![
            ("x-api-key", key.clone()),
            ("authorization", format!("Bearer {key}")),
            ("anthropic-version", "2023-06-01".to_string()),
        ];
        requests += 1;
        let (status, text) = post(
            &format!("{base}/messages"),
            &messages_headers,
            &messages_body,
        );
        println!(
            "{} anthropic_messages [{model}]: HTTP {status} {}",
            if (200..300).contains(&status) {
                "OK  "
            } else {
                "FAIL"
            },
            brief(&text)
        );
        if (200..300).contains(&status) {
            wire_ok = true;
        } else {
            failures.push(format!("anthropic_messages [{model}]"));
        }

        let responses_body = json!({
            "model": model,
            "max_output_tokens": MAX_TOKENS,
            "input": [
                {"type": "message", "role": "user", "content": [
                    {"type": "input_text", "text": USER}
                ]},
                {"type": "function_call", "call_id": "call-1", "name": "bash",
                 "arguments": "{\"cmd\":\"pwd\"}"},
                {"type": "function_call_output", "call_id": "call-1", "output": "exit=0"}
            ],
            "tools": [{
                "type": "function",
                "name": "bash",
                "description": "Run a shell command.",
                "parameters": {
                    "type": "object",
                    "properties": {"cmd": {"type": "string"}},
                    "required": ["cmd"]
                }
            }]
        });
        requests += 1;
        let (status, text) = post(&format!("{base}/responses"), &auth, &responses_body);
        println!(
            "{} openai_responses [{model}]: HTTP {status} {}",
            if (200..300).contains(&status) {
                "OK  "
            } else {
                "FAIL"
            },
            brief(&text)
        );
        if (200..300).contains(&status) {
            wire_ok = true;
        } else {
            failures.push(format!("openai_responses [{model}]"));
        }
        if wire_ok {
            working.push(model.as_str());
        }
    }

    // Token accounting: raw probes report no parsed usage, so charge every
    // raw request its worst case (small prompt + MAX_TOKENS); provider
    // requests already added their real usage. `requests` counts raw posts
    // only (the provider path does not increment it).
    let raw_requests = requests;
    let total_input = provider_input + 300 * u64::from(raw_requests);
    let total_output = provider_output + MAX_TOKENS * u64::from(raw_requests);
    println!(
        "probe totals (worst case): requests={requests} input={total_input} output={total_output}"
    );
    println!("models with working tool history: {working:?}");
    if !failures.is_empty() {
        println!("diagnostic failures: {failures:?}");
    }
    assert!(
        total_input + total_output <= 100_000,
        "probe exceeded 100k tokens: {}",
        total_input + total_output
    );
    assert!(
        !working.is_empty(),
        "no probed model accepted tool history; failures: {failures:?}"
    );
}
