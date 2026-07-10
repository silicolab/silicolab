use super::super::*;

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
    let Some(mut running) = state.jobs.take_engine() else {
        return;
    };
    let task_run_id = state.active_task_run;
    let mut before = state.optimization_origin.take();
    let fingerprint_before = state.entries_fingerprint();

    if ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
        running
            .cancel
            .store(true, std::sync::atomic::Ordering::Relaxed);
        state.set_message(format!("{} {} stopping", running.engine, running.job_kind));
    }

    let mut finished = false;
    let mut saw_progress = false;
    let mut commit_history = false;
    let engine_name = running.engine;
    // The same engine-job machinery backs both "build the MD system" and "run
    // MD"; the job_kind says which task to mark done on completion.
    let task_kind = if running.job_kind == "build-md" {
        TaskKind::BuildMdSystem
    } else {
        TaskKind::RunMd
    };

    while let Ok(message) = running.receiver.try_recv() {
        match message {
            EngineWorkerMessage::Stage(stage) => {
                state.set_message(format!("{engine_name}: {stage}"));
                running.latest_stage = Some(stage);
            }
            EngineWorkerMessage::Log(line) => {
                running.append_log(line);
            }
            EngineWorkerMessage::Finished(success) => {
                // The badge tracks the job kind, not the trajectory, so a relax-only run
                // (which writes no `.xtc`) is still marked; build jobs are not.
                let is_md_run = success.job_kind == "run-md";
                let trajectory = success.trajectory.clone();
                let save_path = structure_io::default_structure_save_path(&success.structure, None);
                let entry_id = add_and_show_entry(state, success.structure, None, save_path);
                if let Some(task_run_id) = task_run_id {
                    record_task_result_entry(state, task_run_id, entry_id);
                }
                if is_md_run {
                    set_md_run_origin(state, entry_id, trajectory);
                }
                state.set_message(success.summary);
                saw_progress = false;
                commit_history = false;
                complete_active_task(state, task_kind, TaskStatus::Completed);
                finished = true;
            }
            EngineWorkerMessage::Failed(error) => {
                state.set_message(format!("{engine_name} failed: {error}"));
                complete_active_task(state, task_kind, TaskStatus::Failed);
                finished = true;
            }
        }
    }

    if !finished {
        state.optimization_origin = before;
        state.jobs.set_engine(running);
        ctx.request_repaint_after(engine_poll_frame());
    } else {
        if commit_history || saw_progress {
            if let Some(before) = before.take() {
                state.history.push_undo(before);
            }
        } else {
            before.take();
        }
        // A completed build adds/edits an entry; persist that result (debounced).
        if state.entries_fingerprint() != fingerprint_before {
            let now = ctx.input(|input| input.time);
            state.request_autosave(now, AUTOSAVE_DEBOUNCE_SECS);
        }
        ctx.request_repaint();
    }
}

pub(crate) fn poll_optimization_job(state: &mut AppState, ctx: &egui::Context) {
    let Some(mut running) = state.jobs.take_optimizer() else {
        return;
    };
    let mut before = state.optimization_origin.take();
    let fingerprint_before = state.entries_fingerprint();

    if ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
        running
            .cancel
            .store(true, std::sync::atomic::Ordering::Relaxed);
        state.set_message(match running.latest_report {
            Some(report) => format!(
                "forcefield optimization stopping: energy {:.3} -> {:.3} in {} steps",
                report.initial_energy, report.final_energy, report.steps
            ),
            None => "forcefield optimization stopping".to_string(),
        });
    }

    let mut finished = false;
    let mut saw_progress = false;
    let mut commit_history = false;
    while let Ok(message) = running.receiver.try_recv() {
        match message {
            OptimizationWorkerMessage::Progress { structure, report } => {
                *state.structure_mut() = structure;
                state.mark_structure_changed();
                running.latest_report = Some(report);
                if running.energy_trace.is_empty() {
                    running
                        .energy_trace
                        .push([0.0, f64::from(report.initial_energy)]);
                }
                running
                    .energy_trace
                    .push([report.steps as f64, f64::from(report.final_energy)]);
                saw_progress = true;
                state.set_source_path(None);
                state.set_message(format!(
                    "forcefield optimizing: step {}, energy {:.3}; press Esc to stop",
                    report.steps, report.final_energy
                ));
            }
            OptimizationWorkerMessage::Finished { structure, report } => {
                *state.structure_mut() = structure;
                state.mark_structure_changed();
                running.latest_report = Some(report);
                saw_progress = true;
                commit_history = true;
                state.set_source_path(None);
                state.set_message(optimization_finished_message(report));
                complete_active_task(state, TaskKind::OptimizeGeometry, TaskStatus::Completed);
                complete_active_task(
                    state,
                    TaskKind::OptimizeCrystalGeometry,
                    TaskStatus::Completed,
                );
                finished = true;
            }
            OptimizationWorkerMessage::Failed(error) => {
                state.set_message(format!("forcefield optimization failed: {error}"));
                complete_active_task(state, TaskKind::OptimizeGeometry, TaskStatus::Failed);
                complete_active_task(state, TaskKind::OptimizeCrystalGeometry, TaskStatus::Failed);
                finished = true;
            }
        }
    }

    if !finished {
        state.optimization_origin = before;
        state.jobs.set_optimizer(running);
        request_next_optimization_poll(ctx);
    } else {
        if commit_history || saw_progress {
            if let Some(before) = before.take() {
                state.history.push_undo(before);
            }
        } else if let Some(before) = before.take() {
            state.restore_edit_snapshot(before);
        }
        // Persist the finished (or reverted) geometry once, not per step.
        if state.entries_fingerprint() != fingerprint_before {
            let now = ctx.input(|input| input.time);
            state.request_autosave(now, AUTOSAVE_DEBOUNCE_SECS);
        }
        ctx.request_repaint();
    }
}

