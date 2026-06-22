//! OpenAI-compatible adapter — `POST {base_url}/chat/completions`.
//!
//! One adapter, base-URL swap, covers GPT, GLM (Z.ai/Zhipu), DeepSeek,
//! OpenRouter, and local servers (Ollama/vLLM/LM Studio). It implements the same
//! [`LlmProvider`] trait as the native Anthropic adapter, so the loop and tools
//! are untouched — this is what proves the boundary abstraction.
//!
//! Quirks handled: tool-call `arguments` arrive as a JSON **string** (parsed
//! here); tool results reply as separate `{role:"tool"}` messages; reasoning
//! effort maps to `reasoning_effort` only where supported; and DeepSeek thinking
//! mode requires the prior assistant's `reasoning_content` to be replayed on
//! tool-continuation turns (or it returns HTTP 400).

use std::sync::{Arc, atomic::AtomicBool};
use std::time::Duration;

use serde_json::{Value, json};

use super::provider::{LlmProvider, ProviderCaps};
use super::types::{
    AssistantTurn, ChatMessage, ContentBlock, Effort, LlmConfig, LlmError, ReasoningBlob, Role,
    StopReason, StreamEvent, ToolCall, ToolDef, Usage,
};

const MAX_RESPONSE_BYTES: u64 = 16 * 1024 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(300);

/// An OpenAI-compatible chat provider. Holds the key, endpoint, model, resolved
/// capabilities, and whether to round-trip `reasoning_content` on replay.
pub struct OpenAiCompatProvider {
    api_key: String,
    base_url: String,
    model: String,
    caps: ProviderCaps,
    /// DeepSeek-style: re-inject the prior assistant's `reasoning_content` on
    /// replay (required on tool-continuation turns, or the API 400s).
    reasoning_replay: bool,
    id: String,
}

impl OpenAiCompatProvider {
    pub fn new(
        api_key: String,
        base_url: String,
        model: String,
        caps: ProviderCaps,
        reasoning_replay: bool,
        id: impl Into<String>,
    ) -> Self {
        Self {
            api_key,
            base_url,
            model,
            caps,
            reasoning_replay,
            id: id.into(),
        }
    }

    fn endpoint(&self) -> String {
        format!("{}/chat/completions", self.base_url.trim_end_matches('/'))
    }

    fn build_request_body(
        &self,
        cfg: &LlmConfig,
        tools: &[ToolDef],
        history: &[ChatMessage],
    ) -> Value {
        let mut messages: Vec<Value> = Vec::new();
        // System prompt as the first message (no vendor cache_control here; these
        // providers cache automatically or not at all).
        messages.push(json!({ "role": "system", "content": cfg.system }));
        for message in history {
            append_messages(message, self.reasoning_replay, &mut messages);
        }

        let mut body = json!({
            "model": self.model,
            "messages": messages,
            "max_tokens": cfg.max_output_tokens,
        });
        if !tools.is_empty() {
            body["tools"] = Value::Array(tools.iter().map(tool_to_json).collect());
        }
        // Reasoning effort only where the model accepts it; OpenAI exposes three
        // levels, so the abstract scale collapses onto low|medium|high.
        if self.caps.supports_effort {
            body["reasoning_effort"] = json!(reasoning_effort(cfg.effort));
        }
        body
    }
}

impl LlmProvider for OpenAiCompatProvider {
    fn complete(
        &self,
        cfg: &LlmConfig,
        tools: &[ToolDef],
        history: &[ChatMessage],
        cancel: &Arc<AtomicBool>,
        _on_event: &mut dyn FnMut(StreamEvent),
    ) -> Result<AssistantTurn, LlmError> {
        use std::sync::atomic::Ordering;
        if cancel.load(Ordering::Relaxed) {
            return Err(LlmError::Cancelled);
        }
        if !super::endpoint_is_safe(&self.base_url) {
            return Err(LlmError::BadRequest(format!(
                "refusing to send the API key to {} over plaintext HTTP; use an https:// base URL \
                 (http:// is allowed only for a localhost endpoint)",
                self.base_url
            )));
        }

        let body = self.build_request_body(cfg, tools, history);
        let payload = serde_json::to_vec(&body)
            .map_err(|error| LlmError::BadRequest(format!("could not encode request: {error}")))?;

        let config = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(Some(REQUEST_TIMEOUT))
            .build();
        let agent = ureq::Agent::new_with_config(config);
        let response = agent
            .post(self.endpoint())
            .header("authorization", &format!("Bearer {}", self.api_key))
            .header("content-type", "application/json")
            .send(&payload[..]);

        let mut response = match response {
            Ok(response) => response,
            Err(error) => return Err(LlmError::Network(error.to_string())),
        };

        let status = response.status().as_u16();
        let retry_after = response
            .headers()
            .get("retry-after")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.trim().parse::<u64>().ok())
            .map(Duration::from_secs);

