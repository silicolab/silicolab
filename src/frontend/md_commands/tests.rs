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

/// End-to-end check of the agent's async MD path against a real GROMACS:
/// build a structure, build the same `GromacsPipelineRequest` the agent
/// spawns, run it through `spawn_gromacs_pipeline_job`, and poll it exactly as
/// `poll_heavy_engine` does. Asserts the path reaches GROMACS (request built,
/// job spawned, stages streamed back, a terminal message delivered) — that is
/// the agent-integration contract; whether the *system* converges is an MD
/// concern. Ignored by default (needs GROMACS); run with
/// `cargo test -- --ignored agent_md_simulate`. The bare argon lattice here
/// is a minimal smoke system, not an equilibrated one.
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
        crate::engines::registry::EngineId::GROMACS
            .as_str()
            .to_string(),
        EngineLaunch {
            command_prefix: vec!["wsl.exe".to_string(), "-e".to_string()],
            program: "/usr/local/gromacs/bin/gmx".to_string(),
        },
    );

    // A 3×3×3 argon lattice in a cubic box.
    let spacing = 3.8_f32;
    let length = spacing * 3.0;
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

    let request = build_agent_md_request(
        &state,
        &[
            "simulate".to_string(),
            "--time".to_string(),
            "1".to_string(),
            "--no-trajectory".to_string(),
        ],
    )
    .expect("agent md request should build");

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
                // An MD/grompp failure on this minimal smoke system is fine;
                // it still proves the agent reached and ran GROMACS.
                println!("agent MD reached GROMACS, run failed: {error}");
                terminal = true;
                break;
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
    assert!(terminal, "expected a terminal Finished/Failed message");
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
