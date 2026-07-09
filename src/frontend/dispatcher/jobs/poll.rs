use super::super::*;

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

/// Drain the latest utilization sample into state and keep frames coming while
/// the gauges are shown. Mirrors the other pollers' repaint-while-active pattern.
pub(crate) fn poll_metrics(state: &mut AppState, ctx: &egui::Context) {
    let Some(sampler) = state.jobs.metrics.as_ref() else {
        return;
    };
    while let Ok(metrics) = sampler.receiver.try_recv() {
        state.ui.cpu_pct = metrics.cpu_pct;
        state.ui.mem_pct = metrics.mem_pct;
        // Per-card (bus_id, util) for the history — one series per GPU, not an
        // average. Built before `metrics.gpus` is moved into state.
        let gpu_utils: Vec<(String, Option<f32>)> = metrics
            .gpus
            .iter()
            .map(|s| (s.pci_bus_id.clone(), s.util_pct))
            .collect();
        state.ui.gpus = metrics.gpus;
        state
            .ui
            .monitor_history
            .push(metrics.cpu_pct, metrics.mem_pct, &gpu_utils);
    }

    // Drive the sampler's cadence from the UI. Minimized → suspend entirely
    // (nothing on screen, so neither the sampler nor the GPU probe runs).
    // Unfocused but still on-screen → keep the gauges live but no faster than the
    // Low cadence, so a background window stays cheap without freezing. Pause
    // always stays paused. Repaint only while actively sampling, so a
    // paused/minimized monitor falls back to fully reactive painting.
    let chosen = crate::frontend::jobs::refresh_interval(state.config.monitor_refresh);
    let interval = if ctx.input(|i| i.viewport().minimized.unwrap_or(false)) {
        None
    } else if ctx.input(|i| i.focused) {
        chosen
    } else {
        // Throttle an unfocused (background) window to at most the Low cadence.
        let low =
            crate::frontend::jobs::refresh_interval(crate::backend::config::MonitorRefresh::Low);
        chosen.map(|c| c.max(low.unwrap_or(c)))
    };
    sampler.set_interval(interval);
    if let Some(dur) = interval {
        ctx.request_repaint_after(dur);
    }
}

/// Drain a finished remote hardware probe into the settings cache, or report the
/// error. Runs off the UI thread so a slow or dead host never blocks rendering.
pub(crate) fn poll_remote_hardware(state: &mut AppState, ctx: &egui::Context) {
    use crate::frontend::jobs::RemoteHardwareOutcome;

    let Some(fetch) = state.jobs.remote_hardware.take() else {
        return;
    };
    match fetch.receiver.try_recv() {
        Ok(RemoteHardwareOutcome::Ok(info)) => {
            state
                .ui
                .settings
                .remote_hardware
                .insert(fetch.host_id, info);
            state.set_message("Fetched remote hardware".to_string());
        }
        Ok(RemoteHardwareOutcome::Failed(error)) => {
            state.set_message(format!("Could not read remote hardware: {error}"));
        }
        Err(std::sync::mpsc::TryRecvError::Empty) => {
            state.jobs.remote_hardware = Some(fetch);
            ctx.request_repaint_after(std::time::Duration::from_millis(400));
        }
        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
            state.set_message("Remote hardware probe stopped unexpectedly".to_string());
        }
    }
}

/// Drain live remote-GPU samples into `remote_gpu_live`, then keep frames coming
/// while the monitor runs. The sampler thread polls the host every ~15 s; this just
/// applies whatever has arrived and requests a near-term repaint to render it.
pub(crate) fn poll_remote_gpu_monitor(state: &mut AppState, ctx: &egui::Context) {
    if state.jobs.remote_gpu_monitor.is_none() {
        return;
    }
    let mut disconnected = false;
    while let Some(monitor) = state.jobs.remote_gpu_monitor.as_ref() {
        match monitor.receiver.try_recv() {
            Ok(Ok(stats)) => {
                if let Some(live) = state.ui.settings.remote_gpu_live.as_mut() {
                    live.apply(stats);
                }
            }
            Ok(Err(error)) => {
                if let Some(live) = state.ui.settings.remote_gpu_live.as_mut() {
                    live.last_error = Some(error);
                }
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => break,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                disconnected = true;
                break;
            }
        }
    }
    if disconnected {
        state.jobs.remote_gpu_monitor = None;
        return;
    }
    ctx.request_repaint_after(std::time::Duration::from_millis(500));
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
    poll_metrics(state, ctx);
    poll_remote_probe(state, ctx);
    poll_remote_hardware(state, ctx);
    poll_remote_gpu_monitor(state, ctx);
    ensure_remote_jobs_loaded(state);
    poll_remote_submit(state, ctx);
    poll_remote_jobs_refresh(state, ctx);
    let assistant_before = (state.workspace.is_project()
        && (state.jobs.agent.is_some() || !state.jobs.agent_jobs.is_empty()))
    .then(|| state.assistant_fingerprint());
    crate::frontend::agent::poll_agent_turn(state, ctx);
    crate::frontend::agent::poll_agent_jobs(state, ctx);
    crate::frontend::agent::poll_model_fetch(state, ctx);
    if let Some(before) = assistant_before
        && state.assistant_fingerprint() != before
    {
        let now = ctx.input(|input| input.time);
        state.request_autosave(now, super::super::AUTOSAVE_DEBOUNCE_SECS);
    }
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
