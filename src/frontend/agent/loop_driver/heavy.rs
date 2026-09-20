use super::*;

use eframe::egui;

use crate::backend::tasks::{TaskStatus, task_controller_by_id};
use crate::frontend::agent::session::{AssistantConversationId, PendingTurn, TranscriptEntry};
use crate::frontend::jobs::{
    AgentHeavyJob, DockingWorkerMessage, EngineWorkerMessage, QmWorkerMessage, RunningDockingJob,
    RunningEngineJob, RunningQmJob, TrackedAgentJob, spawn_docking_job, spawn_gromacs_pipeline_job,
};
use crate::frontend::state::{AppState, LogLevel};
use crate::io::llm::types::ToolCall;
use crate::job::JobId;

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

pub fn spawn_agent_online_structure_search(
    state: &mut AppState,
    call: &ToolCall,
    ctx: &egui::Context,
) -> Option<bool> {
    if call.name != "run_command" {
        return None;
    }
    let command = call.input.get("command").and_then(|value| value.as_str())?;
    let parsed = match crate::frontend::console::parse_find_query(command) {
        Ok(Some(query)) => query,
        Ok(None) => return None,
        Err(error) => {
            record_result(state, call, error.to_string(), true);
            return Some(false);
        }
    };
    let id = state.jobs.next_agent_job_id;
    state.jobs.next_agent_job_id += 1;
    let running = crate::frontend::jobs::spawn_online_structure_search(parsed, id);
    state.jobs.agent_online_structures.push(
        crate::frontend::jobs::TrackedAgentOnlineStructureJob {
            id,
            conversation: state.ui.agent.active_conversation,
            running,
        },
    );
    record_result(
        state,
        call,
        format!(
            "Started background online-structure lookup #{id}. The candidate list will be returned when it finishes."
        ),
        false,
    );
    ctx.request_repaint_after(AGENT_POLL);
    Some(true)
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
    state.tasks.create_task_run(controller)
}

