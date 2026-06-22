//! Assemble a simulation-ready system with GROMACS' own bundled force fields.
//!
//! This hands the solute to the engine and lets it assign types, charges, and
//! connectivity from the force field the user picked. The pipeline is the
//! standard prepare-a-box sequence — process the solute, define the periodic
//! cell, fill it with water, and replace some solvent with ions — each step a
//! single `gmx` sub-tool:
//!
//! 1. `pdb2gmx -ff <ff> -water <model>` → per-atom topology (`topol.top`).
//! 2. `editconf` → center the solute in a periodic box of the chosen shape/size.
//! 3. `solvate` (optional) → fill the box with the water model's solvent box.
//! 4. `grompp` + `genion` (optional) → neutralize / add a salt bath.
//!
//! The result is the final solvated structure plus the engine topology a run
//! reuses.

use std::{
    path::PathBuf,
    sync::{Arc, atomic::AtomicBool},
    time::Duration,
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::{
    domain::Structure,
    engines::{
        gromacs::{
            exec::run_gmx,
            runner::{GromacsProgress, subprocess_failure},
        },
        remote::Compute,
    },
    io::formats::{gro::parse_gro, pdb::to_pdb},
    workflows::molecular_dynamics::{BoxShape, BoxSizing, MdSystemConfig, WaterModel},
};

const ANGSTROM_TO_NM: f32 = 0.1;

/// Minimal run parameters that let `grompp` build the `.tpr` `genion` needs. No
/// steps are taken; it exists only to produce a processable topology for ion
/// placement.
const IONS_MDP: &str = "\
; SilicoLab-generated parameters for genion preprocessing
integrator    = steep
nsteps        = 0
cutoff-scheme = Verlet
coulombtype   = PME
rvdw          = 1.0
rcoulomb      = 1.0
";

/// Ion placement for the build's `genion` step.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IonOptions {
    /// Add the minimum ions needed to make the system net-neutral.
    pub neutralize: bool,
    /// Target background salt concentration in mol/L, if any.
    pub concentration_molar: Option<f32>,
    /// Cation / anion residue names the force field defines (e.g. `NA`, `CL`).
    pub positive_ion: String,
    pub negative_ion: String,
}

impl IonOptions {
    fn is_active(&self) -> bool {
        self.neutralize || self.concentration_molar.is_some()
    }
}

/// A request to assemble a system with GROMACS. `structure` is the bare solute;
/// the periodic cell is created by `editconf`, so any existing cell is ignored.
pub struct BuildRequest {
    pub structure: Structure,
    pub working_dir: PathBuf,
    /// How to launch `gmx` and where it runs (local or remote over SSH).
    pub compute: Compute,
    /// `pdb2gmx -ff` token (e.g. `amber99sb-ildn`).
    pub force_field: String,
    pub water: WaterModel,
    pub box_config: MdSystemConfig,
    pub solvate: bool,
    pub ions: Option<IonOptions>,
    pub max_duration: Duration,
}

/// A successful GROMACS build.
pub struct BuildOutcome {
    /// Final coordinates (solvated/ionized as requested), with the periodic box.
    pub structure: Structure,
    pub working_dir: PathBuf,
    /// GROMACS topology written by the pipeline (`topol.top`).
    pub topology_file: PathBuf,
    pub summary: String,
}

