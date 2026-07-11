use super::super::*;

use super::{JobContext, JobPoll, JobRuntime, drive};
use crate::frontend::jobs::{
    EngineSuccess, LocalJobSlot, RunningEngineJob, RunningOptimization, RunningQmJob,
};
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
        Err(error) => state
            .output_log
            .push(format!("failed to create QM run directory: {error}")),
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
        state.set_message(format!("{} {} stopping", self.engine, self.job_kind));
        CancelSignal::Accepted
    }

    fn poll(&mut self, state: &mut AppState, cx: &JobContext) -> JobPoll {
        let engine_name = self.engine;
        loop {
            match self.receiver.try_recv() {
                Ok(EngineWorkerMessage::Stage(stage)) => {
                    state.set_message(format!("{engine_name}: {stage}"));
                    self.latest_stage = Some(stage);
                }
                Ok(EngineWorkerMessage::Log(line)) => self.append_log(line),
                Ok(EngineWorkerMessage::Finished(success)) => {
                    apply_engine_outcome(state, cx, *success);
                    // A completed build discards its pre-build undo snapshot.
                    state.optimization_origin = None;
                    return JobPoll::Terminal(TaskStatus::Completed);
                }
                Ok(EngineWorkerMessage::Failed(error)) => {
                    state.set_message(format!("{engine_name} failed: {error}"));
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
    }
    state.set_message(success.summary);
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
        state.set_message(match self.latest_report {
            Some(report) => format!(
                "forcefield optimization stopping: energy {:.3} -> {:.3} in {} steps",
                report.initial_energy, report.final_energy, report.steps
            ),
            None => "forcefield optimization stopping".to_string(),
        });
        CancelSignal::Accepted
    }

    fn poll(&mut self, state: &mut AppState, _cx: &JobContext) -> JobPoll {
        // Optimization streams into the *live* active structure, so its pre-run
        // snapshot is committed to undo history when it produced a result and
        // restored (reverting the geometry) when it was stopped before any step.
        let before = state.optimization_origin.take();
        let mut saw_progress = false;
        let outcome = loop {
            match self.receiver.try_recv() {
                Ok(OptimizationWorkerMessage::Progress { structure, report }) => {
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
                    state.set_message(format!(
                        "forcefield optimizing: step {}, energy {:.3}; press Esc to stop",
                        report.steps, report.final_energy
                    ));
                }
                Ok(OptimizationWorkerMessage::Finished { structure, report }) => {
                    *state.structure_mut() = structure;
                    state.mark_structure_changed();
                    self.latest_report = Some(report);
                    saw_progress = true;
                    state.set_source_path(None);
                    state.set_message(optimization_finished_message(report));
                    break JobPoll::Terminal(TaskStatus::Completed);
                }
                Ok(OptimizationWorkerMessage::Failed(error)) => {
                    state.set_message(format!("forcefield optimization failed: {error}"));
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
        state.set_message("QM calculation stopping".to_string());
        CancelSignal::Accepted
    }

    fn poll(&mut self, state: &mut AppState, cx: &JobContext) -> JobPoll {
        loop {
            match self.receiver.try_recv() {
                Ok(QmWorkerMessage::Progress { stage }) => {
                    state.set_message(format!("QM: {stage}; press Esc to stop"));
                    self.latest_stage = Some(stage);
                }
                Ok(QmWorkerMessage::Finished(outcome)) => {
                    if self.cancel_requested {
                        state.set_message("QM calculation cancelled".to_string());
                        return JobPoll::Terminal(TaskStatus::Cancelled);
                    }
                    apply_qm_outcome(state, cx, *outcome);
                    return JobPoll::Terminal(TaskStatus::Completed);
                }
                Ok(QmWorkerMessage::Failed(error)) => {
                    if self.cancel_requested {
                        state.set_message("QM calculation cancelled".to_string());
                        return JobPoll::Terminal(TaskStatus::Cancelled);
                    }
                    state.set_message(format!("QM calculation failed: {error}"));
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
    for line in outcome.summary.lines() {
        state.output_log.push(line.to_string());
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
    state.set_message(format!(
        "QM complete: energy {:.6} Eh{}",
        outcome.energy_hartree,
        if outcome.converged {
            " (converged)"
        } else {
            " (not converged)"
        }
    ));
}
