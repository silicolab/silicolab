use super::*;

use crate::frontend::agent::session::{AgentPhase, ModelFetchStatus, TranscriptEntry};
use crate::frontend::state::AppState;
use crate::io::llm::types::{
    AssistantTurn, ChatMessage, ContentBlock, LlmError, ReasoningBlob, Role, StopReason, ToolCall,
    Usage,
};
use eframe::egui;
use serde_json::json;

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
    state.ui.agent.phase = AgentPhase::AwaitingHeavyJob;
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

    set_assistant_base_url(&mut state, "https://llm.ducksoft.site/v1");

    assert_eq!(
        state.config.assistant.base_url.as_deref(),
        Some("https://llm.ducksoft.site/v1")
    );
    assert_eq!(state.ui.agent.model_fetch, ModelFetchStatus::Idle);
}
