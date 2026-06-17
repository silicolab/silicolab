use super::*;

use eframe::egui;

use crate::backend::entries::EntryOrigin;
use crate::frontend::agent::session::AgentPhase;
use crate::frontend::jobs::{
    AgentHeavyJob, EngineWorkerMessage, QmWorkerMessage, RunningEngineJob, RunningQmJob,
    spawn_gromacs_pipeline_job, spawn_qm_job,
};
use crate::frontend::state::AppState;
use crate::io::llm::types::ToolCall;
use crate::io::structure_io::default_structure_save_path;

/// Heavy compute commands the agent runs off the UI thread.
#[derive(Clone, Copy)]
pub enum HeavyKind {
    Md,
    Qm,
}

/// Classify a tool call as a heavy off-thread command (`md run|simulate`, `qm
/// energy|optimize|freq`), else `None` (runs inline).
pub fn heavy_kind_of(call: &ToolCall) -> Option<HeavyKind> {
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

/// Build the request and spawn a heavy job into the agent-owned slot. On a build
/// error, records an `is_error` result and returns `false` so the batch
/// continues; on success returns `true` to pause in `AwaitingHeavyJob`.
pub fn spawn_heavy(
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