/// Run the build pipeline, reporting each `gmx` step's progress.
pub fn build_system<F>(
    req: BuildRequest,
    cancel: Arc<AtomicBool>,
    mut report: F,
) -> Result<BuildOutcome>
where
    F: FnMut(GromacsProgress),
{
    let wd = req.working_dir.clone();
    std::fs::create_dir_all(&wd)
        .with_context(|| format!("creating GROMACS working directory {}", wd.display()))?;

    let run =
        |args: Vec<String>, stdin: Option<Vec<u8>>, tool: &str, report: &mut F| -> Result<()> {
            let outcome = run_gmx(
                &req.compute,
                &wd,
                args,
                stdin,
                req.max_duration,
                Arc::clone(&cancel),
                report,
            )?;
            if outcome.success() {
                Ok(())
            } else {
                Err(subprocess_failure(tool, &outcome))
            }
        };

    // 1. Write the solute with its real residue/atom names so the engine can
    //    match force-field residue templates.
    report(GromacsProgress::Stage("writing solute.pdb".to_string()));
    std::fs::write(wd.join("solute.pdb"), to_pdb(&req.structure)?)
        .with_context(|| "writing solute.pdb".to_string())?;

    // 2. pdb2gmx: assign per-atom types and charges from the chosen force field.
    //    `-ignh` discards any hydrogens in the input and lets pdb2gmx add them
    //    fresh in the protonation the force field expects. Input hydrogens from
    //    PDBs routinely use naming/protonation the force field's residue
    //    templates don't recognize, which otherwise aborts pdb2gmx with a fatal
    //    "hydrogen not found" error (GROMACS itself recommends -ignh there). For
    //    a system builder, regenerating hydrogens is the right default anyway.
    report(GromacsProgress::Stage(format!(
        "gmx pdb2gmx (-ff {}, -water {})",
        req.force_field,
        req.water.db_token()
    )));
    run(
        vec![
            "pdb2gmx".into(),
            "-f".into(),
            "solute.pdb".into(),
            "-o".into(),
            "processed.gro".into(),
            "-p".into(),
            "topol.top".into(),
            "-i".into(),
            "posre.itp".into(),
            "-ff".into(),
            req.force_field.clone(),
            "-water".into(),
            req.water.db_token().into(),
            "-ignh".into(),
        ],
        None,
        "pdb2gmx",
        &mut report,
    )?;

    // 3. editconf: center the solute in a periodic box of the chosen geometry.
    report(GromacsProgress::Stage("gmx editconf".to_string()));
    let mut editconf = vec![
        "editconf".into(),
        "-f".into(),
        "processed.gro".into(),
        "-o".into(),
        "boxed.gro".into(),
        "-c".into(),
    ];
    editconf.extend(editconf_box_args(&req.box_config));
    run(editconf, None, "editconf", &mut report)?;

    let mut current = "boxed.gro".to_string();

    // 4. solvate: fill the box with the water model's pre-equilibrated solvent.
    if req.solvate {
        report(GromacsProgress::Stage("gmx solvate".to_string()));
        run(
            vec![
                "solvate".into(),
                "-cp".into(),
                current.clone(),
                "-cs".into(),
                solvent_box(req.water).into(),
                "-o".into(),
                "solvated.gro".into(),
                "-p".into(),
                "topol.top".into(),
            ],
            None,
            "solvate",
            &mut report,
        )?;
        current = "solvated.gro".to_string();

        // 5. genion: replace some water with ions to neutralize / add salt.
        if let Some(ions) = req.ions.as_ref().filter(|i| i.is_active()) {
            report(GromacsProgress::Stage("gmx grompp (ions)".to_string()));
            std::fs::write(wd.join("ions.mdp"), IONS_MDP)
                .with_context(|| "writing ions.mdp".to_string())?;
            run(
                vec![
                    "grompp".into(),
                    "-f".into(),
                    "ions.mdp".into(),
                    "-c".into(),
                    current.clone(),
                    "-p".into(),
                    "topol.top".into(),
                    "-o".into(),
                    "ions.tpr".into(),
                    "-maxwarn".into(),
                    "5".into(),
                ],
                None,
                "grompp (ions)",
                &mut report,
            )?;

            report(GromacsProgress::Stage("gmx genion".to_string()));
            let mut genion = vec![
                "genion".into(),
                "-s".into(),
                "ions.tpr".into(),
                "-o".into(),
                "ionized.gro".into(),
                "-p".into(),
                "topol.top".into(),
                "-pname".into(),
                ions.positive_ion.clone(),
                "-nname".into(),
                ions.negative_ion.clone(),
            ];
            if ions.neutralize {
                genion.push("-neutral".into());
            }
            if let Some(conc) = ions.concentration_molar {
                genion.push("-conc".into());
                genion.push(format!("{conc}"));
            }
            // genion prompts for the continuous group to replace with ions;
            // feed it the solvent group.
            run(genion, Some(b"SOL\n".to_vec()), "genion", &mut report)?;
            current = "ionized.gro".to_string();
        }
    }

    // 6. Load the final coordinates back as the entry structure.
    let final_path = wd.join(&current);
    let gro = std::fs::read_to_string(&final_path)
        .with_context(|| format!("reading final coordinates {}", final_path.display()))?;
    let structure = parse_gro(&gro).with_context(|| format!("parsing {}", final_path.display()))?;

    let summary = format!(
        "Built MD system with GROMACS ({} / {}): {} atoms",
        req.force_field,
        req.water.db_token(),
        structure.atoms.len()
    );

    Ok(BuildOutcome {
        structure,
        topology_file: wd.join("topol.top"),
        working_dir: wd,
        summary,
    })
}

