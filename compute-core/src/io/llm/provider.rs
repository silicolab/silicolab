//! The provider boundary: one thin trait every LLM backend implements.
//!
//! The agent loop and tools depend only on this trait and the neutral
//! [`types`](super::types). Vendor lock-in stops here — adding a provider is a
//! new adapter (often just the OpenAI-compatible one with a different base URL),
//! never a change to the loop.

use std::sync::{Arc, atomic::AtomicBool};

use super::types::{AssistantTurn, ChatMessage, LlmConfig, LlmError, StreamEvent, ToolDef};

/// What knobs a provider/model accepts. Drives both the settings UI (which
/// controls to show) and request shaping (which params to send). For example
/// Haiku 4.5 must NOT receive `effort`/adaptive-thinking, so its caps report
/// `supports_effort = supports_thinking = false`.
#[derive(Debug, Clone, Copy)]
pub struct ProviderCaps {
    pub supports_effort: bool,
    pub supports_thinking: bool,
    pub supports_prompt_cache: bool,
    pub supports_streaming: bool,
}

pub trait LlmProvider: Send {
    /// One model turn, blocking. Runs on a worker thread.
    ///
    /// The adapter translates `history` + `tools` + `cfg` to vendor JSON, POSTs,
    /// and parses the reply back into an [`AssistantTurn`]. `cancel` is checked
    /// between retries; `on_event` receives streaming events (a no-op closure
    /// for non-streaming adapters).
    fn complete(
        &self,
        cfg: &LlmConfig,
        tools: &[ToolDef],
        history: &[ChatMessage],
        cancel: &Arc<AtomicBool>,
        on_event: &mut dyn FnMut(StreamEvent),
    ) -> Result<AssistantTurn, LlmError>;

    /// Encode a completed assistant turn back into neutral history for replay.
    /// The adapter decides what to do with `turn.reasoning` (re-attach verbatim,
    /// or carry it as an opaque blob its own request builder later renders). The
    /// loop calls this and never branches on provider for reasoning.
    fn encode_assistant_for_replay(&self, turn: &AssistantTurn) -> ChatMessage;

    /// Stable identifier, e.g. `"anthropic"` or `"openai-compat"`.
    fn id(&self) -> &str;

    fn caps(&self) -> ProviderCaps;
}
