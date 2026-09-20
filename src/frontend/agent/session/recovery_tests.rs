use super::*;
use crate::io::llm::types::{ReasoningBlob, Role};
use serde_json::json;

fn partial_session() -> AgentSession {
    let mut session = AgentSession::default();
    session
        .history
        .push(ChatMessage::user_text("original goal"));
    let mut content = vec![ContentBlock::OpaqueReasoning(ReasoningBlob::Anthropic(
        vec![json!({"type": "thinking", "thinking": "opaque", "signature": "signature"})],
    ))];
    for id in ["started", "failed", "pending", "unknown"] {
        content.push(ContentBlock::ToolUse {
            id: id.into(),
            name: "run_command".into(),
            input: json!({"command": "qm energy"}),
        });
    }
    session.history.push(ChatMessage {
        role: Role::Assistant,
        content,
    });
    for (id, content, is_error) in [
        ("started", "Started job #42, task 7", false),
        ("failed", "engine unavailable", true),
    ] {
        session.collected_results.push(ContentBlock::ToolResult {
            tool_use_id: id.into(),
            content: content.into(),
            is_error,
        });
        session.transcript.push(TranscriptEntry::Tool {
            summary: id.into(),
            result: Some(content.into()),
            is_error,
        });
    }
    session.pending_calls.push_back(ToolCall {
        id: "pending".into(),
        name: "run_command".into(),
        input: json!({}),
    });
    session.approved_ids.insert("pending".into());
    session.allowed_verbs.insert("qm".into());
    session.phase = AgentPhase::AwaitingApproval;
    session
}

#[test]
fn recovery_preserves_results_reasoning_and_goal_and_is_idempotent() {
    let mut session = partial_session();
    session.recover_interrupted("turn cancelled");
    assert_eq!(session.history.len(), 3);
    assert!(
        matches!(&session.history[0].content[0], ContentBlock::Text(t) if t == "original goal")
    );
    assert!(
        matches!(&session.history[1].content[0], ContentBlock::OpaqueReasoning(ReasoningBlob::Anthropic(b)) if b[0]["signature"] == "signature")
    );
    let results = &session.history[2].content;
    assert_eq!(results.len(), 4);
    for (index, id, text, error) in [
        (0, "started", "Started job #42, task 7", false),
        (1, "failed", "engine unavailable", true),
        (2, "pending", "Not executed: turn cancelled.", true),
    ] {
        assert!(
            matches!(&results[index], ContentBlock::ToolResult {tool_use_id, content, is_error} if tool_use_id == id && content == text && *is_error == error)
        );
    }
    assert!(
        matches!(&results[3], ContentBlock::ToolResult { content, is_error: true, .. } if content.contains("Result unknown") && content.contains("Verify") && !content.contains("Not executed"))
    );
    assert!(session.pending_calls.is_empty());
    assert!(session.collected_results.is_empty());
    assert!(session.approved_ids.is_empty());
    assert!(
        matches!(&session.transcript[0], TranscriptEntry::Tool { result: Some(t), is_error: false, .. } if t == "Started job #42, task 7")
    );
    let before = session.project_snapshot();
    session.recover_interrupted("another recovery");
    assert_eq!(session.project_snapshot(), before);
}

#[test]
fn unanswered_goal_and_other_conversations_survive_recovery() {
    let mut session = partial_session();
    session.phase = AgentPhase::Done;
    let first = session.active_conversation;
    let history = format!("{:?}", session.history);
    session.start_new_conversation(Default::default());
    session.history.push(ChatMessage::user_text("unanswered"));
    session.recover_interrupted("resume");
    assert_eq!(session.history.len(), 1);
    let original = session.conversation_mut(first).unwrap();
    assert_eq!(format!("{:?}", original.history), history);
    assert_eq!(original.collected_results.len(), 2);
    assert_eq!(original.pending_calls.len(), 1);
}

