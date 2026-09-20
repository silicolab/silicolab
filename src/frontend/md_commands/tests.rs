use super::*;

use crate::workflows::molecular_dynamics::run::MdParameters;

fn args(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|s| s.to_string()).collect()
}

#[test]
fn flags_parse_values_flags_and_equals_form() {
    let f = Flags::parse(&args(&["--time", "1ns", "--temperature=300", "--no-relax"])).unwrap();
    assert_eq!(f.str("time"), Some("1ns"));
    assert_eq!(f.str("temperature"), Some("300"));
    assert!(f.flag("no-relax"));
    assert!(!f.flag("relax"));
}

#[test]
fn unprefixed_argument_is_rejected() {
    assert!(Flags::parse(&args(&["time", "1ns"])).is_err());
}

#[test]
fn parse_time_handles_ns_ps_and_bare() {
    assert_eq!(parse_time_ps("200ns").unwrap(), 200_000.0);
    assert_eq!(parse_time_ps("500ps").unwrap(), 500.0);
    assert_eq!(parse_time_ps("250").unwrap(), 250.0);
}

#[test]
fn overrides_read_x_and_no_x_flags() {
    let flags = Flags::parse(&args(&["--membrane", "--no-ligand"])).unwrap();
    let overrides = parse_overrides(&flags);
    assert_eq!(overrides.membrane, Some(true));
    assert_eq!(overrides.ligand, Some(false));
    // Unspecified axis stays None (trust detection).
    assert_eq!(overrides.nucleic, None);
}

#[test]
fn parse_set_maps_keys_to_tiered_parameters() {
    let mut params = MdParameters::default();
    parse_set_into(&mut params, "coulomb_cutoff=1.1, pme_order=6 , seed=42").unwrap();
    assert_eq!(params.coulomb_cutoff_nm, Some(1.1));
    assert_eq!(params.pme_order, Some(6));
    assert_eq!(params.random_seed, Some(42));
    // An unknown key is a hard error, not silently dropped.
    assert!(parse_set_into(&mut params, "bogus=1").is_err());
    // A malformed entry is rejected.
    assert!(parse_set_into(&mut params, "coulomb_cutoff").is_err());
}

