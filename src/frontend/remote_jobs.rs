//! Detached remote compute jobs: the off-thread submit + opt-in refresh that
//! drive a job through the SSH launcher and the global `jobs.db` registry.
//!
//! These wrap the wire-agnostic launcher primitives in `engines::remote` so the
//! GUI never blocks on SSH: a submission deploys, stages, and launches a job
//! off-thread and returns the durable handle to record; a refresh probes
//! liveness and retrieves a finished `outcome.json`. The detached model (no
//! automatic polling) is what lets a remote run survive an app restart — the
//! dispatcher drains and applies these results in `dispatcher::jobs`.

use std::path::PathBuf;
use std::sync::mpsc::Receiver;

use crate::engines::qm::{MemoryVerdict, QmJob, memory_verdict};

/// The durable registry fields a successful submission produces.
pub struct RemoteSubmitted {
    pub run_uuid: String,
    pub host_id: String,
    pub host_label: String,
    pub remote_dir: String,
    pub scheduler: String,
    pub launch_handle: String,
    pub cluster: Option<String>,
    pub engine_id: String,
    pub job_kind: String,
    pub project_root: Option<String>,
    pub local_run_dir: PathBuf,
    /// The exact worker deployment identity confirmed on the host.
    pub deployment_id: String,
    pub initial_phase: crate::engines::remote::launcher::RemoteJobPhase,
    /// Engine launches this submission probed on the host because none was
    /// configured. The dispatcher caches them back onto the host so the next
    /// submission skips the SSH round trip.
    pub detected_launches: Vec<DetectedLaunch>,
}

/// An engine launch discovered on a host at submit time.
pub struct DetectedLaunch {
    pub engine: crate::engines::registry::EngineId,
    pub launch: crate::engines::registry::EngineLaunch,
    pub version: Option<String>,
}

/// Result of an off-thread detached remote submission. The success payload is
/// boxed — it is far larger than the failure string.
pub enum RemoteSubmitOutcome {
    Submitted(Box<RemoteSubmitted>),
    Failed(String),
}

/// In-flight detached remote submission. The blocking deploy + SSH staging +
/// launch run off the UI thread; the dispatcher drains the result and records it.
pub struct RunningRemoteSubmit {
    pub task_run_id: Option<u64>,
    pub receiver: Receiver<RemoteSubmitOutcome>,
}

/// Resolve the requested core count for a remote job: an explicit per-job override
/// wins, else the host's per-host default, else the app-wide core count. This is
/// the *requested* count; it is clamped to the remote inventory by [`clamp_cores`]
/// later, once the host has been probed ([`probe_remote_inventory`]).
pub(crate) fn resolve_requested_cores(
    per_job: Option<usize>,
    host: &crate::backend::config::RemoteHost,
    fallback: usize,
) -> usize {
    per_job
        .or(host.resources.cpus_per_task.map(|value| value as usize))
        .unwrap_or(fallback)
}

/// Clamp a requested core count to a remote host's probed inventory. Prefers the
/// logical thread count, falls back to physical cores, and passes the request
/// through (never below 1) when the probe found neither — an un-probeable host
/// runs at the requested count rather than being forced to a single thread.
fn clamp_to_remote_inventory(
    requested: usize,
    info: &crate::engines::remote::hardware::RemoteHardwareInfo,
) -> usize {
    match info.threads.or(info.cores) {
        Some(bound) => crate::backend::hardware::clamp_core_count(requested, bound),
        None => requested.max(1),
    }
}

/// Probe the remote host's CPU/RAM inventory over SSH, the single source for both
/// the core-count clamp and the in-core memory budget. `None` on a probe failure
/// (logged): callers fall open rather than block a job the deploy step in the same
/// closure already proved reachable, so a missing `lscpu`/`nproc`/`free` is not
/// fatal.
fn probe_remote_inventory(
    target: &crate::engines::remote::RemoteTarget,
) -> Option<crate::engines::remote::hardware::RemoteHardwareInfo> {
    use crate::engines::remote::hardware::{PROBE_SCRIPT, parse_remote_hardware};
    match crate::engines::remote::run_probe_command(
        target,
        PROBE_SCRIPT,
        std::time::Duration::from_secs(30),
    ) {
        Ok(stdout) => Some(parse_remote_hardware(&stdout)),
        Err(error) => {
            eprintln!("remote hardware probe failed; resource limits fall open: {error}");
            None
        }
    }
}