#[test]
fn saved_partial_and_completed_batches_roundtrip_sqlite_without_mutating_live_session() {
    use crate::backend::{project::ProjectSession, storage::*};
    for completed in [false, true] {
        let mut session = partial_session();
        if completed {
            session.recover_interrupted("turn cancelled");
            session.phase = AgentPhase::Done;
        }
        let before = format!("{:?}", session.active());
        let snapshot = session.project_snapshot();
        assert_eq!(format!("{:?}", session.active()), before);
        let root =
            std::env::temp_dir().join(format!("silicolab-recovery-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join(".silicolab")).unwrap();
        let project = ProjectSession::from_root(root.clone(), "Recovery".into());
        initialize_project_databases(&project).unwrap();
        save_project_snapshot(
            &project,
            &ProjectSnapshot {
                name: "Recovery".into(),
                project_id: String::new(),
                entries: crate::backend::entries::EntryStore::new_empty(),
                tasks: Default::default(),
                materializations: Default::default(),
                view: Default::default(),
                history: Default::default(),
                assistant: snapshot.clone(),
                warnings: Vec::new(),
            },
            true,
        )
        .unwrap();
        let loaded = load_project_snapshot(&project).unwrap();
        assert!(loaded.warnings.is_empty());
        assert_eq!(loaded.assistant, snapshot);
        let restored = AgentSession::from_project_snapshot(loaded.assistant, Default::default());
        assert_eq!(restored.history.len(), 3);
        assert_eq!(restored.history[2].content.len(), 4);
        assert_eq!(restored.phase, AgentPhase::Done);
        assert!(restored.pending_calls.is_empty());
        assert!(restored.collected_results.is_empty());
        assert!(restored.approved_ids.is_empty());
        assert!(restored.allowed_verbs.is_empty());
        assert!(restored.allowed_risks.is_empty());
        assert!(restored.approval_inputs.is_none());
        assert_eq!(restored.project_snapshot(), snapshot);
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn existing_results_take_precedence_over_collected_duplicates() {
    let mut session = partial_session();
    let recorded = session.collected_results[0].clone();
    session.history.push(ChatMessage {
        role: Role::Tool,
        content: vec![recorded.clone(), recorded],
    });
    session.collected_results[0] = ContentBlock::ToolResult {
        tool_use_id: "started".into(),
        content: "duplicate".into(),
        is_error: true,
    };
    session.recover_interrupted("restore");
    assert_eq!(session.history[2].content.len(), 4);
    assert!(
        matches!(&session.history[2].content[0], ContentBlock::ToolResult { content, is_error: false, .. } if content == "Started job #42, task 7")
    );
}

#[test]
fn live_results_are_not_attached_to_an_earlier_batch_with_reused_ids() {
    let mut session = partial_session();
    let earlier = session.history[1].clone();
    session.history.insert(1, earlier);
    session.recover_interrupted("restore");
    assert_eq!(session.history.len(), 5);
    assert!(session.history[2].content.iter().all(|b| matches!(b, ContentBlock::ToolResult {content, is_error: true, ..} if content.contains("Result unknown"))));
    assert!(
        matches!(&session.history[4].content[0], ContentBlock::ToolResult {content, is_error: false, ..} if content == "Started job #42, task 7")
    );
}

#[test]
fn restoring_incomplete_legacy_history_marks_missing_results_unknown() {
    let mut session = partial_session();
    let mut snapshot = session.project_snapshot();
    snapshot.conversations[0].history.pop();
    let restored = AgentSession::from_project_snapshot(snapshot, Default::default());
    assert_eq!(restored.phase, AgentPhase::Done);
    assert_eq!(restored.history.len(), 3);
    assert!(restored.history[2].content.iter().all(|b| matches!(b, ContentBlock::ToolResult {content, is_error: true, ..} if content.contains("Result unknown"))));
    session.history.truncate(1);
    session.pending_calls.clear();
    session.collected_results.clear();
    let restored =
        AgentSession::from_project_snapshot(session.project_snapshot(), Default::default());
    assert_eq!(restored.history.len(), 1);
    assert!(
        matches!(&restored.history[0].content[0], ContentBlock::Text(t) if t == "original goal")
    );
}
