//! Run the `wsl_gromacs_*` acceptance tests with `--test-threads=1`. Each spawns a
//! `gmx mdrun` that helps itself to every core, so running them concurrently makes
//! them fight for the machine and time out — a failure that looks like a bug in the
//! pipeline and is not one.

use nalgebra::Point3;

use super::*;
use crate::domain::{Atom, UnitCell};

#[test]
fn mdrun_args_map_resources_to_gmx_flags() {
    let args = |cores, gpu| {
        mdrun_args(
            "nvt",
            "out.gro",
            ComputeResources { cores, gpu },
            input::Integrator::Leapfrog,
        )
    };
    let has =
        |a: &[String], flag: &str, val: &str| a.windows(2).any(|w| w[0] == flag && w[1] == val);

    // No request -> just the I/O flags; gmx picks its own threads/GPU.
    let auto = args(0, 0);
    assert!(auto.contains(&"-deffnm".to_string()));
    assert!(
        !auto
            .iter()
            .any(|a| a == "-nt" || a == "-ntmpi" || a == "-nb")
    );

    // CPU-only core request -> -nt N, no GPU offload.
    let cpu = args(6, 0);
    assert!(has(&cpu, "-nt", "6"));
    assert!(!cpu.iter().any(|a| a == "-nb"));

    // Single GPU -> one rank, full offload, cores as OpenMP threads, no PME rank.
    let gpu1 = args(8, 1);
    assert!(has(&gpu1, "-ntmpi", "1"));
    assert!(has(&gpu1, "-nb", "gpu"));
    assert!(has(&gpu1, "-pme", "gpu"));
    assert!(has(&gpu1, "-bonded", "gpu"));
    assert!(has(&gpu1, "-update", "gpu"));
    assert!(has(&gpu1, "-ntomp", "8"));
    assert!(!gpu1.iter().any(|a| a == "-npme"));
    assert!(!gpu1.iter().any(|a| a == "-nt"));

    // Multiple GPUs -> rank per GPU plus a dedicated PME rank; gmx maps ranks
    // to GPUs (no -gpu_id pin, which can't express device ids >= 10).
    let gpu2 = args(0, 2);
    assert!(has(&gpu2, "-ntmpi", "2"));
    assert!(has(&gpu2, "-npme", "1"));
    assert!(!gpu2.iter().any(|a| a == "-gpu_id"));

    // A many-GPU request stays well-formed (no malformed concatenated ids).
    let gpu12 = args(0, 12);
    assert!(has(&gpu12, "-ntmpi", "12"));
    assert!(!gpu12.iter().any(|a| a == "-gpu_id"));
}

#[test]
fn mdrun_args_do_not_force_unsupported_gpu_tasks_for_minimization() {
    let args = mdrun_args(
        "em",
        "em_out.gro",
        ComputeResources { cores: 16, gpu: 1 },
        input::Integrator::SteepestDescent,
    );
    let has = |flag: &str, val: &str| args.windows(2).any(|w| w[0] == flag && w[1] == val);

    assert!(has("-ntmpi", "1"));
    assert!(has("-nb", "gpu"));
    assert!(has("-ntomp", "16"));
    assert!(!args.iter().any(|arg| arg == "-pme"));
    assert!(!args.iter().any(|arg| arg == "-bonded"));
    assert!(!args.iter().any(|arg| arg == "-update"));
    assert!(!args.iter().any(|arg| arg == "-npme"));
}

#[test]
fn extract_fatal_error_pulls_block_even_when_progress_trails_it() {
    // GROMACS buffers stdout, so on a crash the (unbuffered) stderr error
    // can land before the trailing buffered progress — a plain tail would
    // return the progress, not the error. Extraction must find the error.
    let log = "\
-------------------------------------------------------
Fatal error:
Atom HE2 in residue HIS7 was not found in rtp entry.
Option -ignh will ignore all hydrogens in the input.
-------------------------------------------------------
Processing chain 1 'P' (457 atoms, 30 residues)
Identified residue ARG36 as a ending terminus.
";
    let extracted = extract_fatal_error(log).expect("fatal error block found");
    assert!(extracted.starts_with("Fatal error:"));
    assert!(extracted.contains("not found in rtp entry"));
    assert!(extracted.contains("-ignh"));
    // The trailing progress noise must not be carried into the message.
    assert!(!extracted.contains("ending terminus"));
}

