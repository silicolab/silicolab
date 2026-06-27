//! Native Anthropic (Claude) adapter — `POST /v1/messages`.
//!
//! Blocking `ureq` (same transport as `io/pdb_fetch` / `io/update_check`); no
//! async runtime. The request shape branches by model via [`ProviderCaps`]:
//! adaptive-thinking models (Opus 4.x, Sonnet 4.6, Fable 5) get
//! `thinking: {type:"adaptive"}` + `output_config.effort`, while Haiku 4.5 — which
//! *errors* on both — gets neither. Sampling params are never sent (removed on
//! Opus 4.8). Two prompt-cache breakpoints are placed: one on the static
//! tools+system prefix, one rolling on the last message, so the growing
//! transcript caches instead of paying full price each turn.

use std::sync::{Arc, atomic::AtomicBool};
use std::time::Duration;

use serde_json::{Value, json};

use super::provider::{LlmProvider, ProviderCaps};
use super::types::{
    AssistantTurn, ChatMessage, ContentBlock, Effort, LlmConfig, LlmError, ReasoningBlob, Role,
    StopReason, StreamEvent, ToolCall, ToolDef, Usage,
};

const API_URL: &str = "https://api.anthropic.com/v1/messages";
const ANTHROPIC_VERSION: &str = "2023-06-01";
/// A messages response is tens of KB at most; cap generously.
const MAX_RESPONSE_BYTES: u64 = 16 * 1024 * 1024;
/// A high-effort turn with large thinking can take minutes.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(300);

/// Native Claude provider. Holds only the API key, model id, and the model's
/// resolved capabilities (cheap strings) — so it is `Send` and moves into a
/// worker thread freely.
pub struct AnthropicProvider {
    api_key: String,
    model: String,
    caps: ProviderCaps,
}

impl AnthropicProvider {
    pub fn new(api_key: String, model: String) -> Self {
        let caps = caps_for_model(&model);
        Self {
            api_key,
            model,
            caps,
        }
    }

    /// Build the request body, branching the request shape by model capability.
    fn build_request_body(
        &self,
        cfg: &LlmConfig,
        tools: &[ToolDef],
        history: &[ChatMessage],
    ) -> Value {
        let mut messages: Vec<Value> = history.iter().map(message_to_json).collect();
        // Rolling cache breakpoint on the last block of the last message, so the
        // growing transcript caches turn-over-turn (the static tools+system
        // breakpoint below covers the prefix). Up to 4 breakpoints are allowed;
        // we use 2.
        if self.caps.supports_prompt_cache {
            add_cache_control_to_last_block(&mut messages);
        }

        // Render order is tools -> system -> messages. A single cache_control on
        // the last system block caches tools+system together.
        let system = if self.caps.supports_prompt_cache {
            json!([{
                "type": "text",
                "text": cfg.system,
                "cache_control": { "type": "ephemeral" }
            }])
        } else {
            json!([{ "type": "text", "text": cfg.system }])
        };

        let mut body = json!({
            "model": self.model,
            "max_tokens": cfg.max_output_tokens,
            "system": system,
            "tools": tools.iter().map(tool_to_json).collect::<Vec<_>>(),
            "messages": messages,
        });

        // Adaptive thinking + effort live only on models that accept them; both
        // *error* on Haiku 4.5. Sampling params (`temperature`/`top_p`) are never
        // sent — they are removed on Opus 4.8.
        if self.caps.supports_thinking {
            body["thinking"] = json!({ "type": "adaptive" });
        }
        if self.caps.supports_effort {
            // `effort` lives inside `output_config`, not in `thinking`.
            body["output_config"] = json!({ "effort": anthropic_effort(cfg.effort) });
        }
        if cfg.stream {
            body["stream"] = json!(true);
        }
        body
    }
}

