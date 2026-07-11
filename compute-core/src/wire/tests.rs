use nalgebra::Point3;

use super::*;
use crate::domain::{Atom, Structure};
use crate::engines::qm::{
    QmCalculation, QmEngine, QmKind, QmMethod, QmOptions, QmOutcome, QmRequest,
};
use crate::launch::EngineLaunch;

fn builtin_request(engine: Engine) -> EngineRequest {
    EngineRequest::builtin(engine, None)
}

/// The launches a GROMACS job must carry.
fn gmx_launches(program: &str) -> EngineLaunches {
    let mut launches = EngineLaunches::new();
    launches.insert(EngineId::GROMACS, EngineLaunch::native(program));
    launches
}

/// A GROMACS request whose `gmx` sits at a non-default path — the launch the
/// worker must honor rather than rediscover.
const TEST_GMX: &str = "/opt/gromacs-2022.5/bin/gmx";

fn gromacs_request(engine: Engine) -> EngineRequest {
    EngineRequest::new(engine, None, gmx_launches(TEST_GMX)).expect("gromacs launch supplied")
}

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
    builtin_request(Engine::Qm(QmJob::molecular(
        QmEngine::Hartree,
        QmRequest {
            structure,
            method: QmMethod::Rhf,
            basis: "sto-3g".to_string(),
            charge: 0,
            multiplicity: 1,
            kind: QmKind::SinglePoint,
            options: QmOptions::default(),
            ts: None,
        },
    )))
}

