//! End-to-end checks of the detached frontend path (deploy → submit → opt-in
//! refresh → retrieve) against a real SSH host — e.g. a local WSL with
//! passwordless login configured. Every test here is `#[ignore]`: it depends on
//! the machine's environment, not just this process.
//!
//! Build the current worker first, then run one by name:
//!
//! ```text
//! cargo xtask build-dev-worker
//! SILICOLAB_TEST_SSH_HOST=<ip> SILICOLAB_TEST_SSH_USER=<user> \
//! cargo test -p silicolab --features dev-worker --lib -- --ignored remote_gromacs
//! ```

use std::time::Duration;

use crate::backend::config::{JobResources, RemoteHost};
use crate::backend::storage::jobs::{RemoteJob, RemoteJobStatus};
use crate::engines::registry::{EngineId, EngineLaunch, EngineLaunches};
use crate::frontend::remote_jobs::{
    RemoteJobOutcome, RemoteSubmitOutcome, RemoteSubmitted, spawn_remote_jobs_refresh,
    spawn_remote_submit,
};
use crate::wire::{Engine, EngineOutcome};

/// The SSH host under test, or `None` when the environment is not configured.
fn test_host(engines: EngineLaunches) -> Option<RemoteHost> {
    let hostname = std::env::var("SILICOLAB_TEST_SSH_HOST").ok()?;
    let username = std::env::var("SILICOLAB_TEST_SSH_USER").unwrap_or_else(|_| "root".to_string());
    let prelude = std::env::var("SILICOLAB_TEST_GMX_PRELUDE")
        .ok()
        .map(|line| vec![line])
        .unwrap_or_default();
    Some(RemoteHost {
        id: "wsl".to_string(),
        label: "WSL".to_string(),
        hostname,
        username,
        prelude,
        engines,
        ..Default::default()
    })
}