/// Launch a heavy command as a detached background job and record an immediate
/// "started" tool result, so the model hands control back at once instead of
/// blocking. Heavy jobs are serialized ([`MAX_AGENT_HEAVY`]): while one runs, a
/// second launch is refused with a "wait" result. A build error records an
/// `is_error` result. This never pauses the turn.
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
            true,
        );
        return false;
    }

    let words: Vec<String> = command.split_whitespace().map(str::to_string).collect();
    let args = &words[1..]; // drop the `md` / `qm` / `dock` verb

    let mut prepared_task = None;
    let mut prepare = |state: &mut AppState| -> anyhow::Result<std::path::PathBuf> {
        let inputs =
            heavy_inputs(state, call)?.ok_or_else(|| anyhow::anyhow!("missing compute inputs"))?;
        let task_id = register_agent_task_run(state, kind, &command);
        prepared_task = Some(task_id);
        crate::frontend::dispatcher::bind_task_inputs(state, task_id, inputs)?;
        let task_kind = state
            .tasks
            .task_run(task_id)
            .ok_or_else(|| anyhow::anyhow!("missing task"))?
            .kind;
        crate::frontend::dispatcher::ensure_task_run_dir(state, task_id, task_kind, None)
    };
    let spawned: Result<AgentHeavyJob, String> = match kind {
        HeavyKind::Qm => crate::frontend::qm_commands::build_agent_qm_request(state, args)
            .and_then(|job| {
                let mut launches = crate::engines::registry::EngineLaunches::new();
                if job.engine == crate::engines::qm::QmEngine::Orca {
                    let launch = crate::backend::engine_launch::resolve_engine_launch(
                        crate::backend::engine_launch::LaunchTarget::Local(
                            &state.config.engine_overrides,
                        ),
                        crate::engines::registry::EngineId::ORCA,
                    )?
                    .launch;
                    launches.insert(crate::engines::registry::EngineId::ORCA, launch);
                }
                prepare(state)?;
                Ok(AgentHeavyJob::Qm(
                    crate::frontend::jobs::spawn_qm_job_with_launches(job, None, launches)?,
                ))
            })
            .map_err(|error| error.to_string()),
        HeavyKind::Md => crate::frontend::md_commands::build_agent_md_request(state, args)
            .and_then(|request| {
                let request = request.with_working_dir(prepare(state)?);
                Ok(AgentHeavyJob::Engine(spawn_gromacs_pipeline_job(request)))
            })
            .map_err(|error| error.to_string()),
        HeavyKind::Dock => crate::frontend::docking_commands::build_agent_dock_request(state, args)
            .and_then(|request| {
                prepare(state)?;
                Ok(AgentHeavyJob::Docking(spawn_docking_job(request)))
            })
            .map_err(|error| error.to_string()),
    };

    match spawned {
        Ok(job) => {
            let id = state.jobs.next_agent_job_id;
            state.jobs.next_agent_job_id += 1;
            let conversation = state.ui.agent.active_conversation;
            let Some(task_run_id) = prepared_task else {
                record_result(state, call, "missing prepared task".to_string(), true);
                return false;
            };
            crate::frontend::dispatcher::mark_task_status(state, task_run_id, TaskStatus::Running);
            let job_kind = state
                .tasks
                .task_run(task_run_id)
                .map(|task| task.controller_id.to_string());
            let job_id = crate::frontend::dispatcher::begin_job_execution(
                state,
                task_run_id,
                crate::backend::run_attempt::Placement::Local,
                job_kind,
            );
            state.jobs.agent_jobs.push(TrackedAgentJob {
                id,
                conversation,
                label: label.clone(),
                task_run_id,
                job_id,
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
            true
        }
        Err(reason) => {
            if let Some(id) = prepared_task {
                crate::frontend::dispatcher::mark_task_status(state, id, TaskStatus::Failed);
            }
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

/// Drain every background job (called from `poll_jobs`). A completion adds its
/// result to the workspace, posts a notice to the originating conversation, and
/// enqueues a `JobDone` to wake the model; survivors keep polling. After a
/// completion the queue is pumped, so an idle agent auto-continues the workflow.
pub fn poll_agent_jobs(state: &mut AppState, ctx: &egui::Context) {
    poll_agent_online_structure_jobs(state, ctx);
    if state.jobs.agent_jobs.is_empty() {
        return;
    }
    let jobs = std::mem::take(&mut state.jobs.agent_jobs);
    let mut survivors = Vec::with_capacity(jobs.len());
    let mut any_completed = false;
    for mut tracked in jobs {
        let job_id = tracked.job_id;
        let completion = match &mut tracked.job {
            AgentHeavyJob::Qm(running) => drain_qm(state, running, tracked.task_run_id, job_id),
            AgentHeavyJob::Engine(running) => {
                drain_engine(state, running, tracked.task_run_id, job_id)
            }
            AgentHeavyJob::Docking(running) => drain_docking(state, running, job_id),
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

fn poll_agent_online_structure_jobs(state: &mut AppState, ctx: &egui::Context) {
    if state.jobs.agent_online_structures.is_empty() {
        return;
    }
    let jobs = std::mem::take(&mut state.jobs.agent_online_structures);
    let mut survivors = Vec::with_capacity(jobs.len());
    let mut completed = false;
    for tracked in jobs {
        match tracked.running.receiver.try_recv() {
            Ok(crate::frontend::jobs::OnlineStructureJobOutcome::Search(result)) => {
                let (summary, is_error) = match result {
                    Ok(result) => (
                        crate::frontend::console::format_structure_search_result(&result),
                        false,
                    ),
                    Err(error) => (format!("online structure lookup failed: {error}"), true),
                };
                if let Some(conversation) = state.ui.agent.conversation_mut(tracked.conversation) {
                    conversation
                        .transcript
                        .push(TranscriptEntry::Notice(format!(
                            "Background online-structure lookup #{} finished.",
                            tracked.id
                        )));
                    conversation.queued.push_back(PendingTurn::JobDone {
                        label: "find online structure".to_string(),
                        summary,
                        is_error,
                    });
                }
                completed = true;
            }
            Ok(crate::frontend::jobs::OnlineStructureJobOutcome::Fetch(_)) => {}
            Err(std::sync::mpsc::TryRecvError::Empty) => survivors.push(tracked),
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                if let Some(conversation) = state.ui.agent.conversation_mut(tracked.conversation) {
                    conversation.queued.push_back(PendingTurn::JobDone {
                        label: "find online structure".to_string(),
                        summary: "online structure worker stopped unexpectedly".to_string(),
                        is_error: true,
                    });
                }
                completed = true;
            }
        }
    }
    state.jobs.agent_online_structures = survivors;
    if !state.jobs.agent_online_structures.is_empty() {
        ctx.request_repaint_after(AGENT_POLL);
    }
    if completed {
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
    let mut cancelled_jobs = Vec::new();
    state.jobs.agent_jobs.retain(|job| {
        if job.conversation == conversation {
            job.job.cancel();
            cancelled_jobs.push(job.job_id);
            false
        } else {
            true
        }
    });
    let before = state.jobs.agent_online_structures.len();
    state
        .jobs
        .agent_online_structures
        .retain(|job| job.conversation != conversation);
    for job_id in &cancelled_jobs {
        crate::frontend::dispatcher::complete_local_job(
            state,
            Some(*job_id),
            TaskStatus::Cancelled,
        );
    }
    cancelled_jobs.len() + before - state.jobs.agent_online_structures.len()
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
    let status = if cancelled {
        TaskStatus::Cancelled
    } else if is_error {
        TaskStatus::Failed
    } else {
        TaskStatus::Completed
    };
    // Finalize execution and task through the run graph exactly as a manual job
    // does, then surface the same job-scoped feedback.
    crate::frontend::dispatcher::complete_local_job(state, Some(tracked.job_id), status);
    match status {
        TaskStatus::Completed if matches!(&tracked.job, AgentHeavyJob::Qm(_)) => {}
        TaskStatus::Completed => state.job_succeeded(tracked.job_id, summary.clone()),
        TaskStatus::Failed => state.job_failed(tracked.job_id, summary.clone()),
        _ => state.job_notice(tracked.job_id, summary.clone()),
    }
    let verb = if cancelled {
        "cancelled"
    } else if is_error {
        "failed"
    } else {
        "finished"
    };
    let is_qm = matches!(&tracked.job, AgentHeavyJob::Qm(_));
    let result = state
        .tasks
        .runs
        .execution(&tracked.job_id.to_string())
        .and_then(|e| e.qm_result.clone());
    let issue =
        is_qm && !cancelled && (is_error || result.as_ref().is_none_or(|r| r.needs_diagnosis()));
    let evidence = state
        .tasks
        .task_run(tracked.task_run_id)
        .and_then(|t| t.run_dir.as_ref())
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "unknown".into());
    let note = if is_qm {
        format!(
            "Background job #{} ({}, job {}, task {}) {verb}. {}\n{}\nEvidence directory: {}",
            tracked.id,
            tracked.label,
            tracked.job_id,
            tracked.task_run_id,
            summary,
            result
                .as_ref()
                .map(|r| r.summary())
                .unwrap_or_else(|| "QM result status unknown".into()),
            evidence
        )
    } else {
        format!("Background job #{} ({}) {verb}.", tracked.id, tracked.label)
    };
    if issue
        && tracked.conversation == state.ui.agent.active_conversation
        && let Some(job) = state.jobs.agent.take()
    {
        job.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    if let Some(conversation) = state.ui.agent.conversation_mut(tracked.conversation) {
        if issue {
            conversation.recover_interrupted("QM issue requires read-only diagnosis");
            conversation.qm_diagnostic_only = true;
            let dropped = conversation
                .queued
                .iter()
                .filter(|p| matches!(p, PendingTurn::UserMessage(_)))
                .count();
            conversation.queued.clear();
            if dropped > 0 {
                conversation
                    .transcript
                    .push(TranscriptEntry::Notice(format!(
                        "Discarded {dropped} queued message(s) — QM requires diagnosis."
                    )));
            }
            conversation.streaming_text.clear();
            conversation.current_backlog = None;
            conversation.phase = crate::frontend::agent::session::AgentPhase::Idle;
            conversation.history.push(crate::io::llm::types::ChatMessage::user_text(format!("{note}\nDiagnose read only, explain evidence and suggestions, then wait for new user instructions.")));
        }
        conversation.transcript.push(TranscriptEntry::Notice(note));
        if cancelled {
            return;
        }
        if is_qm {
            conversation.queued.push_back(PendingTurn::QmDone {
                job_id: tracked.job_id.to_string(),
                summary,
                result,
                execution_failed: is_error,
            });
        } else {
            conversation.queued.push_back(PendingTurn::JobDone {
                label: tracked.label.clone(),
                summary,
                is_error,
            });
        }
    }
}

fn drain_docking(
    state: &mut AppState,
    running: &mut RunningDockingJob,
    job_id: JobId,
) -> Option<(String, bool)> {
    let mut completion = None;
    while let Ok(message) = running.receiver.try_recv() {
        match message {
            DockingWorkerMessage::Progress { stage } => running.latest_stage = Some(stage),
            DockingWorkerMessage::Finished(outcome) => {
                let outcome = *outcome;
                let summary = outcome.summary.clone();
                let cx = crate::frontend::dispatcher::JobContext {
                    job_id: Some(job_id),
                    task_run_id: state.tasks.runs.task_run_id_for_job(&job_id.to_string()),
                };
                crate::frontend::dispatcher::apply_docking_outcome(state, &cx, outcome);
                completion = Some((summary, false));
            }
            DockingWorkerMessage::Failed(error) => {
                completion = Some((format!("docking failed: {error}"), true));
            }
        }
    }
    completion
}

fn drain_qm(
    state: &mut AppState,
    running: &mut RunningQmJob,
    task_run_id: u64,
    job_id: JobId,
) -> Option<(String, bool)> {
    loop {
        match running.receiver.try_recv() {
            Ok(QmWorkerMessage::Progress { stage }) => running.latest_stage = Some(stage),
            Ok(QmWorkerMessage::Finished(outcome)) => {
                if running.cancel_requested {
                    return Some(("QM calculation cancelled".to_string(), true));
                }
                let summary = outcome.summary.clone();
                let cx = crate::frontend::dispatcher::JobContext {
                    job_id: Some(job_id),
                    task_run_id: Some(task_run_id),
                };
                crate::frontend::dispatcher::apply_qm_outcome(state, &cx, *outcome);
                return Some((summary, false));
            }
            Ok(QmWorkerMessage::Failed(error)) => {
                let summary = if running.cancel_requested {
                    "QM calculation cancelled".to_string()
                } else {
                    format!("QM calculation failed: {error}")
                };
                return Some((summary, true));
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => return None,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                let summary = if running.cancel_requested {
                    "QM calculation cancelled"
                } else {
                    "QM worker stopped without a result"
                };
                return Some((summary.to_string(), true));
            }
        }
    }
}

fn drain_engine(
    state: &mut AppState,
    running: &mut RunningEngineJob,
    task_run_id: u64,
    job_id: JobId,
) -> Option<(String, bool)> {
    let mut completion = None;
    while let Ok(message) = running.receiver.try_recv() {
        match message {
            EngineWorkerMessage::Stage(stage) => {
                running.latest_stage = Some(stage);
            }
            EngineWorkerMessage::Log(line) => state.append_job_log(job_id, LogLevel::Info, line),
            EngineWorkerMessage::Finished(success) => {
                let summary = success.summary.clone();
                let cx = crate::frontend::dispatcher::JobContext {
                    job_id: Some(job_id),
                    task_run_id: Some(task_run_id),
                };
                crate::frontend::dispatcher::apply_engine_outcome(state, &cx, *success);
                completion = Some((summary, false));
            }
            EngineWorkerMessage::Failed(error) => {
                completion = Some((format!("molecular dynamics failed: {error}"), true));
            }
        }
    }
    completion
}

pub(crate) fn heavy_inputs(
    state: &AppState,
    call: &ToolCall,
) -> anyhow::Result<Option<Vec<crate::backend::tasks::TaskInput>>> {
    let Some(kind) = heavy_kind_of(call) else {
        return Ok(None);
    };
    let command = call
        .input
        .get("command")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("missing command"))?;
    let words: Vec<String> = command
        .split_whitespace()
        .skip(1)
        .map(str::to_string)
        .collect();
    let inputs = match kind {
        HeavyKind::Qm => crate::frontend::qm_commands::agent_qm_inputs(state, &words)?,
        HeavyKind::Md => vec![crate::frontend::entry_ref::primary_input(state)?],
        HeavyKind::Dock => crate::frontend::docking_commands::agent_dock_inputs(state, &words)?,
    };
    Ok(Some(inputs))
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
    fn register_creates_a_ready_task_run() {
        let mut state = AppState::scratch(Default::default(), Vec::new());
        let id = register_agent_task_run(&mut state, HeavyKind::Qm, "qm optimize");
        let task = state.tasks.task_run(id).expect("task run created");
        assert_eq!(task.controller_id, "qm-optimize");
        assert_eq!(task.status, TaskStatus::Ready);
    }

    #[test]
    fn agent_qm_completion_creates_the_optimized_entry_and_records_the_result() {
        // QM vertical slice (agent placement): an agent-driven QM run drains to
        // completion, adds its optimized geometry as an entry, and records it as the
        // task's result — attributed by the TrackedAgentJob's task_run_id.
        let mut state = AppState::scratch(Default::default(), Vec::new());
        let task = register_agent_task_run(&mut state, HeavyKind::Qm, "qm optimize");
        let job_id = crate::frontend::dispatcher::begin_job_execution(
            &mut state,
            task,
            crate::backend::run_attempt::Placement::Local,
            Some("qm-optimize".to_string()),
        );

        let (tx, rx) = std::sync::mpsc::channel();
        tx.send(QmWorkerMessage::Finished(Box::new(
            crate::engines::qm::QmOutcome {
                energy_hartree: -1.0,
                converged: true,
                optimized_structure: Some(crate::domain::Structure::empty()),
                summary: "energy -1.0 Eh".to_string(),
                scf_trace: Vec::new(),
                opt_trace: Vec::new(),
                frequencies: Vec::new(),
            },
        )))
        .unwrap();
        let mut running = RunningQmJob {
            cancel: crate::wire::JobCancelHandle::from_flag(std::sync::Arc::new(
                std::sync::atomic::AtomicBool::new(false),
            )),
            receiver: rx,
            latest_stage: None,
            cancel_requested: false,
        };

        let completion = drain_qm(&mut state, &mut running, task, job_id);
        let (_summary, is_error) = completion.expect("the job completes");
        assert!(!is_error, "a converged run is not an error");
        assert!(
            state
                .tasks
                .task_run(task)
                .unwrap()
                .result_entry_id
                .is_some(),
            "the optimized geometry is recorded as the task result"
        );
    }

    #[test]
    fn complete_marks_terminal_status() {
        // An agent-launched job finalizes through the same run graph a manual job
        // does: completing its bound execution marks the task terminal.
        let mut state = AppState::scratch(Default::default(), Vec::new());
        let ok = register_agent_task_run(&mut state, HeavyKind::Qm, "qm energy");
        let ok_job = crate::frontend::dispatcher::begin_job_execution(
            &mut state,
            ok,
            crate::backend::run_attempt::Placement::Local,
            Some("qm-energy".to_string()),
        );
        crate::frontend::dispatcher::complete_local_job(
            &mut state,
            Some(ok_job),
            TaskStatus::Completed,
        );
        assert_eq!(
            state.tasks.task_run(ok).unwrap().status,
            TaskStatus::Completed
        );

        let bad = register_agent_task_run(&mut state, HeavyKind::Md, "md run");
        let bad_job = crate::frontend::dispatcher::begin_job_execution(
            &mut state,
            bad,
            crate::backend::run_attempt::Placement::Local,
            Some("md-run".to_string()),
        );
        crate::frontend::dispatcher::complete_local_job(
            &mut state,
            Some(bad_job),
            TaskStatus::Failed,
        );
        assert_eq!(
            state.tasks.task_run(bad).unwrap().status,
            TaskStatus::Failed
        );
    }
}
