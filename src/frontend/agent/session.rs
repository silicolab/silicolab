//! The provider-agnostic agent session: neutral conversation history, a small
//! state machine, the in-flight tool batch, and a display transcript for the
//! Chat tab. Held as `UiState::agent`; mutated only through the dispatcher and
//! the poll-driven [`loop_driver`](super::loop_driver).

use std::collections::{HashMap, VecDeque};

use crate::io::llm::types::{ChatMessage, ContentBlock, ToolCall, Usage};

/// Where the session is in the turn cycle.
///
/// ```text
/// Idle ──send──▶ AwaitingModel ──turn done──▶ ExecutingTools ──┐
///   ▲                  │                          │            │ (gated call)
///   │                  │ error/cancel             │            ▼
///  Done ◀──end_turn────┴──────────────◀──────────┴──── AwaitingApproval
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AgentPhase {
    /// Nothing running; ready for a user message.
    #[default]
    Idle,
    /// A network turn is in flight on a worker thread.
    AwaitingModel,
    /// Running the tool calls the last turn requested (on the UI thread).
    ExecutingTools,
    /// A heavy tool (md/qm) is running off the UI thread; the gated call sits at
    /// the front of `pending_calls` until it completes.
    AwaitingHeavyJob,
    /// Paused on a destructive/expensive tool call awaiting user confirmation.
    AwaitingApproval,
    /// The exchange finished (end_turn / max_tokens / surfaced error).
    Done,
}

/// One rendered line in the Chat tab. The neutral `history` drives the model;
/// this drives the display.
#[derive(Debug, Clone)]
pub enum TranscriptEntry {
    User(String),
    Assistant(String),
    /// A tool the assistant invoked, with a one-line description.
    ToolCall {
        summary: String,
    },
    /// The tool's (possibly truncated) result.
    ToolResult {
        summary: String,
        is_error: bool,
    },
    /// Status / error / loop-bound notices.
    Notice(String),
}

/// Where a live `/models` fetch stands. Drives the spinner / error note next to
/// the model picker in settings; the fetched ids themselves live in
/// [`AgentSession::fetched_models`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum ModelFetchStatus {
    /// Idle — either never fetched, or a fetch finished (the cached list, if any,
    /// is in `fetched_models`).
    #[default]
    Idle,
    /// A `/models` request is in flight on a worker thread.
    Fetching,
    /// The last fetch failed; the string is a short user-facing reason.
    Error(String),
}

#[derive(Default)]
pub struct AgentSession {
    /// Neutral conversation history replayed to the provider each turn (includes
    /// prior assistant turns with their opaque reasoning blobs). The system
    /// prompt is *not* stored here — it is rebuilt per turn into `LlmConfig`.
    pub history: Vec<ChatMessage>,
    /// What the Chat tab renders.
    pub transcript: Vec<TranscriptEntry>,
    /// The chat input box.
    pub input: String,
    pub phase: AgentPhase,
    /// Model turns spent on the current user message (loop bound).
    pub iterations: usize,
    /// Running token total for the session.
    pub session_usage: Usage,
    /// Usage of the most recent turn.
    pub last_usage: Option<Usage>,
    /// Tool calls from the current turn still to execute. The front element is
    /// the one a confirmation gate (if any) applies to.
    pub pending_calls: VecDeque<ToolCall>,
    /// `tool_result` blocks gathered so far in the current batch; flushed into a
    /// single neutral `Tool` message when the batch completes.
    pub collected_results: Vec<ContentBlock>,
    /// Live preview of the assistant text streaming in this turn. Shown beneath
    /// the transcript while `AwaitingModel`, then cleared and replaced by the
    /// authoritative final text when the turn completes.
    pub streaming_text: String,
    /// Cached "is an API key available for the active provider" flag. Resolving
    /// it reads env + the key store, so the hot render path reads this cache
    /// instead; it is refreshed on provider/key changes (and once at startup).
    /// `None` until first computed.
    pub key_available: Option<bool>,
    /// Live model ids fetched from each provider's `/models` endpoint, keyed by
    /// provider id. Merged ahead of the static list in the model picker; empty
    /// until the user refreshes (the static list always shows regardless).
    pub fetched_models: HashMap<String, Vec<String>>,
    /// Where the most recent live model fetch stands (for the settings UI).
    pub model_fetch: ModelFetchStatus,
}

impl AgentSession {
    /// Whether a turn or tool batch is actively running (input should be locked).
    pub fn is_busy(&self) -> bool {
        matches!(
            self.phase,
            AgentPhase::AwaitingModel | AgentPhase::ExecutingTools | AgentPhase::AwaitingHeavyJob
        )
    }

    /// The tool call currently awaiting user approval, if any.
    pub fn pending_approval(&self) -> Option<&ToolCall> {
        if self.phase == AgentPhase::AwaitingApproval {
            self.pending_calls.front()
        } else {
            None
        }
    }

    /// Trim the history back to a clean continuation boundary: the last
    /// assistant message that made no tool call (or empty). Drops a dangling
    /// `tool_use` without results, an unanswered user turn, or a half-finished
    /// tool batch — all invalid as the prefix before a new user message.
    pub fn truncate_to_resumable(&mut self) {
        while let Some(last) = self.history.last() {
            if last.is_resumable_assistant() {
                break;
            }
            self.history.pop();
        }
    }
}