#[test]
fn validate_request_rejects_empty_nan_and_accepts_h2() {
    // Empty atoms → rejected.
    let empty = builtin_request(Engine::Qm(QmJob::molecular(
        QmEngine::Hartree,
        QmRequest {
            structure: Structure::new("empty", Vec::new()),
            method: QmMethod::Rhf,
            basis: "sto-3g".to_string(),
            charge: 0,
            multiplicity: 1,
            kind: QmKind::SinglePoint,
            options: QmOptions::default(),
            ts: None,
        },
    )));
    assert!(validate_request(&empty).is_err());

    // A non-finite coordinate → rejected, message names the atom index. The
    // structure is built clean (bond inference rejects non-finite input), then
    // a coordinate is poked to infinity to exercise the validator.
    let mut nan_structure = Structure::new(
        "nan",
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
    nan_structure.atoms[1].position.y = f32::INFINITY;
    let nan = builtin_request(Engine::Qm(QmJob::molecular(
        QmEngine::Hartree,
        QmRequest {
            structure: nan_structure,
            method: QmMethod::Rhf,
            basis: "sto-3g".to_string(),
            charge: 0,
            multiplicity: 1,
            kind: QmKind::SinglePoint,
            options: QmOptions::default(),
            ts: None,
        },
    )));
    let error = validate_request(&nan).unwrap_err().to_string();
    assert!(
        error.contains("atom 1"),
        "message should name atom 1: {error}"
    );

    // A clean H2 → accepted.
    assert!(validate_request(&h2_single_point()).is_ok());
}

/// An external-engine job with no launch is refused when constructed, and again
/// when parsed back off the wire — a hand-edited or stale `request.json` must not
/// reach the engine and be silently run against whatever binary the node has.
#[test]
fn a_gromacs_job_without_a_launch_is_rejected_at_both_ends() {
    let job = Engine::Gromacs(GromacsJob::Build(minimal_build_request()));

    let error = EngineRequest::new(job.clone(), None, EngineLaunches::new())
        .expect_err("a GROMACS job needs a gmx launch")
        .to_string();
    assert!(
        error.contains("gromacs"),
        "message names the engine: {error}"
    );

    // Forge the envelope the constructor refuses, as a stale `request.json` would.
    let forged = EngineRequest {
        engine: job,
        cores: None,
        launches: EngineLaunches::new(),
    };
    assert!(validate_request(&forged).is_err());

    // An entry with an empty program counts as absent, not as a launch.
    let mut blank = EngineLaunches::new();
    blank.insert(EngineId::GROMACS, EngineLaunch::native(""));
    assert!(!blank.contains(EngineId::GROMACS));
}

#[test]
fn an_orca_job_requires_its_configured_launch() {
    let mut request = h2_single_point();
    let Engine::Qm(job) = &mut request.engine else {
        unreachable!();
    };
    job.engine = QmEngine::Orca;
    let error = EngineRequest::new(request.engine.clone(), None, EngineLaunches::new())
        .expect_err("an ORCA job needs an explicit launch")
        .to_string();
    assert!(error.contains("orca"));

    let mut launches = EngineLaunches::new();
    launches.insert(EngineId::ORCA, EngineLaunch::native("/opt/orca/orca"));
    let bound = EngineRequest::new(request.engine, None, launches).expect("ORCA launch supplied");
    let json = serde_json::to_vec(&bound).unwrap();
    let back: EngineRequest = serde_json::from_slice(&json).unwrap();
    assert!(back.launches.contains(EngineId::ORCA));
    let Engine::Qm(job) = back.engine else {
        panic!("expected QM job");
    };
    assert_eq!(job.engine, QmEngine::Orca);
}

/// The smallest build request that satisfies the structure validator.
fn minimal_build_request() -> crate::workflows::gromacs::GromacsBuildRequest {
    use crate::md::{MdSystemConfig, WaterModel};
    use crate::workflows::gromacs::GromacsBuildRequest;
    GromacsBuildRequest {
        structure: Structure::new(
            "ar",
            vec![Atom {
                element: "Ar".to_string(),
                position: Point3::new(0.0, 0.0, 0.0),
                charge: 0.0,
            }],
        ),
        force_field: "amber14sb".to_string(),
        water: WaterModel::Tip3p,
        box_config: MdSystemConfig::default(),
        solvate: false,
        ions: None,
        max_duration: Duration::from_secs(60),
    }
}

#[test]
fn validate_request_rejects_a_non_finite_lattice() {
    use crate::domain::UnitCell;
    use crate::engines::qm::PeriodicQmRequest;

    let mut cell = UnitCell::from_parameters(5.43, 5.43, 5.43, 90.0, 90.0, 90.0);
    cell.vectors[0].x = f32::NAN;
    let structure = Structure::with_cell(
        "si",
        vec![Atom {
            element: "Si".to_string(),
            position: Point3::new(0.0, 0.0, 0.0),
            charge: 0.0,
        }],
        cell,
    );
    let request = builtin_request(Engine::Qm(QmJob::periodic(PeriodicQmRequest::new(
        structure,
    ))));
    let error = validate_request(&request).unwrap_err().to_string();
    assert!(
        error.contains("lattice"),
        "message should name the lattice: {error}"
    );
}

#[test]
fn engine_request_round_trips() {
    let request = h2_single_point();
    let json = serde_json::to_vec(&request).unwrap();
    let back: EngineRequest = serde_json::from_slice(&json).unwrap();
    match back.engine {
        Engine::Qm(QmJob {
            calculation: QmCalculation::Molecular(req),
            ..
        }) => {
            assert_eq!(req.basis, "sto-3g");
            assert_eq!(req.structure.atoms.len(), 2);
        }
        _ => panic!("expected a molecular QM request"),
    }
}

#[test]
fn ts_request_round_trips_with_two_endpoint_product() {
    use crate::engines::qm::{QmTsConfig, QmTsEndpoints, QmTsGuess};
    // The two-endpoint product is a second Structure carried over the wire via
    // the structure_serde adapter — exercise that it survives the round trip.
    let product = Structure::new(
        "product",
        vec![
            Atom {
                element: "H".to_string(),
                position: Point3::new(0.0, 0.0, 0.0),
                charge: 0.0,
            },
            Atom {
                element: "H".to_string(),
                position: Point3::new(0.0, 0.0, 1.40),
                charge: 0.0,
            },
        ],
    );
    let mut request = match h2_single_point().engine {
        Engine::Qm(QmJob {
            calculation: QmCalculation::Molecular(req),
            ..
        }) => req,
        _ => unreachable!(),
    };
    request.kind = QmKind::TransitionState;
    request.ts = Some(QmTsConfig {
        guess: QmTsGuess::TwoEndpoint(Box::new(QmTsEndpoints::new(product))),
        ..QmTsConfig::default()
    });
    let wrapped = builtin_request(Engine::Qm(QmJob::molecular(QmEngine::Hartree, request)));
    let json = serde_json::to_vec(&wrapped).unwrap();
    let back: EngineRequest = serde_json::from_slice(&json).unwrap();
    let Engine::Qm(QmJob {
        calculation: QmCalculation::Molecular(req),
        ..
    }) = back.engine
    else {
        panic!("expected a molecular QM request");
    };
    assert!(matches!(req.kind, QmKind::TransitionState));
    match req.ts.expect("ts config survives the wire").guess {
        QmTsGuess::TwoEndpoint(endpoints) => {
            assert_eq!(endpoints.product.atoms.len(), 2);
            assert!((endpoints.product.atoms[1].position.z - 1.40).abs() < 1e-6);
        }
        _ => panic!("expected the two-endpoint guess route"),
    }
}

#[test]
fn engine_outcome_round_trips_with_optimized_structure() {
    // Exercises the Option<Structure> wire adapter's Some branch — an optimize
    // job returns relaxed geometry, where a single point returns None.
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
                position: Point3::new(0.0, 0.0, 0.71),
                charge: 0.0,
            },
        ],
    );
    let outcome = EngineOutcome::Qm(QmOutcome {
        energy_hartree: -1.117,
        converged: true,
        optimized_structure: Some(structure),
        summary: "relaxed".to_string(),
        scf_trace: vec![-0.9, -1.0],
        opt_trace: Vec::new(),
        frequencies: Vec::new(),
    });
    let json = serde_json::to_vec(&outcome).unwrap();
    let EngineOutcome::Qm(back) = serde_json::from_slice(&json).unwrap() else {
        panic!("expected a QM outcome");
    };
    let relaxed = back
        .optimized_structure
        .expect("optimized structure survives the wire");
    assert_eq!(relaxed.atoms.len(), 2);
    assert!((relaxed.atoms[1].position.z - 0.71).abs() < 1e-6);
    assert!(back.converged);
    assert_eq!(back.scf_trace, vec![-0.9, -1.0]);
}

