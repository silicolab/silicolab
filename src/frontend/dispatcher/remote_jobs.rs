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

/// Launch a detached remote job for any built-in engine: verify SSH, create the
/// run directory, and submit — all off the UI thread. The durable handle is
/// recorded in the registry when the submission returns; status is tracked via the
/// opt-in refresh, so the run survives an app restart. The engine and its optional
/// core count ride in the request, so QM and docking share this one path.
pub(crate) fn start_remote_engine(
    state: &mut AppState,
    host: crate::backend::config::RemoteHost,
    engine: crate::wire::Engine,
    cores: Option<usize>,
) {
    let Some(task_run_id) = state.active_task_run else {
        state.set_message("no active task to run remotely".to_string());
        return;
    };
    let Some(task) = state.tasks.task_run(task_run_id).cloned() else {
        return;
    };
    let engine_name = remote_engine_name(&engine);
    if let Err(error) = crate::engines::remote::ensure_ssh_available() {
        state.set_message(format!("remote {engine_name} unavailable: {error}"));
        fail_active_task(state, task_run_id);
        return;
    }
    let local_run_dir = match ensure_active_task_run_dir(state, task.kind, None) {
        Ok(dir) => dir,
        Err(error) => {
            state.set_message(format!("could not create run directory: {error}"));
            fail_active_task(state, task_run_id);
            return;
        }
    };
    let project_root = state
        .workspace
        .project()
        .map(|project| project.root.to_string_lossy().to_string());
    let handle = crate::frontend::remote_jobs::spawn_remote_submit(
        host.clone(),
        engine,
        cores,
        task.run_uuid.clone(),
        Some(task_run_id),
        task.controller_id.to_string(),
        project_root,
        local_run_dir,
    );
    state.jobs.remote_submit = Some(handle);
    mark_task_status(state, task_run_id, TaskStatus::Running);
    state.set_message(format!(
        "Deploying & submitting {engine_name} to {} (use Refresh Remote to track it)…",
        host.label
    ));
}

/// Mark the active task failed and clear it — the engine-agnostic failure path for
/// a remote submission that never started.
fn fail_active_task(state: &mut AppState, task_run_id: u64) {
    mark_task_status(state, task_run_id, TaskStatus::Failed);
    state.active_task_run = None;
}

/// Short label for a wire engine, used in remote-submission status messages.
fn remote_engine_name(engine: &crate::wire::Engine) -> &'static str {
    match engine {
        crate::wire::Engine::Qm(_) => "QM",
        crate::wire::Engine::Docking(_) => "docking",
        crate::wire::Engine::Gromacs(_) => "GROMACS",
    }
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

/// Whether a retrieved remote job's result belongs in the **currently open
/// workspace**. A job's origin (`project_root`, captured at submit) must match
/// what is open now: an ownerless job — submitted from a scratch session, so
/// `project_root` is `None` — belongs to a scratch session (also `None`); an owned
/// job belongs only to its own project. When this is false the job was launched
/// from a *different* project, so its result is left untouched (open that project
/// and refresh) rather than dumped into an unrelated workspace.
///
/// This is the gate every engine's remote applier shares before it materializes a
/// result entry + run-dir artifacts. It deliberately admits the `(None, None)`
/// case: a build submitted with no project open used to be silently discarded.
pub(crate) fn outcome_belongs_to_current_workspace(
    state: &AppState,
    row: &registry::RemoteJob,
) -> bool {
    let current_root = state
        .workspace
        .project()
        .map(|project| project.root.to_string_lossy().to_string());
    project_root_matches(row.project_root.as_deref(), current_root.as_deref())
}

/// Pure core of [`outcome_belongs_to_current_workspace`], split out so the gate is
/// unit-testable without an `AppState`. `(None, None)` is the scratch-session case
/// — an ownerless job materializes into scratch — which the earlier `is_some()`
/// guard wrongly excluded, discarding the retrieved result.
fn project_root_matches(job_root: Option<&str>, current_root: Option<&str>) -> bool {
    job_root == current_root
}

/// Apply a retrieved remote outcome to the current workspace, dispatched by engine.
/// The detached refresh round-trips a `wire::EngineOutcome`, so each engine's result
/// is handled by its own applier; a new engine adds its arm here.
fn apply_remote_outcome(
    state: &mut AppState,
    row: &registry::RemoteJob,
    outcome: crate::wire::EngineOutcome,
) {
    match outcome {
        crate::wire::EngineOutcome::Qm(qm) => apply_remote_qm_outcome(state, row, qm),
        crate::wire::EngineOutcome::Docking(docking) => {
            apply_remote_docking_outcome(state, row, docking)
        }
        crate::wire::EngineOutcome::Gromacs(gromacs) => {
            apply_remote_gromacs_outcome(state, row, gromacs)
        }
    }
}

/// Apply a retrieved remote QM outcome: log the report, save it beside the run,
/// add the optimized geometry as an entry (only when the job belongs to the
/// current workspace — see [`outcome_belongs_to_current_workspace`]), and mark the
/// task complete.
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
    let series = crate::backend::runs::QmSeries::from_outcome(&outcome);
    if !series.is_empty()
        && let Err(error) = crate::backend::runs::save_qm_series_file(&run_dir, &series)
    {
        state
            .output_log
            .push(format!("failed to save QM series: {error}"));
    }

    let belongs_here = outcome_belongs_to_current_workspace(state, row);
    let task_id = state
        .tasks
        .task_run_by_uuid(&row.run_uuid)
        .map(|task| task.id);

    if belongs_here && let Some(optimized) = outcome.optimized_structure {
        let save_path = structure_io::default_structure_save_path(&optimized, None);
        let entry_id = add_and_show_entry(state, optimized, None, save_path);
        if let Some(task_id) = task_id {
            record_task_result_entry(state, task_id, entry_id);
        }
        set_qm_run_origin(state, entry_id, output_path);
        state.ui.chart_availability.remove(&entry_id);
    }
    if let Some(task_id) = task_id {
        mark_task_status(state, task_id, TaskStatus::Completed);
        state.ui.task_chart_thumbnails.remove(&task_id);
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

#[cfg(test)]
mod tests {
    use super::project_root_matches;

    #[test]
    fn ownerless_job_in_scratch_session_belongs_here() {
        // The reported regression: a build submitted with no project open (`None`)
        // and refreshed in a scratch session (`None`) was discarded by the old
        // `is_some()` guard. Equality of origins now materializes it into scratch.
        assert!(project_root_matches(None, None));
    }

    #[test]
    fn matching_project_belongs_here() {
        assert!(project_root_matches(Some("/work/a"), Some("/work/a")));
    }

    #[test]
    fn mismatched_origin_is_left_for_its_own_workspace() {
        // A different project open, an owned job with no project open, or an
        // ownerless job with a project open: none is dumped into the wrong place.
        assert!(!project_root_matches(Some("/work/a"), Some("/work/b")));
        assert!(!project_root_matches(Some("/work/a"), None));
        assert!(!project_root_matches(None, Some("/work/b")));
    }
}
