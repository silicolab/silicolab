use super::*;

/// Drain the background GitHub release check. A found update is surfaced in
/// the status bar (message + persistent link); "up to date" and failures
/// (offline, rate-limited, no releases yet) are logged quietly to the Output
/// tab only — an automatic check must never nag.
pub(crate) fn poll_update_check(state: &mut AppState, ctx: &egui::Context) {
    let Some(check) = state.jobs.update_check.take() else {
        return;
    };
    match check.receiver.try_recv() {
        Ok(Ok(Some(update))) => {
            state.set_message(format!(
                "SilicoLab {} is available (you have {})",
                update.version,
                env!("CARGO_PKG_VERSION")
            ));
            state.ui.available_update = Some(update);
            // Honor the opt-in auto-install preference: a found update starts
            // downloading immediately when enabled (and writable), otherwise it
            // just waits for the one-click button in the title bar.
            maybe_auto_install_update(state);
        }
        Ok(Ok(None)) => {
            state
                .output_log
                .push("Update check: SilicoLab is up to date".to_string());
        }
        Ok(Err(error)) => {
            state
                .output_log
                .push(format!("Update check failed: {error}"));
        }
        Err(std::sync::mpsc::TryRecvError::Empty) => {
            // Request still in flight; keep the handle and look again shortly.
            state.jobs.update_check = Some(check);
            ctx.request_repaint_after(Duration::from_millis(500));
        }
        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
            state
                .output_log
                .push("Update check failed: worker stopped".to_string());
        }
    }
}

/// Drain the in-flight one-click self-update. On success the executable has
/// already been replaced on disk; we record the installed version so the title
/// bar can offer a restart. Failures surface in the status bar and reset the
/// status to `Failed` (the releases link remains as a manual fallback).
pub(crate) fn poll_self_update(state: &mut AppState, ctx: &egui::Context) {
    let Some(job) = state.jobs.self_update.take() else {
        return;
    };
    match job.receiver.try_recv() {
        Ok(Ok(version)) => {
            state.set_message(format!("SilicoLab {version} installed — restart to apply"));
            state.ui.self_update = SelfUpdateStatus::Installed { version };
        }
        Ok(Err(error)) => {
            state.set_message(format!("Update failed: {error}"));
            state
                .output_log
                .push(format!("Self-update failed: {error}"));
            state.ui.self_update = SelfUpdateStatus::Failed {
                error: error.to_string(),
            };
        }
        Err(std::sync::mpsc::TryRecvError::Empty) => {
            // Still downloading/replacing; keep the handle and poll again.
            state.jobs.self_update = Some(job);
            ctx.request_repaint_after(Duration::from_millis(500));
        }
        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
            state.set_message("Update failed: worker stopped".to_string());
            state.ui.self_update = SelfUpdateStatus::Failed {
                error: "worker stopped".to_string(),
            };
        }
    }
}

pub fn poll_jobs(state: &mut AppState, ctx: &egui::Context) {
    // Resolve the assistant key availability once (it reads env + the key store),
    // so the Assistant tab's per-frame render reads a cached flag instead.
    if state.ui.agent.key_available.is_none() {
        crate::frontend::agent::refresh_key_status(state);
    }
    poll_engine_job(state, ctx);
    poll_optimization_job(state, ctx);
    poll_disorder_job(state, ctx);
    poll_qm_job(state, ctx);
    poll_docking_job(state, ctx);
    poll_trajectory_jobs(state, ctx);
    poll_update_check(state, ctx);
    poll_self_update(state, ctx);
    poll_remote_probe(state, ctx);
    crate::frontend::agent::poll_agent_turn(state, ctx);
    crate::frontend::agent::poll_agent_heavy(state, ctx);
    crate::frontend::agent::poll_model_fetch(state, ctx);
}