#[test]
fn periodic_request_round_trips_with_cell() {
    use crate::domain::{Structure, UnitCell};
    use crate::engines::qm::PeriodicQmRequest;

    let structure = Structure::with_cell(
        "si",
        vec![Atom {
            element: "Si".to_string(),
            position: Point3::new(0.0, 0.0, 0.0),
            charge: 0.0,
        }],
        UnitCell::from_parameters(5.43, 5.43, 5.43, 90.0, 90.0, 90.0),
    );
    let request = builtin_request(Engine::Qm(QmJob::periodic(PeriodicQmRequest::new(
        structure,
    ))));
    let json = serde_json::to_vec(&request).unwrap();
    let back: EngineRequest = serde_json::from_slice(&json).unwrap();
    match back.engine {
        Engine::Qm(QmJob {
            calculation: QmCalculation::Periodic(req),
            ..
        }) => {
            assert!(req.structure.cell.is_some());
        }
        _ => panic!("expected a periodic QM request"),
    }
}

#[test]
fn in_process_runs_to_completion() {
    let running = run_job(h2_single_point(), Executor::InProcess);
    let outcome = loop {
        match running.updates().recv().expect("worker stays alive") {
            JobUpdate::Finished(outcome) => break outcome,
            JobUpdate::Failed(error) => panic!("in-process job failed: {error}"),
            JobUpdate::Progress { .. } => {}
        }
    };
    let EngineOutcome::Qm(outcome) = *outcome else {
        panic!("expected a QM outcome");
    };
    assert!(outcome.converged);
}

