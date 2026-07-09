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
    domain::{
        AtomCategory, Structure,
        glycan::linkage_topology::{self, BondLinkage},
    },
    engines::{
        gromacs::{
            carb_topology::build_glycan_topology,
            exec::run_gmx,
            forcefield_assets::{self, bundle},
            glycoprotein_topology::{self, merge_glycan_into_protein_topology},
            runner::{GromacsProgress, subprocess_failure},
            topgen::render_top,
        },
        remote::Compute,
    },
    io::formats::{gro::parse_gro, pdb::to_pdb},
    md::{BoxShape, BoxSizing, MdSystemConfig, WaterModel},
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

    if bundle(&req.force_field).is_some() {
        report(GromacsProgress::Stage(format!(
            "staging {} force field",
            req.force_field
        )));
        forcefield_assets::stage_forcefield(&req.force_field, &wd)?;
    }

    let run_pdb2gmx = |solute: &Structure, report: &mut F| -> Result<()> {
        report(GromacsProgress::Stage("writing solute.pdb".to_string()));
        std::fs::write(wd.join("solute.pdb"), to_pdb(solute)?)
            .with_context(|| "writing solute.pdb".to_string())?;

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
            report,
        )
    };

    let editconf_input = if is_pure_glycan(&req.structure) {
        build_glycan_inputs(&req, &wd, &mut report)?
    } else if is_glycoprotein(&req.structure) {
        let protein_only = glycoprotein_topology::protein_only_structure(&req.structure)?;
        run_pdb2gmx(&protein_only, &mut report)?;
        merge_glycoprotein_topology(&req, &wd, &mut report)?
    } else {
        run_pdb2gmx(&req.structure, &mut report)?;
        "processed.gro".to_string()
    };

    // 3. editconf: center the solute in a periodic box of the chosen geometry.
    report(GromacsProgress::Stage("gmx editconf".to_string()));
    let mut editconf = vec![
        "editconf".into(),
        "-f".into(),
        editconf_input,
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

fn is_pure_glycan(structure: &Structure) -> bool {
    if structure.atoms.is_empty() || structure.biopolymer.is_none() {
        return false;
    }
    (0..structure.atoms.len()).all(|i| structure.atom_category(i) == AtomCategory::Carbohydrate)
}

fn is_glycoprotein(structure: &Structure) -> bool {
    if structure.atoms.is_empty() || structure.biopolymer.is_none() {
        return false;
    }
    let mut has_protein = false;
    let mut has_carbohydrate = false;
    for i in 0..structure.atoms.len() {
        match structure.atom_category(i) {
            AtomCategory::Protein => has_protein = true,
            AtomCategory::Carbohydrate => has_carbohydrate = true,
            _ => {}
        }
    }
    has_protein
        && has_carbohydrate
        && structure
            .biopolymer
            .as_ref()
            .map(|biopolymer| {
                linkage_topology::cross_residue_linkages(structure, biopolymer)
                    .iter()
                    .any(|cross| matches!(cross.linkage, BondLinkage::GlycanProtein { .. }))
            })
            .unwrap_or(false)
}

fn merge_glycoprotein_topology<F>(
    req: &BuildRequest,
    wd: &std::path::Path,
    report: &mut F,
) -> Result<String>
where
    F: FnMut(GromacsProgress),
{
    report(GromacsProgress::Stage(
        "merging glycan into protein topology".to_string(),
    ));
    let top_path = wd.join("topol.top");
    let protein_top = std::fs::read_to_string(&top_path)
        .with_context(|| format!("reading {}", top_path.display()))?;
    let merged =
        merge_glycan_into_protein_topology(&protein_top, &req.structure, &req.force_field)?;
    std::fs::write(&top_path, merged).with_context(|| format!("writing {}", top_path.display()))?;

    let gro_path = wd.join("processed.gro");
    let processed = std::fs::read_to_string(&gro_path)
        .with_context(|| format!("reading {}", gro_path.display()))?;
    let with_glycan = glycoprotein_topology::append_glycan_coordinates(&processed, &req.structure)?;
    std::fs::write(&gro_path, with_glycan)
        .with_context(|| format!("writing {}", gro_path.display()))?;
    Ok("processed.gro".to_string())
}

fn build_glycan_inputs<F>(
    req: &BuildRequest,
    wd: &std::path::Path,
    report: &mut F,
) -> Result<String>
where
    F: FnMut(GromacsProgress),
{
    report(GromacsProgress::Stage(
        "generating glycan topology".to_string(),
    ));
    let topology = build_glycan_topology(&req.structure, &req.force_field)?;
    std::fs::write(wd.join("solute.pdb"), to_pdb(&req.structure)?)
        .with_context(|| "writing solute.pdb".to_string())?;
    std::fs::write(wd.join("topol.top"), render_top(&topology))
        .with_context(|| "writing topol.top".to_string())?;
    Ok("solute.pdb".to_string())
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
    use crate::md::{BoxShape, MdSystemConfig};

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

        let pdb = include_str!("../../md/fixtures/capped_ala.pdb");
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

    #[test]
    #[ignore = "requires GROMACS inside WSL (Windows acceptance environment)"]
    fn wsl_gromacs_build_glycan_topology_runs_end_to_end() {
        use crate::workflows::glycan::glycan_to_structure;

        let working_dir = std::env::temp_dir().join("silicolab_gmx_build_glycan");
        let _ = std::fs::remove_dir_all(&working_dir);

        let structure = glycan_to_structure(
            "Man(a1-3)[Man(a1-6)]Man(b1-4)GlcNAc(b1-4)GlcNAc",
            Some("n-glycan-core"),
        )
        .expect("glycan builds");
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
                force_field: "charmm36".to_string(),
                water: WaterModel::Tip3p,
                box_config: MdSystemConfig::with_uniform_padding(12.0, BoxShape::Cubic),
                solvate: false,
                ions: None,
                max_duration: Duration::from_secs(600),
            },
            Arc::new(AtomicBool::new(false)),
            |_| {},
        )
        .expect("glycan build should complete");

        assert!(
            outcome.topology_file.exists(),
            "topol.top should be written"
        );
        assert!(
            outcome.structure.cell.is_some(),
            "built glycan must have a periodic box"
        );
        assert_eq!(
            outcome.structure.atoms.len(),
            solute_atoms,
            "the un-solvated glycan keeps all atoms"
        );
    }

    const TRIPEPTIDE_ASN_PDB: &str = "\
ATOM      1  N   ALA A   1       0.000   0.000   0.000  1.00  0.00           N
ATOM      2  CA  ALA A   1       1.458   0.000   0.000  1.00  0.00           C
ATOM      3  C   ALA A   1       2.009   1.420   0.000  1.00  0.00           C
ATOM      4  O   ALA A   1       1.251   2.390   0.000  1.00  0.00           O
ATOM      5  CB  ALA A   1       1.988  -0.773  -1.199  1.00  0.00           C
ATOM      6  N   ASN A   2       3.332   1.540   0.000  1.00  0.00           N
ATOM      7  CA  ASN A   2       3.999   2.840   0.000  1.00  0.00           C
ATOM      8  CB  ASN A   2       5.520   2.680   0.000  1.00  0.00           C
ATOM      9  CG  ASN A   2       6.230   4.020   0.000  1.00  0.00           C
ATOM     10  OD1 ASN A   2       5.620   5.090   0.000  1.00  0.00           O
ATOM     11  ND2 ASN A   2       7.560   4.000   0.000  1.00  0.00           N
ATOM     12  C   ASN A   2       3.560   3.660   1.210  1.00  0.00           C
ATOM     13  O   ASN A   2       3.450   3.130   2.320  1.00  0.00           O
ATOM     14  N   ALA A   3       3.310   4.960   1.030  1.00  0.00           N
ATOM     15  CA  ALA A   3       2.880   5.860   2.100  1.00  0.00           C
ATOM     16  C   ALA A   3       1.430   5.560   2.470  1.00  0.00           C
ATOM     17  O   ALA A   3       0.580   5.420   1.590  1.00  0.00           O
ATOM     18  CB  ALA A   3       3.010   7.320   1.660  1.00  0.00           C
END
";

    #[test]
    #[ignore = "requires GROMACS inside WSL (Windows acceptance environment)"]
    fn wsl_gromacs_build_glycoprotein_topology_runs_end_to_end() {
        use crate::domain::ResidueId;
        use crate::io::formats::pdb::parse_pdb;
        use crate::workflows::glycan::glycoprotein::glycosylate_protein;

        let working_dir = std::env::temp_dir().join("silicolab_gmx_build_glycoprotein");
        let _ = std::fs::remove_dir_all(&working_dir);

        let protein = parse_pdb(TRIPEPTIDE_ASN_PDB).expect("fixture parses");
        let anchor = ResidueId::new('A', 2, ' ');
        let structure = glycosylate_protein(&protein, "GlcNAc", anchor, None, None)
            .expect("glycosylation succeeds")
            .structure;
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
                force_field: "charmm36".to_string(),
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
        .expect("glycoprotein build should complete");

        assert!(
            outcome.topology_file.exists(),
            "topol.top should be written"
        );
        assert!(
            outcome.structure.cell.is_some(),
            "built glycoprotein must have a periodic box"
        );
        assert!(
            outcome.structure.atoms.len() > solute_atoms,
            "solvating the glycoprotein adds atoms ({solute_atoms} -> {})",
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

        let pdb = include_str!("../../md/fixtures/capped_ala.pdb");
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
