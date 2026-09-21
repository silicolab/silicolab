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

use super::documents::{self, DocMode, Resolved};
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
        cancel: &AtomicBool,
    ) -> Result<Value, LlmError> {
        let mode = if self.caps.supports_pdf_input {
            DocMode::Native(documents::OPENAI_LIMITS)
        } else {
            DocMode::ExtractedText
        };
        let resolved = documents::resolve(history, mode, cancel)?;
        let mut messages: Vec<Value> = Vec::new();
        // System prompt as the first message (no vendor cache_control here; these
        // providers cache automatically or not at all).
        messages.push(json!({ "role": "system", "content": cfg.system }));
        for message in resolved.messages.iter() {
            append_messages(message, self.reasoning_replay, &resolved, &mut messages);
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
        Ok(body)
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

        let body = self.build_request_body(cfg, tools, history, cancel)?;
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
fn append_messages(
    message: &ChatMessage,
    reasoning_replay: bool,
    resolved: &Resolved,
    out: &mut Vec<Value>,
) {
    match message.role {
        Role::System => {
            out.push(json!({ "role": "system", "content": collect_text(message) }));
        }
        Role::User => {
            out.push(json!({ "role": "user", "content": user_content(message, resolved) }));
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

/// A plain string unless the message carries a native PDF, so a request
/// without attachments is byte-identical to what every compatible server
/// already accepts.
fn user_content(message: &ChatMessage, resolved: &Resolved) -> Value {
    let mut parts: Vec<Value> = message
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Document(document) => Some((document, resolved.payload(document)?)),
            _ => None,
        })
        .map(|(document, data)| {
            json!({
                "type": "file",
                "file": {
                    "filename": document.name,
                    "file_data": format!("data:application/pdf;base64,{data}"),
                }
            })
        })
        .collect();
    let text = collect_text(message);
    if parts.is_empty() {
        return json!(text);
    }
    if !text.is_empty() {
        parts.push(json!({ "type": "text", "text": text }));
    }
    Value::Array(parts)
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
            ContentBlock::ToolResult { .. } | ContentBlock::Document(_) => {}
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
mod tests;