#[test]
fn in_process_and_exec_agree_within_tolerance() {
    // Parity is to convergence tolerance, not bit-for-bit: the same source runs
    // both paths, so a small SCF-level delta is the only allowed difference.
    let in_process = run_job(h2_single_point(), Executor::InProcess);
    let local = loop {
        match in_process.updates().recv().expect("worker stays alive") {
            JobUpdate::Finished(outcome) => {
                let EngineOutcome::Qm(outcome) = *outcome else {
                    panic!("expected a QM outcome");
                };
                break outcome;
            }
            JobUpdate::Failed(error) => panic!("in-process job failed: {error}"),
            JobUpdate::Progress { .. } => {}
        }
    };

    let dir = std::env::temp_dir().join("silicolab-exec-parity");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let request_path = dir.join("request.json");
    let outcome_path = dir.join("outcome.json");
    std::fs::write(
        &request_path,
        serde_json::to_vec(&h2_single_point()).unwrap(),
    )
    .unwrap();
    exec(&request_path, &outcome_path).expect("exec succeeds");
    let bytes = std::fs::read(&outcome_path).unwrap();
    let EngineOutcome::Qm(via_exec) = serde_json::from_slice(&bytes).unwrap() else {
        panic!("expected a QM outcome");
    };

    assert!(via_exec.converged);
    assert!(
        (local.energy_hartree - via_exec.energy_hartree).abs() < 1e-6,
        "in-process {} vs exec {} exceeded SCF tolerance",
        local.energy_hartree,
        via_exec.energy_hartree
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A small docking request whose receptor and ligand are butane skeletons,
/// prepared heuristically from structures (exercising the payload bridge on the
/// `DockingInput::Structure` variant). `ScoreOnly` keeps it a single, fast
/// evaluation.
fn butane_score_request() -> EngineRequest {
    use crate::domain::{Bond, BondType};
    use crate::engines::docking::{DockingConfig, DockingInput, DockingKind, DockingRequest};

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
    builtin_request(Engine::Docking(DockingRequest {
        receptor: DockingInput::Structure(Box::new(skeleton())),
        ligand: DockingInput::Structure(Box::new(skeleton())),
        box_center: [1.8, 0.6, 0.0],
        box_size: [20.0, 20.0, 20.0],
        config: DockingConfig::default(),
        kind: DockingKind::ScoreOnly,
    }))
}

#[test]
fn docking_request_round_trips_through_the_payload_bridge() {
    use crate::engines::docking::DockingInput;

    let request = butane_score_request();
    let json = serde_json::to_vec(&request).unwrap();
    let back: EngineRequest = serde_json::from_slice(&json).unwrap();
    match back.engine {
        Engine::Docking(docking) => {
            let DockingInput::Structure(receptor) = &docking.receptor else {
                panic!("expected a structure receptor");
            };
            assert_eq!(receptor.atoms.len(), 4);
            assert_eq!(receptor.bonds.len(), 3);
            assert_eq!(docking.box_size, [20.0, 20.0, 20.0]);
        }
        _ => panic!("expected a docking request"),
    }
}

#[test]
fn docking_outcome_round_trips() {
    let outcome = EngineOutcome::Docking(crate::engines::docking::DockingOutcome {
        poses: vec![crate::engines::docking::DockedPose {
            affinity: -5.5,
            intermolecular: -6.0,
            internal: 0.5,
            torsional: 0.0,
            structure: Structure::new(
                "pose 1",
                vec![Atom {
                    element: "C".to_string(),
                    position: Point3::new(0.0, 0.0, 0.0),
                    charge: 0.0,
                }],
            ),
            pdbqt: "ATOM      1  C   LIG A   1       0.000   0.000   0.000\n".to_string(),
        }],
        notes: vec!["prepared heuristically".to_string()],
        summary: "Score only:".to_string(),
    });
    let json = serde_json::to_vec(&outcome).unwrap();
    let EngineOutcome::Docking(back) = serde_json::from_slice(&json).unwrap() else {
        panic!("expected a docking outcome");
    };
    assert_eq!(back.poses.len(), 1);
    assert!((back.poses[0].affinity + 5.5).abs() < 1e-9);
    assert_eq!(back.poses[0].structure.atoms.len(), 1);
}

#[test]
fn in_process_docking_scores_a_pose() {
    let running = run_job(butane_score_request(), Executor::InProcess);
    let outcome = loop {
        match running.updates().recv().expect("worker stays alive") {
            JobUpdate::Finished(outcome) => break outcome,
            JobUpdate::Failed(error) => panic!("in-process docking failed: {error}"),
            JobUpdate::Progress { .. } => {}
        }
    };
    let EngineOutcome::Docking(outcome) = *outcome else {
        panic!("expected a docking outcome");
    };
    assert_eq!(outcome.poses.len(), 1);
    assert!(outcome.poses[0].affinity.is_finite());
}

#[test]
fn gromacs_run_request_round_trips_through_the_payload_bridge() {
    use crate::domain::UnitCell;
    use crate::engines::gromacs::{FreezeSelection, MdpSettings, StageLinks, StageSpec};
    use crate::workflows::gromacs::{GromacsJob, GromacsRunRequest, WireTopology};

    let structure = Structure::with_cell(
        "argon",
        vec![
            Atom {
                element: "Ar".to_string(),
                position: Point3::new(1.0, 1.0, 1.0),
                charge: 0.0,
            },
            Atom {
                element: "Ar".to_string(),
                position: Point3::new(2.0, 1.0, 1.0),
                charge: 0.0,
            },
        ],
        UnitCell::from_parameters(20.0, 20.0, 20.0, 90.0, 90.0, 90.0),
    );
    let request = gromacs_request(Engine::Gromacs(GromacsJob::Run(GromacsRunRequest {
        structure,
        topology: WireTopology {
            top: "; topol\n".to_string(),
            includes: vec![("posre.itp".to_string(), "; restraints\n".to_string())],
        },
        stages: vec![
            StageSpec {
                stage_name: "em".to_string(),
                settings: MdpSettings::energy_minimization(),
                links: StageLinks::from_prepared(),
            },
            StageSpec {
                stage_name: "nvt".to_string(),
                settings: MdpSettings::nvt(300.0),
                links: StageLinks::from_prepared(),
            },
        ],
        max_duration_per_stage: Duration::from_secs(3600),
        freeze: Some(FreezeSelection {
            group: "Framework".to_string(),
            atom_indices: vec![0, 1],
        }),
        resources: crate::launch::ComputeResources { cores: 4, gpu: 1 },
    })));
    let json = serde_json::to_vec(&request).unwrap();
    let back: EngineRequest = serde_json::from_slice(&json).unwrap();
    // The configured `gmx` must survive the wire: a worker that re-derived it
    // would run whichever GROMACS happens to sit on the node.
    assert_eq!(
        back.launches
            .get(EngineId::GROMACS)
            .map(|l| l.program.as_str()),
        Some(TEST_GMX)
    );
    let Engine::Gromacs(GromacsJob::Run(req)) = back.engine else {
        panic!("expected a GROMACS run job");
    };
    assert_eq!(req.structure.atoms.len(), 2);
    assert!(req.structure.cell.is_some());
    assert_eq!(req.stages.len(), 2);
    assert_eq!(req.topology.includes.len(), 1);
    assert_eq!(
        req.freeze.expect("freeze survives").atom_indices,
        vec![0, 1]
    );
    // The CPU/GPU resource request must survive the wire to the worker.
    assert_eq!(req.resources.cores, 4);
    assert_eq!(req.resources.gpu, 1);
}

#[test]
fn gromacs_build_request_round_trips() {
    use crate::engines::gromacs::IonOptions;
    use crate::workflows::gromacs::{GromacsBuildRequest, GromacsJob};
    use crate::workflows::molecular_dynamics::{BoxShape, MdSystemConfig, WaterModel};

    let structure = Structure::new(
        "solute",
        vec![Atom {
            element: "C".to_string(),
            position: Point3::new(0.0, 0.0, 0.0),
            charge: 0.0,
        }],
    );
    let request = gromacs_request(Engine::Gromacs(GromacsJob::Build(GromacsBuildRequest {
        structure,
        force_field: "amber99sb-ildn".to_string(),
        water: WaterModel::Tip3p,
        box_config: MdSystemConfig::with_uniform_padding(10.0, BoxShape::Cubic),
        solvate: true,
        ions: Some(IonOptions {
            neutralize: true,
            concentration_molar: Some(0.15),
            positive_ion: "NA".to_string(),
            negative_ion: "CL".to_string(),
        }),
        max_duration: Duration::from_secs(3600),
    })));
    let json = serde_json::to_vec(&request).unwrap();
    let back: EngineRequest = serde_json::from_slice(&json).unwrap();
    let Engine::Gromacs(GromacsJob::Build(req)) = back.engine else {
        panic!("expected a GROMACS build job");
    };
    assert_eq!(req.force_field, "amber99sb-ildn");
    assert_eq!(req.water, WaterModel::Tip3p);
    assert!(req.solvate);
    let ions = req.ions.expect("ions survive");
    assert!(ions.neutralize);
    assert_eq!(ions.positive_ion, "NA");
}

#[test]
fn gromacs_material_request_round_trips_with_cell_override() {
    use crate::domain::UnitCell;
    use crate::workflows::gromacs::{GromacsJob, GromacsMaterialRequest};
    use crate::workflows::molecular_dynamics::FrameworkMode;

    let structure = Structure::new(
        "sheet",
        vec![Atom {
            element: "C".to_string(),
            position: Point3::new(0.0, 0.0, 0.0),
            charge: 0.0,
        }],
    );
    // A hexagonal (gamma = 120°) cell: confirms the lattice VECTORS survive the
    // cell payload bridge, not just the six scalar parameters.
    let cell = UnitCell::from_parameters(2.46, 2.46, 12.0, 90.0, 90.0, 120.0);
    let original_vectors = cell.vectors;
    let request = gromacs_request(Engine::Gromacs(GromacsJob::BuildMaterial(
        GromacsMaterialRequest {
            structure,
            mode: FrameworkMode::Rigid,
            solvation: None,
            custom_force_field: Some("[ atomtypes ]\n".to_string()),
            cell_override: Some(cell),
            solvent_gap_angstrom: 25.0,
            cutoff_nm: 1.0,
            max_duration: Duration::from_secs(3600),
        },
    )));
    let json = serde_json::to_vec(&request).unwrap();
    let back: EngineRequest = serde_json::from_slice(&json).unwrap();
    let Engine::Gromacs(GromacsJob::BuildMaterial(req)) = back.engine else {
        panic!("expected a GROMACS material job");
    };
    assert_eq!(req.mode, FrameworkMode::Rigid);
    assert!(req.custom_force_field.is_some());
    let restored = req.cell_override.expect("cell survives the bridge");
    for (original, restored) in original_vectors.iter().zip(restored.vectors.iter()) {
        assert!((original.x - restored.x).abs() < 1e-6);
        assert!((original.y - restored.y).abs() < 1e-6);
        assert!((original.z - restored.z).abs() < 1e-6);
    }
}

#[test]
fn gromacs_outcome_round_trips_with_trajectory() {
    use crate::workflows::gromacs::{GromacsOutcome, GromacsStageReport, GromacsTrajectory};

    let outcome = EngineOutcome::Gromacs(GromacsOutcome {
        structure: Structure::new(
            "final",
            vec![Atom {
                element: "Ar".to_string(),
                position: Point3::new(0.0, 0.0, 0.0),
                charge: 0.0,
            }],
        ),
        summary: "GROMACS MD complete".to_string(),
        stages: vec![GromacsStageReport {
            stage_name: "em".to_string(),
            final_potential_energy: Some(-12.3),
            wall_time: Duration::from_millis(500),
        }],
        trajectory: Some(GromacsTrajectory {
            file_name: "prod.xtc".to_string(),
            bytes: vec![1, 2, 3, 4],
        }),
        topology: None,
        system_context: None,
        material: None,
    });
    let json = serde_json::to_vec(&outcome).unwrap();
    let EngineOutcome::Gromacs(back) = serde_json::from_slice(&json).unwrap() else {
        panic!("expected a GROMACS outcome");
    };
    assert_eq!(back.structure.atoms.len(), 1);
    assert_eq!(back.stages.len(), 1);
    let trajectory = back.trajectory.expect("trajectory survives");
    assert_eq!(trajectory.file_name, "prod.xtc");
    assert_eq!(trajectory.bytes, vec![1, 2, 3, 4]);
}

#[test]
fn validate_gromacs_rejects_an_empty_structure() {
    use crate::workflows::gromacs::{GromacsBuildRequest, GromacsJob};
    use crate::workflows::molecular_dynamics::{BoxShape, MdSystemConfig, WaterModel};

    let request = gromacs_request(Engine::Gromacs(GromacsJob::Build(GromacsBuildRequest {
        structure: Structure::new("empty", Vec::new()),
        force_field: "amber99sb-ildn".to_string(),
        water: WaterModel::Spc,
        box_config: MdSystemConfig::with_uniform_padding(10.0, BoxShape::Cubic),
        solvate: false,
        ions: None,
        max_duration: Duration::from_secs(60),
    })));
    assert!(validate_request(&request).is_err());
}

/// Outcomes written by pre-trace workers must still parse — the new fields
/// default to empty (same contract as `QmRequest::ts`).
#[test]
fn qm_outcome_without_trace_fields_deserializes() {
    let json = r#"{
        "energy_hartree": -1.0,
        "converged": true,
        "optimized_structure": null,
        "summary": "E = -1.0 Eh"
    }"#;
    let outcome: QmOutcome = serde_json::from_str(json).expect("legacy outcome parses");
    assert!(outcome.scf_trace.is_empty());
    assert!(outcome.opt_trace.is_empty());
    assert!(outcome.frequencies.is_empty());
}