#[test]
fn extract_fatal_error_absent_returns_none() {
    assert!(extract_fatal_error("all good\nWriting topology\n").is_none());
}

/// Hand-written, fully self-contained argon topology: Lennard-Jones only,
/// no bonded terms, no charges, and crucially no dependence on any external
/// force-field data files. Sigma/epsilon are given directly under
/// combination rule 2 (Lorentz-Berthelot). Eight single-atom `AR` molecules
/// line up with the eight atoms [`prepare_system`] writes to `conf.gro`.
const ARGON_TOP: &str = "\
[ defaults ]
1         2          no         1.0      1.0

[ atomtypes ]
; name  at.num  mass      charge  ptype  sigma     epsilon
  Ar    18      39.948    0.000   A      0.34050   0.99600

[ moleculetype ]
; name  nrexcl
  AR    1

[ atoms ]
; nr  type  resnr  residue  atom  cgnr  charge  mass
  1    Ar    1      AR       Ar    1     0.000   39.948

[ system ]
Argon

[ molecules ]
AR  8
";

/// Build a hermetic eight-atom argon box: a 2x2x2 grid at 5 angstrom
/// spacing centered in a 30 angstrom (3 nm) cubic cell. The spacing sits
/// well outside the LJ minimum so the starting energy is finite, and the
/// box comfortably exceeds twice the 1 nm cutoff so the Verlet
/// minimum-image check in grompp passes.
fn argon_box() -> Structure {
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
    Structure::with_cell(
        "argon",
        atoms,
        UnitCell::from_parameters(30.0, 30.0, 30.0, 90.0, 90.0, 90.0),
    )
}

fn wsl_gmx_launch() -> EngineLaunch {
    EngineLaunch {
        command_prefix: vec!["wsl.exe".to_string(), "-e".to_string()],
        program: "/usr/local/gromacs/bin/gmx".to_string(),
    }
}

/// Steps every acceptance stage is capped at.
const ACCEPTANCE_STEPS: u64 = 500;

/// Cap each realized stage at [`ACCEPTANCE_STEPS`] and rewrite its trajectory
/// cadence to match.
///
/// The protocol's equilibration lengths are the ones a real run wants (100 ps of
/// NVT and NPT apiece), so shortening has to happen here rather than in the
/// protocol. These tests assert the pipeline's *shape* — stage linking, checkpoint
/// threading, a decodable `.xtc` per dynamics stage — none of which needs more
/// than a few hundred steps of eight argon atoms. Leaving the lengths alone cost
/// minutes per run and bought no coverage.
///
/// The cadence must be rewritten too: it is derived from the full step count, so a
/// shortened stage would write zero frames and the trajectory assertions would
/// fail for a reason that has nothing to do with the code under test.
fn shorten_for_acceptance(stages: &mut [StageSpec]) {
    for spec in stages {
        spec.settings.nsteps = spec.settings.nsteps.min(ACCEPTANCE_STEPS);
        if let Some(output) = spec.settings.output.as_mut()
            && output.nstxout_compressed > 0
        {
            output.nstxout_compressed = (ACCEPTANCE_STEPS / 10) as u32;
        }
    }
}

