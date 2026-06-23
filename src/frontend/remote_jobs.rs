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
    /// The worker version `ensure_worker_deployed` confirmed on the host.
    pub deployed_version: String,
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
                deployed_version: deployed.version,
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
mod tests {
    use super::*;

    fn host_with_cores(cores: Option<usize>) -> crate::backend::config::RemoteHost {
        use crate::backend::config::{RemoteHost, ResourceSpec};
        RemoteHost {
            id: "h".into(),
            label: "H".into(),
            hostname: "example.com".into(),
            username: "alice".into(),
            port: 22,
            work_root: "~/.silicolab".into(),
            prelude: Vec::new(),
            engines: Default::default(),
            engine_versions: Default::default(),
            resources: ResourceSpec {
                cores,
                ..Default::default()
            },
        }
    }

    #[test]
    fn requested_cores_precedence() {
        let host = host_with_cores(Some(4));
        assert_eq!(resolve_requested_cores(Some(2), &host, 16), 2); // per-job wins
        assert_eq!(resolve_requested_cores(None, &host, 16), 4); // then per-host
        let host = host_with_cores(None);
        assert_eq!(resolve_requested_cores(None, &host, 16), 16); // then fallback
    }

    #[test]
    fn clamp_prefers_threads_then_cores_then_passthrough() {
        use crate::engines::remote::hardware::RemoteHardwareInfo;
        let both = RemoteHardwareInfo {
            threads: Some(8),
            cores: Some(4),
            ..Default::default()
        };
        assert_eq!(clamp_to_remote_inventory(32, &both), 8); // clamp to logical threads
        assert_eq!(clamp_to_remote_inventory(2, &both), 2); // already under the bound
        let phys = RemoteHardwareInfo {
            threads: None,
            cores: Some(4),
            ..Default::default()
        };
        assert_eq!(clamp_to_remote_inventory(32, &phys), 4); // fall back to physical cores
        let none = RemoteHardwareInfo::default();
        assert_eq!(clamp_to_remote_inventory(32, &none), 32); // un-probeable → pass through
        assert_eq!(clamp_to_remote_inventory(0, &none), 1); // never below 1
    }

    #[test]
    fn remote_memory_rejection_names_host_and_advises() {
        let can_direct = MemoryVerdict::ExceedsCanDirect {
            estimate: 20_u64 << 30,
            budget: 16_u64 << 30,
        };
        let msg = remote_qm_memory_rejection(&can_direct, "cluster").expect("should reject");
        assert!(msg.contains("cluster"), "names the host: {msg}");
        assert!(
            msg.contains("integral-direct"),
            "offers the cheaper backend"
        );

        let must_reduce = MemoryVerdict::ExceedsMustReduce {
            estimate: 20_u64 << 30,
            budget: 16_u64 << 30,
        };
        let msg = remote_qm_memory_rejection(&must_reduce, "cluster").expect("should reject");
        assert!(msg.contains("cluster"));
        assert!(msg.contains("smaller"), "advises reducing the system");

        // A job that fits is not rejected.
        assert!(remote_qm_memory_rejection(&MemoryVerdict::Ok, "cluster").is_none());
    }

