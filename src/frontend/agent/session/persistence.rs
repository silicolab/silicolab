use std::collections::HashMap;

use crate::backend::storage::{
    PersistedAssistantConversation, PersistedChatMessage, PersistedContentBlock,
    PersistedReasoningBlob, PersistedRole, PersistedTranscriptEntry, PersistedUsage,
    ProjectAssistantSnapshot,
};
use crate::io::llm::types::{ChatMessage, ContentBlock, ReasoningBlob, Role, Usage};

use super::{
    AgentPhase, AgentSession, AssistantConversation, AssistantConversationId, ModelFetchStatus,
    TranscriptEntry, normalize_title,
};

impl AgentSession {
    pub fn project_snapshot(&self) -> ProjectAssistantSnapshot {
        let conversations = self
            .conversations
            .iter()
            .map(persist_conversation)
            .collect();
        ProjectAssistantSnapshot {
            version: crate::backend::storage::ASSISTANT_STATE_FORMAT as u32,
            active_conversation: self.active_conversation.raw(),
            next_conversation_id: self.next_conversation_id,
            next_conversation_number: self.next_conversation_number,
            conversations,
        }
    }

    pub fn from_project_snapshot(snapshot: ProjectAssistantSnapshot) -> Self {
        if snapshot.conversations.is_empty() {
            return Self::default();
        }
        let mut conversations: Vec<AssistantConversation> = snapshot
            .conversations
            .into_iter()
            .filter(|conversation| conversation.id != 0)
            .map(restore_conversation)
            .collect();
        if conversations.is_empty() {
            return Self::default();
        }
        conversations.sort_by_key(|conversation| conversation.id.raw());
        let active_conversation = conversations
            .iter()
            .find(|conversation| conversation.id.raw() == snapshot.active_conversation)
            .map(|conversation| conversation.id)
            .unwrap_or(conversations[0].id);
        let max_id = conversations
            .iter()
            .map(|conversation| conversation.id.raw())
            .max()
            .unwrap_or(1);
        let next_conversation_id = snapshot.next_conversation_id.max(max_id + 1).max(2);
        let next_conversation_number = snapshot
            .next_conversation_number
            .max(conversations.len() as u64 + 1)
            .max(2);
        Self {
            conversations,
            active_conversation,
            skills: Vec::new(),
            skills_loaded: false,
            next_conversation_id,
            next_conversation_number,
            renaming_conversation: None,
            rename_buffer: String::new(),
            key_available: None,
            fetched_models: HashMap::new(),
            model_fetch: ModelFetchStatus::Idle,
        }
    }
}

fn persist_conversation(conversation: &AssistantConversation) -> PersistedAssistantConversation {
    let mut resumable = conversation.clone();
    let interrupted = !matches!(conversation.phase, AgentPhase::Idle | AgentPhase::Done)
        || !conversation.streaming_text.is_empty()
        || !conversation.pending_calls.is_empty()
        || !conversation.collected_results.is_empty();
    resumable.truncate_to_resumable();
    let mut transcript = resumable
        .transcript
        .iter()
        .map(persist_transcript_entry)
        .collect::<Vec<_>>();
    if interrupted {
        transcript.push(PersistedTranscriptEntry::Notice {
            text: "Previous assistant turn was interrupted before it could be restored."
                .to_string(),
        });
    }
    PersistedAssistantConversation {
        id: conversation.id.raw(),
        title: conversation.title.clone(),
        history: resumable.history.iter().map(persist_message).collect(),
        transcript,
        input: conversation.input.clone(),
        session_usage: persist_usage(conversation.session_usage),
        last_usage: conversation.last_usage.map(persist_usage),
    }
}

fn restore_conversation(payload: PersistedAssistantConversation) -> AssistantConversation {
    let id = AssistantConversationId::new(payload.id);
    let title = normalize_title(&payload.title);
    let mut conversation = AssistantConversation::new(id, title);
    conversation.history = payload.history.into_iter().map(restore_message).collect();
    conversation.transcript = payload
        .transcript
        .into_iter()
        .map(restore_transcript_entry)
        .collect();
    conversation.input = payload.input;
    conversation.session_usage = restore_usage(payload.session_usage);
    conversation.last_usage = payload.last_usage.map(restore_usage);
    conversation.phase = if conversation.has_activity() {
        AgentPhase::Done
    } else {
        AgentPhase::Idle
    };
    conversation
}

fn persist_message(message: &ChatMessage) -> PersistedChatMessage {
    PersistedChatMessage {
        role: persist_role(message.role),
        content: message.content.iter().map(persist_content_block).collect(),
    }
}

fn restore_message(message: PersistedChatMessage) -> ChatMessage {
    ChatMessage {
        role: restore_role(message.role),
        content: message
            .content
            .into_iter()
            .map(restore_content_block)
            .collect(),
    }
}

