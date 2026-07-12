use super::super::*;

use super::{JobContext, JobPoll, JobRuntime, drive};
use crate::frontend::jobs::{
    EngineSuccess, LocalJobSlot, RunningEngineJob, RunningOptimization, RunningQmJob,
};
use crate::frontend::state::{LogLevel, SystemSubsystem};
use crate::job::CancelSignal;

/// Persist a locally-run QM calculation's artifacts into the active task's run
/// directory, creating it on demand. A no-op when there is no active QM task run
/// to anchor them to.
pub(crate) fn save_qm_run_artifacts(state: &mut AppState, outcome: &crate::engines::qm::QmOutcome) {
    let Some(task_run_id) = state.active_task_run else {
        return;
    };
    let Some(kind) = state.tasks.task_run(task_run_id).map(|task| task.kind) else {
        return;
    };
    if !kind.is_qm() {
        return;
    }
    match ensure_active_task_run_dir(state, kind, None) {
        Ok(run_dir) => save_qm_artifacts(state, &run_dir, outcome),
        Err(error) => state.report_system_error(
            SystemSubsystem::Storage,
            format!("failed to create QM run directory: {error}"),
        ),
    }
    state.ui.task_chart_thumbnails.remove(&task_run_id);
}

pub(crate) fn poll_engine_job(state: &mut AppState, ctx: &egui::Context) {
    let Some(running) = state.jobs.take_engine() else {
        return;
    };
    if let Some(running) = drive(state, ctx, running) {
        state.jobs.set_engine(running);
    }
}

impl JobRuntime for RunningEngineJob {
    fn slot(&self) -> LocalJobSlot {
        LocalJobSlot::Engine
    }

    fn request_cancel(&mut self, state: &mut AppState) -> CancelSignal {
        // The `gmx` subprocess is spawned with this flag, so an in-flight mdrun is
        // killed and the pipeline stops between stages.
        self.cancel
            .store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some(job_id) = state.jobs.local_execution(self.slot()) {
            state.job_notice(
                job_id,
                format!("{} {} stopping", self.engine, self.job_kind),
            );
        }
        CancelSignal::Accepted
    }

    fn poll(&mut self, state: &mut AppState, cx: &JobContext) -> JobPoll {
        let engine_name = self.engine;
        loop {
            match self.receiver.try_recv() {
                Ok(EngineWorkerMessage::Stage(stage)) => {
                    self.latest_stage = Some(stage);
                }
                Ok(EngineWorkerMessage::Log(line)) => {
                    if let Some(job_id) = cx.job_id {
                        state.append_job_log(job_id, LogLevel::Info, line);
                    }
                }
                Ok(EngineWorkerMessage::Finished(success)) => {
                    apply_engine_outcome(state, cx, *success);
                    // A completed build discards its pre-build undo snapshot.
                    state.optimization_origin = None;
                    return JobPoll::Terminal(TaskStatus::Completed);
                }
                Ok(EngineWorkerMessage::Failed(error)) => {
                    if let Some(job_id) = cx.job_id {
                        state.job_failed(job_id, format!("{engine_name} failed: {error}"));
                    }
                    state.optimization_origin = None;
                    return JobPoll::Terminal(TaskStatus::Failed);
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => return JobPoll::Running,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => return JobPoll::ChannelLost,
            }
        }
    }
}

/// Apply a finished engine run: add the result structure as a new entry, mark it
/// an MD-run output when it produced a trajectory, and record it in the ledger.
fn apply_engine_outcome(state: &mut AppState, cx: &JobContext, success: EngineSuccess) {
    // The badge tracks the job kind, not the trajectory, so a relax-only run (which
    // writes no `.xtc`) is still marked; build jobs are not.
    let is_md_run = success.job_kind == "run-md";
    let trajectory = success.trajectory.clone();
    let save_path = structure_io::default_structure_save_path(&success.structure, None);
    let entry_id = add_and_show_entry(state, success.structure, None, save_path);
    if let Some(task_run_id) = cx.task_run_id {
        record_task_result_entry(state, task_run_id, entry_id);
    }
    if is_md_run {
        set_md_run_origin(state, entry_id, trajectory);
    }
    if let Some(job_id) = cx.job_id {
        let role = if is_md_run { "md" } else { "structure" };
        record_materialization(
            state,
            &job_id.to_string(),
            role,
            Some(entry_id),
            &[entry_id],
        );
        state.job_succeeded(job_id, success.summary);
    }
}

pub(crate) fn poll_optimization_job(state: &mut AppState, ctx: &egui::Context) {
    let Some(running) = state.jobs.take_optimizer() else {
        return;
    };
    if let Some(running) = drive(state, ctx, running) {
        state.jobs.set_optimizer(running);
    }
}

impl JobRuntime for RunningOptimization {
    fn slot(&self) -> LocalJobSlot {
        LocalJobSlot::Optimizer
    }

