use super::*;

use std::sync::atomic::Ordering;

use eframe::egui;

use crate::frontend::agent::registry;
use crate::frontend::agent::session::{AgentPhase, TranscriptEntry};
use crate::frontend::agent::tools;
use crate::frontend::jobs::{AgentTurnEvent, spawn_agent_turn};
use crate::frontend::state::AppState;
use crate::io::llm::types::{AssistantTurn, ChatMessage, LlmConfig, LlmError, StopReason};

/// Start an agent exchange from a user message. Validates the provider/key,
/// appends the user turn, and spawns the first model turn.
pub fn send_agent_message(state: &mut AppState, text: &str, ctx: &egui::Context) {
    let text = text.trim();
    if text.is_empty() {
        return;
    }
    if !state.config.assistant.enabled {
        notice(
            state,
            "The assistant is disabled — enable it in Settings ▸ Assistant.",
        );
        return;
    }
    if state.ui.agent.is_busy() || state.ui.agent.pending_approval().is_some() {
        return;
    }
    // Surface a missing key / bad provider up front, before recording the turn.
    if let Err(reason) = registry::build_provider(&state.config.assistant) {
        notice(state, &reason);
        return;
    }

    // Keep the replayed history valid if a prior exchange was interrupted.
    state.ui.agent.truncate_to_resumable();
    state.ui.agent.maybe_title_from_first_user_message(text);
    state.ui.agent.history.push(ChatMessage::user_text(text));
    state
        .ui
        .agent
        .transcript
        .push(TranscriptEntry::User(text.to_string()));
    state.ui.agent.iterations = 0;
    spawn_next_turn(state, ctx);
}

/// Build the request from current history + tools + config and spawn the worker.
pub fn spawn_next_turn(state: &mut AppState, ctx: &egui::Context) {
    if state.ui.agent.iterations >= MAX_ITERATIONS {
        notice(
            state,
            &format!("Stopped after {MAX_ITERATIONS} steps (loop bound)."),
        );
        state.ui.agent.phase = AgentPhase::Done;
        ctx.request_repaint();
        return;
    }

    let provider = match registry::build_provider(&state.config.assistant) {
        Ok(provider) => provider,
        Err(reason) => {
            notice(state, &reason);
            state.ui.agent.phase = AgentPhase::Idle;
            ctx.request_repaint();
            return;
        }
    };

    if compact_history(&mut state.ui.agent.history) {
        notice(state, "Trimmed older conversation to stay within context.");
    }

    let stream = registry::active_provider(&state.config.assistant)
        .caps_for(&state.config.assistant.model)
        .supports_streaming;
    let cfg = LlmConfig {
        model: state.config.assistant.model.clone(),
        effort: state.config.assistant.effort,
        max_output_tokens: MAX_OUTPUT_TOKENS,
        stream,
        system: system_prompt(),
    };
    let tools = tools::tool_defs();
    let history = state.ui.agent.history.clone();

    state.ui.agent.iterations += 1;
    state.ui.agent.phase = AgentPhase::AwaitingModel;
    state.jobs.agent = Some(spawn_agent_turn(provider, cfg, tools, history));
    ctx.request_repaint_after(AGENT_POLL);
}

/// Drain the in-flight agent turn (called from `poll_jobs`). Esc cancels.
pub fn poll_agent_turn(state: &mut AppState, ctx: &egui::Context) {
    let Some(job) = state.jobs.agent.take() else {
        return;
    };

    if ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
        job.cancel.store(true, Ordering::Relaxed);
        // Drop the handle; a late worker result lands on a closed channel.
        state.ui.agent.streaming_text.clear();
        notice(state, "Cancelled.");
        state.ui.agent.phase = AgentPhase::Idle;
        ctx.request_repaint();
        return;
    }

    // Drain every event available this frame: accumulate streamed text deltas
    // for a live preview, finalize on `Done`.
    loop {
        match job.receiver.try_recv() {
            Ok(AgentTurnEvent::TextDelta(text)) => {
                state.ui.agent.streaming_text.push_str(&text);
            }
            Ok(AgentTurnEvent::Done(result)) => {
                state.ui.agent.streaming_text.clear();
                handle_turn_result(state, result, ctx);
                return;
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                state.jobs.agent = Some(job);
                ctx.request_repaint_after(AGENT_POLL);
                return;
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                state.ui.agent.streaming_text.clear();
                notice(state, "Assistant worker stopped unexpectedly.");
                state.ui.agent.phase = AgentPhase::Idle;
                ctx.request_repaint();
                return;
            }
        }
    }
}

/// Process one completed turn: render text, fold usage, record the assistant
/// turn into history, then either run its tools or finish.
pub fn handle_turn_result(
    state: &mut AppState,
    result: Result<AssistantTurn, LlmError>,
    ctx: &egui::Context,
) {
    let turn = match result {
        Ok(turn) => turn,
        Err(error) => {
            let message = error.user_message();
            notice(state, &format!("Assistant error: {message}"));
            state.output_log.push(format!("assistant error: {message}"));
            state.ui.agent.phase = if matches!(error, LlmError::Cancelled) {
                AgentPhase::Idle
            } else {
                AgentPhase::Done
            };
            ctx.request_repaint();
            return;
        }
    };

    state.ui.agent.last_usage = Some(turn.usage);
    state.ui.agent.session_usage.add(&turn.usage);

    if !turn.text.trim().is_empty() {
        state
            .ui
            .agent
            .transcript
            .push(TranscriptEntry::Assistant(turn.text.clone()));
        state
            .output_log
            .push(format!("assistant> {}", turn.text.trim()));
    }

    // Record the assistant turn for replay via the provider's encoder (with a
    // provider-agnostic fallback when one can't be built, e.g. in tests).
    let assistant_message = match registry::build_provider(&state.config.assistant) {
        Ok(provider) => provider.encode_assistant_for_replay(&turn),
        Err(_) => fallback_encode(&turn),
    };
    state.ui.agent.history.push(assistant_message);

    if turn.stop == StopReason::ToolUse && !turn.tool_calls.is_empty() {
        state.ui.agent.pending_calls = turn.tool_calls.into_iter().collect();
        state.ui.agent.collected_results.clear();
        state.ui.agent.phase = AgentPhase::ExecutingTools;
        run_tool_batch(state, ctx);
    } else {
        match turn.stop {
            StopReason::MaxTokens => notice(state, "Response was cut off (max tokens reached)."),
            StopReason::Refusal => notice(state, "The model declined to respond."),
            _ => {}
        }
        state.ui.agent.phase = AgentPhase::Done;
        ctx.request_repaint();
    }
}

/// Cancel the assistant: stop the in-flight turn and any pending batch.
pub fn cancel_agent(state: &mut AppState, ctx: &egui::Context) {
    if let Some(job) = state.jobs.agent.take() {
        job.cancel.store(true, Ordering::Relaxed);
    }
    if let Some(job) = state.jobs.agent_heavy.take() {
        job.cancel();
    }
    fill_pending_tool_entry(state, "Cancelled.", true);
    state.ui.agent.pending_calls.clear();
    state.ui.agent.collected_results.clear();
    if state.ui.agent.phase != AgentPhase::Idle {
        notice(state, "Cancelled.");
    }
    state.ui.agent.phase = AgentPhase::Idle;
    ctx.request_repaint();
}