impl LlmProvider for AnthropicProvider {
    fn complete(
        &self,
        cfg: &LlmConfig,
        tools: &[ToolDef],
        history: &[ChatMessage],
        cancel: &Arc<AtomicBool>,
        on_event: &mut dyn FnMut(StreamEvent),
    ) -> Result<AssistantTurn, LlmError> {
        use std::sync::atomic::Ordering;
        if cancel.load(Ordering::Relaxed) {
            return Err(LlmError::Cancelled);
        }

        let body = self.build_request_body(cfg, tools, history);
        // Serialize the JSON ourselves and send raw bytes: ureq's `send_json`
        // lives behind its `json` feature, which this crate does not enable.
        let payload = serde_json::to_vec(&body)
            .map_err(|error| LlmError::BadRequest(format!("could not encode request: {error}")))?;

        // `http_status_as_error(false)` so 4xx/5xx come back as a normal response
        // we can classify and whose error body we can read, rather than an opaque
        // transport error. Configured on a one-off agent because per-request
        // `.config()` erases the with-body marker that `send` needs.
        let config = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(Some(REQUEST_TIMEOUT))
            .build();
        let agent = ureq::Agent::new_with_config(config);
        let response = agent
            .post(API_URL)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
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

        if status != 200 {
            let text = response
                .body_mut()
                .with_config()
                .limit(MAX_RESPONSE_BYTES)
                .read_to_string()
                .map_err(|error| LlmError::Network(error.to_string()))?;
            return Err(classify_status(status, &text, retry_after));
        }

        if cfg.stream {
            parse_sse(response.body_mut().as_reader(), cancel, on_event)
        } else {
            let text = response
                .body_mut()
                .with_config()
                .limit(MAX_RESPONSE_BYTES)
                .read_to_string()
                .map_err(|error| LlmError::Network(error.to_string()))?;
            let json: Value = serde_json::from_str(&text)
                .map_err(|error| LlmError::BadRequest(format!("malformed response: {error}")))?;
            parse_response(&json)
        }
    }

    fn encode_assistant_for_replay(&self, turn: &AssistantTurn) -> ChatMessage {
        encode_assistant(turn)
    }

    fn id(&self) -> &str {
        "anthropic"
    }

    fn caps(&self) -> ProviderCaps {
        self.caps
    }
}

/// Resolve a model id to its capabilities. Effort + adaptive thinking are
/// supported on Fable 5, Opus 4.x, and Sonnet 4.6, but **error** on Haiku 4.5
/// and Sonnet 4.5 — exactly why request shaping is caps-gated.
pub fn caps_for_model(model: &str) -> ProviderCaps {
    let adaptive = model_supports_effort(model);
    ProviderCaps {
        supports_effort: adaptive,
        supports_thinking: adaptive,
        supports_prompt_cache: true,
        supports_streaming: true,
    }
}

fn model_supports_effort(model: &str) -> bool {
    let model = model.to_ascii_lowercase();
    if model.contains("haiku") {
        return false;
    }
    // Sonnet 4.5 (any punctuation) does not support effort/adaptive thinking.
    if model.contains("sonnet-4-5") || model.contains("sonnet-4.5") {
        return false;
    }
    model.contains("opus-4")
        || model.contains("sonnet-4-6")
        || model.contains("sonnet-4.6")
        || model.contains("fable")
}

/// Map the abstract effort onto Anthropic's `low|medium|high|xhigh|max`.
fn anthropic_effort(effort: Effort) -> &'static str {
    match effort {
        Effort::Minimal | Effort::Low => "low",
        Effort::Medium => "medium",
        Effort::High => "high",
        Effort::XHigh => "xhigh",
        Effort::Max => "max",
    }
}

/// Render a neutral message into Anthropic's `{role, content:[blocks]}` shape.
/// `Tool` results map to a `user` message of `tool_result` blocks; foreign
/// reasoning blobs are stripped (Anthropic only understands its own `thinking`).
fn message_to_json(message: &ChatMessage) -> Value {
    let role = match message.role {
        Role::Assistant => "assistant",
        // Anthropic has only user/assistant turns; tool results ride in a user
        // message, and a stray System message (shouldn't occur) folds to user.
        Role::User | Role::Tool | Role::System => "user",
    };

    let mut blocks: Vec<Value> = Vec::new();
    for block in &message.content {
        match block {
            ContentBlock::Text(text) => blocks.push(json!({ "type": "text", "text": text })),
            ContentBlock::ToolUse { id, name, input } => blocks.push(json!({
                "type": "tool_use",
                "id": id,
                "name": name,
                "input": input,
            })),
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => blocks.push(json!({
                "type": "tool_result",
                "tool_use_id": tool_use_id,
                "content": content,
                "is_error": is_error,
            })),
            ContentBlock::OpaqueReasoning(blob) => {
                if let ReasoningBlob::Anthropic(thinking) = blob {
                    // Re-attach verbatim; Anthropic requires thinking blocks to
                    // precede other content, and `encode_assistant` places them
                    // first, so insertion order here is already correct.
                    blocks.extend(thinking.iter().cloned());
                }
                // Any non-Anthropic blob is silently dropped on this wire.
            }
        }
    }

    // Anthropic rejects an empty `content` array. An assistant turn can carry
    // nothing renderable (an empty-text end_turn, or a refusal with no body once
    // any foreign reasoning is stripped above), which would otherwise replay as
    // `content: []` and 400 the next request. Emit a minimal placeholder so the
    // transcript stays valid.
    if blocks.is_empty() {
        blocks.push(json!({ "type": "text", "text": "(no content)" }));
    }

    json!({ "role": role, "content": blocks })
}