/// Clamp `requested` to the probed inventory before it is baked into `request.json`.
/// The worker trusts `cores` verbatim to size its thread pool, so an unclamped
/// laptop count would oversubscribe the node. An un-probed host (`None`) passes the
/// request through (never below 1).
fn clamp_cores(
    requested: Option<usize>,
    inventory: Option<&crate::engines::remote::hardware::RemoteHardwareInfo>,
) -> Option<usize> {
    let requested = requested?;
    Some(match inventory {
        Some(info) => clamp_to_remote_inventory(requested, info),
        None => requested.max(1),
    })
}

/// The client-side rejection message for an in-core QM job that would exceed the
/// remote host's RAM, naming the host and the way forward; `None` if it fits. This
/// pre-flights the verdict against the *target* host's budget (not the laptop's),
/// so an oversized job is refused before it wastes a remote allocation.
fn remote_qm_memory_rejection(verdict: &MemoryVerdict, host_label: &str) -> Option<String> {
    let detail = verdict.detail(host_label)?;
    let advice = match verdict {
        MemoryVerdict::ExceedsCanDirect { .. } => {
            " Switch the SCF backend to integral-direct to run the same single point with far less memory."
        }
        MemoryVerdict::ExceedsMustReduce { .. } => {
            " This calculation type needs in-core integrals — choose a smaller basis set or a smaller system."
        }
        MemoryVerdict::Ok => "",
    };
    Some(format!("{detail}{advice}"))
}

/// The stable `EngineId` token recorded for a wire engine — the `jobs.db`
/// `engine_id` for a submitted job. A new engine adds its arm here.
fn engine_id_token(engine: &crate::wire::Engine) -> &'static str {
    use crate::engines::registry::EngineId;
    match engine {
        crate::wire::Engine::Qm(job) => match job.engine {
            crate::engines::qm::QmEngine::Hartree => EngineId::HARTREE.as_str(),
            crate::engines::qm::QmEngine::Orca => EngineId::ORCA.as_str(),
        },
        crate::wire::Engine::Docking(_) => EngineId::DOCKING.as_str(),
        crate::wire::Engine::Gromacs(_) => EngineId::GROMACS.as_str(),
    }
}

