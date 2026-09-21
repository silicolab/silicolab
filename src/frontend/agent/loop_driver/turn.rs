use super::*;

use std::sync::atomic::Ordering;

use eframe::egui;

use crate::frontend::agent::registry;
use crate::frontend::agent::session::{AgentPhase, PendingTurn, TranscriptEntry};
use crate::frontend::agent::tools;
use crate::frontend::jobs::{AgentTurnEvent, spawn_agent_turn};
use crate::frontend::state::{AppState, LogLevel};
use crate::io::llm::types::{AssistantTurn, ChatMessage, LlmConfig, LlmError, StopReason};

/// Handle a user message. Idle → start a turn immediately; busy or paused on
/// approval → queue it (type-ahead) so `pump_queue` sends it once the agent is
/// free, instead of dropping it.
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
    if state.ui.agent.qm_diagnostic_only {
        if let Some(job) = state.jobs.agent.take() {
            job.cancel.store(true, Ordering::Relaxed);
        }
        state
            .ui
            .agent
            .recover_interrupted("continuing the conversation");
        state.ui.agent.streaming_text.clear();
        state.ui.agent.queued.clear();
        state.ui.agent.phase = AgentPhase::Idle;
        state.ui.agent.qm_diagnostic_only = false;
    }
    // Busy, paused on approval, or a follow-up is already queued (e.g. a finished
    // job's wake): enqueue so FIFO order holds, then pump in case we are idle.
    if state.ui.agent.is_busy()
        || state.ui.agent.pending_approval().is_some()
        || !state.ui.agent.queued.is_empty()
    {
        state
            .ui
            .agent
            .queued
            .push_back(PendingTurn::UserMessage(text.to_string()));
        pump_queue(state, ctx);
        return;
    }
    begin_user_turn(state, text, ctx);
}

/// Record a user message into history + transcript and spawn its first model
/// turn. Shared by the immediate send and the queue pump; surfaces a missing
/// key / bad provider without mutating history.
fn begin_user_turn(state: &mut AppState, text: &str, ctx: &egui::Context) {
    // Surface a missing key / bad provider up front, before recording the turn.
    if let Err(reason) =
        registry::build_provider(&state.config.assistant, &state.ui.agent.selection)
    {
        notice(state, &reason);
        return;
    }

    // Keep the replayed history valid if a prior exchange was interrupted.
    state
        .ui
        .agent
        .recover_interrupted("continuing the conversation");
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
        state
            .ui
            .agent
            .recover_interrupted("iteration limit reached");
        state.ui.agent.phase = AgentPhase::Done;
        ctx.request_repaint();
        return;
    }

    let selection = state.ui.agent.selection.clone();
    let (assistant_config, tools) = request_config_and_tools(state);
    let provider = match registry::build_provider(&assistant_config, &selection) {
        Ok(provider) => provider,
        Err(reason) => {
            state
                .ui
                .agent
                .recover_interrupted(&format!("model request could not start: {reason}"));
            notice(state, &reason);
            state.ui.agent.phase = AgentPhase::Idle;
            ctx.request_repaint();
            return;
        }
    };

    if compact_history(&mut state.ui.agent.history) {
        notice(state, "Trimmed older conversation to stay within context.");
    }

    let stream = registry::provider_spec(&selection.provider)
        .unwrap_or(&registry::PROVIDERS[0])
        .caps_for(&selection.model)
        .supports_streaming;
    let project_root = state
        .workspace
        .project()
        .map(|project| project.root.clone());
    state.ui.agent.ensure_skills_loaded(project_root.clone());
    let mut cfg = LlmConfig {
        model: selection.model,
        effort: state.config.assistant.effort,
        max_output_tokens: MAX_OUTPUT_TOKENS,
        stream,
        system: system_prompt(&state.ui.agent.skills),
        working_dir: project_root,
    };
    if state.ui.agent.qm_diagnostic_only {
        cfg.system.push_str("\nQM diagnosis only. Explain the problem, available evidence and recommended next steps, then wait for a new user instruction. Do not rerun, repair files or accept an unconverged result. Only inspect, list_jobs and recommend_method are allowed.");
    }
    if let Some(goal) = state
        .ui
        .agent
        .transcript
        .iter()
        .rev()
        .find_map(|entry| match entry {
            TranscriptEntry::User(text) => Some(text),
            _ => None,
        })
    {
        cfg.system.push_str(
            "\nCurrent user instruction (bounded; complete message remains in conversation):\n",
        );
        cfg.system.extend(goal.chars().take(1000));
        if goal.chars().count() > 1000 {
            cfg.system
                .push_str("\nUser instruction truncated by context budget; inspect view=intent retrieves the full original with detail_offset.");
        }
    }
    cfg.system
        .push_str("\nProject working context (user authority, not model inference):\n");
    cfg.system.push_str(
        &state.tasks.runs.records.context(
            state.active_task_run,
            state
                .active_task_run
                .and_then(|id| state.tasks.task_run(id))
                .map(|t| t.run_uuid.as_str()),
            state.ui.agent.active_conversation.raw(),
            2400,
        ),
    );
    let history = state.ui.agent.history.clone();

    state.ui.agent.iterations += 1;
    state.ui.agent.phase = AgentPhase::AwaitingModel;
    state.jobs.agent = Some(spawn_agent_turn(provider, cfg, tools, history));
    ctx.request_repaint_after(AGENT_POLL);
}

