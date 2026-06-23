use super::*;

use crate::frontend::agent::session::{AgentPhase, ModelFetchStatus, PendingTurn, TranscriptEntry};
use crate::frontend::state::AppState;
use crate::io::llm::types::{
    AssistantTurn, ChatMessage, ContentBlock, LlmError, ReasoningBlob, Role, StopReason, ToolCall,
    Usage,
};
use eframe::egui;
use serde_json::json;

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use crate::engines::qm::QmOutcome;
use crate::frontend::jobs::{AgentHeavyJob, QmWorkerMessage, RunningQmJob, TrackedAgentJob};

/// A background QM job whose channel the test controls, so completion/back-pressure
/// can be exercised without running a real calculation.
fn fake_qm_job(
    id: u64,
    conversation: crate::frontend::agent::AssistantConversationId,
    sender: std::sync::mpsc::Receiver<QmWorkerMessage>,
) -> TrackedAgentJob {
    TrackedAgentJob {
        id,
        conversation,
        label: "qm optimize".to_string(),
        job: AgentHeavyJob::Qm(RunningQmJob {
            cancel: Arc::new(AtomicBool::new(false)),
            receiver: sender,
        }),
    }
}

fn turn_with_tool(command: &str) -> AssistantTurn {
    AssistantTurn {
        text: "Working on it.".to_string(),
        tool_calls: vec![ToolCall {
            id: "call_1".to_string(),
            name: "run_command".to_string(),
            input: json!({ "command": command }),
        }],
        reasoning: ReasoningBlob::None,
        stop: StopReason::ToolUse,
        usage: Usage {
            input: 10,
            output: 4,
            ..Usage::default()
        },
    }
}

fn end_turn(text: &str) -> AssistantTurn {
    AssistantTurn {
        text: text.to_string(),
        tool_calls: Vec::new(),
        reasoning: ReasoningBlob::None,
        stop: StopReason::EndTurn,
        usage: Usage::default(),
    }
}

#[test]
fn read_only_tool_runs_inline_and_records_result() {
    let mut state = AppState::scratch(Default::default(), Vec::new());
    let ctx = egui::Context::default();
    // No API key in tests, so the follow-up turn fails to spawn gracefully;
    // the tool itself still executes and is recorded.
    handle_turn_result(&mut state, Ok(turn_with_tool("inspect")), &ctx);
    // (the model asked for run_command "inspect" — a console command — which
    // is unknown to the console and returns an error result; the point is the
    // tool dispatched and produced a tool_result.)
    assert!(
        state
            .output_log
            .iter()
            .any(|line| line.starts_with("agent>")),
        "tool command should echo into the shared output log"
    );
    // Usage accumulated.
    assert_eq!(state.ui.agent.session_usage.input, 10);
}

#[test]
fn gated_command_pauses_for_approval() {
    let mut state = AppState::scratch(Default::default(), Vec::new());
    let ctx = egui::Context::default();
    handle_turn_result(&mut state, Ok(turn_with_tool("delete chain A")), &ctx);
    assert_eq!(state.ui.agent.phase, AgentPhase::AwaitingApproval);
    let pending = state.ui.agent.pending_approval().expect("a pending call");
    assert_eq!(pending.id, "call_1");
}

#[test]
fn rejecting_records_error_result_and_clears_pending() {
    let mut state = AppState::scratch(Default::default(), Vec::new());
    let ctx = egui::Context::default();
    handle_turn_result(&mut state, Ok(turn_with_tool("delete chain A")), &ctx);
    reject_tool_call(&mut state, "call_1", &ctx);
    assert!(state.ui.agent.pending_approval().is_none());
    // A tool_result (is_error) was appended to history as a Tool message
    // when the batch finished and the next turn was attempted.
    let has_tool_message = state
        .ui
        .agent
        .history
        .iter()
        .any(|message| message.role == Role::Tool);
    assert!(has_tool_message);
}

#[test]
fn rejecting_shows_a_resolved_tool_entry() {
    let mut state = AppState::scratch(Default::default(), Vec::new());
    let ctx = egui::Context::default();
    handle_turn_result(&mut state, Ok(turn_with_tool("delete chain A")), &ctx);
    reject_tool_call(&mut state, "call_1", &ctx);
    let resolved = state.ui.agent.transcript.iter().any(|entry| {
        matches!(
            entry,
            TranscriptEntry::Tool {
                result: Some(_),
                is_error: true,
                ..
            }
        )
    });
    assert!(resolved);
}