/// Deploy the worker (fail-closed, version-pinned), stage `request.json` + a
/// `run.sh` bundle, and launch the job detached, off the UI thread. The handle's
/// fields become the `jobs.db` row; the job then runs without the app attached.
/// Engine-agnostic: the engine rides in `request.json` and is dispatched by the
/// worker, so the same path serves every built-in engine.
#[allow(clippy::too_many_arguments)]
pub fn spawn_remote_submit(
    host: crate::backend::config::RemoteHost,
    mut engine: crate::wire::Engine,
    resources: crate::backend::config::JobResources,
    run_uuid: String,
    task_run_id: Option<u64>,
    job_kind: String,
    project_root: Option<String>,
    local_run_dir: PathBuf,
) -> RunningRemoteSubmit {
    use crate::backend::engine_launch::{LaunchTarget, resolve_engine_launch};
    use crate::engines::remote::launcher::{self, Launcher};
    use crate::engines::remote::{self, RemoteTarget, deploy};
    use crate::wire::{Engine, EngineRequest};

    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let result = (|| -> anyhow::Result<RemoteSubmitOutcome> {
            let target = RemoteTarget::for_run(&host, &run_uuid);
            let engine_id = engine_id_token(&engine).to_string();
            let (launcher, profile) = match &host.scheduler {
                crate::backend::config::SchedulerConfig::Direct => (Launcher::Direct, None),
                crate::backend::config::SchedulerConfig::Slurm(profile) => {
                    (Launcher::Slurm, Some(profile.clone()))
                }
            };
            let mut resources = resources.resolved_with(&host.resources);
            let inventory = (launcher == Launcher::Direct)
                .then(|| probe_remote_inventory(&target))
                .flatten();
            // Pre-flight an in-core QM ERI allocation against the REMOTE host's RAM,
            // before deploying, so an oversized job is refused client-side rather
            // than left to OOM mid-SCF on the node. Only in-core molecular QM has
            // this tensor; every other engine (and periodic QM) skips the guard,
            // as does a host whose RAM we could not probe (falling open).
            if let Engine::Qm(QmJob {
                engine: crate::engines::qm::QmEngine::Hartree,
                calculation: crate::engines::qm::QmCalculation::Molecular(request),
            }) = &engine
                && let Some(ram) = resources
                    .memory_mib
                    .map(|mib| mib.saturating_mul(1024 * 1024))
                    .or_else(|| inventory.as_ref().and_then(|info| info.ram_bytes))
            {
                let verdict =
                    memory_verdict(request, crate::backend::hardware::qm_incore_budget_for(ram));
                if let Some(message) = remote_qm_memory_rejection(&verdict, &host.label) {
                    return Ok(RemoteSubmitOutcome::Failed(message));
                }
            }
            // Resolve how to launch every external engine this job needs, on THIS
            // host, before anything is deployed or uploaded: a host with no usable
            // engine should fail fast and cheaply. The resolved launches travel in
            // `request.json`, so the worker runs the binary the user configured
            // rather than whichever one it can find on the node.
            let mut launches = crate::engines::registry::EngineLaunches::new();
            let mut detected_launches = Vec::new();
            for id in engine.required_engines() {
                let resolved = resolve_engine_launch(LaunchTarget::Remote(&host), *id)?;
                if resolved.detected {
                    detected_launches.push(DetectedLaunch {
                        engine: *id,
                        launch: resolved.launch.clone(),
                        version: resolved.version,
                    });
                }
                launches.insert(*id, resolved.launch);
            }

            let deployed = deploy::ensure_worker_deployed(&host, &target)?;
            std::fs::create_dir_all(&local_run_dir)?;
            resources.cpus_per_task = clamp_cores(
                resources.cpus_per_task.map(|value| value as usize),
                inventory.as_ref(),
            )
            .map(|value| value as u32);
            if let Engine::Gromacs(crate::workflows::gromacs::GromacsJob::Run(request)) =
                &mut engine
            {
                request.resources.cores = resources.cpus_per_task.unwrap_or(0);
                request.resources.gpu = resources.gpu.count();
            }
            let request = EngineRequest::new(
                engine,
                resources.cpus_per_task.map(|value| value as usize),
                launches,
            )?;
            let json = serde_json::to_vec(&request)?;
            std::fs::write(local_run_dir.join(launcher::REQUEST_FILE), json)?;
            remote::write_run_record(&target, &local_run_dir, launcher, None, &resources);
            let handle = launcher.submit(
                &target,
                &local_run_dir,
                &deployed.remote_path,
                &resources,
                profile.as_ref(),
            )?;
            remote::write_run_record(&target, &local_run_dir, launcher, Some(&handle), &resources);
            Ok(RemoteSubmitOutcome::Submitted(Box::new(RemoteSubmitted {
                run_uuid: run_uuid.clone(),
                host_id: host.id.clone(),
                host_label: host.label.clone(),
                remote_dir: target.remote_dir.clone(),
                scheduler: launcher.token().to_string(),
                launch_handle: handle.id,
                cluster: handle.cluster,
                engine_id,
                job_kind,
                project_root,
                local_run_dir: local_run_dir.clone(),
                deployment_id: deployed.deployment_id,
                initial_phase: if launcher == Launcher::Slurm {
                    crate::engines::remote::launcher::RemoteJobPhase::Queued
                } else {
                    crate::engines::remote::launcher::RemoteJobPhase::Running
                },
                detected_launches,
            })))
        })();
        let _ = sender
            .send(result.unwrap_or_else(|error| RemoteSubmitOutcome::Failed(error.to_string())));
    });
    RunningRemoteSubmit {
        task_run_id,
        receiver,
    }
}