/// Attach a single ephemeral `cache_control` to the final block of the final
/// message — the rolling transcript breakpoint.
fn add_cache_control_to_last_block(messages: &mut [Value]) {
    let Some(last_message) = messages.last_mut() else {
        return;
    };
    let Some(blocks) = last_message
        .get_mut("content")
        .and_then(Value::as_array_mut)
    else {
        return;
    };
    if let Some(last_block) = blocks.last_mut()
        && let Some(object) = last_block.as_object_mut()
    {
        object.insert("cache_control".to_string(), json!({ "type": "ephemeral" }));
    }
}

fn tool_to_json(tool: &ToolDef) -> Value {
    json!({
        "name": tool.name,
        "description": tool.description,
        "input_schema": tool.input_schema,
    })
}

/// Encode a completed turn into neutral history for replay: thinking first (so
/// the wire render keeps Anthropic's required ordering), then text, then any
/// tool-use blocks.
fn encode_assistant(turn: &AssistantTurn) -> ChatMessage {
    let mut content: Vec<ContentBlock> = Vec::new();
    if let ReasoningBlob::Anthropic(blocks) = &turn.reasoning
        && !blocks.is_empty()
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

/// Parse a streaming (SSE) messages response into an [`AssistantTurn`], emitting
/// `TextDelta` events as text arrives. Handles text, thinking, and tool_use
/// blocks, reconstructing each from its `*_delta` fragments.
fn parse_sse(
    reader: impl std::io::Read,
    cancel: &Arc<AtomicBool>,
    on_event: &mut dyn FnMut(StreamEvent),
) -> Result<AssistantTurn, LlmError> {
    use std::io::BufRead;
    use std::sync::atomic::Ordering;

    let mut buf_reader = std::io::BufReader::new(reader);
    let mut line = String::new();

    let mut text = String::new();
    let mut tool_calls: Vec<ToolCall> = Vec::new();
    let mut reasoning: Vec<Value> = Vec::new();
    let mut stop = StopReason::EndTurn;
    let mut usage = Usage::default();

    // The content block currently streaming (by index), accumulating fragments.
    enum Block {
        Text,
        Thinking {
            text: String,
            signature: String,
        },
        ToolUse {
            id: String,
            name: String,
            args: String,
        },
        Other,
    }
    let mut current: Option<Block> = None;

    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(LlmError::Cancelled);
        }
        line.clear();
        let read = buf_reader
            .read_line(&mut line)
            .map_err(|error| LlmError::Network(error.to_string()))?;
        if read == 0 {
            break; // EOF
        }
        let Some(data) = line.trim_end().strip_prefix("data: ") else {
            continue; // `event:` lines and blanks — the JSON `type` is enough
        };
        let Ok(event) = serde_json::from_str::<Value>(data) else {
            continue;
        };
        match event.get("type").and_then(Value::as_str) {
            Some("message_start") => {
                usage = parse_usage(event.pointer("/message/usage"));
            }
            Some("content_block_start") => {
                let block = event.get("content_block");
                current = Some(
                    match block.and_then(|b| b.get("type")).and_then(Value::as_str) {
                        Some("text") => Block::Text,
                        Some("thinking") => Block::Thinking {
                            text: String::new(),
                            signature: String::new(),
                        },
                        Some("redacted_thinking") => {
                            if let Some(block) = block {
                                reasoning.push(block.clone());
                            }
                            Block::Other
                        }
                        Some("tool_use") => Block::ToolUse {
                            id: block
                                .and_then(|b| b.get("id"))
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_string(),
                            name: block
                                .and_then(|b| b.get("name"))
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_string(),
                            args: String::new(),
                        },
                        _ => Block::Other,
                    },
                );
            }
            Some("content_block_delta") => {
                let delta = event.get("delta");
                match delta.and_then(|d| d.get("type")).and_then(Value::as_str) {
                    Some("text_delta") => {
                        if let Some(chunk) =
                            delta.and_then(|d| d.get("text")).and_then(Value::as_str)
                        {
                            text.push_str(chunk);
                            on_event(StreamEvent::TextDelta(chunk.to_string()));
                        }
                    }
                    Some("thinking_delta") => {
                        if let Some(Block::Thinking { text: t, .. }) = current.as_mut()
                            && let Some(chunk) = delta
                                .and_then(|d| d.get("thinking"))
                                .and_then(Value::as_str)
                        {
                            t.push_str(chunk);
                        }
                    }
                    Some("signature_delta") => {
                        if let Some(Block::Thinking { signature, .. }) = current.as_mut()
                            && let Some(chunk) = delta
                                .and_then(|d| d.get("signature"))
                                .and_then(Value::as_str)
                        {
                            signature.push_str(chunk);
                        }
                    }
                    Some("input_json_delta") => {
                        if let Some(Block::ToolUse { args, .. }) = current.as_mut()
                            && let Some(chunk) = delta
                                .and_then(|d| d.get("partial_json"))
                                .and_then(Value::as_str)
                        {
                            args.push_str(chunk);
                        }
                    }
                    _ => {}
                }
            }
            Some("content_block_stop") => match current.take() {
                Some(Block::Thinking { text, signature }) => {
                    reasoning.push(json!({
                        "type": "thinking",
                        "thinking": text,
                        "signature": signature,
                    }));
                }
                Some(Block::ToolUse { id, name, args }) => {
                    let input = serde_json::from_str(&args).unwrap_or_else(|_| json!({}));
                    tool_calls.push(ToolCall { id, name, input });
                }
                _ => {}
            },
            Some("message_delta") => {
                if let Some(reason) = event.pointer("/delta/stop_reason").and_then(Value::as_str) {
                    stop = map_stop_reason(reason);
                }
                // The cumulative output token count arrives here.
                if let Some(output) = event
                    .pointer("/usage/output_tokens")
                    .and_then(Value::as_u64)
                {
                    usage.output = output as u32;
                }
            }
            Some("error") => {
                let message = event
                    .pointer("/error/message")
                    .and_then(Value::as_str)
                    .unwrap_or("stream error");
                return Err(LlmError::BadRequest(message.to_string()));
            }
            _ => {}
        }
    }

    Ok(AssistantTurn {
        text,
        tool_calls,
        reasoning: ReasoningBlob::Anthropic(reasoning),
        stop,
        usage,
    })
}

