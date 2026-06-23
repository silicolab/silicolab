//! The provider-agnostic agent session: neutral conversation history, a small
//! state machine, the in-flight tool batch, and a display transcript for the
//! Assistant tab. Held as `UiState::agent`; mutated only through the dispatcher and
//! the poll-driven [`loop_driver`](super::loop_driver).

use std::collections::{HashMap, VecDeque};
use std::ops::{Deref, DerefMut};

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
    /// Paused on a destructive/expensive tool call awaiting user confirmation.
    AwaitingApproval,
    /// The exchange finished (end_turn / max_tokens / surfaced error).
    Done,
}

/// One rendered line in the Assistant tab. The neutral `history` drives the model;
/// this drives the display.
#[derive(Debug, Clone)]
pub enum TranscriptEntry {
    User(String),
    Assistant(String),
    Tool {
        summary: String,
        result: Option<String>,
        is_error: bool,
    },
    Notice(String),
}

/// A follow-up turn waiting to run the moment the agent returns to rest, FIFO.
/// A message the user submits while the agent is busy queues here (type-ahead)
/// instead of being dropped; [`pump_queue`](super::loop_driver::pump_queue) then
/// dispatches the front item — one at a time — once `phase` is `Idle`/`Done`.
#[derive(Debug, Clone)]
pub enum PendingTurn {
    /// A user message submitted while the agent was busy or awaiting approval.
    UserMessage(String),
    /// A background job finished; wake the model with its result so it can
    /// continue the workflow (e.g. optimize → frequencies).
    JobDone {
        label: String,
        summary: String,
        is_error: bool,
    },
}

impl PendingTurn {
    /// Short label for the composer's queued-message strip.
    pub fn preview(&self) -> &str {
        match self {
            PendingTurn::UserMessage(text) => text,
            PendingTurn::JobDone { label, .. } => label,
        }
    }
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AssistantConversationId(u64);

impl AssistantConversationId {
    pub fn new(raw: u64) -> Self {
        Self(raw)
    }
}

#[derive(Debug, Clone)]
pub struct AssistantConversation {
    pub id: AssistantConversationId,
    pub title: String,
    /// Neutral conversation history replayed to the provider each turn (includes
    /// prior assistant turns with their opaque reasoning blobs). The system
    /// prompt is *not* stored here — it is rebuilt per turn into `LlmConfig`.
    pub history: Vec<ChatMessage>,
    /// What the Assistant tab renders.
    pub transcript: Vec<TranscriptEntry>,
    /// The assistant input box.
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
    /// Follow-up turns to dispatch the moment the agent returns to rest, FIFO. A
    /// message submitted while busy lands here instead of being dropped; the loop
    /// pumps it once `phase` is `Idle`/`Done` (see [`PendingTurn`]).
    pub queued: VecDeque<PendingTurn>,
}

impl AssistantConversation {
    fn new(id: AssistantConversationId, title: String) -> Self {
        Self {
            id,
            title,
            history: Vec::new(),
            transcript: Vec::new(),
            input: String::new(),
            phase: AgentPhase::Idle,
            iterations: 0,
            session_usage: Usage::default(),
            last_usage: None,
            pending_calls: VecDeque::new(),
            collected_results: Vec::new(),
            streaming_text: String::new(),
            queued: VecDeque::new(),
        }
    }