    fn request_cancel(&mut self, state: &mut AppState) -> CancelSignal {
        self.cancel
            .store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some(job_id) = state.jobs.local_execution(self.slot()) {
            state.job_notice(
                job_id,
                match self.latest_report {
                    Some(report) => format!(
                        "forcefield optimization stopping: energy {:.3} -> {:.3} in {} steps",
                        report.initial_energy, report.final_energy, report.steps
                    ),
                    None => "forcefield optimization stopping".to_string(),
                },
            );
        }
        CancelSignal::Accepted
    }

    fn poll(&mut self, state: &mut AppState, cx: &JobContext) -> JobPoll {
        // Optimization streams into the *live* active structure, so its pre-run
        // snapshot is committed to undo history when it produced a result and
        // restored (reverting the geometry) when it was stopped before any step.
        let before = state.optimization_origin.take();
        let mut saw_progress = false;
        let outcome = loop {
            match self.receiver.try_recv() {
                Ok(OptimizationWorkerMessage::Progress { structure, report }) => {
                    // Per-step energy is structured progress (the energy trace and
                    // latest report Activity reads), never an appended log row.
                    let first = self.latest_report.is_none();
                    *state.structure_mut() = structure;
                    state.mark_structure_changed();
                    self.latest_report = Some(report);
                    if self.energy_trace.is_empty() {
                        self.energy_trace
                            .push([0.0, f64::from(report.initial_energy)]);
                    }
                    self.energy_trace
                        .push([report.steps as f64, f64::from(report.final_energy)]);
                    saw_progress = true;
                    state.set_source_path(None);
                    if first {
                        state.status_neutral("Optimizing forcefield… press Esc to stop");
                    }
                }
                Ok(OptimizationWorkerMessage::Finished { structure, report }) => {
                    *state.structure_mut() = structure;
                    state.mark_structure_changed();
                    self.latest_report = Some(report);
                    saw_progress = true;
                    state.set_source_path(None);
                    match cx.job_id {
                        Some(job_id) => {
                            state.job_succeeded(job_id, optimization_finished_message(report))
                        }
                        None => state.status_success(optimization_finished_message(report)),
                    }
                    break JobPoll::Terminal(TaskStatus::Completed);
                }
                Ok(OptimizationWorkerMessage::Failed(error)) => {
                    let text = format!("forcefield optimization failed: {error}");
                    match cx.job_id {
                        Some(job_id) => state.job_failed(job_id, text),
                        None => state.status_neutral(text),
                    }
                    break JobPoll::Terminal(TaskStatus::Failed);
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break JobPoll::Running,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => break JobPoll::ChannelLost,
            }
        };
        match outcome {
            JobPoll::Running => {
                state.optimization_origin = before;
                JobPoll::Running
            }
            terminal => {
                if let Some(before) = before {
                    if saw_progress {
                        state.history.push_undo(before);
                    } else {
                        state.restore_edit_snapshot(before);
                    }
                }
                terminal
            }
        }
    }
}

pub(crate) fn poll_qm_job(state: &mut AppState, ctx: &egui::Context) {
    let Some(running) = state.jobs.take_qm() else {
        return;
    };
    if let Some(running) = drive(state, ctx, running) {
        state.jobs.set_qm(running);
    }
}

impl JobRuntime for RunningQmJob {
    fn slot(&self) -> LocalJobSlot {
        LocalJobSlot::Qm
    }

