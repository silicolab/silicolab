use super::*;

use eframe::egui;

use crate::backend::config::ApprovalMode;
use crate::frontend::agent::session::{AgentPhase, TranscriptEntry};
use crate::frontend::agent::tools;
use crate::frontend::console::RiskLevel;
use crate::frontend::state::AppState;
use crate::io::llm::types::{ChatMessage, ContentBlock, Role, ToolCall};

/// Run queued tool calls in order until the batch empties or one hits the
/// approval gate. A failing call still yields an `is_error` result; the batch is
/// not aborted (the model recovers from the error). `Plan` mode diverts to
/// [`propose_in_plan_mode`].
pub fn run_tool_batch(state: &mut AppState, ctx: &egui::Context) {
    if state.config.assistant.approval_mode == ApprovalMode::Plan {
        propose_in_plan_mode(state, ctx);
        return;
    }
    loop {
        let Some(call) = state.ui.agent.pending_calls.front().cloned() else {
            finish_tool_batch(state, ctx);
            return;
        };
        if gate_blocks(state, &call) {
            state.ui.agent.phase = AgentPhase::AwaitingApproval;
            ctx.request_repaint();
            return;
        }
        state.ui.agent.approved_ids.remove(&call.id);
        dispatch_call(state, &call, ctx);
        state.ui.agent.pending_calls.pop_front();
    }
}

/// Whether `call` should pause the batch for approval: gated by the policy and
/// not already pre-approved by the user in this batch.
fn gate_blocks(state: &AppState, call: &ToolCall) -> bool {
    if state.ui.agent.approved_ids.contains(&call.id) {
        return false;
    }
    let mode = state.config.assistant.approval_mode;
    let conversation = state.ui.agent.active();
    tools::needs_confirmation(
        call,
        mode,
        &conversation.allowed_verbs,
        &conversation.allowed_risks,
    )
}

/// The pending calls awaiting a user decision (gated, not yet approved). Drives
/// the approval-card list; empty unless `AwaitingApproval`.
pub fn gated_pending(state: &AppState) -> Vec<ToolCall> {
    if state.ui.agent.phase != AgentPhase::AwaitingApproval {
        return Vec::new();
    }
    state
        .ui
        .agent
        .pending_calls
        .iter()
        .filter(|call| gate_blocks(state, call))
        .cloned()
        .collect()
}

/// In `Plan` mode the doer tools (`run_command`, `save_script`) never execute —
/// each is recorded as a not-run proposal. Read-only perception (`inspect`,
/// `recommend_method`) still runs, since it changes nothing and the model needs
/// to see the workspace to propose a grounded plan.
fn propose_in_plan_mode(state: &mut AppState, ctx: &egui::Context) {
    while let Some(call) = state.ui.agent.pending_calls.pop_front() {
        if matches!(
            call.name.as_str(),
            "run_command" | "save_script" | "cancel_job"
        ) {
            push_tool_call_entry(state, &call);
            let summary = format!(
                "Plan mode: not executed. Proposed {}.",
                describe_call(&call)
            );
            record_result(state, &call, summary, false);
        } else {
            dispatch_call(state, &call, ctx);
        }
    }
    finish_tool_batch(state, ctx);
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
    state.ui.agent.approved_ids.clear();
    let results = std::mem::take(&mut state.ui.agent.collected_results);
    state.ui.agent.history.push(ChatMessage {
        role: Role::Tool,
        content: results,
    });
    spawn_next_turn(state, ctx);
}

/// A pending call by id, cloned, regardless of position in the batch.
fn pending_call(state: &AppState, id: &str) -> Option<ToolCall> {
    state
        .ui
        .agent
        .pending_calls
        .iter()
        .find(|call| call.id == id)
        .cloned()
}

/// Approve the gated call `id`: mark it approved and resume the batch. It runs
/// when the loop reaches it, so execution stays in queue order; if other gated
/// calls remain undecided the loop pauses again on the next one.
pub fn approve_tool_call(state: &mut AppState, id: &str, ctx: &egui::Context) {
    if state.ui.agent.phase != AgentPhase::AwaitingApproval {
        return;
    }
    state.ui.agent.approved_ids.insert(id.to_string());
    state.ui.agent.phase = AgentPhase::ExecutingTools;
    run_tool_batch(state, ctx);
}

/// Approve `id` and auto-allow its command verb for the rest of the conversation,
/// so repeats of the same command stop prompting.
pub fn always_allow_command(state: &mut AppState, id: &str, ctx: &egui::Context) {
    if let Some(call) = pending_call(state, id) {
        state
            .ui
            .agent
            .allowed_verbs
            .insert(tools::call_allow_key(&call));
    }
    approve_tool_call(state, id, ctx);
}

/// Approve `id` and auto-allow every command of its risk level (never
/// `Destructive`) for the rest of the conversation.
pub fn always_allow_risk(state: &mut AppState, id: &str, ctx: &egui::Context) {
    if let Some(call) = pending_call(state, id) {
        let risk = tools::risk_of_call(&call);
        if risk != RiskLevel::Destructive {
            state.ui.agent.allowed_risks.insert(risk);
        }
    }
    approve_tool_call(state, id, ctx);
}

/// Reject the gated call `id`: drop it from the queue and record an `is_error`
/// result so the model learns it was declined, then resume the batch.
pub fn reject_tool_call(state: &mut AppState, id: &str, ctx: &egui::Context) {
    if state.ui.agent.phase != AgentPhase::AwaitingApproval {
        return;
    }
    let Some(position) = state
        .ui
        .agent
        .pending_calls
        .iter()
        .position(|call| call.id == id)
    else {
        return;
    };
    let call = state
        .ui
        .agent
        .pending_calls
        .remove(position)
        .expect("position just found");
    state.ui.agent.approved_ids.remove(id);
    push_tool_call_entry(state, &call);
    record_result(
        state,
        &call,
        "The user declined to run this command.".to_string(),
        true,
    );
    state.ui.agent.phase = AgentPhase::ExecutingTools;
    run_tool_batch(state, ctx);
}
