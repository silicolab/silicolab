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
            cpus_per_task: cores.map(|value| value as u32),
            ..Default::default()
        },
        scheduler: Default::default(),
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

/// End-to-end check of the detached frontend path (deploy → submit → opt-in
/// refresh → retrieve) against a real SSH host. `#[ignore]`: a
/// developer-occasional test requiring an SSH host (e.g. a local WSL) with
/// passwordless login configured. Build the current worker first, then run:
///
/// ```text
/// cargo xtask build-dev-worker
/// SILICOLAB_TEST_SSH_HOST=<ip> SILICOLAB_TEST_SSH_USER=<user> \
/// cargo test -p silicolab --features dev-worker --lib -- --ignored remote_qm_submit_then_refresh
/// ```
#[cfg(feature = "dev-worker")]
#[test]
#[ignore = "requires an SSH host (set SILICOLAB_TEST_SSH_HOST)"]
fn remote_qm_submit_then_refresh_against_ssh_host() {
    use crate::backend::config::RemoteHost;
    use crate::backend::storage::jobs::{RemoteJob, RemoteJobStatus};
    use crate::domain::{Atom, Structure};
    use crate::engines::qm::{QmKind, QmMethod, QmOptions, QmRequest};
    use nalgebra::Point3;
    use std::time::Duration;

    let Ok(hostname) = std::env::var("SILICOLAB_TEST_SSH_HOST") else {
        eprintln!("skip: set SILICOLAB_TEST_SSH_HOST to run the remote frontend test");
        return;
    };
    let username = std::env::var("SILICOLAB_TEST_SSH_USER").unwrap_or_else(|_| "root".to_string());

    let host = RemoteHost {
        id: "wsl".to_string(),
        label: "WSL".to_string(),
        hostname,
        username,
        port: 22,
        work_root: "~/.silicolab".to_string(),
        prelude: Vec::new(),
        engines: Default::default(),
        engine_versions: Default::default(),
        resources: Default::default(),
        scheduler: Default::default(),
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
        crate::backend::config::JobResources {
            cpus_per_task: Some(1),
            ..Default::default()
        },
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
    assert!(
        submitted.deployment_id.starts_with("dev:"),
        "the dev-worker test must never fall back to a release artifact"
    );

    let row = RemoteJob {
        run_uuid: submitted.run_uuid,
        host_id: submitted.host_id,
        host_label: submitted.host_label,
        remote_dir: submitted.remote_dir,
        scheduler: submitted.scheduler,
        launch_handle: submitted.launch_handle,
        cluster: submitted.cluster,
        engine_id: submitted.engine_id,
        job_kind: submitted.job_kind,
        project_root: submitted.project_root,
        local_run_dir: submitted.local_run_dir.to_string_lossy().to_string(),
        status: RemoteJobStatus::Running,
        submitted_at_ms: 0,
        last_polled_at_ms: None,
        exit_code: None,
        scheduler_state: None,
        reason: None,
        console_offset: 0,
        unknown_since_ms: None,
    };

    // Opt-in refresh, retried until the detached job finishes.
    let outcome = loop {
        let refresh = spawn_remote_jobs_refresh(vec![(row.clone(), host.clone())]);
        let mut updates = refresh.receiver.recv().expect("refresh worker stays alive");
        match updates.pop().expect("one update per job").outcome {
            RemoteJobOutcome::Done(outcome, _) => break *outcome,
            RemoteJobOutcome::Observed(observation)
                if !matches!(
                    observation.phase,
                    crate::engines::remote::launcher::RemoteJobPhase::Failed
                        | crate::engines::remote::launcher::RemoteJobPhase::Lost
                        | crate::engines::remote::launcher::RemoteJobPhase::Cancelled
                ) =>
            {
                std::thread::sleep(Duration::from_millis(500))
            }
            RemoteJobOutcome::Observed(observation) => {
                panic!("remote job ended as {:?}", observation.phase)
            }
            RemoteJobOutcome::OutcomeUnreadable(error, _) => {
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
/// requires an SSH host. Build the current worker first, then run:
///
/// ```text
/// cargo xtask build-dev-worker
/// SILICOLAB_TEST_SSH_HOST=<ip> SILICOLAB_TEST_SSH_USER=<user> \
/// cargo test -p silicolab --features dev-worker --lib -- --ignored remote_docking_submit_then_refresh
/// ```
#[cfg(feature = "dev-worker")]
#[test]
#[ignore = "requires an SSH host (set SILICOLAB_TEST_SSH_HOST)"]
fn remote_docking_submit_then_refresh_against_ssh_host() {
    use crate::backend::config::RemoteHost;
    use crate::backend::storage::jobs::{RemoteJob, RemoteJobStatus};
    use crate::domain::{Atom, Bond, BondType, Structure};
    use crate::engines::docking::{DockingConfig, DockingInput, DockingKind, DockingRequest};
    use nalgebra::Point3;
    use std::time::Duration;

    let Ok(hostname) = std::env::var("SILICOLAB_TEST_SSH_HOST") else {
        eprintln!("skip: set SILICOLAB_TEST_SSH_HOST to run the remote docking test");
        return;
    };
    let username = std::env::var("SILICOLAB_TEST_SSH_USER").unwrap_or_else(|_| "root".to_string());

    let host = RemoteHost {
        id: "wsl".to_string(),
        label: "WSL".to_string(),
        hostname,
        username,
        port: 22,
        work_root: "~/.silicolab".to_string(),
        prelude: Vec::new(),
        engines: Default::default(),
        engine_versions: Default::default(),
        resources: Default::default(),
        scheduler: Default::default(),
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
        Default::default(),
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
    assert!(
        submitted.deployment_id.starts_with("dev:"),
        "the dev-worker test must never fall back to a release artifact"
    );

    let row = RemoteJob {
        run_uuid: submitted.run_uuid,
        host_id: submitted.host_id,
        host_label: submitted.host_label,
        remote_dir: submitted.remote_dir,
        scheduler: submitted.scheduler,
        launch_handle: submitted.launch_handle,
        cluster: submitted.cluster,
        engine_id: submitted.engine_id,
        job_kind: submitted.job_kind,
        project_root: submitted.project_root,
        local_run_dir: submitted.local_run_dir.to_string_lossy().to_string(),
        status: RemoteJobStatus::Running,
        submitted_at_ms: 0,
        last_polled_at_ms: None,
        exit_code: None,
        scheduler_state: None,
        reason: None,
        console_offset: 0,
        unknown_since_ms: None,
    };

    let outcome = loop {
        let refresh = spawn_remote_jobs_refresh(vec![(row.clone(), host.clone())]);
        let mut updates = refresh.receiver.recv().expect("refresh worker stays alive");
        match updates.pop().expect("one update per job").outcome {
            RemoteJobOutcome::Done(outcome, _) => break *outcome,
            RemoteJobOutcome::Observed(observation)
                if !matches!(
                    observation.phase,
                    crate::engines::remote::launcher::RemoteJobPhase::Failed
                        | crate::engines::remote::launcher::RemoteJobPhase::Lost
                        | crate::engines::remote::launcher::RemoteJobPhase::Cancelled
                ) =>
            {
                std::thread::sleep(Duration::from_millis(500))
            }
            RemoteJobOutcome::Observed(observation) => {
                panic!("remote job ended as {:?}", observation.phase)
            }
            RemoteJobOutcome::OutcomeUnreadable(error, _) => {
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
/// stage report came back in `EngineOutcome::Gromacs`. `#[ignore]` — it needs an
/// SSH host with a working `gmx`. Set the optional
/// `SILICOLAB_TEST_GMX_PRELUDE` to a shell line (e.g. `. /usr/local/gromacs/bin/GMXRC`)
/// when `gmx` needs its environment sourced first. Build the current worker, then run:
///
/// ```text
/// cargo xtask build-dev-worker
/// SILICOLAB_TEST_SSH_HOST=<ip> SILICOLAB_TEST_SSH_USER=<user> \
/// cargo test -p silicolab --features dev-worker --lib -- --ignored remote_gromacs_submit_then_refresh
/// ```
#[cfg(feature = "dev-worker")]
#[test]
#[ignore = "requires an SSH host with a working gmx (set SILICOLAB_TEST_SSH_HOST)"]
fn remote_gromacs_submit_then_refresh_against_ssh_host() {
    use crate::backend::config::RemoteHost;
    use crate::backend::storage::jobs::{RemoteJob, RemoteJobStatus};
    use crate::domain::{Atom, Structure, UnitCell};
    use crate::engines::gromacs::{MdpSettings, StageLinks, StageSpec};
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
    let username = std::env::var("SILICOLAB_TEST_SSH_USER").unwrap_or_else(|_| "root".to_string());
    let prelude = std::env::var("SILICOLAB_TEST_GMX_PRELUDE")
        .ok()
        .map(|line| vec![line])
        .unwrap_or_default();

    let host = RemoteHost {
        id: "wsl".to_string(),
        label: "WSL".to_string(),
        hostname,
        username,
        port: 22,
        work_root: "~/.silicolab".to_string(),
        prelude,
        engines: Default::default(),
        engine_versions: Default::default(),
        resources: Default::default(),
        scheduler: Default::default(),
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
        Default::default(),
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
    assert!(
        submitted.deployment_id.starts_with("dev:"),
        "the dev-worker test must never fall back to a release artifact"
    );

    let row = RemoteJob {
        run_uuid: submitted.run_uuid,
        host_id: submitted.host_id,
        host_label: submitted.host_label,
        remote_dir: submitted.remote_dir,
        scheduler: submitted.scheduler,
        launch_handle: submitted.launch_handle,
        cluster: submitted.cluster,
        engine_id: submitted.engine_id,
        job_kind: submitted.job_kind,
        project_root: submitted.project_root,
        local_run_dir: submitted.local_run_dir.to_string_lossy().to_string(),
        status: RemoteJobStatus::Running,
        submitted_at_ms: 0,
        last_polled_at_ms: None,
        exit_code: None,
        scheduler_state: None,
        reason: None,
        console_offset: 0,
        unknown_since_ms: None,
    };

    let outcome = loop {
        let refresh = spawn_remote_jobs_refresh(vec![(row.clone(), host.clone())]);
        let mut updates = refresh.receiver.recv().expect("refresh worker stays alive");
        match updates.pop().expect("one update per job").outcome {
            RemoteJobOutcome::Done(outcome, _) => break *outcome,
            RemoteJobOutcome::Observed(observation)
                if !matches!(
                    observation.phase,
                    crate::engines::remote::launcher::RemoteJobPhase::Failed
                        | crate::engines::remote::launcher::RemoteJobPhase::Lost
                        | crate::engines::remote::launcher::RemoteJobPhase::Cancelled
                ) =>
            {
                std::thread::sleep(Duration::from_millis(500))
            }
            RemoteJobOutcome::Observed(observation) => {
                panic!("remote job ended as {:?}", observation.phase)
            }
            RemoteJobOutcome::OutcomeUnreadable(error, _) => {
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