#[test]
fn cancel_resolves_a_running_tool_entry() {
    let mut state = AppState::scratch(Default::default(), Vec::new());
    let ctx = egui::Context::default();
    state.ui.agent.transcript.push(TranscriptEntry::Tool {
        summary: "md run".to_string(),
        result: None,
        is_error: false,
    });
    state.ui.agent.phase = AgentPhase::ExecutingTools;
    cancel_agent(&mut state, &ctx);
    let still_running = state
        .ui
        .agent
        .transcript
        .iter()
        .any(|entry| matches!(entry, TranscriptEntry::Tool { result: None, .. }));
    assert!(!still_running);
    let cancelled = state.ui.agent.transcript.iter().any(|entry| {
        matches!(
            entry,
            TranscriptEntry::Tool {
                result: Some(text),
                is_error: true,
                ..
            } if text == "Cancelled."
        )
    });
    assert!(cancelled);
}

#[test]
fn heavy_commands_are_classified() {
    let heavy = |command: &str| {
        heavy_kind_of(&ToolCall {
            id: "x".into(),
            name: "run_command".into(),
            input: json!({ "command": command }),
        })
        .is_some()
    };
    assert!(heavy("qm energy --basis sto-3g"));
    assert!(heavy("qm optimize"));
    assert!(heavy("md run --preset standard"));
    assert!(heavy("md simulate --time 1ns"));
    // Fast commands are not heavy.
    assert!(!heavy("md build"));
    assert!(!heavy("qm recommend general"));
    assert!(!heavy("open x.pdb"));
    assert!(!heavy("color hetero"));
}

#[test]
fn end_turn_finishes_without_tools() {
    let mut state = AppState::scratch(Default::default(), Vec::new());
    let ctx = egui::Context::default();
    handle_turn_result(&mut state, Ok(end_turn("All done.")), &ctx);
    assert_eq!(state.ui.agent.phase, AgentPhase::Done);
    assert!(state.ui.agent.pending_calls.is_empty());
}

#[test]
fn loop_bound_stops_spawning() {
    let mut state = AppState::scratch(Default::default(), Vec::new());
    let ctx = egui::Context::default();
    state.ui.agent.iterations = MAX_ITERATIONS;
    spawn_next_turn(&mut state, &ctx);
    assert_eq!(state.ui.agent.phase, AgentPhase::Done);
    assert!(state.jobs.agent.is_none());
}

#[test]
fn error_result_is_surfaced() {
    let mut state = AppState::scratch(Default::default(), Vec::new());
    let ctx = egui::Context::default();
    handle_turn_result(
        &mut state,
        Err(LlmError::BadRequest("bad shape".to_string())),
        &ctx,
    );
    assert!(
        state
            .output_log
            .iter()
            .any(|line| line.contains("bad shape"))
    );
}

#[test]
fn compaction_keeps_a_valid_user_boundary() {
    // Build many short exchanges: User, Assistant, User, Assistant, ...
    let mut history: Vec<ChatMessage> = Vec::new();
    for i in 0..MAX_HISTORY_MESSAGES + 20 {
        let role = if i % 2 == 0 {
            Role::User
        } else {
            Role::Assistant
        };
        history.push(ChatMessage {
            role,
            content: vec![ContentBlock::Text(format!("m{i}"))],
        });
    }
    let trimmed = compact_history(&mut history);
    assert!(trimmed);
    assert!(history.len() <= TARGET_HISTORY_MESSAGES);
    // The kept history must start at a genuine user turn.
    assert_eq!(history.first().map(|m| m.role), Some(Role::User));
}

#[test]
fn compaction_noop_when_short() {
    let mut history = vec![ChatMessage::user_text("hi")];
    assert!(!compact_history(&mut history));
    assert_eq!(history.len(), 1);
}

/// A scratch state with the assistant enabled, so `send_agent_message` reaches
/// the busy/queue logic (it gates on `assistant.enabled` first).
fn enabled_state() -> AppState {
    let mut state = AppState::scratch(Default::default(), Vec::new());
    state.config.assistant.enabled = true;
    state
}