        let text = response
            .body_mut()
            .with_config()
            .limit(MAX_RESPONSE_BYTES)
            .read_to_string()
            .map_err(|error| LlmError::Network(error.to_string()))?;

        if status == 200 {
            match serde_json::from_str::<Value>(&text) {
                Ok(json) => parse_response(&json),
                Err(_) => Err(LlmError::BadRequest(non_json_response_message(&text))),
            }
        } else {
            Err(classify_status(status, &text, retry_after))
        }
    }

    fn encode_assistant_for_replay(&self, turn: &AssistantTurn) -> ChatMessage {
        encode_assistant(turn)
    }

    fn id(&self) -> &str {
        &self.id
    }

    fn caps(&self) -> ProviderCaps {
        self.caps
    }
}

/// Map the abstract effort onto OpenAI's three `reasoning_effort` levels.
fn reasoning_effort(effort: Effort) -> &'static str {
    match effort {
        Effort::Minimal | Effort::Low => "low",
        Effort::Medium => "medium",
        Effort::High | Effort::XHigh | Effort::Max => "high",
    }
}

fn tool_to_json(tool: &ToolDef) -> Value {
    json!({
        "type": "function",
        "function": {
            "name": tool.name,
            "description": tool.description,
            "parameters": tool.input_schema,
        }
    })
}

/// Expand one neutral message into OpenAI wire messages. A `Tool` message
/// becomes one `{role:"tool"}` message per result block; everything else maps
/// 1:1.
fn append_messages(message: &ChatMessage, reasoning_replay: bool, out: &mut Vec<Value>) {
    match message.role {
        Role::System => {
            out.push(json!({ "role": "system", "content": collect_text(message) }));
        }
        Role::User => {
            out.push(json!({ "role": "user", "content": collect_text(message) }));
        }
        Role::Tool => {
            for block in &message.content {
                if let ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    ..
                } = block
                {
                    out.push(json!({
                        "role": "tool",
                        "tool_call_id": tool_use_id,
                        "content": content,
                    }));
                }
            }
        }
        Role::Assistant => out.push(assistant_to_json(message, reasoning_replay)),
    }
}

fn collect_text(message: &ChatMessage) -> String {
    let mut text = String::new();
    for block in &message.content {
        if let ContentBlock::Text(chunk) = block {
            text.push_str(chunk);
        }
    }
    text
}

fn assistant_to_json(message: &ChatMessage, reasoning_replay: bool) -> Value {
    let mut text = String::new();
    let mut tool_calls: Vec<Value> = Vec::new();
    let mut reasoning: Option<String> = None;

    for block in &message.content {
        match block {
            ContentBlock::Text(chunk) => text.push_str(chunk),
            ContentBlock::ToolUse { id, name, input } => tool_calls.push(json!({
                "id": id,
                "type": "function",
                "function": {
                    "name": name,
                    // `arguments` must be a JSON string on the wire.
                    "arguments": serde_json::to_string(input).unwrap_or_else(|_| "{}".to_string()),
                }
            })),
            ContentBlock::ToolResult { .. } => {}
            ContentBlock::OpaqueReasoning(ReasoningBlob::OpenAiCompat { reasoning_content }) => {
                reasoning = reasoning_content.clone();
            }
            ContentBlock::OpaqueReasoning(_) => {}
        }
    }

    let mut object = serde_json::Map::new();
    object.insert("role".to_string(), json!("assistant"));
    // `content` is required; use null when the turn was tool-calls only.
    object.insert(
        "content".to_string(),
        if text.is_empty() {
            Value::Null
        } else {
            json!(text)
        },
    );
    if !tool_calls.is_empty() {
        object.insert("tool_calls".to_string(), Value::Array(tool_calls));
    }
    // DeepSeek thinking mode requires the reasoning_content back on replay.
    if reasoning_replay && let Some(reasoning) = reasoning {
        object.insert("reasoning_content".to_string(), json!(reasoning));
    }
    Value::Object(object)
}

