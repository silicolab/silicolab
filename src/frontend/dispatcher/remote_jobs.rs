//! Dispatcher glue for detached remote jobs: registry-backed, opt-in refresh
//! (never auto-polled). The off-thread submit/refresh themselves live in
//! `frontend::remote_jobs`; these functions drain those results on the UI thread,
//! update the global `jobs.db` registry, and apply finished outcomes to the
//! project — the established background-work → dispatcher flow.

use super::*;

use crate::backend::storage::jobs as registry;

/// One-shot reconnect read on the first frame: load the registry snapshot so a
/// reopened session shows its in-flight remote jobs before any manual refresh.
pub(crate) fn ensure_remote_jobs_loaded(state: &mut AppState) {
    if state.jobs.remote_jobs_loaded {
        return;
    }
    state.jobs.remote_jobs_loaded = true;
    reload_remote_jobs(state);
}

/// Refresh the UI's registry snapshot from `jobs.db` — the open project's rows
/// when a project is open, else the non-terminal set.
pub(crate) fn reload_remote_jobs(state: &mut AppState) {
    let rows = (|| -> anyhow::Result<Vec<registry::RemoteJob>> {
        let conn = registry::open()?;
        match state.workspace.project() {
            Some(project) => registry::list_for_project(&conn, &project.root.to_string_lossy()),
            None => registry::list_non_terminal(&conn),
        }
    })()
    .unwrap_or_default();
    state.ui.remote_jobs = rows;
}

/// Drain a finished detached-remote submission (any engine): record the durable
/// row in `jobs.db`, persist the deployed worker version on the host (so the next
/// run skips redeploy), and mark the task running or failed.
pub(crate) fn poll_remote_submit(state: &mut AppState, ctx: &egui::Context) {
    use crate::frontend::remote_jobs::RemoteSubmitOutcome;
    let Some(submit) = state.jobs.remote_submit.take() else {
        return;
    };
    match submit.receiver.try_recv() {
        Ok(RemoteSubmitOutcome::Submitted(submitted)) => {
            let crate::frontend::remote_jobs::RemoteSubmitted {
                run_uuid,
                host_id,
                host_label,
                remote_dir,
                scheduler,
                launch_handle,
                engine_id,
                job_kind,
                project_root,
                local_run_dir,
                deployed_version,
            } = *submitted;
            if let Some(host) = state.config.remote_hosts.get_mut(&host_id) {
                host.engine_versions.insert(
                    crate::engines::remote::deploy::WORKER_VERSION_KEY.to_string(),
                    deployed_version,
                );
                if let Err(error) = save_config(&state.config) {
                    state
                        .output_log
                        .push(format!("could not persist worker version: {error}"));
                }
            }
            let row = registry::RemoteJob {
                run_uuid,
                host_id,
                host_label: host_label.clone(),
                remote_dir,
                scheduler,
                launch_handle,
                engine_id,
                job_kind,
                project_root,
                local_run_dir: local_run_dir.to_string_lossy().to_string(),
                status: registry::RemoteJobStatus::Running,
                submitted_at_ms: registry::now_ms(),
                last_polled_at_ms: None,
                exit_code: None,
            };
            if let Err(error) = registry::open().and_then(|conn| registry::upsert(&conn, &row)) {
                state
                    .output_log
                    .push(format!("jobs.db write failed: {error}"));
            }
            reload_remote_jobs(state);
            state.set_message(format!(
                "Submitted to {host_label}; use Refresh Remote to check status"
            ));
        }
        Ok(RemoteSubmitOutcome::Failed(error)) => {
            if let Some(task_run_id) = submit.task_run_id {
                mark_task_status(state, task_run_id, TaskStatus::Failed);
            }
            state.set_message(format!("Remote submission failed: {error}"));
        }
        Err(std::sync::mpsc::TryRecvError::Empty) => {
            state.jobs.remote_submit = Some(submit);
            ctx.request_repaint_after(Duration::from_millis(400));
        }
        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
            if let Some(task_run_id) = submit.task_run_id {
                mark_task_status(state, task_run_id, TaskStatus::Failed);
            }
            state.set_message("Remote submission worker stopped unexpectedly".to_string());
        }
    }
}