/// Per-job result of a remote-jobs refresh.
pub enum RemoteJobOutcome {
    Observed(crate::engines::remote::launcher::LauncherObservation),
    /// Finished cleanly; the retrieved, parsed outcome. Boxed (it carries a full
    /// structure) to keep this enum small.
    Done(
        Box<crate::wire::EngineOutcome>,
        crate::engines::remote::launcher::LauncherObservation,
    ),
    /// Finished, its `outcome.json` retrieved, but the JSON could not be parsed.
    /// Terminal (the file will not become parseable on a retry) → marked failed.
    OutcomeUnreadable(
        String,
        crate::engines::remote::launcher::LauncherObservation,
    ),
    /// A transient probe/transport error — the job's recorded status is left
    /// unchanged so a flaky network does not declare it lost.
    ProbeError(String),
}

pub struct RemoteJobRefreshUpdate {
    pub run_uuid: String,
    pub outcome: RemoteJobOutcome,
}

/// In-flight off-thread refresh of detached remote jobs.
pub struct RunningRemoteJobsRefresh {
    pub receiver: Receiver<Vec<RemoteJobRefreshUpdate>>,
}

pub struct RunningRemoteCancel {
    pub run_uuid: String,
    pub receiver: Receiver<Result<RemoteJobOutcome, String>>,
}

pub struct RunningRemoteCleanup {
    pub run_uuid: String,
    pub receiver: Receiver<Result<(), String>>,
}

pub fn spawn_remote_cleanup(
    row: crate::backend::storage::jobs::RemoteJob,
    host: crate::backend::config::RemoteHost,
) -> RunningRemoteCleanup {
    let (sender, receiver) = std::sync::mpsc::channel();
    let run_uuid = row.run_uuid.clone();
    std::thread::spawn(move || {
        let result = crate::engines::remote::RemoteTarget::from_remote_dir(&host, &row.remote_dir)
            .and_then(|target| crate::engines::remote::remove_remote_scratch(&target))
            .map_err(|error| error.to_string());
        let _ = sender.send(result);
    });
    RunningRemoteCleanup { run_uuid, receiver }
}

/// Cancel a detached remote job and wait for the launcher to confirm it. The
/// confirmation poll goes through [`observe_remote_job`], so a job that finishes
/// in the window between the request and the kill still has its `outcome.json`
/// retrieved rather than being discarded as "finished as done".
pub fn spawn_remote_cancel(
    row: crate::backend::storage::jobs::RemoteJob,
    host: crate::backend::config::RemoteHost,
) -> RunningRemoteCancel {
    use crate::engines::remote::RemoteTarget;
    use crate::engines::remote::launcher::{LaunchHandle, Launcher, RemoteJobPhase};
    let (sender, receiver) = std::sync::mpsc::channel();
    let run_uuid = row.run_uuid.clone();
    std::thread::spawn(move || {
        let result = (|| -> anyhow::Result<RemoteJobOutcome> {
            let target = RemoteTarget::from_remote_dir(&host, &row.remote_dir)?;
            let launcher = Launcher::from_token(&row.scheduler)?;
            let handle = LaunchHandle {
                id: row.launch_handle,
                cluster: row.cluster,
            };
            launcher.cancel(&target, &handle)?;
            let mut console_offset = row.console_offset;
            for _ in 0..60 {
                let outcome = observe_remote_job(
                    &target,
                    launcher,
                    &handle,
                    console_offset,
                    true,
                    &row.local_run_dir,
                );
                match &outcome {
                    RemoteJobOutcome::Observed(observation) => {
                        console_offset = observation.console.next_offset;
                        if matches!(
                            observation.phase,
                            RemoteJobPhase::Failed
                                | RemoteJobPhase::Cancelled
                                | RemoteJobPhase::Lost
                        ) {
                            return Ok(outcome);
                        }
                    }
                    // Transient: keep waiting on the same console offset.
                    RemoteJobOutcome::ProbeError(_) => {}
                    RemoteJobOutcome::Done(..) | RemoteJobOutcome::OutcomeUnreadable(..) => {
                        return Ok(outcome);
                    }
                }
                std::thread::sleep(std::time::Duration::from_secs(1));
            }
            anyhow::bail!("scheduler did not confirm cancellation within 60 seconds")
        })();
        let _ = sender.send(result.map_err(|error| error.to_string()));
    });
    RunningRemoteCancel { run_uuid, receiver }
}