/// Drain a finished Remote Hosts probe (passwordless check / GROMACS detect) and
/// apply it: connection status, or the detected engine launch + version cached
/// onto the host. Runs off the UI thread, so the panel stays responsive while a
/// slow or dead host is probed.
pub(crate) fn poll_remote_probe(state: &mut AppState, ctx: &egui::Context) {
    use crate::engines::registry::EngineLaunch;
    use crate::frontend::jobs::RemoteProbeOutcome;
    use crate::frontend::state::RemoteHostStatus;

    let Some(probe) = state.jobs.remote_probe.take() else {
        return;
    };
    match probe.receiver.try_recv() {
        Ok(RemoteProbeOutcome::Passwordless(true)) => {
            state
                .ui
                .settings
                .remote_status
                .insert(probe.host_id.clone(), RemoteHostStatus::Ready);
            // Clear a now-satisfied bootstrap prompt for this host.
            if matches!(&state.ui.settings.remote_bootstrap, Some((id, _)) if *id == probe.host_id)
            {
                state.ui.settings.remote_bootstrap = None;
            }
            state.set_message("Connected: passwordless login works".to_string());
        }
        Ok(RemoteProbeOutcome::Passwordless(false)) => {
            state
                .ui
                .settings
                .remote_status
                .insert(probe.host_id, RemoteHostStatus::NeedsSetup);
            state.set_message(
                "Reachable, but passwordless login isn't set up — use 'Set up passwordless login'."
                    .to_string(),
            );
        }
        Ok(RemoteProbeOutcome::Detected(Some((program, version)))) => {
            let key = EngineId::GROMACS.as_str().to_string();
            if let Some(host) = state.config.remote_hosts.get_mut(&probe.host_id) {
                host.engines
                    .insert(key.clone(), EngineLaunch::native(&program));
                host.engine_versions.insert(key, version.clone());
            }
            if let Some(draft) = state.ui.settings.remote_host_drafts.get_mut(&probe.host_id) {
                draft.gmx_program = program;
            }
            if let Err(error) = save_config(&state.config) {
                state.set_message(format!("Detected GROMACS, but could not save: {error}"));
            } else {
                state.set_message(format!("Detected GROMACS {version} on the remote host"));
            }
        }
        Ok(RemoteProbeOutcome::Detected(None)) => {
            state.set_message(
                "No GROMACS found on the remote host — set its path manually, or check the prelude."
                    .to_string(),
            );
        }
        Err(std::sync::mpsc::TryRecvError::Empty) => {
            state.jobs.remote_probe = Some(probe);
            ctx.request_repaint_after(Duration::from_millis(400));
        }
        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
            state.ui.settings.remote_status.insert(
                probe.host_id,
                RemoteHostStatus::Unreachable("probe worker stopped".to_string()),
            );
        }
    }
}

/// Drain a finished background trajectory decode (if any) into playback state,
/// then advance the playing frame on the wall clock.
pub(crate) fn poll_trajectory_jobs(state: &mut AppState, ctx: &egui::Context) {
    if let Some(load) = state.jobs.trajectory_load.take() {
        match load.receiver.try_recv() {
            Ok(Ok(trajectory)) => {
                if trajectory.is_empty() {
                    state.set_message("Trajectory contains no frames");
                } else {
                    let (view_center, view_radius) =
                        view_center_and_radius(&load.base_structure, load.include_cell);
                    let frames = trajectory.frame_count();
                    let subsampled = trajectory.stride() > 1;
                    let source_frames = trajectory.source_frame_count();
                    let now = ctx.input(|input| input.time);
                    let mut playback = TrajectoryPlayback {
                        entry_id: load.entry_id,
                        source: load.source,
                        trajectory,
                        scratch: load.base_structure,
                        current_frame: 0,
                        playing: true,
                        fps: DEFAULT_PLAYBACK_FPS,
                        last_advance_secs: now,
                        view_center,
                        view_radius,
                    };
                    playback.sync_scratch();
                    state.ui.trajectory = Some(playback);
                    if subsampled {
                        state.set_message(format!(
                            "Playing trajectory: {frames} of {source_frames} frames (subsampled)"
                        ));
                    } else {
                        state.set_message(format!("Playing trajectory: {frames} frames"));
                    }
                    ctx.request_repaint();
                }
            }
            Ok(Err(error)) => {
                state.set_message(format!("Trajectory load failed: {error}"));
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                // Still decoding; keep the handle and poll again next frame.
                state.jobs.trajectory_load = Some(load);
                ctx.request_repaint_after(engine_poll_frame());
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                state.set_message("Trajectory load failed: worker stopped");
            }
        }
    }

    advance_trajectory_playback(state, ctx);
}

/// While a trajectory is playing and its entry is active, step the current
/// frame forward on the wall clock at the configured rate.
pub(crate) fn advance_trajectory_playback(state: &mut AppState, ctx: &egui::Context) {
    let active_entry = state.entries.active_entry_id();
    let Some(playback) = state.ui.trajectory.as_mut() else {
        return;
    };
    if !playback.playing || active_entry != Some(playback.entry_id) || playback.frame_count() <= 1 {
        return;
    }
    let now = ctx.input(|input| input.time);
    let interval = 1.0 / playback.fps.max(1.0) as f64;
    if now - playback.last_advance_secs >= interval {
        playback.advance_frame();
        playback.last_advance_secs = now;
    }
    ctx.request_repaint_after(Duration::from_secs_f64(interval));
}

