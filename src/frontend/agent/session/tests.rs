use super::*;
use crate::io::llm::types::{ContentBlock, Role};

fn text_message(role: Role, text: &str) -> ChatMessage {
    ChatMessage {
        role,
        content: vec![ContentBlock::Text(text.to_string())],
    }
}

fn selection() -> AssistantModelSelection {
    AssistantModelSelection::default()
}

#[test]
fn new_conversation_switches_to_empty_state_and_preserves_old_content() {
    let mut session = AgentSession::default();
    let first = session.active_conversation;
    session.history.push(ChatMessage::user_text("old"));
    session.transcript.push(TranscriptEntry::User("old".into()));
    session.input = "draft".to_string();

    session.start_new_conversation(selection());

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
fn invalidate_skills_clears_loaded_flag_so_next_turn_reloads() {
    let mut session = AgentSession {
        skills_loaded: true,
        ..Default::default()
    };
    session.invalidate_skills();
    assert!(
        !session.skills_loaded,
        "invalidate_skills must clear the cache so ensure_skills_loaded reloads"
    );
}

#[test]
fn switching_restores_input_usage_history_and_transcript() {
    let mut session = AgentSession::default();
    let first = session.active_conversation;
    session.history.push(ChatMessage::user_text("first"));
    session
        .transcript
        .push(TranscriptEntry::User("first".into()));
    session.input = "first draft".to_string();
    session.session_usage.input = 7;
    session.last_usage = Some(Usage {
        input: 3,
        output: 2,
        ..Usage::default()
    });

    session.start_new_conversation(selection());
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
fn project_snapshot_restores_conversations_without_runtime_state() {
    let mut session = AgentSession::default();
    let first = session.active_conversation;
    let second_selection = AssistantModelSelection {
        provider: "openai".to_string(),
        model: "gpt-5.5".to_string(),
    };
    session.start_new_conversation(second_selection.clone());
    session
        .transcript
        .push(TranscriptEntry::Assistant("second".to_string()));
    let active = session.active_conversation;
    session.switch_conversation(first);
    session.history.push(ChatMessage::user_text("prepare 1ubq"));
    session
        .transcript
        .push(TranscriptEntry::User("prepare 1ubq".into()));
    session.input = "draft".to_string();
    session.session_usage.input = 9;
    session.phase = AgentPhase::AwaitingApproval;
    session.pending_calls.push_back(ToolCall {
        id: "call_1".to_string(),
        name: "run_command".to_string(),
        input: serde_json::json!({ "command": "fetch 1ubq" }),
    });
    session.active_conversation = active;

    let snapshot = session.project_snapshot();
    let restored = AgentSession::from_project_snapshot(snapshot, selection());

    assert_eq!(restored.active_conversation, active);
    assert_eq!(restored.conversations.len(), 2);
    assert_eq!(restored.active().selection, second_selection);
    let first_restored = restored
        .conversations
        .iter()
        .find(|conversation| conversation.id == first)
        .expect("first conversation restored");
    assert_eq!(first_restored.input, "draft");
    assert_eq!(first_restored.session_usage.input, 9);
    assert!(first_restored.pending_calls.is_empty());
    assert_eq!(first_restored.phase, AgentPhase::Done);
    assert!(first_restored.transcript.iter().any(
        |entry| matches!(entry, TranscriptEntry::Notice(text) if text.contains("interrupted"))
    ));
}

#[test]
fn empty_project_snapshot_uses_configured_default_selection() {
    let default_selection = AssistantModelSelection {
        provider: "openai".to_string(),
        model: "gpt-5.5".to_string(),
    };

    let restored = AgentSession::from_project_snapshot(
        crate::backend::storage::ProjectAssistantSnapshot::default(),
        default_selection.clone(),
    );

    assert_eq!(restored.selection, default_selection);
}

#[test]
fn deleting_active_conversation_selects_neighbor() {
    let mut session = AgentSession::default();
    let first = session.active_conversation;
    session.start_new_conversation(selection());
    let second = session.active_conversation;
    session.start_new_conversation(selection());
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
    session.start_new_conversation(selection());
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

#[test]
fn flush_current_backlog_clears_backlog() {
    let mut conv =
        AssistantConversation::new(AssistantConversationId::new(1), "c".into(), selection());
    conv.note_backlog_start("compare methods".into(), 0);
    conv.flush_current_backlog();
    assert!(conv.current_backlog.is_none());
}

#[test]
fn flush_without_backlog_is_noop() {
    let mut conv =
        AssistantConversation::new(AssistantConversationId::new(1), "c".into(), selection());
    conv.flush_current_backlog();
    assert!(conv.current_backlog.is_none());
}

#[test]
fn resolve_drops_backlog_that_spawned_a_job() {
    let mut conv =
        AssistantConversation::new(AssistantConversationId::new(1), "c".into(), selection());
    conv.note_backlog_start("optimize this".into(), 0);
    conv.resolve_current_backlog(1);
    assert!(conv.current_backlog.is_none());
}

#[test]
fn resolve_clears_backlog_when_no_new_job() {
    let mut conv =
        AssistantConversation::new(AssistantConversationId::new(1), "c".into(), selection());
    conv.note_backlog_start("just answer".into(), 2);
    conv.resolve_current_backlog(2);
    assert!(conv.current_backlog.is_none());
}

#[test]
fn a_user_message_puts_documents_before_the_text() {
    let message = UserMessage {
        text: "which functional?".into(),
        attachments: vec![DocumentRef {
            path: "/a/paper.pdf".into(),
            name: "paper.pdf".into(),
            bytes: 1,
            modified_ms: 0,
            pages: 1,
        }],
    };
    let chat = message.to_chat_message();
    assert!(matches!(chat.content[0], ContentBlock::Document(_)));
    assert!(matches!(&chat.content[1], ContentBlock::Text(text) if text == "which functional?"));

    let bare = UserMessage {
        text: String::new(),
        ..message
    };
    assert_eq!(bare.to_chat_message().content.len(), 1);
}