/// `editconf` box arguments for the chosen shape and sizing. Padding mode uses a
/// single uniform clearance (`-d`) — the largest requested per-axis padding, so
/// no axis ends up tighter than asked. Absolute mode sets explicit rectangular
/// box vectors (`-box`).
fn editconf_box_args(config: &MdSystemConfig) -> Vec<String> {
    let bt = match config.shape {
        BoxShape::Orthorhombic => "triclinic",
        BoxShape::Cubic => "cubic",
        BoxShape::RhombicDodecahedron => "dodecahedron",
        BoxShape::TruncatedOctahedron => "octahedron",
    };
    match config.sizing {
        BoxSizing::Padding { padding_angstrom } => {
            let d_nm = padding_angstrom.iter().copied().fold(0.0_f32, f32::max) * ANGSTROM_TO_NM;
            vec!["-bt".into(), bt.into(), "-d".into(), format!("{d_nm}")]
        }
        BoxSizing::Absolute { edges_angstrom } => vec![
            "-box".into(),
            format!("{}", edges_angstrom[0] * ANGSTROM_TO_NM),
            format!("{}", edges_angstrom[1] * ANGSTROM_TO_NM),
            format!("{}", edges_angstrom[2] * ANGSTROM_TO_NM),
        ],
    }
}