/// Requires GROMACS in WSL; run with `cargo test --release -- --ignored agent_md_simulate`.
#[test]
#[ignore = "requires GROMACS in WSL (set the launch below to your install)"]
fn agent_md_simulate_runs_against_gromacs() {
    use crate::domain::{Atom, Structure, UnitCell};
    use crate::engines::registry::EngineLaunch;
    use crate::frontend::jobs::{EngineWorkerMessage, spawn_gromacs_pipeline_job};
    use crate::frontend::state::AppState;
    use crate::io::structure_io::default_structure_save_path;
    use nalgebra::{Point3, Vector3};
    use std::time::{Duration, Instant};

    let mut state = AppState::scratch(Default::default(), Vec::new());
    state.config.engine_overrides.insert(
        crate::engines::registry::EngineId::GROMACS,
        EngineLaunch {
            command_prefix: vec!["wsl.exe".to_string(), "-e".to_string()],
            program: "/usr/local/gromacs/bin/gmx".to_string(),
        },
    );

    // A 3×3×3 argon lattice in a cubic box.
    let spacing = 3.8_f32;
    let length = 30.0_f32;
    let mut atoms = Vec::new();
    for x in 0..3 {
        for y in 0..3 {
            for z in 0..3 {
                atoms.push(Atom {
                    element: "Ar".to_string(),
                    position: Point3::new(
                        x as f32 * spacing + 0.5,
                        y as f32 * spacing + 0.5,
                        z as f32 * spacing + 0.5,
                    ),
                    charge: 0.0,
                });
            }
        }
    }
    let cell = UnitCell::from_vectors([
        Vector3::new(length, 0.0, 0.0),
        Vector3::new(0.0, length, 0.0),
        Vector3::new(0.0, 0.0, length),
    ]);
    let structure = Structure::with_cell("argon", atoms, cell);
    let save_path = default_structure_save_path(&structure, None);
    state.entries.add_entry(structure, None, save_path);

    let draft = build_agent_md_request(
        &state,
        &[
            "simulate".to_string(),
            "--time".to_string(),
            "1".to_string(),
            "--no-trajectory".to_string(),
        ],
    )
    .expect("agent md request should build");

    let controller = *crate::backend::tasks::task_controller_by_id("run-md").unwrap();
    let task_id = state.tasks.create_task_run(controller);
    let input = crate::frontend::entry_ref::primary_input(&state).unwrap();
    crate::frontend::dispatcher::bind_task_inputs(&mut state, task_id, vec![input]).unwrap();
    let run_dir = crate::frontend::dispatcher::ensure_task_run_dir(
        &mut state,
        task_id,
        controller.kind,
        None,
    )
    .unwrap();
    let mut request = draft.with_working_dir(run_dir.clone());
    request.compute.resources.cores = 1;
    for stage in &mut request.stages {
        stage.settings.nsteps = stage.settings.nsteps.min(500);
    }
    assert_eq!(
        state.tasks.task_run(task_id).unwrap().run_dir.as_ref(),
        Some(&request.working_dir)
    );
    let job = spawn_gromacs_pipeline_job(request);
    let deadline = Instant::now() + Duration::from_secs(600);
    let mut saw_stage = false;
    let mut terminal = false;
    while Instant::now() < deadline {
        match job.receiver.try_recv() {
            Ok(EngineWorkerMessage::Finished(success)) => {
                println!("agent MD finished: {}", success.summary);
                terminal = true;
                break;
            }
            Ok(EngineWorkerMessage::Failed(error)) => {
                let log = std::fs::read_to_string(run_dir.join("gromacs.log")).unwrap_or_default();
                panic!("agent MD failed: {error}\n{log}");
            }
            Ok(EngineWorkerMessage::Stage(stage)) => {
                println!("stage: {stage}");
                saw_stage = true;
            }
            Ok(EngineWorkerMessage::Log(_)) => {}
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => break,
        }
    }
    // The agent-integration contract: the off-thread GROMACS pipeline started
    // (stages streamed) and delivered a terminal result back through the
    // channel the agent loop drains.
    assert!(saw_stage, "expected GROMACS stages to stream back");
    assert!(terminal, "expected a successful Finished message");
}

#[test]
fn parse_raw_splits_semicolons_into_verbatim_pairs() {
    let lines = parse_raw_lines("pull = yes ; nstcomm=100").unwrap();
    assert_eq!(
        lines,
        vec![
            ("pull".to_string(), "yes".to_string()),
            ("nstcomm".to_string(), "100".to_string()),
        ]
    );
    assert!(parse_raw_lines("missing-equals").is_err());
}

#[test]
fn agent_md_draft_preserves_native_and_wsl_launches_when_binding_directory() {
    use crate::engines::registry::{EngineId, EngineLaunch};
    for launch in [
        EngineLaunch::native("C:/Gromacs/bin/gmx.exe"),
        EngineLaunch {
            command_prefix: vec!["wsl.exe".into(), "-e".into()],
            program: "/usr/local/gromacs/bin/gmx".into(),
        },
    ] {
        let mut state = crate::frontend::state::AppState::scratch(Default::default(), Vec::new());
        state
            .config
            .engine_overrides
            .insert(EngineId::GROMACS, launch.clone());
        let structure = crate::domain::Structure::with_cell(
            "argon",
            vec![crate::domain::Atom {
                element: "Ar".into(),
                position: nalgebra::Point3::origin(),
                charge: 0.0,
            }],
            crate::domain::UnitCell::from_parameters(30.0, 30.0, 30.0, 90.0, 90.0, 90.0),
        );
        state
            .entries
            .add_entry(structure, None, std::path::PathBuf::from("argon.xyz"));
        let draft = build_agent_md_request(&state, &args(&["simulate", "--time", "1"])).unwrap();
        let dir = std::path::PathBuf::from("task-owned-run");
        let request = draft.with_working_dir(dir.clone());
        assert_eq!(request.compute.launch, launch);
        assert_eq!(request.working_dir, dir);
    }
}
