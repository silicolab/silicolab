use super::*;

use eframe::egui;

use crate::backend::entries::EntryOrigin;
use crate::backend::tasks::{TaskStatus, task_controller_by_id};
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

/// A short cost/impact hint for an approval card, or `None` when the call has no
/// special cost (only heavy commands, which run off-thread one at a time, have one).
pub fn impact_hint(call: &ToolCall) -> Option<String> {
    let kind = heavy_kind_of(call)?;
    let command = call
        .input
        .get("command")
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    Some(format!(
        "{} — heavy compute; runs in the background, one job at a time",
        heavy_label(kind, command)
    ))
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

/// The Task controller that represents an assistant-launched heavy command, so
/// the run is indistinguishable from a hand-launched one in the Task Monitor.
fn agent_task_controller_id(kind: HeavyKind, command: &str) -> &'static str {
    let sub = command.split_whitespace().nth(1).unwrap_or("");
    match kind {
        HeavyKind::Qm => match sub {
            "optimize" | "opt" => "qm-optimize",
            "freq" | "frequencies" => "qm-frequencies",
            "ts" | "saddle" | "transition-state" => "qm-transition-state",
            _ => "qm-energy",
        },
        HeavyKind::Md => "run-md",
        HeavyKind::Dock => "dock-ligand",
    }
}

fn register_agent_task_run(state: &mut AppState, kind: HeavyKind, command: &str) -> u64 {
    let controller = task_controller_by_id(agent_task_controller_id(kind, command))
        .copied()
        .expect("agent heavy controller ids are defined in TASK_CONTROLLERS");
    let task_run_id = state.tasks.create_task_run(controller);
    let source = state.entries.active_entry_id();
    state.tasks.set_source_entry_id(task_run_id, source);
    crate::frontend::dispatcher::mark_task_status(state, task_run_id, TaskStatus::Running);
    task_run_id
}

