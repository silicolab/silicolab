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
    pub engine_id: String,
    pub job_kind: String,
    pub project_root: Option<String>,
    pub local_run_dir: PathBuf,
    /// The exact worker deployment identity confirmed on the host.
    pub deployment_id: String,
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
    per_job.or(host.resources.cores).unwrap_or(fallback)
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
        crate::wire::Engine::Qm(_) => EngineId::HARTREE.as_str(),
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
    engine: crate::wire::Engine,
    cores: Option<usize>,
    run_uuid: String,
    task_run_id: Option<u64>,
    job_kind: String,
    project_root: Option<String>,
    local_run_dir: PathBuf,
) -> RunningRemoteSubmit {
    use crate::engines::remote::launcher::{self, Launcher};
    use crate::engines::remote::{self, RemoteTarget, deploy};
    use crate::wire::{Engine, EngineRequest};

    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let result = (|| -> anyhow::Result<RemoteSubmitOutcome> {
            let target = RemoteTarget::for_run(&host, &run_uuid);
            let engine_id = engine_id_token(&engine).to_string();
            // One probe feeds both the core clamp and the in-core memory budget.
            let inventory = probe_remote_inventory(&target);
            // Pre-flight an in-core QM ERI allocation against the REMOTE host's RAM,
            // before deploying, so an oversized job is refused client-side rather
            // than left to OOM mid-SCF on the node. Only in-core molecular QM has
            // this tensor; every other engine (and periodic QM) skips the guard,
            // as does a host whose RAM we could not probe (falling open).
            if let Engine::Qm(QmJob::Molecular(request)) = &engine
                && let Some(ram) = inventory.as_ref().and_then(|info| info.ram_bytes)
            {
                let verdict =
                    memory_verdict(request, crate::backend::hardware::qm_incore_budget_for(ram));
                if let Some(message) = remote_qm_memory_rejection(&verdict, &host.label) {
                    return Ok(RemoteSubmitOutcome::Failed(message));
                }
            }
            let deployed = deploy::ensure_worker_deployed(&host, &target)?;
            std::fs::create_dir_all(&local_run_dir)?;
            let cores = clamp_cores(cores, inventory.as_ref());
            let request = EngineRequest::with_cores(engine, cores);
            let json = serde_json::to_vec(&request)?;
            std::fs::write(local_run_dir.join(launcher::REQUEST_FILE), json)?;
            remote::write_run_record(&target, &local_run_dir);
            let handle = Launcher::Direct.submit(&target, &local_run_dir, &deployed.remote_path)?;
            Ok(RemoteSubmitOutcome::Submitted(Box::new(RemoteSubmitted {
                run_uuid: run_uuid.clone(),
                host_id: host.id.clone(),
                host_label: host.label.clone(),
                remote_dir: target.remote_dir.clone(),
                scheduler: Launcher::Direct.token().to_string(),
                launch_handle: handle.0,
                engine_id,
                job_kind,
                project_root,
                local_run_dir: local_run_dir.clone(),
                deployment_id: deployed.deployment_id,
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
    /// Still on the system.
    Running,
    /// Finished cleanly; the retrieved, parsed outcome. Boxed (it carries a full
    /// structure) to keep this enum small.
    Done(Box<crate::wire::EngineOutcome>),
    /// Exited non-zero on the host.
    FailedExit(i32),
    /// Gone without an exit code (node crash, OOM, external kill).
    Lost,
    /// Finished, its `outcome.json` retrieved, but the JSON could not be parsed.
    /// Terminal (the file will not become parseable on a retry) → marked failed.
    OutcomeUnreadable(String),
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
    use crate::engines::remote::launcher::{LaunchHandle, Launcher, Liveness, retrieve_outcome};

    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut updates = Vec::with_capacity(items.len());
        for (row, host) in items {
            let target = RemoteTarget::for_run(&host, &row.run_uuid);
            let handle = LaunchHandle(row.launch_handle.clone());
            // Only the Direct launcher exists.
            let outcome = match Launcher::Direct.poll(&target, &handle) {
                Ok((Liveness::Alive, _)) => RemoteJobOutcome::Running,
                Ok((Liveness::Lost, _)) => RemoteJobOutcome::Lost,
                Ok((Liveness::Done(0), _)) => {
                    match retrieve_outcome(&target, std::path::Path::new(&row.local_run_dir)) {
                        Ok(bytes) => {
                            match serde_json::from_slice::<crate::wire::EngineOutcome>(&bytes) {
                                Ok(outcome) => RemoteJobOutcome::Done(Box::new(outcome)),
                                // The job finished but its outcome is corrupt: a
                                // retry cannot fix it, so this is terminal.
                                Err(error) => RemoteJobOutcome::OutcomeUnreadable(format!(
                                    "parse outcome: {error}"
                                )),
                            }
                        }
                        // scp itself failed: transient, retry next refresh.
                        Err(error) => {
                            RemoteJobOutcome::ProbeError(format!("retrieve outcome: {error}"))
                        }
                    }
                }
                Ok((Liveness::Done(code), _)) => RemoteJobOutcome::FailedExit(code),
                Err(error) => RemoteJobOutcome::ProbeError(error.to_string()),
            };
            updates.push(RemoteJobRefreshUpdate {
                run_uuid: row.run_uuid,
                outcome,
            });
        }
        let _ = sender.send(updates);
    });
    RunningRemoteJobsRefresh { receiver }
}

#[cfg(test)]
mod tests;