    fn request_cancel(&mut self, state: &mut AppState) -> CancelSignal {
        // Production runs QM in a subprocess, so this kills the hartree child; the
        // flag re-labels a `Finished`/`Failed` that races the kill as cancelled.
        self.cancel_requested = true;
        self.cancel.cancel();
        if let Some(job_id) = state.jobs.local_execution(self.slot()) {
            state.job_notice(job_id, "QM calculation stopping");
        }
        CancelSignal::Accepted
    }

    fn poll(&mut self, state: &mut AppState, cx: &JobContext) -> JobPoll {
        loop {
            match self.receiver.try_recv() {
                Ok(QmWorkerMessage::Progress { stage }) => {
                    self.latest_stage = Some(stage);
                }
                Ok(QmWorkerMessage::Finished(outcome)) => {
                    if self.cancel_requested {
                        if let Some(job_id) = cx.job_id {
                            state.job_notice(job_id, "QM calculation cancelled");
                        }
                        return JobPoll::Terminal(TaskStatus::Cancelled);
                    }
                    apply_qm_outcome(state, cx, *outcome);
                    return JobPoll::Terminal(TaskStatus::Completed);
                }
                Ok(QmWorkerMessage::Failed(error)) => {
                    if self.cancel_requested {
                        if let Some(job_id) = cx.job_id {
                            state.job_notice(job_id, "QM calculation cancelled");
                        }
                        return JobPoll::Terminal(TaskStatus::Cancelled);
                    }
                    if let Some(job_id) = cx.job_id {
                        state.job_failed(job_id, format!("QM calculation failed: {error}"));
                    }
                    return JobPoll::Terminal(TaskStatus::Failed);
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => return JobPoll::Running,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => return JobPoll::ChannelLost,
            }
        }
    }
}

/// Apply a finished local QM outcome: save the report, surface an optimized
/// geometry as a new entry (or record a report for an entry-less run), and record
/// the outcome in the ledger so a re-poll never re-imports it.
fn apply_qm_outcome(state: &mut AppState, cx: &JobContext, outcome: crate::engines::qm::QmOutcome) {
    if let Some(job_id) = cx.job_id {
        for line in outcome.summary.lines() {
            state.append_job_log(job_id, LogLevel::Info, line);
        }
    }
    // Persist the raw report to the task's run directory before any new entry is
    // added, so the run's source entry is the input structure, not the result.
    save_qm_run_artifacts(state, &outcome);
    // A QM run's optimized geometry is surfaced as a new entry (the original is
    // preserved). A single-point energy or frequency run produces no entry but
    // still records a report in the ledger, so its outcome is durably applied.
    let already = cx
        .job_id
        .is_some_and(|id| outcome_already_materialized(state, &id.to_string()));
    if !already {
        match outcome.optimized_structure {
            Some(optimized) => {
                let save_path = structure_io::default_structure_save_path(&optimized, None);
                let entry_id = add_and_show_entry(state, optimized, None, save_path);
                if let Some(task_run_id) = cx.task_run_id {
                    record_task_result_entry(state, task_run_id, entry_id);
                }
                set_qm_run_origin(state, entry_id);
                if let Some(job_id) = cx.job_id {
                    record_materialization(
                        state,
                        &job_id.to_string(),
                        "optimized",
                        Some(entry_id),
                        &[entry_id],
                    );
                }
            }
            None => {
                if let Some(job_id) = cx.job_id {
                    record_materialization(state, &job_id.to_string(), "report", None, &[]);
                }
            }
        }
    }
    // The run is now the newest QM result of whichever entry it anchors to — the
    // new geometry, or the input structure when there is none — so the memoized
    // per-entry chart availability is stale.
    state.ui.chart_availability.clear();
    let summary = format!(
        "QM complete: energy {:.6} Eh{}",
        outcome.energy_hartree,
        if outcome.converged {
            " (converged)"
        } else {
            " (not converged)"
        }
    );
    match cx.job_id {
        Some(job_id) => state.job_succeeded(job_id, summary),
        None => state.status_success(summary),
    }
}