pub(crate) fn poll_qm_job(state: &mut AppState, ctx: &egui::Context) {
    let Some(mut running) = state.jobs.take_qm() else {
        return;
    };
    let task_run_id = state.active_task_run;
    let fingerprint_before = state.entries_fingerprint();

    if ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
        running.cancel_requested = true;
        running.cancel.cancel();
        if let Some(task_run_id) = task_run_id {
            mark_task_status(state, task_run_id, TaskStatus::Cancelling);
        }
        state.set_message("QM calculation stopping".to_string());
    }

    let mut finished = false;
    let mut changed = false;
    while let Ok(message) = running.receiver.try_recv() {
        match message {
            QmWorkerMessage::Progress { stage } => {
                state.set_message(format!("QM: {stage}; press Esc to stop"));
                running.latest_stage = Some(stage);
            }
            QmWorkerMessage::Finished(outcome) => {
                if running.cancel_requested {
                    state.set_message("QM calculation cancelled".to_string());
                    complete_active_qm_task(state, TaskStatus::Cancelled);
                    finished = true;
                    continue;
                }
                let outcome = *outcome;
                for line in outcome.summary.lines() {
                    state.output_log.push(line.to_string());
                }
                // Persist the raw report to the task's run directory before any
                // new entry is added, so the run's source entry is the input
                // structure, not the optimized result.
                save_qm_run_artifacts(state, &outcome);
                // A QM run is a heavy calculation; its optimized geometry is
                // surfaced as a new entry (the original structure is preserved),
                // matching the convention for entry-producing tasks.
                if let Some(optimized) = outcome.optimized_structure {
                    let save_path = structure_io::default_structure_save_path(&optimized, None);
                    let entry_id = add_and_show_entry(state, optimized, None, save_path);
                    if let Some(task_run_id) = task_run_id {
                        record_task_result_entry(state, task_run_id, entry_id);
                    }
                    set_qm_run_origin(state, entry_id);
                    changed = true;
                }
                // The run just became the newest QM result of whichever entry it
                // anchors to — the new geometry, or the input structure when there
                // is none. Either way the memoized per-entry chart availability is
                // now stale.
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
                complete_active_qm_task(state, TaskStatus::Completed);
                finished = true;
            }
            QmWorkerMessage::Failed(error) => {
                if running.cancel_requested {
                    state.set_message("QM calculation cancelled".to_string());
                    complete_active_qm_task(state, TaskStatus::Cancelled);
                } else {
                    state.set_message(format!("QM calculation failed: {error}"));
                    complete_active_qm_task(state, TaskStatus::Failed);
                }
                finished = true;
            }
        }
    }

    if !finished {
        state.jobs.set_qm(running);
        request_next_optimization_poll(ctx);
    } else {
        // An optimization adds a new entry; persist it once (debounced).
        if changed && state.entries_fingerprint() != fingerprint_before {
            let now = ctx.input(|input| input.time);
            state.request_autosave(now, AUTOSAVE_DEBOUNCE_SECS);
        }
        ctx.request_repaint();
    }
}