fn request_config_and_tools(
    state: &AppState,
) -> (
    crate::backend::config::AssistantConfig,
    Vec<crate::io::llm::types::ToolDef>,
) {
    let mut assistant_config = state.config.assistant.clone();
    assistant_config.external_agent_access = if state.ui.agent.qm_diagnostic_only {
        crate::backend::config::ExternalAgentAccess::Controlled
    } else {
        state.ui.agent.external_access
    };
    let mut tools = tools::tool_defs();
    if state.ui.agent.qm_diagnostic_only {
        tools.retain(|tool| tools::diagnostic_tool_allowed(&tool.name));
    }
    (assistant_config, tools)
}

/// Drain the in-flight agent turn (called from `poll_jobs`). Esc cancels.
pub fn poll_agent_turn(state: &mut AppState, ctx: &egui::Context) {
    let Some(job) = state.jobs.agent.take() else {
        return;
    };

    if ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
        job.cancel.store(true, Ordering::Relaxed);
        state.ui.agent.recover_interrupted("turn cancelled");
        // Drop the handle; a late worker result lands on a closed channel.
        state.ui.agent.streaming_text.clear();
        discard_queued(state, "the turn was cancelled");
        state.ui.agent.current_backlog = None;
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
                state.ui.agent.recover_interrupted("worker disconnected");
                state.ui.agent.streaming_text.clear();
                discard_queued(state, "the turn ended early");
                state.ui.agent.current_backlog = None;
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
            state
                .ui
                .agent
                .recover_interrupted(&format!("model request ended: {message}"));
            notice(state, &format!("Assistant error: {message}"));
            state.log_agent(None, LogLevel::Error, format!("assistant error: {message}"));
            let cancelled = matches!(error, LlmError::Cancelled);
            state.ui.agent.phase = if cancelled {
                AgentPhase::Idle
            } else {
                AgentPhase::Done
            };
            discard_queued(
                state,
                if cancelled {
                    "the turn was cancelled"
                } else {
                    "the turn failed"
                },
            );
            state.ui.agent.current_backlog = None;
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
        state.log_agent(
            None,
            LogLevel::Info,
            format!("assistant> {}", turn.text.trim()),
        );
    }

    // Record the assistant turn for replay via the provider's encoder (with a
    // provider-agnostic fallback when one can't be built, e.g. in tests).
    let assistant_message =
        match registry::build_provider(&state.config.assistant, &state.ui.agent.selection) {
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
        // Send a follow-up the user queued while this turn was running.
        pump_queue(state, ctx);
    }
}

/// Background jobs running for the active conversation, used to tell whether a
/// backlog turn handed its work off to one.
fn conversation_job_count(state: &AppState) -> usize {
    let active = state.ui.agent.active_conversation;
    let compute = state
        .jobs
        .agent_jobs
        .iter()
        .filter(|job| job.conversation == active)
        .count();
    compute
        + state
            .jobs
            .agent_online_structures
            .iter()
            .filter(|job| job.conversation == active)
            .count()
}

/// Dispatch the next queued follow-up when the agent is at rest. One at a time:
/// beginning a turn flips `phase` to `AwaitingModel`, so a single call starts at
/// most one. Called after a turn completes, a background job finishes, or the
/// user switches back to a conversation with a pending follow-up.
pub fn pump_queue(state: &mut AppState, ctx: &egui::Context) {
    if state.ui.agent.is_busy()
        || state.ui.agent.pending_approval().is_some()
        || state.jobs.agent.is_some()
    {
        return;
    }
    // Idle past the guard above ⇒ a running backlog turn (if any) has finished.
    let running_jobs = conversation_job_count(state);
    state.ui.agent.resolve_current_backlog(running_jobs);
    let Some(item) = state.ui.agent.queued.pop_front() else {
        return;
    };
    match item {
        PendingTurn::QmDone {
            job_id,
            summary,
            result,
            execution_failed,
        } => {
            let issue = execution_failed || result.as_ref().is_none_or(|r| r.needs_diagnosis());
            if issue {
                if state.ui.agent.qm_diagnostic_only
                    && state.config.assistant.auto_diagnose_qm_issues
                    && state.config.assistant.enabled
                {
                    state.ui.agent.iterations = 0;
                    spawn_next_turn(state, ctx);
                }
            } else if !state.ui.agent.qm_diagnostic_only {
                let summary = format!(
                    "{summary}\n{}",
                    result.as_ref().map(|r| r.summary()).unwrap_or_default()
                );
                begin_job_followup(state, &format!("QM {job_id}"), &summary, false, ctx);
            }
        }
        PendingTurn::UserMessage(text) => {
            let baseline = conversation_job_count(state);
            state.ui.agent.note_backlog_start(text.clone(), baseline);
            begin_user_turn(state, &text, ctx);
        }
        PendingTurn::JobDone {
            label,
            summary,
            is_error,
        } => begin_job_followup(state, &label, &summary, is_error, ctx),
    }
}