/// Probe one remote job: read its phase and its new console bytes, and — when it
/// finished — retrieve and parse `outcome.json`. The console is appended locally
/// only on a path whose `next_offset` the caller will persist, so a failed
/// retrieval replays the same bytes next time instead of duplicating them.
fn observe_remote_job(
    target: &crate::engines::remote::RemoteTarget,
    launcher: crate::engines::remote::launcher::Launcher,
    handle: &crate::engines::remote::launcher::LaunchHandle,
    console_offset: u64,
    cancelling: bool,
    local_run_dir: &str,
) -> RemoteJobOutcome {
    use crate::engines::remote::launcher::{RemoteJobPhase, retrieve_outcome};
    match launcher.poll(target, handle, console_offset, cancelling) {
        Ok(observation) if observation.phase == RemoteJobPhase::Succeeded => {
            match retrieve_outcome(target, std::path::Path::new(local_run_dir)) {
                Ok(bytes) => {
                    append_console(local_run_dir, &observation.console.text);
                    match serde_json::from_slice::<crate::wire::EngineOutcome>(&bytes) {
                        Ok(outcome) => RemoteJobOutcome::Done(Box::new(outcome), observation),
                        // The job finished but its outcome is corrupt: a retry
                        // cannot fix it, so this is terminal.
                        Err(error) => RemoteJobOutcome::OutcomeUnreadable(
                            format!("parse outcome: {error}"),
                            observation,
                        ),
                    }
                }
                // scp itself failed: transient, retry next refresh.
                Err(error) => RemoteJobOutcome::ProbeError(format!("retrieve outcome: {error}")),
            }
        }
        Ok(observation) => {
            append_console(local_run_dir, &observation.console.text);
            RemoteJobOutcome::Observed(observation)
        }
        Err(error) => RemoteJobOutcome::ProbeError(error.to_string()),
    }
}

/// Probe each job's liveness over SSH and, when one has finished, retrieve and
/// parse its `outcome.json`, off the UI thread. A refresh reads `.exit` **and**
/// the process-group liveness, so a job that died without writing `.exit` is
/// reported [`RemoteJobOutcome::Lost`] rather than shown running forever.
pub fn spawn_remote_jobs_refresh(
    items: Vec<(
        crate::backend::storage::jobs::RemoteJob,
        crate::backend::config::RemoteHost,
    )>,
) -> RunningRemoteJobsRefresh {
    use crate::engines::remote::RemoteTarget;
    use crate::engines::remote::launcher::{LaunchHandle, Launcher};

    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut updates = Vec::with_capacity(items.len());
        for (row, host) in items {
            let target = match RemoteTarget::from_remote_dir(&host, &row.remote_dir) {
                Ok(target) => target,
                Err(error) => {
                    updates.push(RemoteJobRefreshUpdate {
                        run_uuid: row.run_uuid,
                        outcome: RemoteJobOutcome::ProbeError(error.to_string()),
                    });
                    continue;
                }
            };
            let launcher = match Launcher::from_token(&row.scheduler) {
                Ok(launcher) => launcher,
                Err(error) => {
                    updates.push(RemoteJobRefreshUpdate {
                        run_uuid: row.run_uuid,
                        outcome: RemoteJobOutcome::ProbeError(error.to_string()),
                    });
                    continue;
                }
            };
            let handle = LaunchHandle {
                id: row.launch_handle.clone(),
                cluster: row.cluster.clone(),
            };
            let outcome = observe_remote_job(
                &target,
                launcher,
                &handle,
                row.console_offset,
                row.status == crate::backend::storage::jobs::RemoteJobStatus::Cancelling,
                &row.local_run_dir,
            );
            updates.push(RemoteJobRefreshUpdate {
                run_uuid: row.run_uuid,
                outcome,
            });
        }
        let _ = sender.send(updates);
    });
    RunningRemoteJobsRefresh { receiver }
}

fn append_console(local_run_dir: &str, text: &str) {
    if text.is_empty() {
        return;
    }
    use std::io::Write;
    let path = std::path::Path::new(local_run_dir).join("run.console");
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = file.write_all(text.as_bytes());
    }
}

#[cfg(test)]
mod tests;
