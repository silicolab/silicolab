use super::*;

use eframe::egui;

use crate::backend::entries::EntryOrigin;
use crate::frontend::agent::session::{AssistantConversationId, PendingTurn, TranscriptEntry};
use crate::frontend::jobs::{
    AgentHeavyJob, DockingWorkerMessage, EngineWorkerMessage, QmWorkerMessage, RunningDockingJob,
    RunningEngineJob, RunningQmJob, TrackedAgentJob, spawn_docking_job, spawn_gromacs_pipeline_job,
    spawn_qm_job,
};
use crate::frontend::state::AppState;
use crate::io::llm::types::ToolCall;
use crate::io::structure_io::default_structure_save_path;

/// Most heavy jobs the agent may have running at once. Serialized to one by
/// default to bound memory — a ~21-atom QM run can already cost ~19 GB — so a
/// second launch is refused with a "wait" result rather than risking an OOM.
const MAX_AGENT_HEAVY: usize = 1;

/// Heavy compute commands the agent runs off the UI thread.
#[derive(Clone, Copy)]
pub enum HeavyKind {
    Md,
    Qm,
    Dock,
}

/// Classify a tool call as a heavy off-thread command (`md run|simulate`, `qm
/// energy|optimize|freq|ts`, `dock`), else `None` (runs inline). `score` is a cheap
/// single-point evaluation, so it stays inline.
pub fn heavy_kind_of(call: &ToolCall) -> Option<HeavyKind> {
    if call.name != "run_command" {
        return None;
    }
    let command = call.input.get("command").and_then(|value| value.as_str())?;
    let mut words = command.split_whitespace();
    match words.next()? {
        "qm" => matches!(
            words.next(),
            Some(
                "energy"
                    | "sp"
                    | "single-point"
                    | "optimize"
                    | "opt"
                    | "freq"
                    | "frequencies"
                    | "ts"
                    | "saddle"
                    | "transition-state"
            )
        )
        .then_some(HeavyKind::Qm),
        "md" => matches!(words.next(), Some("run" | "simulate")).then_some(HeavyKind::Md),
        "dock" => Some(HeavyKind::Dock),
        _ => None,
    }
}

/// A short human label for a heavy command, e.g. `qm optimize`, `md run`, `dock`.
fn heavy_label(kind: HeavyKind, command: &str) -> String {
    let sub = command.split_whitespace().nth(1).unwrap_or("");
    match kind {
        HeavyKind::Qm => format!("qm {sub}").trim_end().to_string(),
        HeavyKind::Md => format!("md {sub}").trim_end().to_string(),
        HeavyKind::Dock => "dock".to_string(),
    }
}

/// Launch a heavy command as a detached background job and record an immediate
/// "started" tool result, so the model hands control back at once instead of
/// blocking. Heavy jobs are serialized ([`MAX_AGENT_HEAVY`]): while one runs, a
/// second launch is refused with a "wait" result. A build error records an
/// `is_error` result. This never pauses the turn.
pub fn spawn_heavy(state: &mut AppState, call: &ToolCall, kind: HeavyKind, ctx: &egui::Context) {
    let command = call
        .input
        .get("command")
        .and_then(|value| value.as_str())
        .unwrap_or_default()
        .to_string();
    let label = heavy_label(kind, &command);

    if state.jobs.agent_jobs.len() >= MAX_AGENT_HEAVY {
        // Global cap across the whole app (memory safety), so don't promise a
        // per-turn follow-up here — another conversation's job may hold the slot.
        record_result(
            state,
            call,
            format!(
                "Only one heavy computation can run at a time, and one is already \
                 running. Try `{command}` again once it finishes."
            ),
            false,
        );
        return;
    }

    let words: Vec<String> = command.split_whitespace().map(str::to_string).collect();
    let args = &words[1..]; // drop the `md` / `qm` / `dock` verb

    let spawned: Result<AgentHeavyJob, String> = match kind {
        HeavyKind::Qm => crate::frontend::qm_commands::build_agent_qm_request(state, args)
            .map(|request| {
                AgentHeavyJob::Qm(spawn_qm_job(
                    crate::engines::qm::QmJob::Molecular(request),
                    None,
                ))
            })
            .map_err(|error| error.to_string()),
        HeavyKind::Md => crate::frontend::md_commands::build_agent_md_request(state, args)
            .map(|request| AgentHeavyJob::Engine(spawn_gromacs_pipeline_job(request)))
            .map_err(|error| error.to_string()),
        HeavyKind::Dock => crate::frontend::docking_commands::build_agent_dock_request(state, args)
            .map(|request| AgentHeavyJob::Docking(spawn_docking_job(request)))
            .map_err(|error| error.to_string()),
    };

    match spawned {
        Ok(job) => {
            let id = state.jobs.next_agent_job_id;
            state.jobs.next_agent_job_id += 1;
            let conversation = state.ui.agent.active_conversation;
            state.jobs.agent_jobs.push(TrackedAgentJob {
                id,
                conversation,
                label: label.clone(),
                started_at_ms: crate::backend::storage::jobs::now_ms(),
                job,
            });
            record_result(
                state,
                call,
                format!(
                    "Started background job #{id} ({label}). It runs off-thread; you will \
                     get a follow-up message when it finishes. You may keep talking to the \
                     user in the meantime."
                ),
                false,
            );
            notice(
                state,
                &format!("Started `{command}` as background job #{id}."),
            );
            ctx.request_repaint_after(AGENT_POLL);
        }
        Err(reason) => {
            record_result(
                state,
                call,
                format!("could not start `{command}`: {reason}"),
                true,
            );
        }
    }
}