    /// Whether a turn or tool batch is actively running (input should be locked).
    /// Background jobs deliberately do *not* count — the agent stays free while a
    /// heavy computation runs off-thread.
    pub fn is_busy(&self) -> bool {
        matches!(
            self.phase,
            AgentPhase::AwaitingModel | AgentPhase::ExecutingTools
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

    /// Drop the queued follow-up at `index` (a no-op when out of range). Backs
    /// the composer's per-message ✕ buttons.
    pub fn remove_queued(&mut self, index: usize) {
        if index < self.queued.len() {
            self.queued.remove(index);
        }
    }

    pub fn has_activity(&self) -> bool {
        !self.history.is_empty()
            || !self.transcript.is_empty()
            || !self.input.trim().is_empty()
            || self.session_usage.input_total() > 0
            || self.session_usage.output > 0
            || self.last_usage.is_some()
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

pub struct AgentSession {
    pub conversations: Vec<AssistantConversation>,
    pub active_conversation: AssistantConversationId,
    next_conversation_id: u64,
    next_conversation_number: u64,
    pub renaming_conversation: Option<AssistantConversationId>,
    pub rename_buffer: String,
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

impl Default for AgentSession {
    fn default() -> Self {
        let active_conversation = AssistantConversationId::new(1);
        Self {
            conversations: vec![AssistantConversation::new(
                active_conversation,
                "Chat 1".to_string(),
            )],
            active_conversation,
            next_conversation_id: 2,
            next_conversation_number: 2,
            renaming_conversation: None,
            rename_buffer: String::new(),
            key_available: None,
            fetched_models: HashMap::new(),
            model_fetch: ModelFetchStatus::Idle,
        }
    }
}

impl Deref for AgentSession {
    type Target = AssistantConversation;

    fn deref(&self) -> &Self::Target {
        self.active()
    }
}

impl DerefMut for AgentSession {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.active_mut()
    }
}

impl AgentSession {
    pub fn active(&self) -> &AssistantConversation {
        self.conversations
            .iter()
            .find(|conversation| conversation.id == self.active_conversation)
            .or_else(|| self.conversations.first())
            .expect("AgentSession always has at least one conversation")
    }

    pub fn active_mut(&mut self) -> &mut AssistantConversation {
        let active = self.active_conversation;
        let index = self
            .conversations
            .iter()
            .position(|conversation| conversation.id == active)
            .unwrap_or(0);
        self.active_conversation = self.conversations[index].id;
        &mut self.conversations[index]
    }

    /// A specific conversation by id, for routing a background job's result to
    /// the conversation that launched it even if another is now active.
    pub fn conversation_mut(
        &mut self,
        id: AssistantConversationId,
    ) -> Option<&mut AssistantConversation> {
        self.conversations
            .iter_mut()
            .find(|conversation| conversation.id == id)
    }

    /// Whether a turn or tool batch is actively running (input should be locked).
    pub fn is_busy(&self) -> bool {
        self.active().is_busy()
    }

    /// The tool call currently awaiting user approval, if any.
    pub fn pending_approval(&self) -> Option<&ToolCall> {
        self.active().pending_approval()
    }

    pub fn can_manage_conversations(&self) -> bool {
        let active = self.active();
        !active.is_busy() && active.phase != AgentPhase::AwaitingApproval
    }

    pub fn start_new_conversation(&mut self) {
        if !self.can_manage_conversations() {
            return;
        }
        self.renaming_conversation = None;
        self.rename_buffer.clear();
        let id = AssistantConversationId::new(self.next_conversation_id);
        self.next_conversation_id += 1;
        let title = format!("Chat {}", self.next_conversation_number);
        self.next_conversation_number += 1;
        self.conversations
            .push(AssistantConversation::new(id, title));
        self.active_conversation = id;
    }

    pub fn switch_conversation(&mut self, id: AssistantConversationId) {
        if !self.can_manage_conversations() {
            return;
        }
        if self
            .conversations
            .iter()
            .any(|conversation| conversation.id == id)
        {
            self.renaming_conversation = None;
            self.rename_buffer.clear();
            self.active_conversation = id;
        }
    }

    pub fn rename_conversation(&mut self, id: AssistantConversationId, title: &str) {
        if !self.can_manage_conversations() {
            return;
        }
        let title = normalize_title(title);
        if let Some(conversation) = self
            .conversations
            .iter_mut()
            .find(|conversation| conversation.id == id)
        {
            conversation.title = title;
            self.renaming_conversation = None;
            self.rename_buffer.clear();
        }
    }

    pub fn delete_conversation(&mut self, id: AssistantConversationId) {
        if !self.can_manage_conversations() {
            return;
        }
        if self.conversations.len() <= 1 {
            if self.active_conversation == id {
                let conversation = self.active_mut();
                let id = conversation.id;
                let title = conversation.title.clone();
                *conversation = AssistantConversation::new(id, title);
            }
            self.renaming_conversation = None;
            self.rename_buffer.clear();
            return;
        }
        let Some(index) = self
            .conversations
            .iter()
            .position(|conversation| conversation.id == id)
        else {
            return;
        };
        self.conversations.remove(index);
        if self.renaming_conversation == Some(id) {
            self.renaming_conversation = None;
            self.rename_buffer.clear();
        }
        if self.active_conversation == id {
            let next_index = index.min(self.conversations.len() - 1);
            self.active_conversation = self.conversations[next_index].id;
        }
    }

    pub fn maybe_title_from_first_user_message(&mut self, text: &str) {
        let conversation = self.active_mut();
        if conversation.title.starts_with("Chat ") && !conversation.has_activity() {
            conversation.title = title_from_message(text);
        }
    }

    /// Trim the history back to a clean continuation boundary: the last
    /// assistant message that made no tool call (or empty). Drops a dangling
    /// `tool_use` without results, an unanswered user turn, or a half-finished
    /// tool batch — all invalid as the prefix before a new user message.
    pub fn truncate_to_resumable(&mut self) {
        self.active_mut().truncate_to_resumable();
    }
}

fn normalize_title(title: &str) -> String {
    let trimmed = title.trim();
    if trimmed.is_empty() {
        "Untitled".to_string()
    } else {
        trimmed.chars().take(48).collect()
    }
}

fn title_from_message(text: &str) -> String {
    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let title: String = normalized.chars().take(32).collect();
    normalize_title(&title)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::llm::types::{ContentBlock, Role};

    fn text_message(role: Role, text: &str) -> ChatMessage {
        ChatMessage {
            role,
            content: vec![ContentBlock::Text(text.to_string())],
        }
    }

    #[test]
    fn new_conversation_switches_to_empty_state_and_preserves_old_content() {
        let mut session = AgentSession::default();
        let first = session.active_conversation;
        session.history.push(ChatMessage::user_text("old"));
        session
            .transcript
            .push(TranscriptEntry::User("old".to_string()));
        session.input = "draft".to_string();

        session.start_new_conversation();

        assert_ne!(session.active_conversation, first);
        assert!(session.history.is_empty());
        assert!(session.transcript.is_empty());
        assert!(session.input.is_empty());

        session.switch_conversation(first);
        assert_eq!(session.history.len(), 1);
        assert_eq!(session.transcript.len(), 1);
        assert_eq!(session.input, "draft");
    }

    #[test]
    fn switching_restores_input_usage_history_and_transcript() {
        let mut session = AgentSession::default();
        let first = session.active_conversation;
        session.history.push(ChatMessage::user_text("first"));
        session
            .transcript
            .push(TranscriptEntry::User("first".to_string()));
        session.input = "first draft".to_string();
        session.session_usage.input = 7;
        session.last_usage = Some(Usage {
            input: 3,
            output: 2,
            ..Usage::default()
        });

        session.start_new_conversation();
        let second = session.active_conversation;
        session.history.push(ChatMessage::user_text("second"));
        session.input = "second draft".to_string();
        session.session_usage.output = 11;

        session.switch_conversation(first);
        assert_eq!(session.input, "first draft");
        assert_eq!(session.session_usage.input, 7);
        assert_eq!(session.last_usage.map(|usage| usage.output), Some(2));
        assert_eq!(session.history.len(), 1);
        assert_eq!(session.transcript.len(), 1);

        session.switch_conversation(second);
        assert_eq!(session.input, "second draft");
        assert_eq!(session.session_usage.output, 11);
        assert_eq!(session.history.len(), 1);
    }

    #[test]
    fn deleting_active_conversation_selects_neighbor() {
        let mut session = AgentSession::default();
        let first = session.active_conversation;
        session.start_new_conversation();
        let second = session.active_conversation;
        session.start_new_conversation();
        let third = session.active_conversation;

        session.switch_conversation(second);
        session.delete_conversation(second);

        assert_eq!(session.active_conversation, third);
        assert_eq!(session.conversations.len(), 2);
        assert!(
            session
                .conversations
                .iter()
                .any(|conversation| conversation.id == first)
        );
    }

    #[test]
    fn deleting_last_conversation_resets_it_in_place() {
        let mut session = AgentSession::default();
        let id = session.active_conversation;
        session.history.push(ChatMessage::user_text("old"));
        session
            .transcript
            .push(TranscriptEntry::Assistant("done".to_string()));
        session.input = "draft".to_string();
        session.session_usage.input = 10;

        session.delete_conversation(id);

        assert_eq!(session.conversations.len(), 1);
        assert_eq!(session.active_conversation, id);
        assert!(session.history.is_empty());
        assert!(session.transcript.is_empty());
        assert!(session.input.is_empty());
        assert_eq!(session.session_usage.input, 0);
    }

    #[test]
    fn busy_or_approval_state_blocks_management_actions() {
        let mut session = AgentSession::default();
        let original = session.active_conversation;
        session.phase = AgentPhase::AwaitingModel;
        session.start_new_conversation();
        assert_eq!(session.active_conversation, original);
        assert_eq!(session.conversations.len(), 1);

        session.phase = AgentPhase::AwaitingApproval;
        session.rename_conversation(original, "New name");
        assert_eq!(session.active().title, "Chat 1");
        session.delete_conversation(original);
        assert_eq!(session.conversations.len(), 1);
    }

    #[test]
    fn default_title_updates_from_first_user_message() {
        let mut session = AgentSession::default();
        session.maybe_title_from_first_user_message("fetch 1ubq and show it as cartoon");

        assert_eq!(session.active().title, "fetch 1ubq and show it as cartoo");
        session.history.push(text_message(Role::User, "fetch"));
        session.maybe_title_from_first_user_message("different title");
        assert_eq!(session.active().title, "fetch 1ubq and show it as cartoo");
    }
}
