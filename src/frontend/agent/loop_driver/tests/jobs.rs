use super::super::*;
use super::{enabled_state, fake_qm_job};

use crate::engines::qm::QmOutcome;
use crate::frontend::agent::session::{AgentPhase, ModelFetchStatus, PendingTurn, TranscriptEntry};
use crate::frontend::jobs::QmWorkerMessage;
use crate::frontend::state::AppState;
use crate::io::llm::types::{ChatMessage, ContentBlock, LlmError, ReasoningBlob, Role, ToolCall};

use eframe::egui;
use serde_json::json;

#[test]
fn job_done_is_consumed_when_idle() {
    let mut state = enabled_state();
    let ctx = egui::Context::default();
    state.ui.agent.queued.push_back(PendingTurn::JobDone {
        label: "qm optimize".into(),
        summary: "E = -1.0 Ha".into(),
        is_error: false,
    });
    state.ui.agent.phase = AgentPhase::Done;

    pump_queue(&mut state, &ctx);

    assert!(
        state.ui.agent.queued.is_empty(),
        "an idle pump consumes a queued JobDone to wake the model"
    );
}

#[test]
fn heavy_launch_is_refused_while_one_is_running() {
    let mut state = enabled_state();
    let ctx = egui::Context::default();
    let conversation = state.ui.agent.active_conversation;
    let (_tx, rx) = std::sync::mpsc::channel();
    state.jobs.agent_jobs.push(fake_qm_job(1, conversation, rx));

    let call = ToolCall {
        id: "c1".into(),
        name: "run_command".into(),
        input: json!({ "command": "qm energy --basis sto-3g" }),
    };
    dispatch_call(&mut state, &call, &ctx);

    // No second job spawned â€” serialized to one.
    assert_eq!(state.jobs.agent_jobs.len(), 1);
    // The model is told to wait, not handed an error.
    let told_to_wait = state.ui.agent.transcript.iter().any(|entry| {
        matches!(
            entry,
            TranscriptEntry::Tool { result: Some(text), is_error: false, .. }
            if text.contains("already running")
        )
    });
    assert!(told_to_wait);
}

#[test]
fn finished_job_posts_notice_and_clears_the_registry() {
    let mut state = enabled_state();
    let ctx = egui::Context::default();
    let conversation = state.ui.agent.active_conversation;
    let (tx, rx) = std::sync::mpsc::channel();
    tx.send(QmWorkerMessage::Finished(Box::new(QmOutcome {
        energy_hartree: -1.0,
        converged: true,
        optimized_structure: None,
        summary: "E = -1.0 Ha".into(),
        scf_trace: Vec::new(),
        opt_trace: Vec::new(),
        frequencies: Vec::new(),
    })))
    .unwrap();
    drop(tx);
    state.jobs.agent_jobs.push(fake_qm_job(7, conversation, rx));

    poll_agent_jobs(&mut state, &ctx);

    // The completed job leaves the registry...
    assert!(state.jobs.agent_jobs.is_empty());
    // ...and the originating conversation gets a "finished" notice. (The JobDone it
    // enqueued is then pumped; with no API key in tests it is consumed, so the
    // notice is the durable evidence the completion was routed.)
    let finished =
        state.ui.agent.transcript.iter().any(
            |entry| matches!(entry, TranscriptEntry::Notice(text) if text.contains("finished")),
        );
    assert!(finished);
}

#[test]
fn cancel_keeps_jobdone_but_drops_typed_messages() {
    let mut state = enabled_state();
    let ctx = egui::Context::default();
    state
        .ui
        .agent
        .queued
        .push_back(PendingTurn::UserMessage("typed".into()));
    state.ui.agent.queued.push_back(PendingTurn::JobDone {
        label: "qm optimize".into(),
        summary: "done".into(),
        is_error: false,
    });
    state.ui.agent.phase = AgentPhase::AwaitingModel;

    cancel_agent(&mut state, &ctx);

    // The typed follow-up is dropped, but the completed job's result survives.
    assert_eq!(state.ui.agent.queued.len(), 1);
    assert!(matches!(
        state.ui.agent.queued.front(),
        Some(PendingTurn::JobDone { .. })
    ));
}

#[test]
fn idle_send_drains_queued_jobdone_first() {
    let mut state = enabled_state();
    let ctx = egui::Context::default();
    state.ui.agent.queued.push_back(PendingTurn::JobDone {
        label: "qm optimize".into(),
        summary: "done".into(),
        is_error: false,
    });
    state.ui.agent.phase = AgentPhase::Idle;

    send_agent_message(&mut state, "and now this", &ctx);

    // FIFO: the JobDone pumps first (consumed), the new message waits behind it â€”
    // not jumping ahead and not stranding the JobDone.
    assert_eq!(state.ui.agent.queued.len(), 1);
    assert!(matches!(
        state.ui.agent.queued.front(),
        Some(PendingTurn::UserMessage(t)) if t == "and now this"
    ));
}

#[test]
fn stop_cancels_active_conversation_background_jobs() {
    let mut state = enabled_state();
    let ctx = egui::Context::default();
    let conv = state.ui.agent.active_conversation;
    let (_tx, rx) = std::sync::mpsc::channel();
    state.jobs.agent_jobs.push(fake_qm_job(1, conv, rx));
    state.ui.agent.phase = AgentPhase::AwaitingModel;

    cancel_agent(&mut state, &ctx);

    assert!(
        state.jobs.agent_jobs.is_empty(),
        "Stop cancels the active conversation's background jobs"
    );
}