/// Begin an opt-in refresh of all non-terminal remote jobs (liveness + retrieve),
/// off the UI thread. A no-op while one is already running.
pub(crate) fn refresh_remote_jobs(state: &mut AppState) {
    if state.jobs.remote_jobs_refresh.is_some() {
        return;
    }
    let rows = (|| -> anyhow::Result<Vec<registry::RemoteJob>> {
        registry::list_non_terminal(&registry::open()?)
    })()
    .unwrap_or_default();
    let items: Vec<_> = rows
        .into_iter()
        .filter_map(|row| {
            state
                .config
                .remote_hosts
                .get(&row.host_id)
                .cloned()
                .map(|host| (row, host))
        })
        .collect();
    reload_remote_jobs(state);
    if items.is_empty() {
        state.set_message("No active remote jobs to refresh".to_string());
        return;
    }
    state.jobs.remote_jobs_refresh = Some(crate::frontend::remote_jobs::spawn_remote_jobs_refresh(
        items,
    ));
    state.set_message("Refreshing remote jobs…".to_string());
}

/// Drain a finished remote-jobs refresh: update each row's status in `jobs.db`,
/// apply any retrieved QM outcome, and mark its task.
pub(crate) fn poll_remote_jobs_refresh(state: &mut AppState, ctx: &egui::Context) {
    use crate::frontend::remote_jobs::RemoteJobOutcome;
    let Some(refresh) = state.jobs.remote_jobs_refresh.take() else {
        return;
    };
    match refresh.receiver.try_recv() {
        Ok(updates) => {
            let conn = registry::open().ok();
            for update in updates {
                let (status, exit_code) = match &update.outcome {
                    RemoteJobOutcome::Running => (registry::RemoteJobStatus::Running, None),
                    RemoteJobOutcome::Done(_) => (registry::RemoteJobStatus::Done, Some(0i64)),
                    RemoteJobOutcome::FailedExit(code) => {
                        (registry::RemoteJobStatus::Failed, Some(*code as i64))
                    }
                    RemoteJobOutcome::Lost => (registry::RemoteJobStatus::Lost, None),
                    // The job finished with exit 0 but its outcome is unparseable:
                    // terminal failure, not a transient retry.
                    RemoteJobOutcome::OutcomeUnreadable(_) => {
                        (registry::RemoteJobStatus::Failed, Some(0i64))
                    }
                    RemoteJobOutcome::ProbeError(_) => (registry::RemoteJobStatus::Running, None),
                };
                // A transient probe error leaves the recorded status untouched.
                if !matches!(update.outcome, RemoteJobOutcome::ProbeError(_))
                    && let Some(conn) = conn.as_ref()
                {
                    let _ = registry::record_poll(
                        conn,
                        &update.run_uuid,
                        status,
                        exit_code,
                        registry::now_ms(),
                    );
                }
                let row = conn
                    .as_ref()
                    .and_then(|conn| registry::get(conn, &update.run_uuid).ok().flatten());
                match update.outcome {
                    RemoteJobOutcome::Done(outcome) => {
                        if let Some(row) = row {
                            apply_remote_outcome(state, &row, *outcome);
                        }
                    }
                    RemoteJobOutcome::FailedExit(code) => {
                        mark_remote_task(state, &update.run_uuid, TaskStatus::Failed);
                        state.output_log.push(format!(
                            "remote job {} exited with status {code}",
                            short_uuid(&update.run_uuid)
                        ));
                    }
                    RemoteJobOutcome::Lost => {
                        mark_remote_task(state, &update.run_uuid, TaskStatus::Failed);
                        state.output_log.push(format!(
                            "remote job {} was lost (no exit code)",
                            short_uuid(&update.run_uuid)
                        ));
                    }
                    RemoteJobOutcome::OutcomeUnreadable(error) => {
                        mark_remote_task(state, &update.run_uuid, TaskStatus::Failed);
                        state.output_log.push(format!(
                            "remote job {} finished but its outcome was unreadable: {error}",
                            short_uuid(&update.run_uuid)
                        ));
                    }
                    RemoteJobOutcome::ProbeError(error) => state.output_log.push(format!(
                        "remote job {} probe error: {error}",
                        short_uuid(&update.run_uuid)
                    )),
                    RemoteJobOutcome::Running => {}
                }
            }
            reload_remote_jobs(state);
            state.set_message("Remote jobs refreshed".to_string());
        }
        Err(std::sync::mpsc::TryRecvError::Empty) => {
            state.jobs.remote_jobs_refresh = Some(refresh);
            ctx.request_repaint_after(Duration::from_millis(400));
        }
        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
            state.set_message("Remote refresh worker stopped unexpectedly".to_string());
        }
    }
}