#[test]
fn busy_send_enqueues_instead_of_dropping() {
    let mut state = enabled_state();
    let ctx = egui::Context::default();
    state.ui.agent.phase = AgentPhase::AwaitingModel; // busy
    let history_before = state.ui.agent.history.len();

    send_agent_message(&mut state, "do this next", &ctx);

    assert_eq!(state.ui.agent.queued.len(), 1, "busy send should enqueue");
    assert!(matches!(
        state.ui.agent.queued.front(),
        Some(PendingTurn::UserMessage(text)) if text == "do this next"
    ));
    // Not sent yet: history is untouched while busy.
    assert_eq!(state.ui.agent.history.len(), history_before);
}

#[test]
fn send_while_awaiting_approval_enqueues() {
    let mut state = enabled_state();
    let ctx = egui::Context::default();
    handle_turn_result(&mut state, Ok(turn_with_tool("delete chain A")), &ctx);
    assert_eq!(state.ui.agent.phase, AgentPhase::AwaitingApproval);

    send_agent_message(&mut state, "hold on", &ctx);

    assert_eq!(state.ui.agent.queued.len(), 1);
}

#[test]
fn pump_holds_while_busy() {
    let mut state = enabled_state();
    let ctx = egui::Context::default();
    state
        .ui
        .agent
        .queued
        .push_back(PendingTurn::UserMessage("later".into()));
    state.ui.agent.phase = AgentPhase::AwaitingModel;

    pump_queue(&mut state, &ctx);

    assert_eq!(
        state.ui.agent.queued.len(),
        1,
        "must not dispatch while busy"
    );
}

#[test]
fn pump_consumes_front_when_idle() {
    let mut state = enabled_state();
    let ctx = egui::Context::default();
    state
        .ui
        .agent
        .queued
        .push_back(PendingTurn::UserMessage("later".into()));
    state.ui.agent.phase = AgentPhase::Done;

    pump_queue(&mut state, &ctx);

    assert!(
        state.ui.agent.queued.is_empty(),
        "an idle pump consumes the front item"
    );
}

#[test]
fn completing_a_turn_flushes_a_queued_followup() {
    let mut state = enabled_state();
    let ctx = egui::Context::default();
    state
        .ui
        .agent
        .queued
        .push_back(PendingTurn::UserMessage("and then this".into()));

    handle_turn_result(&mut state, Ok(end_turn("done")), &ctx);

    assert!(
        state.ui.agent.queued.is_empty(),
        "finishing a turn should dispatch the queued follow-up"
    );
}

#[test]
fn cancel_clears_the_queue() {
    let mut state = enabled_state();
    let ctx = egui::Context::default();
    state
        .ui
        .agent
        .queued
        .push_back(PendingTurn::UserMessage("a".into()));
    state
        .ui
        .agent
        .queued
        .push_back(PendingTurn::UserMessage("b".into()));
    state.ui.agent.phase = AgentPhase::AwaitingModel;

    cancel_agent(&mut state, &ctx);

    assert!(state.ui.agent.queued.is_empty());
}

#[test]
fn hard_error_clears_the_queue() {
    let mut state = enabled_state();
    let ctx = egui::Context::default();
    state
        .ui
        .agent
        .queued
        .push_back(PendingTurn::UserMessage("a".into()));

    handle_turn_result(&mut state, Err(LlmError::BadRequest("nope".into())), &ctx);

    assert!(state.ui.agent.queued.is_empty());
}

#[test]
fn remove_queued_drops_the_indexed_item() {
    let mut state = enabled_state();
    state
        .ui
        .agent
        .queued
        .push_back(PendingTurn::UserMessage("first".into()));
    state
        .ui
        .agent
        .queued
        .push_back(PendingTurn::UserMessage("second".into()));

    state.ui.agent.remove_queued(0);

    assert_eq!(state.ui.agent.queued.len(), 1);
    assert!(matches!(
        state.ui.agent.queued.front(),
        Some(PendingTurn::UserMessage(text)) if text == "second"
    ));
}

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

    // No second job spawned — serialized to one.
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

    // FIFO: the JobDone pumps first (consumed), the new message waits behind it —
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
    // in tests, so the turn itself doesn't spawn — the point is it no longer sticks).
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