/// Encode a completed turn for replay: reasoning (opaque), text, tool-use blocks.
fn encode_assistant(turn: &AssistantTurn) -> ChatMessage {
    let mut content: Vec<ContentBlock> = Vec::new();
    if let ReasoningBlob::OpenAiCompat { reasoning_content } = &turn.reasoning
        && reasoning_content.is_some()
    {
        content.push(ContentBlock::OpaqueReasoning(turn.reasoning.clone()));
    }
    if !turn.text.is_empty() {
        content.push(ContentBlock::Text(turn.text.clone()));
    }
    for call in &turn.tool_calls {
        content.push(ContentBlock::ToolUse {
            id: call.id.clone(),
            name: call.name.clone(),
            input: call.input.clone(),
        });
    }
    ChatMessage {
        role: Role::Assistant,
        content,
    }
}

fn parse_response(json: &Value) -> Result<AssistantTurn, LlmError> {
    let choice = json
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .ok_or_else(|| LlmError::BadRequest("response had no choices".to_string()))?;
    let message = choice
        .get("message")
        .ok_or_else(|| LlmError::BadRequest("choice had no message".to_string()))?;

    let text = message
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    let mut tool_calls: Vec<ToolCall> = Vec::new();
    if let Some(calls) = message.get("tool_calls").and_then(Value::as_array) {
        for call in calls {
            let id = call.get("id").and_then(Value::as_str).unwrap_or_default();
            let function = call.get("function");
            let name = function
                .and_then(|function| function.get("name"))
                .and_then(Value::as_str)
                .unwrap_or_default();
            // `arguments` is a JSON string — parse it back into a value.
            let input = function
                .and_then(|function| function.get("arguments"))
                .and_then(Value::as_str)
                .and_then(|arguments| serde_json::from_str(arguments).ok())
                .unwrap_or_else(|| json!({}));
            tool_calls.push(ToolCall {
                id: id.to_string(),
                name: name.to_string(),
                input,
            });
        }
    }

    // DeepSeek/OpenRouter reasoning, when present.
    let reasoning_content = message
        .get("reasoning_content")
        .or_else(|| message.get("reasoning"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let reasoning = ReasoningBlob::OpenAiCompat { reasoning_content };

    let stop = match choice.get("finish_reason").and_then(Value::as_str) {
        Some("stop") => StopReason::EndTurn,
        Some("tool_calls") | Some("function_call") => StopReason::ToolUse,
        Some("length") => StopReason::MaxTokens,
        Some("content_filter") => StopReason::Refusal,
        Some(other) => StopReason::Other(other.to_string()),
        // Some providers omit finish_reason when tool_calls are present.
        None if !tool_calls.is_empty() => StopReason::ToolUse,
        None => StopReason::EndTurn,
    };

    let usage = parse_usage(json.get("usage"));

    Ok(AssistantTurn {
        text,
        tool_calls,
        reasoning,
        stop,
        usage,
    })
}

fn parse_usage(usage: Option<&Value>) -> Usage {
    let field = |path: &[&str]| -> u32 {
        let mut node = match usage {
            Some(usage) => usage,
            None => return 0,
        };
        for key in path {
            match node.get(key) {
                Some(next) => node = next,
                None => return 0,
            }
        }
        node.as_u64().unwrap_or(0) as u32
    };
    Usage {
        input: field(&["prompt_tokens"]),
        output: field(&["completion_tokens"]),
        // OpenAI/DeepSeek report cache hits under prompt_tokens_details.
        cache_read: field(&["prompt_tokens_details", "cached_tokens"]),
        cache_write: 0,
    }
}

fn classify_status(status: u16, body: &str, retry_after: Option<Duration>) -> LlmError {
    match status {
        429 => LlmError::RateLimited { retry_after },
        529 => LlmError::Overloaded,
        500..=599 => LlmError::Server(status),
        401 | 403 => LlmError::Auth,
        400 | 413 | 422 => LlmError::BadRequest(extract_error_message(body)),
        other => LlmError::BadRequest(format!("HTTP {other}: {}", truncate(body, 400))),
    }
}

pub fn extract_error_message(body: &str) -> String {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|json| {
            json.get("error")
                .and_then(|error| {
                    error
                        .get("message")
                        .and_then(Value::as_str)
                        .or_else(|| error.as_str())
                })
                .or_else(|| json.get("message").and_then(Value::as_str))
                .map(str::to_string)
        })
        .unwrap_or_else(|| truncate(body, 400))
}

fn truncate(text: &str, max: usize) -> String {
    if text.len() <= max {
        text.to_string()
    } else {
        format!("{}…", &text[..max])
    }
}

/// A helpful message for an HTTP 200 whose body isn't the expected JSON. The
/// usual cause is a Base URL pointing at a web page (e.g. a relay's UI or a
/// landing page) instead of its API root, which answers 200 with HTML — so name
/// that and point at the Base URL rather than surfacing a raw parser offset like
/// "expected value at line 1 column 1". Shared with the live model-list fetch
/// (`frontend::jobs`), which hits the same wrong-Base-URL failure.
pub fn non_json_response_message(body: &str) -> String {
    let trimmed = body.trim_start();
    if trimmed.is_empty() {
        "the endpoint returned an empty response, not JSON — check the Base URL".to_string()
    } else if trimmed.starts_with('<') {
        "the endpoint returned an HTML page, not JSON — check the Base URL points at the API \
         root (it usually ends in /v1)"
            .to_string()
    } else {
        format!(
            "the endpoint returned a non-JSON response — check the Base URL: {}",
            truncate(trimmed, 200)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caps(effort: bool) -> ProviderCaps {
        ProviderCaps {
            supports_effort: effort,
            supports_thinking: effort,
            supports_prompt_cache: false,
            supports_streaming: false,
        }
    }

    #[test]
    fn non_json_200_points_at_the_base_url() {
        // The duckcoding/"New API" symptom: a base URL missing /v1 hits the web
        // UI, which answers 200 with an HTML page.
        let html = non_json_response_message("<!doctype html><html><head><title>New API</title>");
        assert!(html.contains("HTML"), "should name HTML: {html}");
        assert!(html.contains("/v1"), "should hint the API root: {html}");

        // An empty 200 body gets its own clear message.
        let empty = non_json_response_message("   \n");
        assert!(empty.contains("empty"), "should say empty: {empty}");
        assert!(
            empty.contains("Base URL"),
            "should point at Base URL: {empty}"
        );

        // Any other non-JSON 200 still points at the Base URL and shows a snippet
        // instead of a raw parser offset.
        let other = non_json_response_message("upstream timeout");
        assert!(
            other.contains("Base URL"),
            "should point at Base URL: {other}"
        );
        assert!(
            other.contains("upstream timeout"),
            "should echo the body: {other}"
        );
        assert!(
            !other.contains("line 1 column 1"),
            "should not leak the parser offset: {other}"
        );
    }

    #[test]
    fn endpoint_joins_base_url() {
        let provider = OpenAiCompatProvider::new(
            "k".into(),
            "https://api.deepseek.com".into(),
            "deepseek-chat".into(),
            caps(false),
            true,
            "deepseek",
        );
        assert_eq!(
            provider.endpoint(),
            "https://api.deepseek.com/chat/completions"
        );
    }

    #[test]
    fn tools_use_function_envelope_and_effort_is_gated() {
        let provider = OpenAiCompatProvider::new(
            "k".into(),
            "https://api.openai.com/v1".into(),
            "o4".into(),
            caps(true),
            false,
            "openai",
        );
        let cfg = LlmConfig {
            model: "o4".into(),
            effort: Effort::Medium,
            max_output_tokens: 1000,
            stream: false,
            system: "sys".into(),
        };
        let tool = ToolDef {
            name: "run_command".into(),
            description: "run".into(),
            input_schema: json!({ "type": "object" }),
        };
        let body = provider.build_request_body(&cfg, std::slice::from_ref(&tool), &[]);
        assert_eq!(body["tools"][0]["type"], "function");
        assert_eq!(body["tools"][0]["function"]["name"], "run_command");
        assert_eq!(body["reasoning_effort"], "medium");
        // System prompt is the first message.
        assert_eq!(body["messages"][0]["role"], "system");
    }

    #[test]
    fn non_reasoning_model_omits_effort() {
        let provider = OpenAiCompatProvider::new(
            "k".into(),
            "https://x".into(),
            "m".into(),
            caps(false),
            false,
            "local",
        );
        let cfg = LlmConfig {
            model: "m".into(),
            effort: Effort::High,
            max_output_tokens: 100,
            stream: false,
            system: "s".into(),
        };
        let body = provider.build_request_body(&cfg, &[], &[]);
        assert!(body.get("reasoning_effort").is_none());
    }

    #[test]
    fn tool_result_message_expands_to_tool_role_messages() {
        let provider = OpenAiCompatProvider::new(
            "k".into(),
            "https://x".into(),
            "m".into(),
            caps(false),
            false,
            "local",
        );
        let history = vec![ChatMessage {
            role: Role::Tool,
            content: vec![
                ContentBlock::ToolResult {
                    tool_use_id: "a".into(),
                    content: "first".into(),
                    is_error: false,
                },
                ContentBlock::ToolResult {
                    tool_use_id: "b".into(),
                    content: "second".into(),
                    is_error: true,
                },
            ],
        }];
        let cfg = LlmConfig {
            model: "m".into(),
            effort: Effort::Low,
            max_output_tokens: 100,
            stream: false,
            system: "s".into(),
        };
        let body = provider.build_request_body(&cfg, &[], &history);
        let messages = body["messages"].as_array().unwrap();
        // system + two tool messages.
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[1]["role"], "tool");
        assert_eq!(messages[1]["tool_call_id"], "a");
        assert_eq!(messages[2]["tool_call_id"], "b");
    }

    #[test]
    fn parses_tool_call_with_string_arguments() {
        let json = json!({
            "choices": [{
                "message": {
                    "content": null,
                    "tool_calls": [{
                        "id": "call_1",
                        "type": "function",
                        "function": { "name": "run_command",
                                      "arguments": "{\"command\":\"open x.pdb\"}" }
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "usage": { "prompt_tokens": 12, "completion_tokens": 3,
                       "prompt_tokens_details": { "cached_tokens": 5 } }
        });
        let turn = parse_response(&json).unwrap();
        assert_eq!(turn.stop, StopReason::ToolUse);
        assert_eq!(turn.tool_calls.len(), 1);
        assert_eq!(turn.tool_calls[0].input["command"], "open x.pdb");
        assert_eq!(turn.usage.input, 12);
        assert_eq!(turn.usage.cache_read, 5);
    }

    #[test]
    fn deepseek_reasoning_replays_when_enabled() {
        let turn = AssistantTurn {
            text: "ok".into(),
            tool_calls: vec![ToolCall {
                id: "c1".into(),
                name: "run_command".into(),
                input: json!({ "command": "open x" }),
            }],
            reasoning: ReasoningBlob::OpenAiCompat {
                reasoning_content: Some("thinking…".into()),
            },
            stop: StopReason::ToolUse,
            usage: Usage::default(),
        };
        let message = encode_assistant(&turn);
        let rendered = assistant_to_json(&message, true);
        assert_eq!(rendered["reasoning_content"], "thinking…");
        assert_eq!(rendered["tool_calls"][0]["function"]["name"], "run_command");
        // The arguments must be a JSON string on the wire.
        assert!(rendered["tool_calls"][0]["function"]["arguments"].is_string());

        // With replay disabled, reasoning_content is omitted.
        let without = assistant_to_json(&message, false);
        assert!(without.get("reasoning_content").is_none());
    }
}