/// The pre-equilibrated solvent configuration `solvate -cs` should use for a
/// water model: four/five-point models have their own box; everything else uses
/// the standard three-point box.
fn solvent_box(water: WaterModel) -> &'static str {
    match water {
        WaterModel::Tip4p | WaterModel::Tip4pEw => "tip4p.gro",
        WaterModel::Tip5p | WaterModel::Tip5pEwald => "tip5p.gro",
        _ => "spc216.gro",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engines::registry::EngineLaunch;
    use crate::workflows::molecular_dynamics::{BoxShape, MdSystemConfig};

    #[test]
    fn padding_box_uses_largest_clearance_and_shape() {
        let config = MdSystemConfig::with_uniform_padding(10.0, BoxShape::RhombicDodecahedron);
        let args = editconf_box_args(&config);
        assert_eq!(args, ["-bt", "dodecahedron", "-d", "1"]);
    }

    #[test]
    fn absolute_box_emits_nm_vectors() {
        let config =
            MdSystemConfig::with_absolute_edges([30.0, 40.0, 50.0], BoxShape::Orthorhombic);
        let args = editconf_box_args(&config);
        assert_eq!(args, ["-box", "3", "4", "5"]);
    }

    #[test]
    fn solvent_box_maps_four_and_five_point_models() {
        assert_eq!(solvent_box(WaterModel::Tip4p), "tip4p.gro");
        assert_eq!(solvent_box(WaterModel::Tip5pEwald), "tip5p.gro");
        assert_eq!(solvent_box(WaterModel::Spc), "spc216.gro");
        assert_eq!(solvent_box(WaterModel::Tip3p), "spc216.gro");
    }

    /// End-to-end build of a solvated, ionized capped-alanine peptide through the
    /// real engine. Run with
    /// `cargo test --release -- --ignored wsl_gromacs_build_solvated`.
    #[test]
    #[ignore = "requires GROMACS inside WSL (Windows acceptance environment)"]
    fn wsl_gromacs_build_solvated_peptide_runs_end_to_end() {
        use crate::io::formats::pdb::parse_pdb;

        let working_dir = std::env::temp_dir().join("silicolab_gmx_build_peptide");
        let _ = std::fs::remove_dir_all(&working_dir);

        let pdb = include_str!("../../workflows/molecular_dynamics/fixtures/capped_ala.pdb");
        let structure = parse_pdb(pdb).expect("fixture parses");
        let solute_atoms = structure.atoms.len();

        let launch = EngineLaunch {
            command_prefix: vec!["wsl.exe".to_string(), "-e".to_string()],
            program: "/usr/local/gromacs/bin/gmx".to_string(),
        };

        let outcome = build_system(
            BuildRequest {
                structure,
                working_dir: working_dir.clone(),
                compute: launch.into(),
                force_field: "amber99sb-ildn".to_string(),
                water: WaterModel::Tip3p,
                box_config: MdSystemConfig::with_uniform_padding(12.0, BoxShape::Cubic),
                solvate: true,
                ions: Some(IonOptions {
                    neutralize: true,
                    concentration_molar: Some(0.15),
                    positive_ion: "NA".to_string(),
                    negative_ion: "CL".to_string(),
                }),
                max_duration: Duration::from_secs(600),
            },
            Arc::new(AtomicBool::new(false)),
            |_| {},
        )
        .expect("GROMACS build should complete");

        assert!(
            outcome.topology_file.exists(),
            "topol.top should be written"
        );
        assert!(
            outcome.structure.cell.is_some(),
            "built system must have a periodic box"
        );
        assert!(
            outcome.structure.atoms.len() > solute_atoms,
            "solvation should add atoms ({solute_atoms} -> {})",
            outcome.structure.atoms.len()
        );
    }

    /// Minimal end-to-end of the whole feature: GROMACS *builds* a system
    /// (pdb2gmx topology + box) and then a *separate* run directory reuses that
    /// `topol.top` via [`TopologySource::File`] to energy-minimize it — exactly
    /// the System Builder → Run MD path. Proves the generated topology is
    /// grompp-valid and runnable across run directories. Run with
    /// `cargo test --release -- --ignored wsl_gromacs_build_then_minimize`.
    #[test]
    #[ignore = "requires GROMACS inside WSL (Windows acceptance environment)"]
    fn wsl_gromacs_build_then_minimize_runs_end_to_end() {
        use crate::{
            engines::gromacs::{
                MdpSettings, PrepareSystemRequest, StageRequest, TopologySource, prepare_system,
                run_stage,
            },
            io::formats::pdb::parse_pdb,
        };

        let root = std::env::temp_dir().join("silicolab_gmx_build_then_min");
        let _ = std::fs::remove_dir_all(&root);
        let build_dir = root.join("build");
        let run_dir = root.join("run");

        let pdb = include_str!("../../workflows/molecular_dynamics/fixtures/capped_ala.pdb");
        let structure = parse_pdb(pdb).expect("fixture parses");

        let launch = EngineLaunch {
            command_prefix: vec!["wsl.exe".to_string(), "-e".to_string()],
            program: "/usr/local/gromacs/bin/gmx".to_string(),
        };

        // Build: peptide in a box, no solvation, to keep the run fast.
        let outcome = build_system(
            BuildRequest {
                structure,
                working_dir: build_dir,
                compute: launch.clone().into(),
                force_field: "amber99sb-ildn".to_string(),
                water: WaterModel::Tip3p,
                box_config: MdSystemConfig::with_uniform_padding(10.0, BoxShape::Cubic),
                solvate: false,
                ions: None,
                max_duration: Duration::from_secs(600),
            },
            Arc::new(AtomicBool::new(false)),
            |_| {},
        )
        .expect("GROMACS build should complete");

        // Reuse the built topology from a fresh run directory, mirroring how
        // Run MD resolves `topol.top` from the build run.
        let system = prepare_system(PrepareSystemRequest {
            structure: outcome.structure,
            topology: TopologySource::File(outcome.topology_file),
            working_dir: run_dir,
            freeze: None,
        })
        .expect("preparing a run from the built topology should succeed");

        let result = run_stage(
            StageRequest {
                coordinate_input: system.conf_file.clone(),
                checkpoint_input: None,
                system,
                stage_name: "em".to_string(),
                settings: MdpSettings::energy_minimization(),
                compute: launch.into(),
                max_duration: Duration::from_secs(300),
            },
            Arc::new(AtomicBool::new(false)),
            |_| {},
        )
        .expect("energy minimization on the built system should complete");

        let energy = result
            .final_potential_energy
            .expect("minimization should report a final potential energy");
        assert!(
            energy.is_finite(),
            "final energy should be finite: {energy}"
        );
    }
}