    /// End-to-end check of the detached frontend path (deploy fast-path → submit
    /// → opt-in refresh → retrieve) against a real SSH host. `#[ignore]`: a
    /// developer-occasional test requiring an SSH host (e.g. a local WSL) with
    /// the worker pre-placed at `~/.silicolab/bin/silicolab-compute` and
    /// passwordless login configured. Run with:
    ///
    /// ```text
    /// SILICOLAB_TEST_SSH_HOST=<ip> SILICOLAB_TEST_SSH_USER=<user> \
    /// cargo test -p silicolab --lib -- --ignored remote_qm_submit_then_refresh
    /// ```
    ///
    /// The host records the current worker version, so `ensure_worker_deployed`
    /// takes its no-network fast path (the GitHub asset only exists post-release).
    #[test]
    #[ignore = "requires an SSH host with a pre-placed worker (set SILICOLAB_TEST_SSH_HOST)"]
    fn remote_qm_submit_then_refresh_against_ssh_host() {
        use crate::backend::config::RemoteHost;
        use crate::backend::storage::jobs::{RemoteJob, RemoteJobStatus};
        use crate::domain::{Atom, Structure};
        use crate::engines::qm::{QmKind, QmMethod, QmOptions, QmRequest};
        use crate::engines::remote::deploy::WORKER_VERSION_KEY;
        use nalgebra::Point3;
        use std::time::Duration;

        let Ok(hostname) = std::env::var("SILICOLAB_TEST_SSH_HOST") else {
            eprintln!("skip: set SILICOLAB_TEST_SSH_HOST to run the remote frontend test");
            return;
        };
        let username =
            std::env::var("SILICOLAB_TEST_SSH_USER").unwrap_or_else(|_| "root".to_string());

        let mut engine_versions = std::collections::HashMap::new();
        engine_versions.insert(
            WORKER_VERSION_KEY.to_string(),
            env!("CARGO_PKG_VERSION").to_string(),
        );
        let host = RemoteHost {
            id: "wsl".to_string(),
            label: "WSL".to_string(),
            hostname,
            username,
            port: 22,
            work_root: "~/.silicolab".to_string(),
            prelude: Vec::new(),
            engines: Default::default(),
            engine_versions,
            resources: Default::default(),
        };

        let structure = Structure::new(
            "h2",
            vec![
                Atom {
                    element: "H".to_string(),
                    position: Point3::new(0.0, 0.0, 0.0),
                    charge: 0.0,
                },
                Atom {
                    element: "H".to_string(),
                    position: Point3::new(0.0, 0.0, 0.74),
                    charge: 0.0,
                },
            ],
        );
        let job = QmJob::Molecular(QmRequest {
            structure,
            method: QmMethod::Rhf,
            basis: "sto-3g".to_string(),
            charge: 0,
            multiplicity: 1,
            kind: QmKind::SinglePoint,
            options: QmOptions::default(),
            ts: None,
        });

        let run_uuid = uuid::Uuid::new_v4().to_string();
        let local_run_dir = std::env::temp_dir().join(format!("sl-frontend-{run_uuid}"));
        let submit = spawn_remote_submit(
            host.clone(),
            crate::wire::Engine::Qm(job),
            Some(1),
            run_uuid.clone(),
            None,
            "qm-energy".to_string(),
            None,
            local_run_dir.clone(),
        );
        let submitted = match submit.receiver.recv().expect("submit worker stays alive") {
            RemoteSubmitOutcome::Submitted(submitted) => *submitted,
            RemoteSubmitOutcome::Failed(error) => panic!("remote submit failed: {error}"),
        };

        let row = RemoteJob {
            run_uuid: submitted.run_uuid,
            host_id: submitted.host_id,
            host_label: submitted.host_label,
            remote_dir: submitted.remote_dir,
            scheduler: submitted.scheduler,
            launch_handle: submitted.launch_handle,
            engine_id: submitted.engine_id,
            job_kind: submitted.job_kind,
            project_root: submitted.project_root,
            local_run_dir: submitted.local_run_dir.to_string_lossy().to_string(),
            status: RemoteJobStatus::Running,
            submitted_at_ms: 0,
            last_polled_at_ms: None,
            exit_code: None,
        };

        // Opt-in refresh, retried until the detached job finishes.
        let outcome = loop {
            let refresh = spawn_remote_jobs_refresh(vec![(row.clone(), host.clone())]);
            let mut updates = refresh.receiver.recv().expect("refresh worker stays alive");
            match updates.pop().expect("one update per job").outcome {
                RemoteJobOutcome::Done(outcome) => break *outcome,
                RemoteJobOutcome::Running => std::thread::sleep(Duration::from_millis(500)),
                RemoteJobOutcome::FailedExit(code) => panic!("remote job exited {code}"),
                RemoteJobOutcome::Lost => panic!("remote job was lost"),
                RemoteJobOutcome::OutcomeUnreadable(error) => {
                    panic!("outcome unreadable: {error}")
                }
                RemoteJobOutcome::ProbeError(error) => panic!("probe error: {error}"),
            }
        };

        let crate::wire::EngineOutcome::Qm(outcome) = outcome else {
            panic!("expected a QM outcome");
        };
        let _ = std::fs::remove_dir_all(&local_run_dir);
        assert!(outcome.converged, "remote QM did not converge");
    }