#[test]
fn grompp_args_for_single_stage_match_legacy_form() {
    // No checkpoint and no restraints -> the exact minimization argument list.
    let args = build_grompp_args(
        "em.mdp",
        "conf.gro",
        None,
        None,
        "topol.top",
        "em.tpr",
        None,
    );
    let expected: Vec<String> = [
        "grompp",
        "-f",
        "em.mdp",
        "-c",
        "conf.gro",
        "-p",
        "topol.top",
        "-o",
        "em.tpr",
        "-maxwarn",
        "5",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    assert_eq!(args, expected);
}

#[test]
fn grompp_args_include_checkpoint_when_present() {
    let args = build_grompp_args(
        "npt.mdp",
        "nvt_out.gro",
        Some("nvt.cpt"),
        None,
        "topol.top",
        "npt.tpr",
        None,
    );
    let joined = args.join(" ");
    assert!(joined.contains("-t nvt.cpt"), "missing -t: {joined}");
    // -c precedes -t, and -p/-o/-maxwarn trail.
    assert!(joined.contains("-c nvt_out.gro -t nvt.cpt -p topol.top"));
}

#[test]
fn grompp_args_include_restraint_reference_when_restrained() {
    // A restrained stage passes the restraint reference coordinates via `-r`
    // (required by GROMACS >= 2018); reusing the `-c` file is the documented
    // approach.
    let args = build_grompp_args(
        "nvt.mdp",
        "em_out.gro",
        None,
        Some("em_out.gro"),
        "topol.top",
        "nvt.tpr",
        None,
    );
    assert!(
        args.join(" ").contains("-c em_out.gro -r em_out.gro"),
        "missing -r: {args:?}"
    );
}

#[test]
fn index_file_lists_system_and_freeze_groups() {
    let ndx = render_index_file(
        4,
        &FreezeSelection {
            group: "Framework".to_string(),
            atom_indices: vec![0, 1],
        },
    );
    assert!(ndx.contains("[ System ]"));
    assert!(ndx.contains("[ Framework ]"));
    // System covers all four atoms (1-based); the freeze group the first two.
    assert!(ndx.contains("1     2     3     4") || ndx.contains("1") && ndx.contains("4"));
    let frame_section = ndx.split("[ Framework ]").nth(1).unwrap();
    assert!(frame_section.contains('1') && frame_section.contains('2'));
    assert!(!frame_section.contains('3'));
}

#[test]
fn grompp_args_include_index_when_present() {
    let args = build_grompp_args(
        "em.mdp",
        "conf.gro",
        None,
        None,
        "topol.top",
        "em.tpr",
        Some("index.ndx"),
    );
    assert!(args.join(" ").contains("-n index.ndx"), "{args:?}");
}

#[test]
fn stage_links_resolve_against_prepared_system_and_prior_outputs() {
    use std::collections::HashMap;

    let system = PreparedSystem {
        working_dir: PathBuf::from("/wd"),
        conf_file: PathBuf::from("/wd/conf.gro"),
        topology_file: PathBuf::from("/wd/topol.top"),
        index_file: None,
        original_structure: Structure::empty(),
    };

    let mut outputs: HashMap<String, StageOutputs> = HashMap::new();
    outputs.insert(
        "nvt".to_string(),
        StageOutputs {
            output_gro: PathBuf::from("/wd/nvt_out.gro"),
            checkpoint: Some(PathBuf::from("/wd/nvt.cpt")),
            trajectory: None,
        },
    );

    assert_eq!(
        resolve_file_ref(&FileRef::PreparedConf, &system, &outputs).unwrap(),
        PathBuf::from("/wd/conf.gro")
    );
    assert_eq!(
        resolve_file_ref(
            &FileRef::Stage {
                stage: "nvt".to_string(),
                role: StageFileRole::Checkpoint,
            },
            &system,
            &outputs,
        )
        .unwrap(),
        PathBuf::from("/wd/nvt.cpt")
    );
    assert_eq!(
        resolve_file_ref(
            &FileRef::Stage {
                stage: "nvt".to_string(),
                role: StageFileRole::OutputGro,
            },
            &system,
            &outputs,
        )
        .unwrap(),
        PathBuf::from("/wd/nvt_out.gro")
    );
    // A missing trajectory is an error, not a silent empty path.
    assert!(
        resolve_file_ref(
            &FileRef::Stage {
                stage: "nvt".to_string(),
                role: StageFileRole::Trajectory,
            },
            &system,
            &outputs,
        )
        .is_err()
    );
    // Referencing a stage that has not run yet is an error.
    assert!(
        resolve_file_ref(
            &FileRef::Stage {
                stage: "npt".to_string(),
                role: StageFileRole::OutputGro,
            },
            &system,
            &outputs,
        )
        .is_err()
    );
}

#[test]
fn copy_topology_includes_carries_itp_files_and_staged_force_field() {
    let root = std::env::temp_dir().join("silicolab_copy_includes_test");
    let _ = fs::remove_dir_all(&root);
    let build = root.join("build");
    let run = root.join("run");
    fs::create_dir_all(build.join("charmm36.ff")).unwrap();
    fs::create_dir_all(&run).unwrap();

    fs::write(build.join("topol.top"), "; top\n").unwrap();
    fs::write(build.join("posre.itp"), "; restraints\n").unwrap();
    fs::write(build.join("charmm36.ff/ffnonbonded.itp"), "; nb\n").unwrap();

    copy_topology_includes(&build.join("topol.top"), &run).unwrap();

    // Both the sibling .itp and the whole staged force-field tree carry over.
    assert!(run.join("posre.itp").exists());
    assert!(run.join("charmm36.ff/ffnonbonded.itp").exists());

    let _ = fs::remove_dir_all(&root);
}

/// Real end-to-end energy minimization through the WSL GROMACS launch.
/// Ignored by default so it never fails on machines without WSL/GROMACS;
/// run with
/// `cargo test --release -- --ignored wsl_gromacs_energy_minimization`.
///
/// Unlike the `--version` detection check in `registry.rs`, this drives the
/// full Phase 1 path -- `to_gro` + inline topology + `grompp` + `mdrun` +
/// output parsing -- and proves grompp and mdrun both succeed on a
/// self-contained system and that a minimized structure with a finite
/// potential energy comes back out.
#[test]
#[ignore = "requires GROMACS inside WSL (Windows acceptance environment)"]
fn wsl_gromacs_energy_minimization_runs_end_to_end() {
    let working_dir = std::env::temp_dir().join("silicolab_gmx_em_integration");
    let _ = fs::remove_dir_all(&working_dir);

    let system = prepare_system(PrepareSystemRequest {
        structure: argon_box(),
        topology: TopologySource::Inline(ARGON_TOP.to_string()),
        working_dir,
        freeze: None,
    })
    .expect("system preparation should succeed");

    let result = run_stage(
        StageRequest {
            coordinate_input: system.conf_file.clone(),
            checkpoint_input: None,
            system,
            stage_name: "em".to_string(),
            settings: MdpSettings::energy_minimization(),
            compute: wsl_gmx_launch().into(),
            max_duration: Duration::from_secs(120),
        },
        Arc::new(AtomicBool::new(false)),
        |_| {},
    )
    .expect("energy minimization should run to completion");

    assert_eq!(
        result.structure.atoms.len(),
        8,
        "minimized structure should preserve all argon atoms"
    );
    let energy = result
        .final_potential_energy
        .expect("a final potential energy should be parsed from the mdrun log");
    assert!(
        energy.is_finite(),
        "final potential energy should be finite, got {energy}"
    );
}

/// Real end-to-end EM -> NVT -> NPT -> production on the self-contained
/// argon system, exercising [`run_pipeline`] and the stage-linking machinery
/// (coordinates/checkpoint threading) with the actual GROMACS binary.
/// Ignored by default; run with
/// `cargo test --release -- --ignored wsl_gromacs_full_md`.
#[test]
#[ignore = "requires GROMACS inside WSL (Windows acceptance environment)"]
fn wsl_gromacs_full_md_pipeline_runs_end_to_end() {
    use crate::workflows::molecular_dynamics::{MdProtocolOptions, full_protocol};

    let working_dir = std::env::temp_dir().join("silicolab_gmx_full_md_integration");
    let _ = fs::remove_dir_all(&working_dir);

    let system = prepare_system(PrepareSystemRequest {
        structure: argon_box(),
        topology: TopologySource::Inline(ARGON_TOP.to_string()),
        working_dir,
        freeze: None,
    })
    .expect("system preparation should succeed");

    // Trajectory saving is on by default, so every stage writes a compressed `.xtc`.
    let options = MdProtocolOptions {
        production_ps: 20.0,
        timestep_ps: 0.002,
        temperature_k: 94.0,
        relax_before_production: true,
        save_trajectory: true,
    };
    let mut stages = full_protocol(&options);
    shorten_for_acceptance(&mut stages);

    let results = run_pipeline(
        system,
        stages,
        wsl_gmx_launch().into(),
        Duration::from_secs(120),
        Arc::new(AtomicBool::new(false)),
        |_| {},
    )
    .expect("full EM/NVT/NPT/production pipeline should run to completion");

    assert_eq!(results.len(), 4, "expected EM, NVT, NPT, production stages");
    let production = results.last().expect("production stage present");
    assert_eq!(production.structure.atoms.len(), 8);
    assert!(
        production.checkpoint.is_some(),
        "production should write a checkpoint"
    );

    // Every dynamics stage must write a decodable trajectory (the real-tool
    // gate for per-stage playback): each genuine `.xtc` parses into one or
    // more frames over the same atom count, with finite Angstrom coordinates.
    // Minimization (`em`) relaxes to a minimum and writes no motion track.
    let mut dynamics_trajectories = 0;
    for stage in &results {
        if stage.stage_name == "em" {
            assert!(
                stage.trajectory.is_none(),
                "minimization should not write a trajectory"
            );
            continue;
        }
        let trajectory_path = stage
            .trajectory
            .as_ref()
            .unwrap_or_else(|| panic!("stage '{}' should write a trajectory", stage.stage_name));
        let trajectory = crate::io::trajectory::read_xtc(trajectory_path)
            .unwrap_or_else(|_| panic!("decode '{}' .xtc", stage.stage_name));
        assert!(
            trajectory.frame_count() >= 1,
            "stage '{}' trajectory should contain at least one frame",
            stage.stage_name
        );
        assert_eq!(
            trajectory.natoms(),
            stage.structure.atoms.len(),
            "stage '{}' trajectory atom count should match its structure",
            stage.stage_name
        );
        for frame in 0..trajectory.frame_count() {
            for atom in 0..trajectory.natoms() {
                let position = trajectory.position(frame, atom);
                assert!(
                    position.coords.iter().all(|value| value.is_finite()),
                    "stage '{}' frame {frame} atom {atom} has non-finite coordinates",
                    stage.stage_name
                );
            }
        }
        dynamics_trajectories += 1;
    }
    assert_eq!(
        dynamics_trajectories, 3,
        "NVT, NPT and production should each write a trajectory"
    );
}

/// Like [`wsl_gromacs_full_md_pipeline_runs_end_to_end`], but the topology
/// is generated automatically from the structure (no hand-written `.top`),
/// proving the auto-topology path produces a grompp-valid file. Run with
/// `cargo test --release -- --ignored wsl_gromacs_generated_topology`.
#[test]
#[ignore = "requires GROMACS inside WSL (Windows acceptance environment)"]
fn wsl_gromacs_generated_topology_runs_full_md() {
    use crate::engines::gromacs::render_top;
    use crate::md::MdTopology;
    use crate::workflows::molecular_dynamics::{MdProtocolOptions, full_protocol};

    let working_dir = std::env::temp_dir().join("silicolab_gmx_generated_top_integration");
    let _ = fs::remove_dir_all(&working_dir);

    let structure = argon_box();
    // Build the engine-neutral topology, then render it to a GROMACS .top —
    // exactly the path the System Builder + simulate stage take.
    let topology =
        MdTopology::from_structure(&structure).expect("topology generation should succeed");

    let system = prepare_system(PrepareSystemRequest {
        structure,
        topology: TopologySource::Inline(render_top(&topology)),
        working_dir,
        freeze: None,
    })
    .expect("system preparation should succeed");

    let options = MdProtocolOptions {
        production_ps: 20.0,
        timestep_ps: 0.002,
        temperature_k: 94.0,
        relax_before_production: true,
        save_trajectory: true,
    };
    let mut stages = full_protocol(&options);
    shorten_for_acceptance(&mut stages);

    let results = run_pipeline(
        system,
        stages,
        wsl_gmx_launch().into(),
        Duration::from_secs(120),
        Arc::new(AtomicBool::new(false)),
        |_| {},
    )
    .expect("full pipeline with generated topology should run to completion");

    assert_eq!(results.len(), 4);
    assert_eq!(results.last().unwrap().structure.atoms.len(), 8);
}