fn persist_role(role: Role) -> PersistedRole {
    match role {
        Role::System => PersistedRole::System,
        Role::User => PersistedRole::User,
        Role::Assistant => PersistedRole::Assistant,
        Role::Tool => PersistedRole::Tool,
    }
}

fn restore_role(role: PersistedRole) -> Role {
    match role {
        PersistedRole::System => Role::System,
        PersistedRole::User => Role::User,
        PersistedRole::Assistant => Role::Assistant,
        PersistedRole::Tool => Role::Tool,
    }
}

fn persist_content_block(block: &ContentBlock) -> PersistedContentBlock {
    match block {
        ContentBlock::Text(text) => PersistedContentBlock::Text { text: text.clone() },
        ContentBlock::ToolUse { id, name, input } => PersistedContentBlock::ToolUse {
            id: id.clone(),
            name: name.clone(),
            input: input.clone(),
        },
        ContentBlock::ToolResult {
            tool_use_id,
            content,
            is_error,
        } => PersistedContentBlock::ToolResult {
            tool_use_id: tool_use_id.clone(),
            content: content.clone(),
            is_error: *is_error,
        },
        ContentBlock::OpaqueReasoning(reasoning) => PersistedContentBlock::OpaqueReasoning {
            reasoning: persist_reasoning(reasoning),
        },
    }
}

fn restore_content_block(block: PersistedContentBlock) -> ContentBlock {
    match block {
        PersistedContentBlock::Text { text } => ContentBlock::Text(text),
        PersistedContentBlock::ToolUse { id, name, input } => {
            ContentBlock::ToolUse { id, name, input }
        }
        PersistedContentBlock::ToolResult {
            tool_use_id,
            content,
            is_error,
        } => ContentBlock::ToolResult {
            tool_use_id,
            content,
            is_error,
        },
        PersistedContentBlock::OpaqueReasoning { reasoning } => {
            ContentBlock::OpaqueReasoning(restore_reasoning(reasoning))
        }
    }
}

fn persist_reasoning(reasoning: &ReasoningBlob) -> PersistedReasoningBlob {
    match reasoning {
        ReasoningBlob::None => PersistedReasoningBlob::None,
        ReasoningBlob::Anthropic(blocks) => PersistedReasoningBlob::Anthropic {
            blocks: blocks.clone(),
        },
        ReasoningBlob::OpenAiCompat { reasoning_content } => PersistedReasoningBlob::OpenAiCompat {
            reasoning_content: reasoning_content.clone(),
        },
    }
}

fn restore_reasoning(reasoning: PersistedReasoningBlob) -> ReasoningBlob {
    match reasoning {
        PersistedReasoningBlob::None => ReasoningBlob::None,
        PersistedReasoningBlob::Anthropic { blocks } => ReasoningBlob::Anthropic(blocks),
        PersistedReasoningBlob::OpenAiCompat { reasoning_content } => {
            ReasoningBlob::OpenAiCompat { reasoning_content }
        }
    }
}

fn persist_transcript_entry(entry: &TranscriptEntry) -> PersistedTranscriptEntry {
    match entry {
        TranscriptEntry::User(text) => PersistedTranscriptEntry::User { text: text.clone() },
        TranscriptEntry::Assistant(text) => {
            PersistedTranscriptEntry::Assistant { text: text.clone() }
        }
        TranscriptEntry::Tool {
            summary,
            result,
            is_error,
        } => PersistedTranscriptEntry::Tool {
            summary: summary.clone(),
            result: result.clone(),
            is_error: *is_error,
        },
        TranscriptEntry::Notice(text) => PersistedTranscriptEntry::Notice { text: text.clone() },
    }
}

fn restore_transcript_entry(entry: PersistedTranscriptEntry) -> TranscriptEntry {
    match entry {
        PersistedTranscriptEntry::User { text } => TranscriptEntry::User(text),
        PersistedTranscriptEntry::Assistant { text } => TranscriptEntry::Assistant(text),
        PersistedTranscriptEntry::Tool {
            summary,
            result,
            is_error,
        } => TranscriptEntry::Tool {
            summary,
            result,
            is_error,
        },
        PersistedTranscriptEntry::Notice { text } => TranscriptEntry::Notice(text),
    }
}

fn persist_usage(usage: Usage) -> PersistedUsage {
    PersistedUsage {
        input: usage.input,
        output: usage.output,
        cache_read: usage.cache_read,
        cache_write: usage.cache_write,
    }
}

fn restore_usage(usage: PersistedUsage) -> Usage {
    Usage {
        input: usage.input,
        output: usage.output,
        cache_read: usage.cache_read,
        cache_write: usage.cache_write,
    }
}
