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
    resources: crate::backend::config::JobResources,
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
    let project = state.workspace.project();
    let project_root = project.map(|project| project.root.to_string_lossy().to_string());
    let project_id = project
        .and_then(|project| project.project_id.as_ref())
        .map(|id| id.as_str().to_string());
    // The registry row is keyed by this job execution's minted JobId — the durable
    // identity the reverse bridges resolve back to a task through the run graph, no
    // longer the task's `run_uuid`.
    let job_id = begin_job_execution(
        state,
        task_run_id,
        crate::backend::run_attempt::Placement::Remote {
            host: Some(host.label.clone()),
        },
        Some(task.controller_id.to_string()),
    );
    let handle = crate::frontend::remote_jobs::spawn_remote_submit(
        host.clone(),
        engine,
        resources,
        job_id.to_string(),
        Some(task_run_id),
        task.controller_id.to_string(),
        project_id,
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
/// row in `jobs.db`, persist the worker deployment identity on the host (so the
/// next run can verify its cache entry), and mark the task running or failed.
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
                cluster,
                engine_id,
                job_kind,
                project_id,
                project_root,
                local_run_dir,
                deployment_id,
                initial_phase,
                detected_launches,
            } = *submitted;
            if let Some(host) = state.config.remote_hosts.get_mut(&host_id) {
                host.worker_deployment = Some(deployment_id);
                // Cache launches the submission had to probe, so the next one skips
                // the SSH round trip. `cache_detected` never clobbers a configured
                // launch — and a probe only ran because none was configured. A probe
                // verifies what it finds, so its version is a proof of that launch.
                for detected in detected_launches {
                    host.engines
                        .cache_detected(detected.engine, detected.launch, detected.version);
                }
                if let Err(error) = save_config(&state.config) {
                    state.output_log.push(format!(
                        "could not persist worker deployment identity: {error}"
                    ));
                }
            }
            let row = registry::RemoteJob {
                job_id: run_uuid,
                host_id,
                host_label: host_label.clone(),
                remote_dir,
                scheduler,
                launch_handle,
                cluster,
                engine_id,
                job_kind,
                project_id,
                project_root_hint: project_root,
                local_run_dir: local_run_dir.to_string_lossy().to_string(),
                status: phase_status(initial_phase),
                submitted_at_ms: registry::now_ms(),
                last_polled_at_ms: None,
                exit_code: None,
                scheduler_state: None,
                reason: None,
                console_offset: 0,
                unknown_since_ms: None,
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
    let Some(refresh) = state.jobs.remote_jobs_refresh.take() else {
        return;
    };
    match refresh.receiver.try_recv() {
        Ok(updates) => {
            let conn = registry::open().ok();
            for update in updates {
                apply_remote_observation(state, conn.as_ref(), &update.run_uuid, update.outcome);
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

/// Persist one observation of a remote job and apply whatever it settles: a
/// retrieved outcome, a terminal task status, or a logged probe error. The
/// refresh drain and the cancel drain share this so both advance the console
/// offset, record the scheduler state, and materialize a result identically —
/// a job that finishes while its cancellation is in flight still lands.
/// Returns the status written to the registry, if one was.
fn apply_remote_observation(
    state: &mut AppState,
    conn: Option<&rusqlite::Connection>,
    run_uuid: &str,
    outcome: crate::frontend::remote_jobs::RemoteJobOutcome,
) -> Option<registry::RemoteJobStatus> {
    use crate::frontend::remote_jobs::RemoteJobOutcome;
    let prior = conn.and_then(|conn| registry::get(conn, run_uuid).ok().flatten());
    let observation = match &outcome {
        RemoteJobOutcome::Observed(observation)
        | RemoteJobOutcome::Done(_, observation)
        | RemoteJobOutcome::OutcomeUnreadable(_, observation) => Some(observation),
        RemoteJobOutcome::ProbeError(_) => None,
    };
    let mut recorded = None;
    if let (Some(conn), Some(prior), Some(observation)) = (conn, prior.as_ref(), observation) {
        let now = registry::now_ms();
        let (status, unknown_since_ms) = observed_status(prior, observation.phase, now);
        let _ = registry::record_observation(
            conn,
            run_uuid,
            registry::RemoteObservationUpdate {
                status,
                exit_code: observation.exit_code.map(i64::from),
                scheduler_state: observation.scheduler_state.as_deref(),
                reason: observation.reason.as_deref(),
                console_offset: observation.console.next_offset,
                unknown_since_ms,
                polled_at_ms: now,
            },
        );
        recorded = Some(status);
    }
    match outcome {
        RemoteJobOutcome::Done(engine_outcome, _) => {
            if let Some(row) = conn.and_then(|conn| registry::get(conn, run_uuid).ok().flatten()) {
                apply_remote_outcome(state, &row, *engine_outcome);
            }
        }
        RemoteJobOutcome::OutcomeUnreadable(error, _) => {
            mark_remote_task(state, run_uuid, TaskStatus::Failed);
            state.output_log.push(format!(
                "remote job {} finished but its outcome was unreadable: {error}",
                short_uuid(run_uuid)
            ));
        }
        RemoteJobOutcome::ProbeError(error) => state.output_log.push(format!(
            "remote job {} probe error: {error}",
            short_uuid(run_uuid)
        )),
        RemoteJobOutcome::Observed(observation) => match observation.phase {
            crate::engines::remote::launcher::RemoteJobPhase::Failed
            | crate::engines::remote::launcher::RemoteJobPhase::Lost => {
                mark_remote_task(state, run_uuid, TaskStatus::Failed);
            }
            crate::engines::remote::launcher::RemoteJobPhase::Cancelled => {
                mark_remote_task(state, run_uuid, TaskStatus::Cancelled);
            }
            _ => {}
        },
    }
    recorded
}

pub(crate) fn poll_remote_cancel(state: &mut AppState, ctx: &egui::Context) {
    let Some(cancel) = state.jobs.remote_cancel.take() else {
        return;
    };
    match cancel.receiver.try_recv() {
        Ok(Ok(outcome)) => {
            let conn = registry::open().ok();
            match apply_remote_observation(state, conn.as_ref(), &cancel.run_uuid, outcome) {
                Some(registry::RemoteJobStatus::Cancelled) => {
                    state.set_message("Remote cancellation confirmed".to_string());
                }
                Some(status) => {
                    state.set_message(format!("Remote job finished as {}", status.token()));
                }
                None => state.set_message("Remote job state could not be recorded".to_string()),
            }
            reload_remote_jobs(state);
        }
        Ok(Err(error)) => {
            state.set_message(format!("Remote cancellation failed: {error}"));
        }
        Err(std::sync::mpsc::TryRecvError::Empty) => {
            state.jobs.remote_cancel = Some(cancel);
            ctx.request_repaint_after(Duration::from_millis(400));
        }
        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
            state.set_message("Remote cancellation worker stopped unexpectedly".to_string());
        }
    }
}

fn phase_status(
    phase: crate::engines::remote::launcher::RemoteJobPhase,
) -> registry::RemoteJobStatus {
    use crate::engines::remote::launcher::RemoteJobPhase;
    match phase {
        RemoteJobPhase::Queued => registry::RemoteJobStatus::Queued,
        RemoteJobPhase::Cancelling => registry::RemoteJobStatus::Cancelling,
        RemoteJobPhase::Succeeded => registry::RemoteJobStatus::Done,
        RemoteJobPhase::Failed => registry::RemoteJobStatus::Failed,
        RemoteJobPhase::Cancelled => registry::RemoteJobStatus::Cancelled,
        RemoteJobPhase::Lost => registry::RemoteJobStatus::Lost,
        _ => registry::RemoteJobStatus::Running,
    }
}

fn observed_status(
    prior: &registry::RemoteJob,
    phase: crate::engines::remote::launcher::RemoteJobPhase,
    now_ms: i64,
) -> (registry::RemoteJobStatus, Option<i64>) {
    if phase != crate::engines::remote::launcher::RemoteJobPhase::Unknown {
        return (phase_status(phase), None);
    }
    let unknown_since = prior.unknown_since_ms.unwrap_or(now_ms);
    if now_ms.saturating_sub(unknown_since) >= 60_000 {
        (registry::RemoteJobStatus::Lost, Some(unknown_since))
    } else {
        (prior.status, Some(unknown_since))
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
/// case — a build submitted with no project open — whose result would otherwise be silently discarded.
pub(crate) fn outcome_belongs_to_current_workspace(
    state: &AppState,
    row: &registry::RemoteJob,
) -> bool {
    let current_project_id = state
        .workspace
        .project()
        .and_then(|project| project.project_id.as_ref())
        .map(|id| id.as_str().to_string());
    // `project_id` is the path-independent authority: a moved project keeps
    // its id, so ownership is decided by it whenever both the job and the open
    // project carry one. The path hint is a fallback only for the ownerless/scratch
    // case (a job or project predating ids), which preserves the `(None, None)`
    // scratch match.
    match (row.project_id.as_deref(), current_project_id.as_deref()) {
        (Some(job_project), Some(open_project)) => job_project == open_project,
        _ => {
            let current_root = state
                .workspace
                .project()
                .map(|project| project.root.to_string_lossy().to_string());
            project_root_matches(row.project_root_hint.as_deref(), current_root.as_deref())
        }
    }
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
    save_qm_artifacts(state, &run_dir, &outcome);

    let belongs_here = outcome_belongs_to_current_workspace(state, row);
    let already = outcome_already_materialized(state, &row.job_id);
    let task_id = state.tasks.runs.task_run_id_for_job(&row.job_id);

    if belongs_here && !already {
        match outcome.optimized_structure {
            Some(optimized) => {
                let save_path = structure_io::default_structure_save_path(&optimized, None);
                let entry_id = add_and_show_entry(state, optimized, None, save_path);
                if let Some(task_id) = task_id {
                    record_task_result_entry(state, task_id, entry_id);
                }
                set_qm_run_origin(state, entry_id);
                record_materialization(
                    state,
                    &row.job_id,
                    "optimized",
                    Some(entry_id),
                    &[entry_id],
                );
            }
            // A single-point energy or frequency run produces no entry; the ledger
            // still records that its outcome was applied, so it is idempotent.
            None => record_materialization(state, &row.job_id, "report", None, &[]),
        }
    }
    // The run is now the newest QM result of whichever entry it anchors to, so
    // the memoized per-entry chart availability is stale — including for a
    // single-point energy, whose result lands on its input structure.
    state.ui.chart_availability.clear();
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

fn mark_remote_task(state: &mut AppState, job_id: &str, status: TaskStatus) {
    if let Some(task_id) = state.tasks.runs.task_run_id_for_job(job_id) {
        mark_task_status(state, task_id, status);
    }
}

/// Open-project compensation: import any remote result that finished while a
/// *different* project was open. Every successfully-finished registry row owned by
/// the opened project (by its path-independent `project_id`) that is not yet in the
/// ledger is imported from its already-downloaded `outcome.json`. A missing or
/// unreadable file leaves the row unmaterialized — a pending recovery surfaced to
/// the user, never silently dropped — to be retried by a later remote refresh
/// rather than blocking open on an SSH round trip.
pub(crate) fn compensate_open_project(state: &mut AppState) {
    let Some(project_id) = state
        .workspace
        .project()
        .and_then(|project| project.project_id.as_ref())
        .map(|id| id.as_str().to_string())
    else {
        return;
    };
    let rows = (|| -> anyhow::Result<Vec<registry::RemoteJob>> {
        registry::list_completed_for_project_id(&registry::open()?, &project_id)
    })()
    .unwrap_or_default();
    import_completed_remote_jobs(state, rows);
}

/// Import each completed, not-yet-materialized remote row from its local
/// `outcome.json`, counting recoveries and pending-recovery rows. Split from
/// [`compensate_open_project`] so it is testable without the global registry: the
/// gate + local-outcome read + applier reuse is the whole policy.
pub(crate) fn import_completed_remote_jobs(state: &mut AppState, rows: Vec<registry::RemoteJob>) {
    let mut recovered = 0usize;
    let mut pending = 0usize;
    for row in rows {
        if outcome_already_materialized(state, &row.job_id) {
            continue;
        }
        match read_local_outcome(&row.local_run_dir) {
            Some(outcome) => {
                apply_remote_outcome(state, &row, outcome);
                recovered += 1;
            }
            None => {
                pending += 1;
                state.output_log.push(format!(
                    "remote job {} finished but its result is pending recovery \
                     (outcome file missing); it will retry on the next Refresh Remote",
                    short_uuid(&row.job_id)
                ));
            }
        }
    }
    if recovered > 0 {
        // Persist the imported entries + ledger atomically now rather than waiting
        // for the debounced autosave, so the recovery is durable from open.
        let _ = persist_project(state, false);
        state.set_message(format!(
            "Recovered {recovered} remote result(s) for this project"
        ));
    }
    if pending > 0 {
        state.set_message(format!(
            "{pending} remote result(s) pending recovery — use Refresh Remote to retry"
        ));
    }
}

/// Read and parse a completed remote job's already-downloaded `outcome.json` from
/// its local run directory. `None` when the file is absent or unreadable — the
/// pending-recovery signal, kept local (no SSH) so open never blocks on the network.
fn read_local_outcome(local_run_dir: &str) -> Option<crate::wire::EngineOutcome> {
    let path =
        std::path::Path::new(local_run_dir).join(crate::engines::remote::launcher::OUTCOME_FILE);
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Remove a remote job's scratch dir over SSH (a quick, bounded `rm -rf` of the
/// run's own UUID-suffixed dir) and drop its registry row.
pub(crate) fn remove_remote_job_scratch(state: &mut AppState, run_uuid: &str) {
    if state.jobs.remote_cleanup.is_some() {
        state.set_message("A remote cleanup is already running".to_string());
        return;
    }
    let row = (|| -> anyhow::Result<Option<registry::RemoteJob>> {
        registry::get(&registry::open()?, run_uuid)
    })()
    .ok()
    .flatten();
    let Some(row) = row else {
        return;
    };
    if !row.status.is_terminal() {
        state
            .set_message("Cancel the remote job before removing its scratch directory".to_string());
        return;
    }
    let Some(host) = state.config.remote_hosts.get(&row.host_id).cloned() else {
        state.set_message("The host for this job is no longer configured".to_string());
        return;
    };
    state.jobs.remote_cleanup = Some(crate::frontend::remote_jobs::spawn_remote_cleanup(
        row, host,
    ));
    state.set_message("Removing remote scratch…".to_string());
}

pub(crate) fn poll_remote_cleanup(state: &mut AppState, ctx: &egui::Context) {
    let Some(cleanup) = state.jobs.remote_cleanup.take() else {
        return;
    };
    match cleanup.receiver.try_recv() {
        Ok(Ok(())) => {
            if let Ok(conn) = registry::open() {
                let _ = registry::remove(&conn, &cleanup.run_uuid);
            }
            reload_remote_jobs(state);
            state.set_message("Removed the remote scratch directory".to_string());
        }
        Ok(Err(error)) => state.set_message(format!("Could not remove remote scratch: {error}")),
        Err(std::sync::mpsc::TryRecvError::Empty) => {
            state.jobs.remote_cleanup = Some(cleanup);
            ctx.request_repaint_after(Duration::from_millis(400));
        }
        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
            state.set_message("Remote cleanup worker stopped unexpectedly".to_string());
        }
    }
}

fn short_uuid(uuid: &str) -> &str {
    uuid.get(..8).unwrap_or(uuid)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::storage::jobs as registry;

    fn qm_remote_row(job_id: &str, run_dir: &std::path::Path) -> registry::RemoteJob {
        registry::RemoteJob {
            job_id: job_id.to_string(),
            host_id: "h".to_string(),
            host_label: "H".to_string(),
            remote_dir: "~/.silicolab/runs/x".to_string(),
            scheduler: "direct".to_string(),
            launch_handle: "1".to_string(),
            cluster: None,
            engine_id: "hartree".to_string(),
            job_kind: "qm-energy".to_string(),
            project_id: None,
            project_root_hint: None,
            local_run_dir: run_dir.to_string_lossy().to_string(),
            status: registry::RemoteJobStatus::Done,
            submitted_at_ms: 0,
            last_polled_at_ms: None,
            exit_code: None,
            scheduler_state: None,
            reason: None,
            console_offset: 0,
            unknown_since_ms: None,
        }
    }

    #[test]
    fn remote_qm_report_records_once_and_creates_no_entry() {
        // §9 import cardinality (0 entry): a single-point energy report creates no
        // entry but records a ledger row proving the outcome was applied, so a
        // repeated apply neither duplicates nor is treated as un-imported.
        let run_dir = std::path::PathBuf::from("target/test-qm-report-idempotency");
        let _ = std::fs::remove_dir_all(&run_dir);
        std::fs::create_dir_all(&run_dir).unwrap();
        let mut state = AppState::scratch(Default::default(), Vec::new());
        let row = qm_remote_row("job-qm", &run_dir);
        let outcome = crate::engines::qm::QmOutcome {
            energy_hartree: -1.5,
            converged: true,
            optimized_structure: None,
            summary: "energy -1.5 Eh".to_string(),
            scf_trace: Vec::new(),
            opt_trace: Vec::new(),
            frequencies: Vec::new(),
        };

        apply_remote_qm_outcome(&mut state, &row, outcome.clone());
        assert!(
            state.entries.records.is_empty(),
            "an energy report creates no entry"
        );
        let record = state
            .materializations
            .get("job-qm")
            .expect("the report is recorded in the ledger");
        assert!(record.primary_entry_id.is_none());
        assert!(record.entries.is_empty());

        apply_remote_qm_outcome(&mut state, &row, outcome);
        assert!(state.entries.records.is_empty());
        assert_eq!(state.materializations.len(), 1);
    }

    #[test]
    fn open_project_compensation_imports_present_outcome_and_flags_missing() {
        // §9 terminal compensation: a completed row whose outcome.json is on disk is
        // imported (ledger record); one whose file is gone stays unmaterialized —
        // surfaced as a pending recovery, never silently marked done.
        let root = std::path::PathBuf::from("target/test-compensation");
        let _ = std::fs::remove_dir_all(&root);
        let present_dir = root.join("present");
        std::fs::create_dir_all(&present_dir).unwrap();
        let outcome = crate::wire::EngineOutcome::Qm(crate::engines::qm::QmOutcome {
            energy_hartree: -2.0,
            converged: true,
            optimized_structure: None,
            summary: "energy -2.0 Eh".to_string(),
            scf_trace: Vec::new(),
            opt_trace: Vec::new(),
            frequencies: Vec::new(),
        });
        std::fs::write(
            present_dir.join(crate::engines::remote::launcher::OUTCOME_FILE),
            serde_json::to_vec(&outcome).unwrap(),
        )
        .unwrap();

        let mut state = AppState::scratch(Default::default(), Vec::new());
        let present = qm_remote_row("job-present", &present_dir);
        let missing = qm_remote_row("job-missing", &root.join("missing"));

        import_completed_remote_jobs(&mut state, vec![present, missing]);

        assert!(
            state.materializations.contains("job-present"),
            "the downloaded outcome is imported"
        );
        assert!(
            !state.materializations.contains("job-missing"),
            "a missing outcome stays pending, not marked done"
        );
        assert!(
            state
                .output_log
                .iter()
                .any(|line| line.contains("pending recovery")),
            "the pending recovery is surfaced, not silently dropped"
        );
    }

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
