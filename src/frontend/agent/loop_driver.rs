//! Turn orchestration for the agent loop. Driven from the UI thread: the
//! dispatcher's action handlers start/cancel/approve, and
//! [`poll_agent_turn`] (called from `poll_jobs`) drains each finished network
//! turn, runs its tool calls inline, and spawns the next turn — ping-ponging
//! between the worker thread (network) and the UI thread (state-mutating tools)
//! using the existing `JobManager` + channel-poll pattern. No new concurrency
//! primitive.

use std::sync::atomic::Ordering;
use std::time::Duration;

use eframe::egui;

use crate::backend::config::save_config;
use crate::backend::entries::EntryOrigin;
use crate::frontend::agent::registry;
use crate::frontend::agent::session::{
    AgentPhase, AssistantConversationId, ModelFetchStatus, TranscriptEntry,
};
use crate::frontend::agent::tools;
use crate::frontend::jobs::{
    AgentHeavyJob, AgentTurnEvent, EngineWorkerMessage, QmWorkerMessage, RunningEngineJob,
    RunningQmJob, spawn_agent_turn, spawn_gromacs_pipeline_job, spawn_model_fetch, spawn_qm_job,
};
use crate::frontend::state::AppState;
use crate::io::llm::types::{
    AssistantTurn, ChatMessage, ContentBlock, Effort, LlmConfig, LlmError, ReasoningBlob, Role,
    StopReason, ToolCall,
};
use crate::io::structure_io::default_structure_save_path;

/// Hard cap on model turns per user message; on truncation we stop and notice.
const MAX_ITERATIONS: usize = 25;
/// Compact the replayed history once it exceeds this many messages. Generous —
/// even at 1M context a runaway agent session could otherwise grow unbounded.
const MAX_HISTORY_MESSAGES: usize = 240;
/// Target message count to trim down to when compacting.
const TARGET_HISTORY_MESSAGES: usize = 140;
/// Per-turn output budget. Tool-call turns are short; 16k is ample.
const MAX_OUTPUT_TOKENS: u32 = 16_000;
/// How often to re-poll the in-flight turn.
const AGENT_POLL: Duration = Duration::from_millis(120);
/// Display clamp for a transcript line.
const DISPLAY_CLAMP: usize = 600;

const PERSONA: &str = "\
You are SilicoLab's built-in assistant. SilicoLab is a desktop app for molecular \
and materials modeling. You drive it by emitting tool calls — you do not answer in \
the abstract, you act.

Tools:
- `run_command` runs one SilicoLab `.sls` console command (the same line a user types \
in the console). One command per call.
- `inspect` returns a read-only view of the current workspace.
- `save_script` saves a reusable `.sls` workflow of commands to the project.

Working style:
- Call `inspect` before acting when you are unsure of the current state — never guess \
what is loaded.
- Take one concrete step at a time and keep your prose short; the user sees both your \
text and the commands you run.
- Destructive or expensive commands (delete, save, md, qm, running a script) are gated: \
when you call one, the user is asked to approve it first. Call them anyway when they are \
the right step; explain why briefly.
- If a command fails, read the error in the tool result and recover or ask the user.
- When the task is done, stop and say so in one line.

The full command catalog follows.";

