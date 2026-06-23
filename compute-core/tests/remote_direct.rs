//! End-to-end check of the Direct remote launcher against a real SSH host.
//!
//! `#[ignore]` — a developer-occasional test, not run by ordinary `cargo test`
//! or CI. To exercise it against a host (e.g. a local WSL) with the worker
//! pre-placed at `~/.silicolab/bin/silicolab-compute` and the app's dedicated key
//! authorized for passwordless login:
//!
//! ```text
//! SILICOLAB_TEST_SSH_HOST=<ip> SILICOLAB_TEST_SSH_USER=<user> \
//! cargo test -p compute-core --test remote_direct -- --ignored --nocapture
//! ```
//!
//! It asserts the remote worker's energy matches the in-process run to SCF
//! tolerance (parity is bounded, never bit-for-bit).

use compute_core::domain::{Atom, Structure};
use compute_core::engines::qm::{QmJob, QmKind, QmMethod, QmOptions, QmRequest};
use compute_core::engines::remote::RemoteTarget;
use compute_core::engines::remote::launcher::{Launcher, RemoteExecution};
use compute_core::hosts::RemoteHost;
use compute_core::wire::{
    Engine, EngineOutcome, EngineRequest, Executor, JobUpdate, Running, run_job,
};
use nalgebra::Point3;

fn h2_single_point() -> EngineRequest {
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
    EngineRequest::new(Engine::Qm(QmJob::Molecular(QmRequest {
        structure,
        method: QmMethod::Rhf,
        basis: "sto-3g".to_string(),
        charge: 0,
        multiplicity: 1,
        kind: QmKind::SinglePoint,
        options: QmOptions::default(),
        ts: None,
    })))
}

fn drain(running: Running) -> EngineOutcome {
    loop {
        match running.updates().recv().expect("worker stays alive") {
            JobUpdate::Finished(outcome) => return *outcome,
            JobUpdate::Failed(error) => panic!("job failed: {error}"),
            JobUpdate::Progress { stage } => eprintln!("progress: {stage}"),
        }
    }
}

#[test]
#[ignore = "requires an SSH host with a pre-placed worker (set SILICOLAB_TEST_SSH_HOST)"]
fn direct_remote_qm_matches_in_process_within_tolerance() {
    let Ok(hostname) = std::env::var("SILICOLAB_TEST_SSH_HOST") else {
        eprintln!("skip: set SILICOLAB_TEST_SSH_HOST to run the remote end-to-end test");
        return;
    };
    let username = std::env::var("SILICOLAB_TEST_SSH_USER").unwrap_or_else(|_| "root".to_string());
    let worker_path = std::env::var("SILICOLAB_TEST_WORKER")
        .unwrap_or_else(|_| "~/.silicolab/bin/silicolab-compute".to_string());

    // Baseline: the same job in-process.
    let EngineOutcome::Qm(local) = drain(run_job(h2_single_point(), Executor::InProcess)) else {
        panic!("expected a QM outcome");
    };
    assert!(local.converged, "in-process baseline did not converge");

    let host = RemoteHost {
        id: "test".to_string(),
        label: "Test".to_string(),
        hostname,
        username,
        port: 22,
        work_root: "~/.silicolab".to_string(),
        prelude: Vec::new(),
        engines: Default::default(),
        engine_versions: Default::default(),
        resources: Default::default(),
    };
    let run_uuid = uuid::Uuid::new_v4().to_string();
    let target = RemoteTarget::for_run(&host, &run_uuid);
    let working_dir = std::env::temp_dir().join(format!("sl-remote-direct-{run_uuid}"));
    let execution = RemoteExecution {
        target,
        launcher: Launcher::Direct,
        working_dir: working_dir.clone(),
        worker_path,
    };

    let EngineOutcome::Qm(remote) = drain(run_job(
        h2_single_point(),
        Executor::Remote(Box::new(execution)),
    )) else {
        panic!("expected a QM outcome");
    };
    let _ = std::fs::remove_dir_all(&working_dir);

    assert!(remote.converged, "remote run did not converge");
    assert!(
        (local.energy_hartree - remote.energy_hartree).abs() < 1e-6,
        "remote/in-process parity exceeded SCF tolerance: local {} vs remote {}",
        local.energy_hartree,
        remote.energy_hartree
    );
}