fn complete_agent_task_run(state: &mut AppState, task_run_id: u64, is_error: bool) {
    let status = if is_error {
        TaskStatus::Failed
    } else {
        TaskStatus::Completed
    };
    crate::frontend::dispatcher::mark_task_status(state, task_run_id, status);
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
            let task_run_id = register_agent_task_run(state, kind, &command);
            state.jobs.agent_jobs.push(TrackedAgentJob {
                id,
                conversation,
                label: label.clone(),
                task_run_id,
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
            AgentHeavyJob::Qm(running) => drain_qm(state, running, tracked.task_run_id),
            AgentHeavyJob::Engine(running) => drain_engine(state, running, tracked.task_run_id),
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

/// Cancel and remove every background job belonging to `conversation`, returning
/// how many were stopped. Used when the user Stops the agent or deletes a chat, so
/// detached workers and their orphaned results don't linger.
pub fn cancel_conversation_jobs(
    state: &mut AppState,
    conversation: AssistantConversationId,
) -> usize {
    let mut cancelled_runs = Vec::new();
    state.jobs.agent_jobs.retain(|job| {
        if job.conversation == conversation {
            job.job.cancel();
            cancelled_runs.push(job.task_run_id);
            false
        } else {
            true
        }
    });
    for task_run_id in &cancelled_runs {
        crate::frontend::dispatcher::mark_task_status(state, *task_run_id, TaskStatus::Cancelled);
    }
    cancelled_runs.len()
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
    let cancelled = matches!(&tracked.job, AgentHeavyJob::Qm(job) if job.cancel_requested);
    if cancelled {
        crate::frontend::dispatcher::mark_task_status(
            state,
            tracked.task_run_id,
            TaskStatus::Cancelled,
        );
    } else {
        complete_agent_task_run(state, tracked.task_run_id, is_error);
    }
    let verb = if cancelled {
        "cancelled"
    } else if is_error {
        "failed"
    } else {
        "finished"
    };
    let note = format!("Background job #{} ({}) {verb}.", tracked.id, tracked.label);
    if let Some(conversation) = state.ui.agent.conversation_mut(tracked.conversation) {
        conversation.transcript.push(TranscriptEntry::Notice(note));
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

/// Write an agent-driven QM run's report and series into its run directory,
/// creating the directory on demand. The agent registers its task run without an
/// active-task binding, so the run dir is resolved by id rather than through
/// `ensure_active_task_run_dir`.
fn save_agent_qm_artifacts(
    state: &mut AppState,
    task_run_id: u64,
    outcome: &crate::engines::qm::QmOutcome,
) {
    let Some(kind) = state.tasks.task_run(task_run_id).map(|task| task.kind) else {
        return;
    };
    if !kind.is_qm() {
        return;
    }
    match crate::frontend::dispatcher::ensure_task_run_dir(state, task_run_id, kind, None) {
        Ok(run_dir) => crate::frontend::dispatcher::save_qm_artifacts(state, &run_dir, outcome),
        Err(error) => state
            .output_log
            .push(format!("failed to create QM run directory: {error}")),
    }
}

fn drain_qm(
    state: &mut AppState,
    running: &mut RunningQmJob,
    task_run_id: u64,
) -> Option<(String, bool)> {
    let mut completion = None;
    while let Ok(message) = running.receiver.try_recv() {
        match message {
            QmWorkerMessage::Progress { stage } => {
                state.set_message(format!("QM: {stage}; running in background"));
                running.latest_stage = Some(stage);
            }
            QmWorkerMessage::Finished(outcome) => {
                if running.cancel_requested {
                    completion = Some(("QM calculation cancelled".to_string(), true));
                    continue;
                }
                let outcome = *outcome;
                // Persist the report and series into the run directory before any
                // new entry is added, so the run anchors to the input structure —
                // the same ordering, and the same writer, as the local and remote
                // QM paths. Without this an agent-driven run had no artifacts at
                // all, and so no report or chart on either surface.
                save_agent_qm_artifacts(state, task_run_id, &outcome);
                if let Some(optimized) = outcome.optimized_structure {
                    let save_path = default_structure_save_path(&optimized, None);
                    let entry_id = state.entries.add_entry(optimized, None, save_path);
                    state.show_entry(entry_id);
                    state.entries.set_entry_origin(entry_id, EntryOrigin::QmRun);
                    crate::frontend::dispatcher::record_task_result_entry(
                        state,
                        task_run_id,
                        entry_id,
                    );
                }
                state.ui.chart_availability.clear();
                state.ui.task_chart_thumbnails.remove(&task_run_id);
                state.set_message(outcome.summary.clone());
                completion = Some((outcome.summary, false));
            }
            QmWorkerMessage::Failed(error) => {
                completion = if running.cancel_requested {
                    Some(("QM calculation cancelled".to_string(), true))
                } else {
                    Some((format!("QM calculation failed: {error}"), true))
                };
            }
        }
    }
    completion
}

fn drain_engine(
    state: &mut AppState,
    running: &mut RunningEngineJob,
    task_run_id: u64,
) -> Option<(String, bool)> {
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
                crate::frontend::dispatcher::record_task_result_entry(state, task_run_id, entry_id);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::tasks::TaskStatus;
    use crate::frontend::state::AppState;

    #[test]
    fn agent_qm_subcommands_map_to_controllers() {
        assert_eq!(
            agent_task_controller_id(HeavyKind::Qm, "qm energy"),
            "qm-energy"
        );
        assert_eq!(
            agent_task_controller_id(HeavyKind::Qm, "qm opt"),
            "qm-optimize"
        );
        assert_eq!(
            agent_task_controller_id(HeavyKind::Qm, "qm freq"),
            "qm-frequencies"
        );
        assert_eq!(
            agent_task_controller_id(HeavyKind::Qm, "qm ts"),
            "qm-transition-state"
        );
        assert_eq!(agent_task_controller_id(HeavyKind::Md, "md run"), "run-md");
        assert_eq!(
            agent_task_controller_id(HeavyKind::Dock, "dock lig"),
            "dock-ligand"
        );
    }

    #[test]
    fn register_creates_a_running_task_run() {
        let mut state = AppState::scratch(Default::default(), Vec::new());
        let id = register_agent_task_run(&mut state, HeavyKind::Qm, "qm optimize");
        let task = state.tasks.task_run(id).expect("task run created");
        assert_eq!(task.controller_id, "qm-optimize");
        assert_eq!(task.status, TaskStatus::Running);
    }

    #[test]
    fn complete_marks_terminal_status() {
        let mut state = AppState::scratch(Default::default(), Vec::new());
        let ok = register_agent_task_run(&mut state, HeavyKind::Qm, "qm energy");
        complete_agent_task_run(&mut state, ok, false);
        assert_eq!(
            state.tasks.task_run(ok).unwrap().status,
            TaskStatus::Completed
        );

        let bad = register_agent_task_run(&mut state, HeavyKind::Md, "md run");
        complete_agent_task_run(&mut state, bad, true);
        assert_eq!(
            state.tasks.task_run(bad).unwrap().status,
            TaskStatus::Failed
        );
    }
}
