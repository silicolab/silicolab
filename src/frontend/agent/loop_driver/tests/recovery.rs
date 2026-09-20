use super::*;
use crate::backend::config::ApprovalMode;
use crate::frontend::jobs::{AgentTurnEvent, RunningAgentTurn};

fn offline_state() -> AppState {
    let mut state = enabled_state();
    state.ui.agent.selection.provider = "local".into();
    state.ui.agent.selection.model = "offline-test".into();
    state
        .config
        .assistant
        .base_urls
        .insert("local".into(), "invalid://offline".into());
    state
        .ui
        .agent
        .history
        .push(ChatMessage::user_text("create methane then calculate"));
    state
}

fn results(state: &AppState) -> Vec<(&str, &str, bool)> {
    state
        .ui
        .agent
        .history
        .iter()
        .flat_map(|m| &m.content)
        .filter_map(|b| match b {
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => Some((tool_use_id.as_str(), content.as_str(), *is_error)),
            _ => None,
        })
        .collect()
}

#[test]
fn completed_action_survives_model_error_and_user_continuation_without_replay() {
    let mut state = offline_state();
    let ctx = egui::Context::default();
    state.config.assistant.approval_mode = ApprovalMode::Auto;
    handle_turn_result(&mut state, Ok(turn_with_tool("sketch C")), &ctx);
    assert_eq!(state.entries.records.len(), 1);
    assert_eq!(results(&state).len(), 1);
    let result = results(&state)[0].1.to_string();
    state.jobs.agent.take();
    handle_turn_result(
        &mut state,
        Err(LlmError::BadRequest("offline failure".into())),
        &ctx,
    );
    send_agent_message(&mut state, "continue", &ctx);
    assert_eq!(state.entries.records.len(), 1);
    assert_eq!(results(&state), vec![("call_1", result.as_str(), false)]);
    assert!(
        matches!(&state.ui.agent.history[0].content[0], ContentBlock::Text(t) if t == "create methane then calculate")
    );
    assert!(
        matches!(&state.ui.agent.history.last().unwrap().content[0], ContentBlock::Text(t) if t == "continue")
    );
    cancel_agent(&mut state, &ctx);
}

#[test]
fn cancel_after_partial_batch_preserves_success_and_never_runs_approved_tail() {
    let mut state = offline_state();
    let ctx = egui::Context::default();
    state.config.assistant.approval_mode = ApprovalMode::Auto;
    handle_turn_result(
        &mut state,
        Ok(turn_with_tools(&["sketch C", "delete chain A", "sketch O"])),
        &ctx,
    );
    assert_eq!(state.ui.agent.phase, AgentPhase::AwaitingApproval);
    assert_eq!(state.entries.records.len(), 1);
    assert_eq!(state.ui.agent.collected_results.len(), 1);
    cancel_agent(&mut state, &ctx);
    assert_eq!(results(&state).len(), 3);
    assert!(!results(&state)[0].2);
    for (_, content, is_error) in &results(&state)[1..] {
        assert!(*is_error);
        assert!(content.contains("Not executed: turn cancelled"));
    }
    approve_tool_call(&mut state, "call_2", &ctx);
    assert_eq!(state.entries.records.len(), 1);
    assert!(state.ui.agent.pending_calls.is_empty());
    assert!(state.ui.agent.collected_results.is_empty());
    let before = format!("{:?}", state.ui.agent.history);
    cancel_agent(&mut state, &ctx);
    assert_eq!(format!("{:?}", state.ui.agent.history), before);
}

#[test]
fn worker_disconnect_and_cancelled_late_reply_preserve_the_goal() {
    for cancelled in [false, true] {
        let mut state = offline_state();
        let ctx = egui::Context::default();
        let (tx, receiver) = std::sync::mpsc::channel();
        state.jobs.agent = Some(RunningAgentTurn {
            cancel: Arc::new(AtomicBool::new(false)),
            receiver,
        });
        state.ui.agent.phase = AgentPhase::AwaitingModel;
        if cancelled {
            cancel_agent(&mut state, &ctx);
            assert!(
                tx.send(AgentTurnEvent::Done(Ok(turn_with_tool("sketch C"))))
                    .is_err()
            );
        } else {
            drop(tx);
        }
        poll_agent_turn(&mut state, &ctx);
        assert!(state.jobs.agent.is_none());
        assert_eq!(state.ui.agent.history.len(), 1);
        assert!(state.entries.records.is_empty());
    }
}

#[test]
fn background_receipt_and_completion_survive_model_failure_in_owning_conversation() {
    let mut state = offline_state();
    let ctx = egui::Context::default();
    let origin = state.ui.agent.active_conversation;
    let turn = turn_with_tool("qm energy");
    state.ui.agent.history.push(fallback_encode(&turn));
    state
        .ui
        .agent
        .collected_results
        .push(ContentBlock::ToolResult {
            tool_use_id: "call_1".into(),
            content: "Started job #42, task 7".into(),
            is_error: false,
        });
    handle_turn_result(
        &mut state,
        Err(LlmError::BadRequest("offline".into())),
        &ctx,
    );
    state.ui.agent.start_new_conversation(Default::default());
    state
        .ui
        .agent
        .history
        .push(ChatMessage::user_text("unrelated"));
    state
        .ui
        .agent
        .conversation_mut(origin)
        .unwrap()
        .queued
        .push_back(PendingTurn::JobDone {
            label: "job #42, task 7".into(),
            summary: "energy -1.0".into(),
            is_error: false,
        });
    pump_queue(&mut state, &ctx);
    assert_eq!(state.ui.agent.history.len(), 1);
    state.ui.agent.switch_conversation(origin);
    pump_queue(&mut state, &ctx);
    assert_eq!(
        results(&state),
        vec![("call_1", "Started job #42, task 7", false)]
    );
    assert!(
        matches!(&state.ui.agent.history.last().unwrap().content[0], ContentBlock::Text(t) if t.contains("job #42, task 7") && t.contains("energy -1.0"))
    );
    cancel_agent(&mut state, &ctx);
}