/// Apply a retrieved remote outcome to the open project, dispatched by engine. The
/// detached refresh round-trips a `wire::EngineOutcome`, so each engine's result is
/// handled by its own applier; a new engine adds its arm here.
fn apply_remote_outcome(
    state: &mut AppState,
    row: &registry::RemoteJob,
    outcome: crate::wire::EngineOutcome,
) {
    match outcome {
        crate::wire::EngineOutcome::Qm(qm) => apply_remote_qm_outcome(state, row, qm),
    }
}

/// Apply a retrieved remote QM outcome: log the report, save it beside the run,
/// add the optimized geometry as an entry (only when the job belongs to the open
/// project), and mark the task complete.
fn apply_remote_qm_outcome(
    state: &mut AppState,
    row: &registry::RemoteJob,
    outcome: crate::engines::qm::QmOutcome,
) {
    for line in outcome.summary.lines() {
        state.output_log.push(line.to_string());
    }
    let run_dir = PathBuf::from(&row.local_run_dir);
    let _ = std::fs::create_dir_all(&run_dir);
    let mut text = outcome.summary.clone();
    if !text.ends_with('\n') {
        text.push('\n');
    }
    let output_path = std::fs::write(run_dir.join(QM_OUTPUT_FILE), text)
        .ok()
        .map(|()| run_dir.join(QM_OUTPUT_FILE));

    let current_root = state
        .workspace
        .project()
        .map(|project| project.root.to_string_lossy().to_string());
    let same_project = row.project_root.is_some() && row.project_root == current_root;
    let task_id = state
        .tasks
        .task_run_by_uuid(&row.run_uuid)
        .map(|task| task.id);

    if same_project && let Some(optimized) = outcome.optimized_structure {
        let save_path = structure_io::default_structure_save_path(&optimized, None);
        let entry_id = add_and_show_entry(state, optimized, None, save_path);
        if let Some(task_id) = task_id {
            record_task_result_entry(state, task_id, entry_id);
        }
        set_qm_run_origin(state, entry_id, output_path);
    }
    if let Some(task_id) = task_id {
        mark_task_status(state, task_id, TaskStatus::Completed);
    }
    state.set_message(format!(
        "Remote QM complete: energy {:.6} Eh{}",
        outcome.energy_hartree,
        if outcome.converged {
            " (converged)"
        } else {
            " (not converged)"
        }
    ));
}

fn mark_remote_task(state: &mut AppState, run_uuid: &str, status: TaskStatus) {
    if let Some(task_id) = state.tasks.task_run_by_uuid(run_uuid).map(|task| task.id) {
        mark_task_status(state, task_id, status);
    }
}

/// Remove a remote job's scratch dir over SSH (a quick, bounded `rm -rf` of the
/// run's own UUID-suffixed dir) and drop its registry row.
pub(crate) fn remove_remote_job_scratch(state: &mut AppState, run_uuid: &str) {
    let row = (|| -> anyhow::Result<Option<registry::RemoteJob>> {
        registry::get(&registry::open()?, run_uuid)
    })()
    .ok()
    .flatten();
    let Some(row) = row else {
        return;
    };
    let Some(host) = state.config.remote_hosts.get(&row.host_id).cloned() else {
        state.set_message("The host for this job is no longer configured".to_string());
        return;
    };
    let target = crate::engines::remote::RemoteTarget::for_run(&host, &row.run_uuid);
    match crate::engines::remote::remove_remote_scratch(&target) {
        Ok(()) => {
            if let Ok(conn) = registry::open() {
                let _ = registry::remove(&conn, run_uuid);
            }
            reload_remote_jobs(state);
            state.set_message("Removed the remote scratch directory".to_string());
        }
        Err(error) => state.set_message(format!("Could not remove remote scratch: {error}")),
    }
}

fn short_uuid(uuid: &str) -> &str {
    uuid.get(..8).unwrap_or(uuid)
}