    /// The detached docking path against a real SSH host, mirroring the QM E2E
    /// above: submit a `ScoreOnly` job (one fast evaluation), refresh until it
    /// finishes, and assert a pose came back through the payload bridge. `#[ignore]`
    /// for the same reason — it needs a host with a pre-placed worker. Run with:
    ///
    /// ```text
    /// SILICOLAB_TEST_SSH_HOST=<ip> SILICOLAB_TEST_SSH_USER=<user> \
    /// cargo test -p silicolab --lib -- --ignored remote_docking_submit_then_refresh
    /// ```
    #[test]
    #[ignore = "requires an SSH host with a pre-placed worker (set SILICOLAB_TEST_SSH_HOST)"]
    fn remote_docking_submit_then_refresh_against_ssh_host() {
        use crate::backend::config::RemoteHost;
        use crate::backend::storage::jobs::{RemoteJob, RemoteJobStatus};
        use crate::domain::{Atom, Bond, BondType, Structure};
        use crate::engines::docking::{DockingConfig, DockingInput, DockingKind, DockingRequest};
        use crate::engines::remote::deploy::WORKER_VERSION_KEY;
        use nalgebra::Point3;
        use std::time::Duration;

        let Ok(hostname) = std::env::var("SILICOLAB_TEST_SSH_HOST") else {
            eprintln!("skip: set SILICOLAB_TEST_SSH_HOST to run the remote docking test");
            return;
        };
        let username =
            std::env::var("SILICOLAB_TEST_SSH_USER").unwrap_or_else(|_| "root".to_string());

        let mut engine_versions = std::collections::HashMap::new();
        engine_versions.insert(
            WORKER_VERSION_KEY.to_string(),
            env!("CARGO_PKG_VERSION").to_string(),
        );
        let host = RemoteHost {
            id: "wsl".to_string(),
            label: "WSL".to_string(),
            hostname,
            username,
            port: 22,
            work_root: "~/.silicolab".to_string(),
            prelude: Vec::new(),
            engines: Default::default(),
            engine_versions,
            resources: Default::default(),
        };

        let carbon = |x: f32, y: f32, z: f32| Atom {
            element: "C".to_string(),
            position: Point3::new(x, y, z),
            charge: 0.0,
        };
        let skeleton = || {
            Structure::with_bonds(
                "butane",
                vec![
                    carbon(0.0, 0.0, 0.0),
                    carbon(1.5, 0.0, 0.0),
                    carbon(2.2, 1.3, 0.0),
                    carbon(3.7, 1.3, 0.0),
                ],
                vec![
                    Bond::with_type(0, 1, BondType::Single),
                    Bond::with_type(1, 2, BondType::Single),
                    Bond::with_type(2, 3, BondType::Single),
                ],
            )
        };
        let request = DockingRequest {
            receptor: DockingInput::Structure(Box::new(skeleton())),
            ligand: DockingInput::Structure(Box::new(skeleton())),
            box_center: [1.8, 0.6, 0.0],
            box_size: [20.0, 20.0, 20.0],
            config: DockingConfig::default(),
            kind: DockingKind::ScoreOnly,
        };

        let run_uuid = uuid::Uuid::new_v4().to_string();
        let local_run_dir = std::env::temp_dir().join(format!("sl-frontend-dock-{run_uuid}"));
        let submit = spawn_remote_submit(
            host.clone(),
            crate::wire::Engine::Docking(request),
            None,
            run_uuid.clone(),
            None,
            "dock".to_string(),
            None,
            local_run_dir.clone(),
        );
        let submitted = match submit.receiver.recv().expect("submit worker stays alive") {
            RemoteSubmitOutcome::Submitted(submitted) => *submitted,
            RemoteSubmitOutcome::Failed(error) => panic!("remote docking submit failed: {error}"),
        };

        let row = RemoteJob {
            run_uuid: submitted.run_uuid,
            host_id: submitted.host_id,
            host_label: submitted.host_label,
            remote_dir: submitted.remote_dir,
            scheduler: submitted.scheduler,
            launch_handle: submitted.launch_handle,
            engine_id: submitted.engine_id,
            job_kind: submitted.job_kind,
            project_root: submitted.project_root,
            local_run_dir: submitted.local_run_dir.to_string_lossy().to_string(),
            status: RemoteJobStatus::Running,
            submitted_at_ms: 0,
            last_polled_at_ms: None,
            exit_code: None,
        };

        let outcome = loop {
            let refresh = spawn_remote_jobs_refresh(vec![(row.clone(), host.clone())]);
            let mut updates = refresh.receiver.recv().expect("refresh worker stays alive");
            match updates.pop().expect("one update per job").outcome {
                RemoteJobOutcome::Done(outcome) => break *outcome,
                RemoteJobOutcome::Running => std::thread::sleep(Duration::from_millis(500)),
                RemoteJobOutcome::FailedExit(code) => panic!("remote job exited {code}"),
                RemoteJobOutcome::Lost => panic!("remote job was lost"),
                RemoteJobOutcome::OutcomeUnreadable(error) => {
                    panic!("outcome unreadable: {error}")
                }
                RemoteJobOutcome::ProbeError(error) => panic!("probe error: {error}"),
            }
        };

        let crate::wire::EngineOutcome::Docking(outcome) = outcome else {
            panic!("expected a docking outcome");
        };
        let _ = std::fs::remove_dir_all(&local_run_dir);
        assert_eq!(outcome.poses.len(), 1, "ScoreOnly returns one pose");
        assert!(outcome.poses[0].affinity.is_finite());
    }