/// Begin (or resume) playback of one of an entry's MD-run trajectories. The
/// trajectory files live in the run directory and are decoded in the background;
/// this only kicks off the load (or resumes if that exact stage is already
/// loaded). `requested` selects a specific stage's trajectory (in the entry's
/// stored form); `None` plays the entry's default (production) trajectory.
pub(crate) fn load_trajectory(
    state: &mut AppState,
    entry_id: u64,
    requested: Option<PathBuf>,
    ctx: &egui::Context,
) {
    // Resolve which stage trajectory to play: the explicit request, else the
    // entry's recorded default.
    state.ensure_entry_loaded(entry_id);
    let Some(entry) = state.entries.entry(entry_id) else {
        return;
    };
    let relative =
        match requested.or_else(|| entry.origin.trajectory().map(|path| path.to_path_buf())) {
            Some(relative) => relative,
            None => {
                state.set_message("This entry has no trajectory to play");
                return;
            }
        };
    let base_structure = entry.structure.clone();

    // Already playing exactly this stage: just ensure it is running.
    if state
        .ui
        .trajectory
        .as_ref()
        .is_some_and(|p| p.entry_id == entry_id && p.source == relative)
    {
        if let Some(playback) = state.ui.trajectory.as_mut() {
            playback.playing = true;
            playback.last_advance_secs = ctx.input(|input| input.time);
        }
        ctx.request_repaint();
        return;
    }
    // Already decoding exactly this stage.
    if state
        .jobs
        .trajectory_load
        .as_ref()
        .is_some_and(|l| l.entry_id == entry_id && l.source == relative)
    {
        return;
    }

    let Some(project) = state.workspace.project() else {
        state.set_message("Trajectory playback requires an open project");
        return;
    };
    let absolute = project.root.join(&relative);
    if !absolute.exists() {
        state.set_message(format!(
            "Trajectory file is missing: {}",
            absolute.display()
        ));
        return;
    }

    let include_cell = state.ui.viewport.show_cell;
    // Drop any stale playback bound to a different entry or stage.
    state.ui.trajectory = None;
    state.jobs.trajectory_load = Some(spawn_trajectory_load(
        entry_id,
        relative,
        absolute,
        base_structure,
        include_cell,
    ));
    state.set_message("Loading trajectory…");
    ctx.request_repaint_after(engine_poll_frame());
}

pub(crate) fn toggle_trajectory_play(state: &mut AppState, ctx: &egui::Context) {
    if let Some(playback) = state.ui.trajectory.as_mut() {
        playback.playing = !playback.playing;
        playback.last_advance_secs = ctx.input(|input| input.time);
        ctx.request_repaint();
    }
}

pub(crate) fn set_trajectory_frame(state: &mut AppState, frame: usize) {
    if let Some(playback) = state.ui.trajectory.as_mut() {
        playback.set_frame(frame);
        // Scrubbing pauses playback so the chosen frame stays put.
        playback.playing = false;
    }
}

pub(crate) fn stop_trajectory(state: &mut AppState) {
    state.ui.trajectory = None;
    state.jobs.trajectory_load = None;
}

/// The provenance for an MD-run output entry: an [`EntryOrigin::MdRun`] carrying
/// the run's trajectory (when it wrote one) stored relative to the project root
/// so it survives the project being moved — absolute when the run directory
/// lives outside the project, and `None` when the run produced no trajectory.
///
/// The `MdRun` origin (not the trajectory) is what drives the "MD" badge, so a
/// run is marked even when it wrote no playable trajectory (e.g. a relax-only
/// run); playback is offered separately, only when `trajectory` is present.
pub(crate) fn md_run_origin(
    trajectory: Option<PathBuf>,
    project_root: Option<&Path>,
) -> EntryOrigin {
    let trajectory = trajectory.map(|path| match project_root {
        Some(root) => path
            .strip_prefix(root)
            .map(Path::to_path_buf)
            .unwrap_or(path),
        None => path,
    });
    EntryOrigin::MdRun { trajectory }
}

/// Mark an entry as the output of an MD run (provenance badge + playback gating).
pub(crate) fn set_md_run_origin(state: &mut AppState, entry_id: u64, trajectory: Option<PathBuf>) {
    let project_root = state
        .workspace
        .project()
        .map(|project| project.root.clone());
    let origin = md_run_origin(trajectory, project_root.as_deref());
    state.entries.set_entry_origin(entry_id, origin);
}

/// File name of the saved QM output report inside a QM task's run directory.
pub(crate) const QM_OUTPUT_FILE: &str = "output.txt";

