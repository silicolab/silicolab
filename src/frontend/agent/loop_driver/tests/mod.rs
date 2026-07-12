use super::*;

use crate::frontend::agent::session::{AgentPhase, PendingTurn, TranscriptEntry};
use crate::frontend::state::AppState;
use crate::io::llm::types::{
    AssistantTurn, ChatMessage, ContentBlock, LlmError, ReasoningBlob, Role, StopReason, ToolCall,
    Usage,
};
use eframe::egui;
use serde_json::json;

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

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
        task_run_id: 0,
        job_id: crate::job::JobId::new(),
        job: AgentHeavyJob::Qm(RunningQmJob {
            cancel: crate::wire::JobCancelHandle::from_flag(Arc::new(AtomicBool::new(false))),
            receiver: sender,
            latest_stage: None,
            cancel_requested: false,
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

fn turn_with_tools(commands: &[&str]) -> AssistantTurn {
    AssistantTurn {
        text: "Working on it.".to_string(),
        tool_calls: commands
            .iter()
            .enumerate()
            .map(|(index, command)| ToolCall {
                id: format!("call_{}", index + 1),
                name: "run_command".to_string(),
                input: json!({ "command": command }),
            })
            .collect(),
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
    // (the model asked for run_command "inspect" â€” a console command â€” which
    // is unknown to the console and returns an error result; the point is the
    // tool dispatched and produced a tool_result.)
    use crate::frontend::state::{CommandActor, LogFilter, LogQuery, LogScope};
    let query = LogQuery::new(LogFilter::Command);
    assert!(
        state.session_log().query(&query).any(|entry| {
            matches!(
                entry.scope,
                LogScope::Command {
                    actor: CommandActor::Agent,
                    ..
                }
            ) && entry.text == "inspect"
        }),
        "the assistant-issued command is recorded in the Console transcript"
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
fn batch_gates_only_consequential_calls_and_resolves_in_any_order() {
    let mut state = AppState::scratch(Default::default(), Vec::new());
    let ctx = egui::Context::default();
    // Default mode is AutoSafe: the read-only call auto-runs (an unknown command
    // here, so it errors gracefully without touching the empty workspace); the
    // compute and destructive calls wait â€” together, as one batch. They are
    // resolved by reject so nothing executes against the entry-less scratch state.
    handle_turn_result(
        &mut state,
        Ok(turn_with_tools(&[
            "inspect-noop",            // read-only â†’ auto-runs, errors gracefully
            "score --receptor active", // compute â†’ gated in AutoSafe
            "delete chain A",          // destructive â†’ always gated
        ])),
        &ctx,
    );
    assert_eq!(state.ui.agent.phase, AgentPhase::AwaitingApproval);
    let ids: Vec<String> = gated_pending(&state)
        .iter()
        .map(|call| call.id.clone())
        .collect();
    assert_eq!(
        ids,
        vec!["call_2", "call_3"],
        "only the two gated calls wait"
    );

    // Resolve out of order: rejecting the destructive one leaves the batch paused
    // on the still-undecided compute call.
    reject_tool_call(&mut state, "call_3", &ctx);
    assert_eq!(state.ui.agent.phase, AgentPhase::AwaitingApproval);
    let remaining: Vec<String> = gated_pending(&state)
        .iter()
        .map(|call| call.id.clone())
        .collect();
    assert_eq!(remaining, vec!["call_2"]);

    // Resolving the last one drains the batch.
    reject_tool_call(&mut state, "call_2", &ctx);
    assert!(state.ui.agent.pending_approval().is_none());
    assert!(state.ui.agent.pending_calls.is_empty());
}

#[test]
fn plan_mode_proposes_without_executing() {
    let mut state = AppState::scratch(Default::default(), Vec::new());
    state.config.assistant.approval_mode = crate::backend::config::ApprovalMode::Plan;
    let ctx = egui::Context::default();
    handle_turn_result(&mut state, Ok(turn_with_tool("delete chain A")), &ctx);
    // Nothing waits and nothing destructive ran; the call became a proposal.
    assert!(state.ui.agent.pending_approval().is_none());
    assert!(state.ui.agent.pending_calls.is_empty());
    let query = crate::frontend::state::LogQuery::new(crate::frontend::state::LogFilter::Command);
    assert!(
        !state.session_log().query(&query).any(|entry| matches!(
            entry.scope,
            crate::frontend::state::LogScope::Command {
                actor: crate::frontend::state::CommandActor::Agent,
                ..
            }
        )),
        "plan mode must not execute the command"
    );
}

#[test]
fn plan_mode_runs_perception_but_only_proposes_doers() {
    let mut state = AppState::scratch(Default::default(), Vec::new());
    state.config.assistant.approval_mode = crate::backend::config::ApprovalMode::Plan;
    let ctx = egui::Context::default();
    // One perception call (inspect) and one doer (run_command): the perception
    // must execute so the model can ground its plan; the doer must not run.
    let turn = AssistantTurn {
        text: "Planning.".to_string(),
        tool_calls: vec![
            ToolCall {
                id: "call_1".to_string(),
                name: "inspect".to_string(),
                input: json!({}),
            },
            ToolCall {
                id: "call_2".to_string(),
                name: "run_command".to_string(),
                input: json!({ "command": "delete chain A" }),
            },
        ],
        reasoning: ReasoningBlob::None,
        stop: StopReason::ToolUse,
        usage: Usage {
            input: 10,
            output: 4,
            ..Usage::default()
        },
    };
    handle_turn_result(&mut state, Ok(turn), &ctx);

    let results = last_tool_results(&state);
    let inspect = results.get("call_1").expect("inspect produced a result");
    let doer = results.get("call_2").expect("the doer produced a result");
    assert!(
        !inspect.starts_with("Plan mode: not executed"),
        "perception runs in Plan mode, got: {inspect}"
    );
    assert!(
        doer.starts_with("Plan mode: not executed"),
        "doers are proposed, not run, got: {doer}"
    );
}

/// The `tool_use_id` â†’ content map of the last flushed `Tool` message, for
/// asserting what each call in a finished batch returned.
fn last_tool_results(state: &AppState) -> std::collections::HashMap<String, String> {
    state
        .ui
        .agent
        .history
        .iter()
        .rev()
        .find(|message| message.role == Role::Tool)
        .map(|message| {
            message
                .content
                .iter()
                .filter_map(|block| match block {
                    ContentBlock::ToolResult {
                        tool_use_id,
                        content,
                        ..
                    } => Some((tool_use_id.clone(), content.clone())),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
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
    let query = crate::frontend::state::LogQuery::new(crate::frontend::state::LogFilter::Source(
        crate::frontend::state::OutputSource::Agent,
    ));
    assert!(
        state
            .session_log()
            .query(&query)
            .any(|entry| entry.text.contains("bad shape")),
        "the turn error is surfaced as Agent-scoped detail"
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

mod jobs;