    /// The detached GROMACS relay against a real SSH host with GROMACS installed:
    /// submit a tiny single-stage `gmx` Run (energy-minimize a hermetic 8-atom
    /// argon box with an inline topology), let the worker run the whole pipeline in
    /// one allocation, then refresh until it finishes and assert the structure +
    /// stage report came back in `EngineOutcome::Gromacs`. `#[ignore]` — it needs a
    /// host with a pre-placed worker AND a working `gmx`. Set the optional
    /// `SILICOLAB_TEST_GMX_PRELUDE` to a shell line (e.g. `. /usr/local/gromacs/bin/GMXRC`)
    /// when `gmx` needs its environment sourced first. Run with:
    ///
    /// ```text
    /// SILICOLAB_TEST_SSH_HOST=<ip> SILICOLAB_TEST_SSH_USER=<user> \
    /// cargo test -p silicolab --lib -- --ignored remote_gromacs_submit_then_refresh
    /// ```
    #[test]
    #[ignore = "requires an SSH host with a pre-placed worker and a working gmx (set SILICOLAB_TEST_SSH_HOST)"]
    fn remote_gromacs_submit_then_refresh_against_ssh_host() {
        use crate::backend::config::RemoteHost;
        use crate::backend::storage::jobs::{RemoteJob, RemoteJobStatus};
        use crate::domain::{Atom, Structure, UnitCell};
        use crate::engines::gromacs::{MdpSettings, StageLinks, StageSpec};
        use crate::engines::remote::deploy::WORKER_VERSION_KEY;
        use crate::workflows::gromacs::{GromacsJob, GromacsRunRequest, WireTopology};
        use nalgebra::Point3;
        use std::time::Duration;

        // A hermetic argon topology: Lennard-Jones only, no external force-field
        // data, eight single-atom `AR` molecules matching the eight box atoms.
        const ARGON_TOP: &str = "\
[ defaults ]
1         2          no         1.0      1.0

[ atomtypes ]
  Ar    18      39.948    0.000   A      0.34050   0.99600

[ moleculetype ]
  AR    1

[ atoms ]
  1    Ar    1      AR       Ar    1     0.000   39.948

[ system ]
Argon

[ molecules ]
AR  8
";

        let Ok(hostname) = std::env::var("SILICOLAB_TEST_SSH_HOST") else {
            eprintln!("skip: set SILICOLAB_TEST_SSH_HOST to run the remote GROMACS test");
            return;
        };
        let username =
            std::env::var("SILICOLAB_TEST_SSH_USER").unwrap_or_else(|_| "root".to_string());
        let prelude = std::env::var("SILICOLAB_TEST_GMX_PRELUDE")
            .ok()
            .map(|line| vec![line])
            .unwrap_or_default();

        let mut engine_versions = std::collections::HashMap::new();
        engine_versions.insert(
            WORKER_VERSION_KEY.to_string(),
            env!("CARGO_PKG_VERSION").to_string(),
        );
        let host = RemoteHost {
            id: "wsl".to_string(),
            label: "WSL".to_string(),
            hostname,
            username,
            port: 22,
            work_root: "~/.silicolab".to_string(),
            prelude,
            engines: Default::default(),
            engine_versions,
            resources: Default::default(),
        };

        // A 2×2×2 argon grid centered in a 30 Å cubic cell — finite starting energy,
        // box well over twice the 1 nm cutoff.
        let mut atoms = Vec::with_capacity(8);
        for x in [10.0_f32, 15.0] {
            for y in [10.0_f32, 15.0] {
                for z in [10.0_f32, 15.0] {
                    atoms.push(Atom {
                        element: "Ar".to_string(),
                        position: Point3::new(x, y, z),
                        charge: 0.0,
                    });
                }
            }
        }
        let structure = Structure::with_cell(
            "argon",
            atoms,
            UnitCell::from_parameters(30.0, 30.0, 30.0, 90.0, 90.0, 90.0),
        );
        let job = GromacsJob::Run(GromacsRunRequest {
            structure,
            topology: WireTopology {
                top: ARGON_TOP.to_string(),
                includes: Vec::new(),
            },
            stages: vec![StageSpec {
                stage_name: "em".to_string(),
                settings: MdpSettings::energy_minimization(),
                links: StageLinks::from_prepared(),
            }],
            max_duration_per_stage: Duration::from_secs(120),
            freeze: None,
            resources: Default::default(),
        });

        let run_uuid = uuid::Uuid::new_v4().to_string();
        let local_run_dir = std::env::temp_dir().join(format!("sl-frontend-gmx-{run_uuid}"));
        let submit = spawn_remote_submit(
            host.clone(),
            crate::wire::Engine::Gromacs(job),
            None,
            run_uuid.clone(),
            None,
            "run-md".to_string(),
            None,
            local_run_dir.clone(),
        );
        let submitted = match submit.receiver.recv().expect("submit worker stays alive") {
            RemoteSubmitOutcome::Submitted(submitted) => *submitted,
            RemoteSubmitOutcome::Failed(error) => panic!("remote GROMACS submit failed: {error}"),
        };

        let row = RemoteJob {
            run_uuid: submitted.run_uuid,
            host_id: submitted.host_id,
            host_label: submitted.host_label,
            remote_dir: submitted.remote_dir,
            scheduler: submitted.scheduler,
            launch_handle: submitted.launch_handle,
            engine_id: submitted.engine_id,
            job_kind: submitted.job_kind,
            project_root: submitted.project_root,
            local_run_dir: submitted.local_run_dir.to_string_lossy().to_string(),
            status: RemoteJobStatus::Running,
            submitted_at_ms: 0,
            last_polled_at_ms: None,
            exit_code: None,
        };

        let outcome = loop {
            let refresh = spawn_remote_jobs_refresh(vec![(row.clone(), host.clone())]);
            let mut updates = refresh.receiver.recv().expect("refresh worker stays alive");
            match updates.pop().expect("one update per job").outcome {
                RemoteJobOutcome::Done(outcome) => break *outcome,
                RemoteJobOutcome::Running => std::thread::sleep(Duration::from_millis(500)),
                RemoteJobOutcome::FailedExit(code) => panic!("remote job exited {code}"),
                RemoteJobOutcome::Lost => panic!("remote job was lost"),
                RemoteJobOutcome::OutcomeUnreadable(error) => {
                    panic!("outcome unreadable: {error}")
                }
                RemoteJobOutcome::ProbeError(error) => panic!("probe error: {error}"),
            }
        };

        let crate::wire::EngineOutcome::Gromacs(outcome) = outcome else {
            panic!("expected a GROMACS outcome");
        };
        let _ = std::fs::remove_dir_all(&local_run_dir);
        assert_eq!(
            outcome.structure.atoms.len(),
            8,
            "the relayed run preserves all argon atoms"
        );
        assert_eq!(outcome.stages.len(), 1, "one stage was relayed");
        assert!(
            outcome.stages[0].final_potential_energy.is_some(),
            "energy minimization reports a final potential energy"
        );
    }
}
