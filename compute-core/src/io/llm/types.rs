//! Neutral LLM types — the contract the agent loop depends on.
//!
//! The loop builds and consumes only these. No vendor-specific `serde` shapes
//! leak past this module: each provider adapter translates to and from its own
//! wire JSON entirely inside [`complete`](super::provider::LlmProvider::complete).
//! Adding a provider therefore never changes the loop or the tools.

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Conversation role. `Tool` carries tool results back to the model; adapters
/// map it to whatever the vendor wire format uses (Anthropic folds it into a
/// `user` message of `tool_result` blocks, OpenAI uses `role: "tool"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

/// One piece of message content. The neutral superset of what every provider
/// can express; adapters render only the blocks their wire format supports.
#[derive(Debug, Clone)]
pub enum ContentBlock {
    Text(String),
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
        is_error: bool,
    },
    /// Opaque, provider-owned reasoning. The loop NEVER inspects this; only the
    /// originating adapter knows how to render or strip it on replay.
    OpaqueReasoning(ReasoningBlob),
}

#[derive(Debug, Clone)]
pub struct ChatMessage {
    pub role: Role,
    pub content: Vec<ContentBlock>,
}

impl ChatMessage {
    /// A plain user message carrying a single text block.
    pub fn user_text(text: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: vec![ContentBlock::Text(text.into())],
        }
    }

    /// Whether this is a "resumable" assistant turn: an assistant message that
    /// made no tool call, so the conversation can validly continue with a fresh
    /// user message after it. Used to trim an interrupted exchange back to a
    /// clean boundary (a dangling `tool_use` or bare user turn is invalid input
    /// to most providers).
    pub fn is_resumable_assistant(&self) -> bool {
        self.role == Role::Assistant
            && !self
                .content
                .iter()
                .any(|block| matches!(block, ContentBlock::ToolUse { .. }))
    }
}

/// A tool the model may call, described as a JSON Schema. Provider-neutral; each
/// adapter wraps it into its own tool-declaration envelope.
#[derive(Debug, Clone)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    /// JSON Schema for the tool's input object.
    pub input_schema: serde_json::Value,
}

/// Abstract reasoning effort. Each adapter maps this onto the vendor's knob
/// (Anthropic `output_config.effort`, OpenAI `reasoning_effort`) or drops it
/// when the target model does not accept one (gated via
/// [`ProviderCaps`](super::provider::ProviderCaps)).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Effort {
    Minimal,
    Low,
    Medium,
    #[default]
    High,
    XHigh,
    Max,
}

impl Effort {
    pub fn all() -> &'static [Effort] {
        &[
            Effort::Minimal,
            Effort::Low,
            Effort::Medium,
            Effort::High,
            Effort::XHigh,
            Effort::Max,
        ]
    }

    pub fn label(self) -> &'static str {
        match self {
            Effort::Minimal => "Minimal",
            Effort::Low => "Low",
            Effort::Medium => "Medium",
            Effort::High => "High",
            Effort::XHigh => "Extra high",
            Effort::Max => "Maximum",
        }
    }
}

/// Per-turn request shape the loop hands the adapter. Static, cacheable fields
/// (`system`) are kept free of volatile per-turn data so prompt caching hits.
#[derive(Debug, Clone)]
pub struct LlmConfig {
    pub model: String,
    pub effort: Effort,
    pub max_output_tokens: u32,
    pub stream: bool,
    /// Persona + `.sls` catalog. Static across a session, so it caches.
    pub system: String,
}

/// A tool invocation the model emitted this turn.
#[derive(Debug, Clone)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub input: serde_json::Value,
}

/// Why the model stopped generating.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopReason {
    EndTurn,
    ToolUse,
    MaxTokens,
    Refusal,
    Other(String),
}

/// Token accounting for one turn (or accumulated across a session).
#[derive(Debug, Clone, Copy, Default)]
pub struct Usage {
    pub input: u32,
    pub output: u32,
    pub cache_read: u32,
    pub cache_write: u32,
}

impl Usage {
    /// Accumulate another turn's usage into this running total.
    pub fn add(&mut self, other: &Usage) {
        self.input += other.input;
        self.output += other.output;
        self.cache_read += other.cache_read;
        self.cache_write += other.cache_write;
    }

    /// Total billed input tokens (fresh input + cache reads + cache writes).
    pub fn input_total(&self) -> u32 {
        self.input + self.cache_read + self.cache_write
    }
}

/// One completed model turn, normalized from vendor JSON by the adapter.
#[derive(Debug, Clone)]
pub struct AssistantTurn {
    pub text: String,
    pub tool_calls: Vec<ToolCall>,
    /// Opaque reasoning, round-tripped via the trait on replay.
    pub reasoning: ReasoningBlob,
    pub stop: StopReason,
    pub usage: Usage,
}

/// Opaque reasoning payload. The loop treats it as a black box; only the
/// originating adapter renders or strips it. See the adapters' replay rules.
#[derive(Debug, Clone, Default)]
pub enum ReasoningBlob {
    #[default]
    None,
    /// Anthropic `thinking` / `redacted_thinking` blocks, verbatim.
    Anthropic(Vec<serde_json::Value>),
    /// DeepSeek-style singular reasoning string.
    OpenAiCompat { reasoning_content: Option<String> },
}

/// Streaming event. A non-streaming adapter emits only the terminal
/// [`StreamEvent::Done`] — or nothing at all.
#[derive(Debug, Clone)]
pub enum StreamEvent {
    TextDelta(String),
    ReasoningDelta(String),
    ToolCallDelta {
        index: usize,
        id: Option<String>,
        name: Option<String>,
        args_fragment: String,
    },
    Done(AssistantTurn),
}

/// Classified transport/protocol error. Transient classes are retried by
/// [`retry`](super::retry); terminal ones surface to the user.
#[derive(Debug, Clone)]
pub enum LlmError {
    /// HTTP 429. `retry_after` honors the server's `Retry-After` header.
    RateLimited { retry_after: Option<Duration> },
    /// HTTP 529 — the provider is overloaded.
    Overloaded,
    /// Any 5xx.
    Server(u16),
    /// 400/422/413 — almost always a request-shape bug; surface it.
    BadRequest(String),
    /// 401/403.
    Auth,
    /// Transport failure (DNS, TCP, TLS, timeout).
    Network(String),
    /// The shared cancel flag was set.
    Cancelled,
}

impl LlmError {
    /// Whether this class is worth retrying with backoff.
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            LlmError::RateLimited { .. } | LlmError::Overloaded | LlmError::Server(_)
        )
    }

    /// A short, user-facing description.
    pub fn user_message(&self) -> String {
        match self {
            LlmError::RateLimited { .. } => "rate limited by the provider".to_string(),
            LlmError::Overloaded => "the provider is overloaded".to_string(),
            LlmError::Server(code) => format!("provider server error ({code})"),
            LlmError::BadRequest(detail) => detail.clone(),
            LlmError::Auth => {
                "authentication failed — check the API key environment variable".to_string()
            }
            LlmError::Network(detail) => format!("network error: {detail}"),
            LlmError::Cancelled => "cancelled".to_string(),
        }
    }
}

impl std::fmt::Display for LlmError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.user_message())
    }
}