/// Wake the model after a background job finished: record a synthetic user turn
/// describing the result, then spawn the model turn so it continues the workflow
/// (e.g. optimize → frequencies). Surfaces a provider error without mutating
/// history, mirroring [`begin_user_turn`].
fn begin_job_followup(
    state: &mut AppState,
    label: &str,
    summary: &str,
    is_error: bool,
    ctx: &egui::Context,
) {
    if state.ui.agent.qm_diagnostic_only {
        return;
    }
    if let Err(reason) =
        registry::build_provider(&state.config.assistant, &state.ui.agent.selection)
    {
        notice(state, &reason);
        return;
    }
    state
        .ui
        .agent
        .recover_interrupted("continuing the conversation");
    let text = job_followup_text(label, summary, is_error);
    state.ui.agent.history.push(ChatMessage::user_text(&text));
    state.ui.agent.iterations = 0;
    spawn_next_turn(state, ctx);
}

/// The synthetic user message handed to the model when a background job finishes.
fn job_followup_text(label: &str, summary: &str, is_error: bool) -> String {
    let summary = tools::clamp_result(summary);
    let verb = if is_error { "failed" } else { "finished" };
    format!(
        "[Background job] The `{label}` task {verb}. Result:\n{summary}\n\n\
         Continue the task: report this to the user concisely, then take the next step \
         if there is one — otherwise stop."
    )
}

/// Drop queued type-ahead *messages* when a turn is cancelled or fails — they
/// were predicated on the current work completing. A `JobDone` is kept: a
/// background computation that already finished (and applied its result to the
/// workspace) still deserves to be reported, so it survives to fire later.
fn discard_queued(state: &mut AppState, why: &str) {
    let before = state.ui.agent.queued.len();
    state.ui.agent.queued.retain(|item| {
        matches!(
            item,
            PendingTurn::JobDone { .. } | PendingTurn::QmDone { .. }
        )
    });
    let dropped = before - state.ui.agent.queued.len();
    if dropped > 0 {
        notice(
            state,
            &format!("Discarded {dropped} queued message(s) — {why}."),
        );
    }
}

/// Drop the queued follow-up at `index` (composer ✕ button); no-op out of range.
pub fn remove_queued_agent_input(state: &mut AppState, index: usize) {
    state.ui.agent.remove_queued(index);
}

/// Cancel the assistant: stop the in-flight model turn and pending tool batch,
/// cancel the active conversation's background jobs (so a late completion can't
/// re-wake the model after the user stopped), and drop queued type-ahead
/// messages. Individual jobs can still be cancelled from the running-jobs strip.
pub fn cancel_agent(state: &mut AppState, ctx: &egui::Context) {
    if let Some(job) = state.jobs.agent.take() {
        job.cancel.store(true, Ordering::Relaxed);
    }
    let active = state.ui.agent.active_conversation;
    let cancelled_jobs = cancel_conversation_jobs(state, active);
    state.ui.agent.recover_interrupted("turn cancelled");
    state.ui.agent.streaming_text.clear();
    discard_queued(state, "the turn was cancelled");
    state.ui.agent.current_backlog = None;
    if state.ui.agent.phase != AgentPhase::Idle || cancelled_jobs > 0 {
        notice(state, "Cancelled.");
    }
    state.ui.agent.phase = AgentPhase::Idle;
    ctx.request_repaint();
}

#[cfg(test)]
mod qm_request_tests {
    use super::*;
    use crate::backend::config::ExternalAgentAccess;

    #[test]
    fn diagnostic_request_forces_controlled_access_and_only_three_tools() {
        let mut state = AppState::scratch(Default::default(), Vec::new());
        state.ui.agent.external_access = ExternalAgentAccess::Unrestricted;
        state.config.assistant.external_agent_access = ExternalAgentAccess::Unrestricted;
        state.ui.agent.qm_diagnostic_only = true;
        let (config, tools) = request_config_and_tools(&state);
        assert_eq!(
            config.external_agent_access,
            ExternalAgentAccess::Controlled
        );
        let mut names: Vec<_> = tools.iter().map(|tool| tool.name.as_str()).collect();
        names.sort();
        assert_eq!(names, ["inspect", "list_jobs", "recommend_method"]);
        assert_eq!(
            state.ui.agent.external_access,
            ExternalAgentAccess::Unrestricted
        );
        assert_eq!(
            state.config.assistant.external_agent_access,
            ExternalAgentAccess::Unrestricted
        );
        state.ui.agent.qm_diagnostic_only = false;
        let (config, tools) = request_config_and_tools(&state);
        assert_eq!(
            config.external_agent_access,
            ExternalAgentAccess::Unrestricted
        );
        assert!(tools.iter().any(|tool| tool.name == "run_command"));
    }
}