/// Persist a finished QM calculation's output report to the active task's run
/// directory (creating it on demand) and return the written path. Failures are
/// logged to the Output tab and reported as `None` — they never abort result
/// handling, since the in-memory output log already holds the report.
fn save_qm_output(state: &mut AppState, summary: &str) -> Option<PathBuf> {
    let task_run_id = state.active_task_run?;
    let kind = state.tasks.task_run(task_run_id)?.kind;
    if !matches!(
        kind,
        TaskKind::RunQmEnergy | TaskKind::RunQmOptimize | TaskKind::RunQmFrequencies
    ) {
        return None;
    }
    let run_dir = match ensure_active_task_run_dir(state, kind, None) {
        Ok(run_dir) => run_dir,
        Err(error) => {
            state
                .output_log
                .push(format!("failed to create QM run directory: {error}"));
            return None;
        }
    };
    let path = run_dir.join(QM_OUTPUT_FILE);
    let mut text = summary.to_string();
    if !text.ends_with('\n') {
        text.push('\n');
    }
    match std::fs::write(&path, text) {
        Ok(()) => {
            state
                .output_log
                .push(format!("QM output saved to {}", path.display()));
            Some(path)
        }
        Err(error) => {
            state
                .output_log
                .push(format!("failed to save QM output: {error}"));
            None
        }
    }
}

/// Mark an entry as the output of a QM run. Like [`set_md_run_origin`], the
/// report path is stored relative to the project root so it survives the
/// project being moved; the badge tracks the origin, not the path, so the
/// entry is marked even when saving the report failed.
pub(crate) fn set_qm_run_origin(state: &mut AppState, entry_id: u64, output: Option<PathBuf>) {
    let project_root = state
        .workspace
        .project()
        .map(|project| project.root.clone());
    let output = output.map(|path| match project_root.as_deref() {
        Some(root) => path
            .strip_prefix(root)
            .map(Path::to_path_buf)
            .unwrap_or(path),
        None => path,
    });
    state
        .entries
        .set_entry_origin(entry_id, EntryOrigin::QmRun { output });
}

/// Open the saved QM output report of `entry_id` in the shared text viewer.
/// The report is read from disk on every open (it is small), so the viewer
/// never holds a stale copy and nothing extra is persisted in the project
/// database.
pub(crate) fn show_qm_output(state: &mut AppState, entry_id: u64) {
    let Some(entry) = state.entries.entry(entry_id) else {
        return;
    };
    let entry_name = entry.name.clone();
    let Some(relative) = entry.origin.qm_output().map(Path::to_path_buf) else {
        state.set_message("This entry has no saved QM output".to_string());
        return;
    };
    // Stored relative to the project root (absolute when the run directory
    // lives outside a project); `join` keeps an already-absolute path as-is.
    let absolute = match state.workspace.project() {
        Some(project) => project.root.join(&relative),
        None => relative,
    };
    match std::fs::read_to_string(&absolute) {
        Ok(text) => {
            state.ui.text_viewer = Some(crate::frontend::state::TextViewer {
                title: format!("QM Output — {entry_name}"),
                text,
            });
        }
        Err(error) => state.set_message(format!(
            "Could not read QM output {}: {error}",
            absolute.display()
        )),
    }
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
                // Mark MD-run output entries so the UI shows the "MD" badge and,
                // when the run wrote a trajectory, offers playback. The badge
                // tracks the job kind, not the trajectory, so a relax-only run
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
    let Some(running) = state.jobs.take_qm() else {
        return;
    };
    let task_run_id = state.active_task_run;
    let fingerprint_before = state.entries_fingerprint();

    if ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
        running
            .cancel
            .store(true, std::sync::atomic::Ordering::Relaxed);
        // hartree runs the calculation in one opaque call, so a cancel only takes
        // effect at the next stage boundary; the in-flight step runs to the end.
        state.set_message(
            "QM calculation stopping (the current step runs to completion)".to_string(),
        );
    }

    let mut finished = false;
    let mut changed = false;
    while let Ok(message) = running.receiver.try_recv() {
        match message {
            QmWorkerMessage::Progress { stage } => {
                state.set_message(format!("QM: {stage}; press Esc to stop"));
            }
            QmWorkerMessage::Finished(outcome) => {
                let outcome = *outcome;
                for line in outcome.summary.lines() {
                    state.output_log.push(line.to_string());
                }
                // Persist the raw report to the task's run directory before any
                // new entry is added, so the run's source entry is the input
                // structure, not the optimized result.
                let output_path = save_qm_output(state, &outcome.summary);
                // A QM run is a heavy calculation; its optimized geometry is
                // surfaced as a new entry (the original structure is preserved),
                // matching the convention for entry-producing tasks.
                if let Some(optimized) = outcome.optimized_structure {
                    let save_path = structure_io::default_structure_save_path(&optimized, None);
                    let entry_id = add_and_show_entry(state, optimized, None, save_path);
                    if let Some(task_run_id) = task_run_id {
                        record_task_result_entry(state, task_run_id, entry_id);
                    }
                    set_qm_run_origin(state, entry_id, output_path);
                    changed = true;
                }
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
                state.set_message(format!("QM calculation failed: {error}"));
                complete_active_qm_task(state, TaskStatus::Failed);
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