#[test]
fn deleting_a_conversation_cancels_its_background_jobs() {
    let mut state = enabled_state();
    let ctx = egui::Context::default();
    let first = state.ui.agent.active_conversation;
    state.ui.agent.start_new_conversation(); // a 2nd chat, so delete removes `first`
    let (_tx, rx) = std::sync::mpsc::channel();
    state.jobs.agent_jobs.push(fake_qm_job(1, first, rx));

    delete_assistant_conversation(&mut state, first);

    assert!(
        state.jobs.agent_jobs.is_empty(),
        "deleting a chat cancels the jobs it launched"
    );
    let _ = ctx;
}

#[test]
fn job_finishing_in_inactive_conversation_routes_to_its_origin() {
    let mut state = enabled_state();
    let ctx = egui::Context::default();
    let origin = state.ui.agent.active_conversation;
    let (tx, rx) = std::sync::mpsc::channel();
    tx.send(QmWorkerMessage::Finished(Box::new(QmOutcome {
        energy_hartree: -1.0,
        converged: true,
        optimized_structure: None,
        summary: "E = -1.0 Ha".into(),
        scf_trace: Vec::new(),
        opt_trace: Vec::new(),
        frequencies: Vec::new(),
    })))
    .unwrap();
    drop(tx);
    state.jobs.agent_jobs.push(fake_qm_job(5, origin, rx));
    // Make a different conversation active before the job completes.
    state.ui.agent.start_new_conversation();
    let active = state.ui.agent.active_conversation;
    assert_ne!(origin, active);

    poll_agent_jobs(&mut state, &ctx);

    // The active conversation gets nothing; the follow-up lands in the origin.
    assert!(state.ui.agent.queued.is_empty());
    let origin_conv = state
        .ui
        .agent
        .conversation_mut(origin)
        .expect("origin exists");
    assert_eq!(origin_conv.queued.len(), 1);
    assert!(matches!(
        origin_conv.queued.front(),
        Some(PendingTurn::JobDone { .. })
    ));
}

#[test]
fn switching_back_pumps_a_pending_jobdone() {
    let mut state = enabled_state();
    let ctx = egui::Context::default();
    let origin = state.ui.agent.active_conversation;
    state.ui.agent.start_new_conversation(); // switch away from `origin`
    state
        .ui
        .agent
        .conversation_mut(origin)
        .expect("origin exists")
        .queued
        .push_back(PendingTurn::JobDone {
            label: "qm optimize".into(),
            summary: "done".into(),
            is_error: false,
        });

    switch_assistant_conversation(&mut state, origin, &ctx);

    // Returning to the conversation pumps its stranded follow-up (consumed; no key
    // in tests, so the turn itself doesn't spawn â€” the point is it no longer sticks).
    assert!(state.ui.agent.queued.is_empty());
}

#[test]
fn switching_provider_strips_reasoning() {
    let mut state = AppState::scratch(Default::default(), Vec::new());
    state.ui.agent.history.push(ChatMessage {
        role: Role::Assistant,
        content: vec![
            ContentBlock::OpaqueReasoning(ReasoningBlob::Anthropic(vec![json!({})])),
            ContentBlock::Text("hi".to_string()),
        ],
    });
    switch_provider_model(&mut state, "anthropic", "claude-opus-4-8");
    let still_has_reasoning = state.ui.agent.history.iter().any(|message| {
        message
            .content
            .iter()
            .any(|block| matches!(block, ContentBlock::OpaqueReasoning(_)))
    });
    assert!(!still_has_reasoning);
    assert_eq!(state.config.assistant.model, "claude-opus-4-8");
}

#[test]
fn changing_base_url_clears_stale_model_fetch_error() {
    let mut state = AppState::scratch(Default::default(), Vec::new());
    state.ui.agent.model_fetch = ModelFetchStatus::Error("io: No route to host".to_string());

    set_assistant_base_url(&mut state, "https://api.example.com/v1");

    assert_eq!(
        state.config.assistant.base_url.as_deref(),
        Some("https://api.example.com/v1")
    );
    assert_eq!(state.ui.agent.model_fetch, ModelFetchStatus::Idle);
}

#[test]
fn pump_marks_dequeued_follow_up_as_running_backlog() {
    let mut state = AppState::scratch(Default::default(), Vec::new());
    let ctx = egui::Context::default();
    state
        .ui
        .agent
        .queued
        .push_back(PendingTurn::UserMessage("do this next".into()));

    pump_queue(&mut state, &ctx);

    assert!(state.ui.agent.queued.is_empty());
    assert_eq!(
        state.ui.agent.current_backlog.as_deref(),
        Some("do this next")
    );
}

#[test]
fn flush_on_next_pump_clears_backlog() {
    let mut state = AppState::scratch(Default::default(), Vec::new());
    let ctx = egui::Context::default();
    state
        .ui
        .agent
        .note_backlog_start("earlier follow-up".into(), 0);
    state
        .ui
        .agent
        .transcript
        .push(TranscriptEntry::Assistant("Here is the answer.".into()));

    pump_queue(&mut state, &ctx);

    assert!(state.ui.agent.current_backlog.is_none());
}

#[test]
fn errored_backlog_turn_clears_backlog() {
    let mut state = AppState::scratch(Default::default(), Vec::new());
    let ctx = egui::Context::default();
    state
        .ui
        .agent
        .note_backlog_start("risky follow-up".into(), 0);

    handle_turn_result(&mut state, Err(LlmError::BadRequest("nope".into())), &ctx);

    assert!(state.ui.agent.current_backlog.is_none());
}