/// Drain every background job (called from `poll_jobs`). A completion adds its
/// result to the workspace, posts a notice to the originating conversation, and
/// enqueues a `JobDone` to wake the model; survivors keep polling. After a
/// completion the queue is pumped, so an idle agent auto-continues the workflow.
pub fn poll_agent_jobs(state: &mut AppState, ctx: &egui::Context) {
    if state.jobs.agent_jobs.is_empty() {
        return;
    }
    let jobs = std::mem::take(&mut state.jobs.agent_jobs);
    let mut survivors = Vec::with_capacity(jobs.len());
    let mut any_completed = false;
    for mut tracked in jobs {
        let completion = match &mut tracked.job {
            AgentHeavyJob::Qm(running) => drain_qm(state, running),
            AgentHeavyJob::Engine(running) => drain_engine(state, running),
            AgentHeavyJob::Docking(running) => drain_docking(state, running),
        };
        match completion {
            Some((summary, is_error)) => {
                finish_agent_job(state, &tracked, summary, is_error);
                any_completed = true;
            }
            None => survivors.push(tracked),
        }
    }
    state.jobs.agent_jobs = survivors;
    if !state.jobs.agent_jobs.is_empty() {
        ctx.request_repaint_after(AGENT_POLL);
    }
    if any_completed {
        // A finished job enqueued a `JobDone`; wake the model if it is idle.
        pump_queue(state, ctx);
    }
}

/// Cancel one background job by id (composer running-jobs ✕). Leaves a notice in
/// the originating conversation; a no-op if the id is unknown.
pub fn cancel_agent_job(state: &mut AppState, id: u64) {
    let Some(pos) = state.jobs.agent_jobs.iter().position(|job| job.id == id) else {
        return;
    };
    let tracked = state.jobs.agent_jobs.remove(pos);
    tracked.job.cancel();
    if let Some(conversation) = state.ui.agent.conversation_mut(tracked.conversation) {
        conversation
            .transcript
            .push(TranscriptEntry::Notice(format!(
                "Cancelled background job #{id} ({}).",
                tracked.label
            )));
    }
}

/// Cancel and remove every background job belonging to `conversation`, returning
/// how many were stopped. Used when the user Stops the agent or deletes a chat, so
/// detached workers and their orphaned results don't linger.
pub fn cancel_conversation_jobs(
    state: &mut AppState,
    conversation: AssistantConversationId,
) -> usize {
    let mut cancelled = 0;
    state.jobs.agent_jobs.retain(|job| {
        if job.conversation == conversation {
            job.job.cancel();
            cancelled += 1;
            false
        } else {
            true
        }
    });
    cancelled
}

/// Route a finished job to the conversation that launched it: a transcript
/// notice the user can read, plus a `JobDone` in that conversation's queue so the
/// model is woken to continue (e.g. optimize → frequencies).
fn finish_agent_job(
    state: &mut AppState,
    tracked: &TrackedAgentJob,
    summary: String,
    is_error: bool,
) {
    let verb = if is_error { "failed" } else { "finished" };
    let note = format!("Background job #{} ({}) {verb}.", tracked.id, tracked.label);
    let detail = crate::frontend::agent::session::first_nonempty_line(&summary);
    if let Some(conversation) = state.ui.agent.conversation_mut(tracked.conversation) {
        conversation.transcript.push(TranscriptEntry::Notice(note));
        conversation.push_completed(tracked.label.clone(), detail, !is_error);
        conversation.queued.push_back(PendingTurn::JobDone {
            label: tracked.label.clone(),
            summary,
            is_error,
        });
    }
}

fn drain_docking(state: &mut AppState, running: &RunningDockingJob) -> Option<(String, bool)> {
    let mut completion = None;
    while let Ok(message) = running.receiver.try_recv() {
        match message {
            DockingWorkerMessage::Progress { stage } => {
                state.set_message(format!("Docking: {stage}; running in background"));
            }
            DockingWorkerMessage::Finished(outcome) => {
                let outcome = *outcome;
                crate::frontend::docking_commands::add_pose_entries(state, &outcome);
                state.set_message(outcome.summary.clone());
                completion = Some((outcome.summary, false));
            }
            DockingWorkerMessage::Failed(error) => {
                completion = Some((format!("docking failed: {error}"), true));
            }
        }
    }
    completion
}

fn drain_qm(state: &mut AppState, running: &RunningQmJob) -> Option<(String, bool)> {
    let mut completion = None;
    while let Ok(message) = running.receiver.try_recv() {
        match message {
            QmWorkerMessage::Progress { stage } => {
                state.set_message(format!("QM: {stage}; running in background"));
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
    completion
}

fn drain_engine(state: &mut AppState, running: &mut RunningEngineJob) -> Option<(String, bool)> {
    let mut completion = None;
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
    completion
}
