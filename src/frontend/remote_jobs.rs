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

use crate::engines::qm::QmJob;

/// The durable registry fields a successful submission produces.
pub struct RemoteQmSubmitted {
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

/// Result of an off-thread detached remote QM submission. The success payload is
/// boxed — it is far larger than the failure string.
pub enum RemoteQmSubmitOutcome {
    Submitted(Box<RemoteQmSubmitted>),
    Failed(String),
}

/// In-flight detached remote QM submission. The blocking deploy + SSH staging +
/// launch run off the UI thread; the dispatcher drains the result and records it.
pub struct RunningRemoteQmSubmit {
    pub task_run_id: Option<u64>,
    pub receiver: Receiver<RemoteQmSubmitOutcome>,
}

/// Deploy the worker (fail-closed, version-pinned), stage `request.json` + a
/// `run.sh` bundle, and launch the job detached, off the UI thread. The handle's
/// fields become the `jobs.db` row; the job then runs without the app attached.
#[allow(clippy::too_many_arguments)]
pub fn spawn_remote_qm_submit(
    host: crate::backend::config::RemoteHost,
    job: QmJob,
    cores: Option<usize>,
    run_uuid: String,
    task_run_id: Option<u64>,
    job_kind: String,
    project_root: Option<String>,
    local_run_dir: PathBuf,
) -> RunningRemoteQmSubmit {
    use crate::engines::remote::launcher::{self, Launcher};
    use crate::engines::remote::{self, RemoteTarget, deploy};
    use crate::wire::{Engine, EngineRequest};

    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let result = (|| -> anyhow::Result<RemoteQmSubmitOutcome> {
            let target = RemoteTarget::for_run(&host, &run_uuid);
            let deployed = deploy::ensure_worker_deployed(&host, &target)?;
            std::fs::create_dir_all(&local_run_dir)?;
            let request = EngineRequest::with_cores(Engine::Qm(job), cores);
            let json = serde_json::to_vec(&request)?;
            std::fs::write(local_run_dir.join(launcher::REQUEST_FILE), json)?;
            remote::write_run_record(&target, &local_run_dir);
            let handle = Launcher::Direct.submit(&target, &local_run_dir, &deployed.remote_path)?;
            Ok(RemoteQmSubmitOutcome::Submitted(Box::new(
                RemoteQmSubmitted {
                    run_uuid: run_uuid.clone(),
                    host_id: host.id.clone(),
                    host_label: host.label.clone(),
                    remote_dir: target.remote_dir.clone(),
                    scheduler: Launcher::Direct.token().to_string(),
                    launch_handle: handle.0,
                    engine_id: crate::engines::registry::EngineId::HARTREE
                        .as_str()
                        .to_string(),
                    job_kind,
                    project_root,
                    local_run_dir: local_run_dir.clone(),
                    deployed_version: deployed.version,
                },
            )))
        })();
        let _ = sender
            .send(result.unwrap_or_else(|error| RemoteQmSubmitOutcome::Failed(error.to_string())));
    });
    RunningRemoteQmSubmit {
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
        });

        let run_uuid = uuid::Uuid::new_v4().to_string();
        let local_run_dir = std::env::temp_dir().join(format!("sl-frontend-{run_uuid}"));
        let submit = spawn_remote_qm_submit(
            host.clone(),
            job,
            Some(1),
            run_uuid.clone(),
            None,
            "qm-energy".to_string(),
            None,
            local_run_dir.clone(),
        );
        let submitted = match submit.receiver.recv().expect("submit worker stays alive") {
            RemoteQmSubmitOutcome::Submitted(submitted) => *submitted,
            RemoteQmSubmitOutcome::Failed(error) => panic!("remote submit failed: {error}"),
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

        let crate::wire::EngineOutcome::Qm(outcome) = outcome;
        let _ = std::fs::remove_dir_all(&local_run_dir);
        assert!(outcome.converged, "remote QM did not converge");
    }
}