fn map_stop_reason(reason: &str) -> StopReason {
    match reason {
        "end_turn" => StopReason::EndTurn,
        "tool_use" => StopReason::ToolUse,
        "max_tokens" => StopReason::MaxTokens,
        "refusal" => StopReason::Refusal,
        other => StopReason::Other(other.to_string()),
    }
}

/// Parse a successful (`200`) messages response into an [`AssistantTurn`].
fn parse_response(json: &Value) -> Result<AssistantTurn, LlmError> {
    let mut text = String::new();
    let mut tool_calls: Vec<ToolCall> = Vec::new();
    let mut reasoning: Vec<Value> = Vec::new();

    if let Some(blocks) = json.get("content").and_then(Value::as_array) {
        for block in blocks {
            match block.get("type").and_then(Value::as_str) {
                Some("text") => {
                    if let Some(chunk) = block.get("text").and_then(Value::as_str) {
                        text.push_str(chunk);
                    }
                }
                Some("thinking") | Some("redacted_thinking") => reasoning.push(block.clone()),
                Some("tool_use") => {
                    let id = block.get("id").and_then(Value::as_str).unwrap_or_default();
                    let name = block
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    let input = block.get("input").cloned().unwrap_or_else(|| json!({}));
                    tool_calls.push(ToolCall {
                        id: id.to_string(),
                        name: name.to_string(),
                        input,
                    });
                }
                _ => {}
            }
        }
    }

    let stop = json
        .get("stop_reason")
        .and_then(Value::as_str)
        .map(map_stop_reason)
        .unwrap_or(StopReason::EndTurn);

    let usage = parse_usage(json.get("usage"));

    Ok(AssistantTurn {
        text,
        tool_calls,
        reasoning: ReasoningBlob::Anthropic(reasoning),
        stop,
        usage,
    })
}

fn parse_usage(usage: Option<&Value>) -> Usage {
    let field = |name: &str| -> u32 {
        usage
            .and_then(|usage| usage.get(name))
            .and_then(Value::as_u64)
            .unwrap_or(0) as u32
    };
    Usage {
        input: field("input_tokens"),
        output: field("output_tokens"),
        cache_read: field("cache_read_input_tokens"),
        cache_write: field("cache_creation_input_tokens"),
    }
}

/// Map a non-200 status + error body to an [`LlmError`].
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

/// Pull `error.message` out of an Anthropic error envelope, falling back to a
/// truncated raw body.
fn extract_error_message(body: &str) -> String {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|json| {
            json.get("error")
                .and_then(|error| error.get("message"))
                .and_then(Value::as_str)
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

#[cfg(test)]
mod tests;
