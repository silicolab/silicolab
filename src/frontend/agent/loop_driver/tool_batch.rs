use super::*;

use eframe::egui;

use crate::frontend::agent::session::{AgentPhase, TranscriptEntry};
use crate::frontend::agent::tools;
use crate::frontend::state::AppState;
use crate::io::llm::types::{ChatMessage, ContentBlock, Role, ToolCall};

/// Run queued tool calls in order until the batch empties or one hits a
/// confirmation gate. A failing call still yields an `is_error` result; the
/// batch is not aborted (the model recovers from the error).
pub fn run_tool_batch(state: &mut AppState, ctx: &egui::Context) {
    loop {
        let Some(call) = state.ui.agent.pending_calls.front().cloned() else {
            finish_tool_batch(state, ctx);
            return;
        };
        if tools::needs_confirmation(&call) {
            state.ui.agent.phase = AgentPhase::AwaitingApproval;
            notice(state, &format!("Approve to run: {}", describe_call(&call)));
            ctx.request_repaint();
            return;
        }
        dispatch_call(state, &call, ctx);
        state.ui.agent.pending_calls.pop_front();
    }
}

/// Execute a call inline, or launch it as a detached background job. Heavy jobs
/// no longer pause the batch — `spawn_heavy` records a "started" result and the
/// computation runs off-thread, reporting back later through the queue.
pub fn dispatch_call(state: &mut AppState, call: &ToolCall, ctx: &egui::Context) {
    push_tool_call_entry(state, call);
    if let Some(kind) = heavy_kind_of(call) {
        spawn_heavy(state, call, kind, ctx);
        return;
    }
    let outcome = tools::execute_tool(state, call);
    record_result(state, call, outcome.content, outcome.is_error);
}

fn push_tool_call_entry(state: &mut AppState, call: &ToolCall) {
    state.ui.agent.transcript.push(TranscriptEntry::Tool {
        summary: describe_call(call),
        result: None,
        is_error: false,
    });
}

pub fn record_result(state: &mut AppState, call: &ToolCall, content: String, is_error: bool) {
    fill_pending_tool_entry(state, &content, is_error);
    state
        .ui
        .agent
        .collected_results
        .push(ContentBlock::ToolResult {
            tool_use_id: call.id.clone(),
            content: tools::clamp_result(&content),
            is_error,
        });
}

pub fn fill_pending_tool_entry(state: &mut AppState, content: &str, is_error: bool) {
    if let Some(TranscriptEntry::Tool {
        result,
        is_error: result_is_error,
        ..
    }) = state
        .ui
        .agent
        .transcript
        .iter_mut()
        .rev()
        .find(|entry| matches!(entry, TranscriptEntry::Tool { result: None, .. }))
    {
        *result = Some(clamp_display(content));
        *result_is_error = is_error;
    }
}

/// Flush the batch's tool results into one neutral `Tool` message and spawn the
/// next model turn.
fn finish_tool_batch(state: &mut AppState, ctx: &egui::Context) {
    let results = std::mem::take(&mut state.ui.agent.collected_results);
    state.ui.agent.history.push(ChatMessage {
        role: Role::Tool,
        content: results,
    });
    spawn_next_turn(state, ctx);
}

/// Approve the gated tool call with `id` (the front of the batch); run it and
/// continue the batch.
pub fn approve_tool_call(state: &mut AppState, id: &str, ctx: &egui::Context) {
    let Some(front) = state.ui.agent.pending_approval().cloned() else {
        return;
    };
    if front.id != id {
        return;
    }
    state.ui.agent.phase = AgentPhase::ExecutingTools;
    dispatch_call(state, &front, ctx);
    state.ui.agent.pending_calls.pop_front();
    run_tool_batch(state, ctx);
}

/// Reject the gated tool call with `id`: record an `is_error` result so the
/// model learns it was declined, then continue the batch.
pub fn reject_tool_call(state: &mut AppState, id: &str, ctx: &egui::Context) {
    let Some(front) = state.ui.agent.pending_approval().cloned() else {
        return;
    };
    if front.id != id {
        return;
    }
    push_tool_call_entry(state, &front);
    record_result(
        state,
        &front,
        "The user declined to run this command.".to_string(),
        true,
    );
    state.ui.agent.pending_calls.pop_front();
    state.ui.agent.phase = AgentPhase::ExecutingTools;
    run_tool_batch(state, ctx);
}