/// The static, cacheable system prompt: persona + the `.sls` command catalog.
/// Holds no volatile per-turn state (that flows through `inspect`), so it caches.
fn system_prompt() -> String {
    format!(
        "{PERSONA}\n\n{}",
        crate::frontend::console::command_catalog()
    )
}

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
fn spawn_next_turn(state: &mut AppState, ctx: &egui::Context) {
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
fn handle_turn_result(
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

/// Run queued tool calls in order until the batch empties or one hits a
/// confirmation gate. A failing call still yields an `is_error` result; the
/// batch is not aborted (the model recovers from the error).
fn run_tool_batch(state: &mut AppState, ctx: &egui::Context) {
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
        if dispatch_call(state, &call, ctx) {
            // A heavy job was spawned; the batch pauses until it completes.
            return;
        }
        state.ui.agent.pending_calls.pop_front();
    }
}

/// Heavy compute commands the agent runs off the UI thread.
#[derive(Clone, Copy)]
enum HeavyKind {
    Md,
    Qm,
}

/// Classify a tool call as a heavy off-thread command (`md run|simulate`, `qm
/// energy|optimize|freq`), else `None` (runs inline).
fn heavy_kind_of(call: &ToolCall) -> Option<HeavyKind> {
    if call.name != "run_command" {
        return None;
    }
    let command = call.input.get("command").and_then(|value| value.as_str())?;
    let mut words = command.split_whitespace();
    match words.next()? {
        "qm" => matches!(
            words.next(),
            Some("energy" | "sp" | "single-point" | "optimize" | "opt" | "freq" | "frequencies")
        )
        .then_some(HeavyKind::Qm),
        "md" => matches!(words.next(), Some("run" | "simulate")).then_some(HeavyKind::Md),
        _ => None,
    }
}

/// Execute a call inline, or spawn it as a heavy off-thread job. Returns `true`
/// when a heavy job was spawned (the batch pauses in `AwaitingHeavyJob`).
fn dispatch_call(state: &mut AppState, call: &ToolCall, ctx: &egui::Context) -> bool {
    push_tool_call_entry(state, call);
    if let Some(kind) = heavy_kind_of(call) {
        return spawn_heavy(state, call, kind, ctx);
    }
    let outcome = tools::execute_tool(state, call);
    record_result(state, call, outcome.content, outcome.is_error);
    false
}

fn push_tool_call_entry(state: &mut AppState, call: &ToolCall) {
    state.ui.agent.transcript.push(TranscriptEntry::Tool {
        summary: describe_call(call),
        result: None,
        is_error: false,
    });
}

fn record_result(state: &mut AppState, call: &ToolCall, content: String, is_error: bool) {
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

fn fill_pending_tool_entry(state: &mut AppState, content: &str, is_error: bool) {
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

/// Build the request and spawn a heavy job into the agent-owned slot. On a build
/// error, records an `is_error` result and returns `false` so the batch
/// continues; on success returns `true` to pause in `AwaitingHeavyJob`.
fn spawn_heavy(
    state: &mut AppState,
    call: &ToolCall,
    kind: HeavyKind,
    ctx: &egui::Context,
) -> bool {
    let command = call
        .input
        .get("command")
        .and_then(|value| value.as_str())
        .unwrap_or_default()
        .to_string();
    let words: Vec<String> = command.split_whitespace().map(str::to_string).collect();
    let args = &words[1..]; // drop the `md` / `qm` verb

    let spawned: Result<AgentHeavyJob, String> = match kind {
        HeavyKind::Qm => crate::frontend::qm_commands::build_agent_qm_request(state, args)
            .map(|request| {
                AgentHeavyJob::Qm(spawn_qm_job(crate::engines::qm::QmJob::Molecular(request)))
            })
            .map_err(|error| error.to_string()),
        HeavyKind::Md => crate::frontend::md_commands::build_agent_md_request(state, args)
            .map(|request| AgentHeavyJob::Engine(spawn_gromacs_pipeline_job(request)))
            .map_err(|error| error.to_string()),
    };

    match spawned {
        Ok(job) => {
            state.jobs.agent_heavy = Some(job);
            state.ui.agent.phase = AgentPhase::AwaitingHeavyJob;
            notice(
                state,
                &format!("Running `{command}` off-thread; press Esc to cancel."),
            );
            ctx.request_repaint_after(AGENT_POLL);
            true
        }
        Err(reason) => {
            record_result(
                state,
                call,
                format!("could not start `{command}`: {reason}"),
                true,
            );
            false
        }
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
    if dispatch_call(state, &front, ctx) {
        return; // spawned a heavy job; resumes when it completes
    }
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

/// Drain the in-flight heavy job (called from `poll_jobs`). Esc cancels it and
/// the agent turn.
pub fn poll_agent_heavy(state: &mut AppState, ctx: &egui::Context) {
    let Some(job) = state.jobs.agent_heavy.take() else {
        return;
    };
    if ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
        job.cancel();
        fill_pending_tool_entry(state, "Cancelled.", true);
        state.ui.agent.pending_calls.clear();
        state.ui.agent.collected_results.clear();
        notice(state, "Cancelled.");
        state.ui.agent.phase = AgentPhase::Idle;
        ctx.request_repaint();
        return;
    }
    match job {
        AgentHeavyJob::Qm(running) => poll_heavy_qm(state, running, ctx),
        AgentHeavyJob::Engine(running) => poll_heavy_engine(state, running, ctx),
    }
}

fn poll_heavy_qm(state: &mut AppState, running: RunningQmJob, ctx: &egui::Context) {
    let mut completion: Option<(String, bool)> = None;
    while let Ok(message) = running.receiver.try_recv() {
        match message {
            QmWorkerMessage::Progress { stage } => {
                state.set_message(format!("QM: {stage}; press Esc to stop"));
            }
            QmWorkerMessage::Finished(outcome) => {
                let outcome = *outcome;
                if let Some(optimized) = outcome.optimized_structure {
                    let save_path = default_structure_save_path(&optimized, None);
                    let entry_id = state.entries.add_entry(optimized, None, save_path);
                    state.show_entry(entry_id);
                    state
                        .entries
                        .set_entry_origin(entry_id, EntryOrigin::QmRun { output: None });
                }
                state.set_message(outcome.summary.clone());
                completion = Some((outcome.summary, false));
            }
            QmWorkerMessage::Failed(error) => {
                completion = Some((format!("QM calculation failed: {error}"), true));
            }
        }
    }
    match completion {
        Some((summary, is_error)) => heavy_complete(state, summary, is_error, ctx),
        None => {
            state.jobs.agent_heavy = Some(AgentHeavyJob::Qm(running));
            ctx.request_repaint_after(AGENT_POLL);
        }
    }
}

fn poll_heavy_engine(state: &mut AppState, mut running: RunningEngineJob, ctx: &egui::Context) {
    let mut completion: Option<(String, bool)> = None;
    while let Ok(message) = running.receiver.try_recv() {
        match message {
            EngineWorkerMessage::Stage(stage) => {
                state.set_message(format!("{}: {stage}", running.engine));
                running.latest_stage = Some(stage);
            }
            EngineWorkerMessage::Log(line) => running.append_log(line),
            EngineWorkerMessage::Finished(success) => {
                let success = *success;
                let summary = success.summary.clone();
                let trajectory = success.trajectory.clone();
                let save_path = default_structure_save_path(&success.structure, None);
                let entry_id = state.entries.add_entry(success.structure, None, save_path);
                state.show_entry(entry_id);
                let project_root = state
                    .workspace
                    .project()
                    .map(|project| project.root.clone());
                let origin =
                    crate::frontend::dispatcher::md_run_origin(trajectory, project_root.as_deref());
                state.entries.set_entry_origin(entry_id, origin);
                state.set_message(summary.clone());
                completion = Some((summary, false));
            }
            EngineWorkerMessage::Failed(error) => {
                completion = Some((format!("molecular dynamics failed: {error}"), true));
            }
        }
    }
    match completion {
        Some((summary, is_error)) => heavy_complete(state, summary, is_error, ctx),
        None => {
            state.jobs.agent_heavy = Some(AgentHeavyJob::Engine(running));
            ctx.request_repaint_after(AGENT_POLL);
        }
    }
}

/// Record the heavy job's result against the front (awaiting) call, then resume
/// the tool batch.
fn heavy_complete(state: &mut AppState, summary: String, is_error: bool, ctx: &egui::Context) {
    if let Some(call) = state.ui.agent.pending_calls.front().cloned() {
        record_result(state, &call, summary, is_error);
        state.ui.agent.pending_calls.pop_front();
    }
    state.ui.agent.phase = AgentPhase::ExecutingTools;
    run_tool_batch(state, ctx);
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

pub fn new_assistant_conversation(state: &mut AppState) {
    state.ui.agent.start_new_conversation();
}

pub fn switch_assistant_conversation(state: &mut AppState, id: AssistantConversationId) {
    state.ui.agent.switch_conversation(id);
}

pub fn rename_assistant_conversation(
    state: &mut AppState,
    id: AssistantConversationId,
    title: &str,
) {
    state.ui.agent.rename_conversation(id, title);
}

pub fn delete_assistant_conversation(state: &mut AppState, id: AssistantConversationId) {
    state.ui.agent.delete_conversation(id);
}

/// Switch the active provider + model and persist. Strips prior-provider
/// reasoning blobs from the replayed history (ignored-but-billed, or
/// shape-incompatible, on a different provider/model) and clears a stale base-URL
/// override when the provider changes.
pub fn switch_provider_model(state: &mut AppState, provider: &str, model: &str) {
    if state.config.assistant.provider != provider {
        // The base-URL override is provider-specific; drop it on a provider change.
        state.config.assistant.base_url = None;
    }
    state.config.assistant.provider = provider.to_string();
    state.config.assistant.model = model.to_string();
    for conversation in &mut state.ui.agent.conversations {
        strip_reasoning(&mut conversation.history);
    }
    // The fetch status is global; clear it so a prior provider's spinner or
    // error note doesn't bleed onto the newly selected one. The fetched model
    // ids are keyed per provider, so they survive the switch.
    state.ui.agent.model_fetch = ModelFetchStatus::Idle;
    persist(state);
    refresh_key_status(state);
}

/// Enable or disable the assistant and persist.
pub fn set_assistant_enabled(state: &mut AppState, enabled: bool) {
    state.config.assistant.enabled = enabled;
    persist(state);
}

/// Set the reasoning effort and persist.
pub fn set_assistant_effort(state: &mut AppState, effort: Effort) {
    state.config.assistant.effort = effort;
    persist(state);
}

/// Set (or clear, when blank) the base-URL override for an OpenAI-compatible
/// provider and persist.
pub fn set_assistant_base_url(state: &mut AppState, base_url: &str) {
    let trimmed = base_url.trim();
    state.config.assistant.base_url = if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    };
    state.ui.agent.model_fetch = ModelFetchStatus::Idle;
    persist(state);
}

/// Store the active provider's API key in the app key store (never in config).
pub fn set_assistant_api_key(state: &mut AppState, key: &str) {
    let provider = registry::active_provider(&state.config.assistant);
    match crate::backend::secrets::set_stored_key(provider.id, key.trim()) {
        Ok(()) => state.set_message(format!("Saved the API key for {}.", provider.label)),
        Err(error) => state.set_message(format!("Could not save the API key: {error}")),
    }
    refresh_key_status(state);
}

/// Remove a provider's stored key from the app key store. Takes the provider id
/// rather than assuming the active one, so it backs both the active "Clear"
/// button and the per-row Remove in the keys overview.
pub fn clear_stored_key(state: &mut AppState, provider_id: &str) {
    let label = registry::provider_spec(provider_id)
        .map(|spec| spec.label)
        .unwrap_or(provider_id);
    match crate::backend::secrets::clear_stored_key(provider_id) {
        Ok(()) => state.set_message(format!("Removed the stored API key for {label}.")),
        Err(error) => state.set_message(format!("Could not remove the API key: {error}")),
    }
    refresh_key_status(state);
}

/// Recompute whether a key is available for the active provider (reads env + the
/// key store) and cache it on the session, so the render path never hits the
/// key store. Called on provider/key changes and once at startup.
pub fn refresh_key_status(state: &mut AppState) {
    let available =
        registry::api_key_for(registry::active_provider(&state.config.assistant)).is_some();
    state.ui.agent.key_available = Some(available);
}

/// Kick off a live `/models` fetch for the active provider. Resolves the key the
/// same way a turn does (env → key store); with no key it records an error
/// instead of spawning. The result is drained in [`poll_model_fetch`]. A fetch
/// already in flight is left to finish.
pub fn fetch_models(state: &mut AppState, ctx: &egui::Context) {
    if state.jobs.model_fetch.is_some() {
        return;
    }
    let spec = registry::active_provider(&state.config.assistant);
    let Some(key) = registry::api_key_for(spec) else {
        state.ui.agent.model_fetch = ModelFetchStatus::Error(format!(
            "Add a key for {} first to list its models.",
            spec.label
        ));
        ctx.request_repaint();
        return;
    };
    let base_url = registry::effective_base_url(&state.config.assistant, spec);
    state.jobs.model_fetch = Some(spawn_model_fetch(
        spec.id.to_string(),
        spec.kind,
        base_url,
        key,
    ));
    state.ui.agent.model_fetch = ModelFetchStatus::Fetching;
    ctx.request_repaint_after(AGENT_POLL);
}

/// Drain the in-flight model fetch (called from `poll_jobs`). On success the ids
/// are cached under their provider id and the status returns to Idle; on failure
/// the status carries a short reason. The cached list is keyed by provider, so a
/// result arriving after the user switched providers is still stored correctly.
pub fn poll_model_fetch(state: &mut AppState, ctx: &egui::Context) {
    let Some(job) = state.jobs.model_fetch.take() else {
        return;
    };
    match job.receiver.try_recv() {
        Ok(Ok(ids)) => {
            let count = ids.len();
            state.ui.agent.fetched_models.insert(job.provider_id, ids);
            state.ui.agent.model_fetch = ModelFetchStatus::Idle;
            state.set_message(format!("Listed {count} models from the provider."));
            ctx.request_repaint();
        }
        Ok(Err(error)) => {
            state.ui.agent.model_fetch = ModelFetchStatus::Error(error);
            ctx.request_repaint();
        }
        Err(std::sync::mpsc::TryRecvError::Empty) => {
            state.jobs.model_fetch = Some(job);
            ctx.request_repaint_after(AGENT_POLL);
        }
        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
            state.ui.agent.model_fetch =
                ModelFetchStatus::Error("model fetch worker stopped".to_string());
            ctx.request_repaint();
        }
    }
}

// --- helpers -------------------------------------------------------------- //

fn notice(state: &mut AppState, message: &str) {
    state
        .ui
        .agent
        .transcript
        .push(TranscriptEntry::Notice(message.to_string()));
}

fn persist(state: &mut AppState) {
    if let Err(error) = save_config(&state.config) {
        state.set_message(format!("Could not save assistant settings: {error}"));
    }
}

/// A one-line description of a tool call for the transcript.
fn describe_call(call: &ToolCall) -> String {
    match call.name.as_str() {
        "run_command" => call
            .input
            .get("command")
            .and_then(|value| value.as_str())
            .map(|command| command.to_string())
            .unwrap_or_else(|| "run_command".to_string()),
        "inspect" => "inspect".to_string(),
        "save_script" => call
            .input
            .get("filename")
            .and_then(|value| value.as_str())
            .map(|filename| format!("save_script {filename}"))
            .unwrap_or_else(|| "save_script".to_string()),
        other => other.to_string(),
    }
}

fn clamp_display(text: &str) -> String {
    let text = text.trim();
    if text.chars().count() <= DISPLAY_CLAMP {
        return text.to_string();
    }
    let kept: String = text.chars().take(DISPLAY_CLAMP).collect();
    format!("{kept}…")
}

/// Provider-agnostic assistant encoding used when a provider can't be built
/// (no key, tests). Mirrors the adapters: reasoning first, then text, then
/// tool-use blocks.
fn fallback_encode(turn: &AssistantTurn) -> ChatMessage {
    let mut content: Vec<ContentBlock> = Vec::new();
    if !matches!(turn.reasoning, ReasoningBlob::None) {
        content.push(ContentBlock::OpaqueReasoning(turn.reasoning.clone()));
    }
    if !turn.text.is_empty() {
        content.push(ContentBlock::Text(turn.text.clone()));
    }
    for call in &turn.tool_calls {
        content.push(ContentBlock::ToolUse {
            id: call.id.clone(),
            name: call.name.clone(),
            input: call.input.clone(),
        });
    }
    ChatMessage {
        role: Role::Assistant,
        content,
    }
}

/// Trim the oldest whole exchanges when the history grows past
/// [`MAX_HISTORY_MESSAGES`], cutting at a genuine user-turn boundary so the kept
/// history stays valid (starts at a user turn; tool_use/tool_result pairs intact).
/// Returns whether anything was trimmed.
fn compact_history(history: &mut Vec<ChatMessage>) -> bool {
    if history.len() <= MAX_HISTORY_MESSAGES {
        return false;
    }
    // Conversation boundaries are genuine user turns (tool results are Role::Tool).
    let user_turns: Vec<usize> = history
        .iter()
        .enumerate()
        .filter(|(_, message)| message.role == Role::User)
        .map(|(index, _)| index)
        .collect();
    // The earliest boundary whose tail fits the target; else the most recent
    // boundary. Never the very first (index 0) — that would trim nothing.
    let boundary = user_turns
        .iter()
        .copied()
        .find(|&start| history.len() - start <= TARGET_HISTORY_MESSAGES)
        .or_else(|| user_turns.last().copied())
        .unwrap_or(0);
    if boundary == 0 {
        return false;
    }
    history.drain(0..boundary);
    true
}

/// Drop opaque reasoning blocks from every message — done on a provider/model
/// switch, since a different backend ignores foreign reasoning (and is still
/// billed for it) or rejects its shape.
fn strip_reasoning(history: &mut [ChatMessage]) {
    for message in history.iter_mut() {
        message
            .content
            .retain(|block| !matches!(block, ContentBlock::OpaqueReasoning(_)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::llm::types::{StopReason, ToolCall, Usage};
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
}