/// Submit `engine` to `host` and block until the detached job finishes, returning
/// its outcome along with the submission record (for the remote scratch dir).
fn submit_and_wait(
    host: &RemoteHost,
    engine: Engine,
    resources: JobResources,
    job_kind: &str,
) -> (EngineOutcome, RemoteSubmitted) {
    let run_uuid = uuid::Uuid::new_v4().to_string();
    let local_run_dir = std::env::temp_dir().join(format!("sl-e2e-{job_kind}-{run_uuid}"));
    let submit = spawn_remote_submit(
        host.clone(),
        engine,
        resources,
        run_uuid,
        None,
        job_kind.to_string(),
        None,
        local_run_dir.clone(),
    );
    let submitted = match submit.receiver.recv().expect("submit worker stays alive") {
        RemoteSubmitOutcome::Submitted(submitted) => *submitted,
        RemoteSubmitOutcome::Failed(error) => panic!("remote {job_kind} submit failed: {error}"),
    };
    assert!(
        submitted.deployment_id.starts_with("dev:"),
        "the dev-worker test must never fall back to a release artifact"
    );

    let row = RemoteJob {
        run_uuid: submitted.run_uuid.clone(),
        host_id: submitted.host_id.clone(),
        host_label: submitted.host_label.clone(),
        remote_dir: submitted.remote_dir.clone(),
        scheduler: submitted.scheduler.clone(),
        launch_handle: submitted.launch_handle.clone(),
        cluster: submitted.cluster.clone(),
        engine_id: submitted.engine_id.clone(),
        job_kind: submitted.job_kind.clone(),
        project_root: submitted.project_root.clone(),
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
    let _ = std::fs::remove_dir_all(&local_run_dir);
    (outcome, submitted)
}

/// The `gromacs.log` the run left in its remote scratch dir. Its command headers
/// name the `gmx` that actually ran, which is what these tests assert on.
fn remote_gromacs_log(host: &RemoteHost, submitted: &RemoteSubmitted) -> String {
    let target = crate::engines::remote::RemoteTarget::from_remote_dir(host, &submitted.remote_dir)
        .expect("the run anchors a valid remote dir");
    crate::engines::remote::run_probe_command(
        &target,
        &format!("cat {}/gromacs.log", submitted.remote_dir),
        Duration::from_secs(30),
    )
    .expect("the finished run left a gromacs.log")
}

/// A hermetic argon topology: Lennard-Jones only, no external force-field data,
/// eight single-atom `AR` molecules matching the eight box atoms.
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

/// A 2×2×2 argon grid centered in a 30 Å cubic cell — finite starting energy, box
/// well over twice the 1 nm cutoff — energy-minimized in one stage.
fn argon_em_job() -> Engine {
    use crate::domain::{Atom, Structure, UnitCell};
    use crate::engines::gromacs::{MdpSettings, StageLinks, StageSpec};
    use crate::workflows::gromacs::{GromacsJob, GromacsRunRequest, WireTopology};
    use nalgebra::Point3;

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
    Engine::Gromacs(GromacsJob::Run(GromacsRunRequest {
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
    }))
}

#[test]
#[ignore = "requires an SSH host (set SILICOLAB_TEST_SSH_HOST)"]
fn remote_qm_submit_then_refresh_against_ssh_host() {
    use crate::domain::{Atom, Structure};
    use crate::engines::qm::{QmJob, QmKind, QmMethod, QmOptions, QmRequest};
    use nalgebra::Point3;

    let Some(host) = test_host(EngineLaunches::new()) else {
        eprintln!("skip: set SILICOLAB_TEST_SSH_HOST to run the remote frontend test");
        return;
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
    let job = Engine::Qm(QmJob::Molecular(QmRequest {
        structure,
        method: QmMethod::Rhf,
        basis: "sto-3g".to_string(),
        charge: 0,
        multiplicity: 1,
        kind: QmKind::SinglePoint,
        options: QmOptions::default(),
        ts: None,
    }));
    let resources = JobResources {
        cpus_per_task: Some(1),
        ..Default::default()
    };

    let (outcome, _) = submit_and_wait(&host, job, resources, "qm-energy");
    let EngineOutcome::Qm(outcome) = outcome else {
        panic!("expected a QM outcome");
    };
    assert!(outcome.converged, "remote QM did not converge");
}

/// The detached docking path, mirroring the QM E2E: a `ScoreOnly` job (one fast
/// evaluation) whose pose comes back through the payload bridge.
#[test]
#[ignore = "requires an SSH host (set SILICOLAB_TEST_SSH_HOST)"]
fn remote_docking_submit_then_refresh_against_ssh_host() {
    use crate::domain::{Atom, Bond, BondType, Structure};
    use crate::engines::docking::{DockingConfig, DockingInput, DockingKind, DockingRequest};
    use nalgebra::Point3;

    let Some(host) = test_host(EngineLaunches::new()) else {
        eprintln!("skip: set SILICOLAB_TEST_SSH_HOST to run the remote docking test");
        return;
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
    let job = Engine::Docking(DockingRequest {
        receptor: DockingInput::Structure(Box::new(skeleton())),
        ligand: DockingInput::Structure(Box::new(skeleton())),
        box_center: [1.8, 0.6, 0.0],
        box_size: [20.0, 20.0, 20.0],
        config: DockingConfig::default(),
        kind: DockingKind::ScoreOnly,
    });

    let (outcome, _) = submit_and_wait(&host, job, Default::default(), "dock");
    let EngineOutcome::Docking(outcome) = outcome else {
        panic!("expected a docking outcome");
    };
    assert_eq!(outcome.poses.len(), 1, "ScoreOnly returns one pose");
    assert!(outcome.poses[0].affinity.is_finite());
}

/// The detached GROMACS relay with no `gmx` configured on the host: the submission
/// probes the host over SSH, caches what it found, and the run completes.
#[test]
#[ignore = "requires an SSH host with a working gmx (set SILICOLAB_TEST_SSH_HOST)"]
fn remote_gromacs_submit_then_refresh_against_ssh_host() {
    let Some(host) = test_host(EngineLaunches::new()) else {
        eprintln!("skip: set SILICOLAB_TEST_SSH_HOST to run the remote GROMACS test");
        return;
    };
    let (outcome, submitted) = submit_and_wait(&host, argon_em_job(), Default::default(), "run-md");

    // An unconfigured host is probed once, and the result comes back to be cached.
    let detected = submitted
        .detected_launches
        .iter()
        .find(|d| d.engine == EngineId::GROMACS)
        .expect("an unconfigured host is probed at submit time");
    assert!(
        !detected.launch.program.is_empty(),
        "the probe names a program"
    );

    let EngineOutcome::Gromacs(outcome) = outcome else {
        panic!("expected a GROMACS outcome");
    };
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

/// The regression test for the launch contract: a host whose `gmx` sits at a
/// non-standard path, configured in settings and NOT on the non-interactive PATH,
/// must run *that* binary.
///
/// Before the launch travelled in `request.json`, the worker rediscovered `gmx`
/// from a hardcoded candidate list. On a host with no `/usr/local/gromacs` the job
/// failed outright; on a host that had one, the worker silently ran that install
/// instead of the configured one and exited zero — the more dangerous half. Both
/// are asserted here.
///
/// Set `SILICOLAB_TEST_GMX_PROGRAM` to an absolute path to a `gmx` that is not on
/// PATH and not at `/usr/local/gromacs/bin/gmx`, e.g.:
///
/// ```text
/// SILICOLAB_TEST_SSH_HOST=<ip> SILICOLAB_TEST_SSH_USER=<user> \
/// SILICOLAB_TEST_GMX_PROGRAM=/opt/gromacs-2022.5/bin/gmx \
/// cargo test -p silicolab --features dev-worker --lib -- --ignored remote_gromacs_honors
/// ```
#[test]
#[ignore = "requires an SSH host with a non-standard gmx (set SILICOLAB_TEST_GMX_PROGRAM)"]
fn remote_gromacs_honors_a_configured_non_standard_gmx() {
    let Ok(program) = std::env::var("SILICOLAB_TEST_GMX_PROGRAM") else {
        eprintln!("skip: set SILICOLAB_TEST_GMX_PROGRAM to a non-standard gmx path");
        return;
    };
    let mut engines = EngineLaunches::new();
    engines.insert(EngineId::GROMACS, EngineLaunch::native(&program));
    let Some(host) = test_host(engines) else {
        eprintln!("skip: set SILICOLAB_TEST_SSH_HOST to run the remote GROMACS test");
        return;
    };

    let (outcome, submitted) = submit_and_wait(&host, argon_em_job(), Default::default(), "run-md");
    assert!(
        submitted.detected_launches.is_empty(),
        "a configured launch must not be re-probed"
    );
    let EngineOutcome::Gromacs(outcome) = outcome else {
        panic!("expected a GROMACS outcome");
    };
    assert_eq!(outcome.structure.atoms.len(), 8);

    // The log's command headers name the executable that actually ran.
    let log = remote_gromacs_log(&host, &submitted);
    assert!(
        log.contains(&program),
        "the run must use the configured gmx `{program}`, log was:\n{log}"
    );
    // …and must not have silently fallen back to the conventional install.
    if program != "/usr/local/gromacs/bin/gmx" {
        assert!(
            !log.contains("/usr/local/gromacs/bin/gmx"),
            "the run silently used the conventional install instead of `{program}`:\n{log}"
        );
    }
}

/// Verify checks the launch that is configured, not one it goes looking for.
///
/// The old Detect only ever walked the spec's candidate list, so the very paths
/// that need typing by hand — the ones no candidate list can know — were the paths
/// it could never confirm. The button and the field could not reach each other.
/// `SILICOLAB_TEST_GMX_PROGRAM` names exactly such a path.
#[test]
#[ignore = "requires an SSH host with a non-standard gmx (set SILICOLAB_TEST_GMX_PROGRAM)"]
fn verify_confirms_a_remote_gmx_no_candidate_list_would_find() {
    use crate::backend::engine_launch::{LaunchTarget, VerifyOutcome, verify_engine};

    let Ok(program) = std::env::var("SILICOLAB_TEST_GMX_PROGRAM") else {
        eprintln!("skip: set SILICOLAB_TEST_GMX_PROGRAM to a non-standard gmx path");
        return;
    };
    let spec = crate::engines::registry::engine_spec(EngineId::GROMACS).expect("gromacs spec");
    assert!(
        !spec.candidate_executables.contains(&program.as_str()),
        "this test is only meaningful for a path auto-detection would never try"
    );

    let mut engines = EngineLaunches::new();
    engines.insert(EngineId::GROMACS, EngineLaunch::native(&program));
    let Some(host) = test_host(engines) else {
        eprintln!("skip: set SILICOLAB_TEST_SSH_HOST to run the remote verify test");
        return;
    };

    let outcome =
        verify_engine(LaunchTarget::Remote(&host), EngineId::GROMACS).expect("gromacs has a spec");
    let VerifyOutcome::Verified { launch, version } = outcome else {
        panic!("the configured `{program}` should verify");
    };
    assert_eq!(
        launch.program, program,
        "verify must check what is configured"
    );
    assert!(
        !version.is_empty(),
        "a verification carries the reported version"
    );
}

/// With nothing configured, Verify probes the host and hands back the launch it
/// found, so the panel can fill the field in rather than detect invisibly.
#[test]
#[ignore = "requires an SSH host with a working gmx (set SILICOLAB_TEST_SSH_HOST)"]
fn verify_with_no_program_probes_and_returns_the_launch_it_found() {
    use crate::backend::engine_launch::{LaunchTarget, VerifyOutcome, verify_engine};

    let Some(host) = test_host(EngineLaunches::new()) else {
        eprintln!("skip: set SILICOLAB_TEST_SSH_HOST to run the remote verify test");
        return;
    };

    let outcome =
        verify_engine(LaunchTarget::Remote(&host), EngineId::GROMACS).expect("gromacs has a spec");
    let VerifyOutcome::Verified { launch, .. } = outcome else {
        panic!("the host should have a discoverable gmx");
    };
    let spec = crate::engines::registry::engine_spec(EngineId::GROMACS).expect("gromacs spec");
    assert!(
        spec.candidate_executables
            .contains(&launch.program.as_str()),
        "a probe returns one of the candidates, got {:?}",
        launch.program
    );
}
