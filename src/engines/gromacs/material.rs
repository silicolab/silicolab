//! Assemble and run GROMACS systems for covalent framework materials — 2D
//! nanosheets and other bonded periodic structures.
//!
//! Unlike [`build`](super::build), this path does **not** use `pdb2gmx`: a
//! nanosheet has no residue template. Instead the topology is generated directly
//! from the structure's own bonds ([`MdTopology::framework`]) and rendered to a
//! self-contained `.top`. Two run shapes are produced, picked by
//! [`FrameworkMode`]:
//!
//! * **Rigid** — the sheet is frozen by a `Framework` index group; the run sets
//!   `freezegrps`. The topology has no bonded terms, only Lennard-Jones sites and
//!   explicit exclusions.
//! * **Flexible** — the sheet is a single molecule bonded across the periodic
//!   boundary; the run sets `periodic-molecules = yes`.
//!
//! [`framework_run_hints`] reports which `.mdp`/freeze settings a run must apply
//! for a given mode, so the build and the run agree.

use std::{
    path::PathBuf,
    sync::{Arc, atomic::AtomicBool},
    time::Duration,
};

use anyhow::{Context, Result, bail};

use crate::domain::{Structure, UnitCell};
use crate::engines::gromacs::{
    FreezeSelection,
    custom_ff::custom_types,
    exec::run_gmx,
    input::to_gro,
    runner::{GromacsProgress, subprocess_failure},
    topgen::render_top,
};
use crate::engines::remote::{self, Compute, Transport};
use crate::io::formats::gro::parse_gro;
use crate::workflows::molecular_dynamics::{
    FrameworkMode, MdTopology, SolvationOptions, WaterModel, ensure_periodic_cutoff_fits,
    set_slab_c_axis, solvent_definitions,
};

/// The run-time settings a framework system needs, derived from its model.
/// `periodic_molecules` goes onto the stage's `.mdp`; `freeze_group`, when set,
/// is both the `freezegrps` name and the index group [`prepare_system`] writes.
///
/// [`prepare_system`]: super::prepare_system
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameworkRunHints {
    pub periodic_molecules: bool,
    pub freeze_group: Option<String>,
}

/// The index-group name a rigid framework's frozen atoms are written under.
pub const FRAMEWORK_FREEZE_GROUP: &str = "Framework";

/// The `.mdp`/freeze settings a run must apply for `mode`.
pub fn framework_run_hints(mode: FrameworkMode) -> FrameworkRunHints {
    match mode {
        FrameworkMode::Rigid => FrameworkRunHints {
            periodic_molecules: false,
            freeze_group: Some(FRAMEWORK_FREEZE_GROUP.to_string()),
        },
        FrameworkMode::Flexible => FrameworkRunHints {
            periodic_molecules: true,
            freeze_group: None,
        },
    }
}

/// A [`FreezeSelection`] freezing the first `framework_atom_count` atoms (the
/// sheet always precedes any later-added solvent), under [`FRAMEWORK_FREEZE_GROUP`].
pub fn framework_freeze_selection(framework_atom_count: usize) -> FreezeSelection {
    FreezeSelection {
        group: FRAMEWORK_FREEZE_GROUP.to_string(),
        atom_indices: (0..framework_atom_count).collect(),
    }
}

/// A request to assemble a runnable GROMACS system for a framework material.
pub struct MaterialBuildRequest {
    /// The nanosheet: a periodic structure with a cell and bonds.
    pub structure: Structure,
    pub mode: FrameworkMode,
    pub working_dir: PathBuf,
    /// How to launch `gmx` and where it runs (local or remote over SSH).
    pub compute: Compute,
    /// Solvation request; `None` builds a bare (vacuum) periodic system.
    pub solvation: Option<SolvationOptions>,
    /// A user-supplied GROMACS `.itp` force-field fragment merged into the
    /// generated topology via `#include`, enabling elements (or overriding atom
    /// types) the built-in tables don't cover. `None` uses built-in parameters
    /// only. Only the rigid model accepts elements that rest solely on this.
    pub custom_force_field: Option<String>,
    /// Explicit simulation cell to use as the box, preserving its shape (e.g. a
    /// hexagonal nanosheet lattice). The atoms keep their Cartesian positions;
    /// only the periodic vectors are set. When `None`, the out-of-plane axis is
    /// instead opened automatically to fit the cutoff and any solvent column.
    pub cell_override: Option<UnitCell>,
    /// Total solvent column thickness (Å) to open along the out-of-plane axis
    /// when solvating, on top of the sheet's own thickness. Ignored when
    /// `cell_override` fixes the box.
    pub solvent_gap_angstrom: f32,
    /// Nonbonded cutoff (nm) the run will use; the cell must clear its minimum
    /// image.
    pub cutoff_nm: f32,
    pub max_duration: Duration,
}

/// A successfully assembled framework system, ready for a run to reuse.
pub struct MaterialBuildOutcome {
    /// Final coordinates (solvated/ionized as requested), with the periodic cell.
    pub structure: Structure,
    pub working_dir: PathBuf,
    /// The self-contained framework topology (updated by solvate/genion).
    pub topology_file: PathBuf,
    /// Number of leading atoms that make up the framework itself (the freeze
    /// group, when the run freezes the sheet).
    pub framework_atom_count: usize,
    /// The `.mdp`/freeze settings a run must apply for this system's model.
    pub hints: FrameworkRunHints,
    pub summary: String,
}

/// Assemble a framework system: generate the topology directly from the
/// structure's bonds (no `pdb2gmx`), open enough room along the out-of-plane
/// axis, and optionally solvate and ionize with `gmx solvate`/`genion`.
pub fn build_material_system<F>(
    req: MaterialBuildRequest,
    cancel: Arc<AtomicBool>,
    mut report: F,
) -> Result<MaterialBuildOutcome>
where
    F: FnMut(GromacsProgress),
{
    let wd = req.working_dir.clone();
    std::fs::create_dir_all(&wd)
        .with_context(|| format!("creating GROMACS working directory {}", wd.display()))?;

    let cell = req
        .structure
        .cell
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("a framework MD system needs a periodic cell"))?;
    if req.structure.bonds.is_empty() {
        bail!("the structure has no bonds; it is not a covalent framework");
    }

    let framework_atom_count = req.structure.atoms.len();
    let hints = framework_run_hints(req.mode);

    // 1. Establish the periodic box. With an explicit cell (from the System
    //    Builder's cell editor) use it verbatim so the crystal's shape — e.g. a
    //    hexagonal lattice — is preserved; the atoms keep their positions and only
    //    the cell vectors change. Otherwise open the out-of-plane axis so both the
    //    slab gap and the cutoff fit.
    let structure = if let Some(box_cell) = req.cell_override.clone() {
        let mut s = req.structure.clone();
        s.cell = Some(box_cell);
        s
    } else {
        let base_c = cell.vectors[2].norm();
        let min_c = 2.0 * (req.cutoff_nm + 0.1) * 10.0 + 2.0;
        let z_extent = slab_thickness(&req.structure);
        let target_c = match req.solvation {
            Some(_) => (z_extent + req.solvent_gap_angstrom).max(min_c),
            None => base_c.max(min_c),
        };
        if (target_c - base_c).abs() > 1.0e-3 {
            set_slab_c_axis(&req.structure, target_c)?
        } else {
            req.structure.clone()
        }
    };

    // 2. The in-plane lattice cannot be fixed by opening c; verify it now with an
    //    actionable message instead of an opaque grompp failure.
    let cell = structure.cell.as_ref().expect("structure has a cell");
    ensure_periodic_cutoff_fits(cell, req.cutoff_nm)?;

    // 3. Build the framework topology, adding self-contained solvent definitions
    //    so solvate/genion can reference SOL and the ions by name.
    report(GromacsProgress::Stage(
        "generating framework topology".to_string(),
    ));
    // Merge any user-supplied force field: it can both cover elements the
    // built-in tables lack and override built-in atom types. The text is inlined
    // into the .top (kept self-contained), not written as a separate include.
    let custom = req
        .custom_force_field
        .as_deref()
        .map(custom_types)
        .unwrap_or_default();
    let mut topology = MdTopology::framework_with_custom(&structure, req.mode, &custom)?;
    topology.inline_force_field = req.custom_force_field.clone();
    if let Some(solvation) = &req.solvation {
        let defs = solvent_definitions(
            solvation.water,
            &solvation.positive_ion,
            &solvation.negative_ion,
        )?;
        for species in defs.species {
            topology.ensure_species(species);
        }
        for molecule in defs.molecules {
            topology.ensure_molecule(molecule);
        }
    }

    // 4. Write the coordinate and topology files.
    let conf = wd.join("conf.gro");
    std::fs::write(&conf, to_gro(&structure, &structure.title)?)
        .with_context(|| format!("writing {}", conf.display()))?;
    let topology_file = wd.join("topol.top");
    std::fs::write(&topology_file, render_top(&topology))
        .with_context(|| format!("writing {}", topology_file.display()))?;

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

    let mut final_coords = "conf.gro".to_string();
    let mut summary = format!(
        "Built {} framework MD system: {} atoms",
        req.mode.label().to_lowercase(),
        framework_atom_count
    );

    // 5. Solvate (and optionally ionize) with the engine's own tools.
    if let Some(solvation) = &req.solvation {
        report(GromacsProgress::Stage("gmx solvate".to_string()));
        run(
            vec![
                "solvate".into(),
                "-cp".into(),
                final_coords.clone(),
                "-cs".into(),
                solvent_box(solvation.water).into(),
                "-o".into(),
                "solvated.gro".into(),
                "-p".into(),
                "topol.top".into(),
            ],
            None,
            "solvate",
            &mut report,
        )?;
        final_coords = "solvated.gro".to_string();

        let needs_ions = solvation.neutralize || solvation.concentration_molar.is_some();
        if needs_ions {
            report(GromacsProgress::Stage("gmx grompp (ions)".to_string()));
            std::fs::write(wd.join("ions.mdp"), ions_mdp(hints.periodic_molecules))
                .with_context(|| "writing ions.mdp".to_string())?;
            run(
                vec![
                    "grompp".into(),
                    "-f".into(),
                    "ions.mdp".into(),
                    "-c".into(),
                    final_coords.clone(),
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
                solvation.positive_ion.clone(),
                "-nname".into(),
                solvation.negative_ion.clone(),
            ];
            if solvation.neutralize {
                genion.push("-neutral".into());
            }
            if let Some(conc) = solvation.concentration_molar {
                genion.push("-conc".into());
                genion.push(format!("{conc}"));
            }
            run(genion, Some(b"SOL\n".to_vec()), "genion", &mut report)?;
            final_coords = "ionized.gro".to_string();
        }

        // For a remote build, stage back the solvated/ionized coordinates and the
        // topology the run reuses (solvate/genion rewrite topol.top's [molecules]).
        if let Transport::Remote(target) = &req.compute.transport {
            remote::sync_down(target, &wd, &[final_coords.as_str(), "topol.top"], &[])
                .context("staging back the solvated framework system")?;
        }

        let gro = std::fs::read_to_string(wd.join(&final_coords))
            .with_context(|| format!("reading {final_coords}"))?;
        let solvated = parse_gro(&gro).with_context(|| format!("parsing {final_coords}"))?;
        summary = format!(
            "{summary}; solvated to {} atoms ({})",
            solvated.atoms.len(),
            solvation.water.label()
        );
        return Ok(MaterialBuildOutcome {
            structure: solvated,
            working_dir: wd,
            topology_file,
            framework_atom_count,
            hints,
            summary,
        });
    }

    Ok(MaterialBuildOutcome {
        structure,
        working_dir: wd,
        topology_file,
        framework_atom_count,
        hints,
        summary,
    })
}

/// Out-of-plane (z) extent of the sheet's atoms.
fn slab_thickness(structure: &Structure) -> f32 {
    let zs = structure.atoms.iter().map(|a| a.position.z);
    let (mut min, mut max) = (f32::INFINITY, f32::NEG_INFINITY);
    for z in zs {
        min = min.min(z);
        max = max.max(z);
    }
    if min.is_finite() { max - min } else { 0.0 }
}

/// The pre-equilibrated three-point solvent box `gmx solvate -cs` fills with.
/// Material solvation only supports three-point water, so this is always the SPC
/// box.
fn solvent_box(_water: WaterModel) -> &'static str {
    "spc216.gro"
}

/// A minimal `.mdp` that lets `grompp` build the `.tpr` `genion` needs (no steps
/// are taken). `periodic-molecules` is set for a flexible framework so grompp
/// accepts the molecule bonded across the boundary.
fn ions_mdp(periodic_molecules: bool) -> String {
    let mut mdp = String::from(
        "; SilicoLab-generated parameters for genion preprocessing\n\
         integrator    = steep\n\
         nsteps        = 0\n\
         cutoff-scheme = Verlet\n\
         coulombtype   = cutoff\n\
         rvdw          = 1.0\n\
         rcoulomb      = 1.0\n\
         pbc           = xyz\n",
    );
    if periodic_molecules {
        mdp.push_str("periodic-molecules = yes\n");
    }
    mdp
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{Arc, atomic::AtomicBool},
        time::Duration,
    };

    use super::*;
    use crate::engines::gromacs::{
        FreezeGroup, MdpSettings, PrepareSystemRequest, StageRequest, TopologySource,
        prepare_system, render_top, run_stage,
    };
    use crate::engines::registry::EngineLaunch;
    use crate::workflows::molecular_dynamics::{MdTopology, SolvationOptions, set_slab_c_axis};
    use crate::workflows::nanosheet::{NanosheetSpec, SheetKind, build_nanosheet};

    fn wsl_gmx_launch() -> EngineLaunch {
        EngineLaunch {
            command_prefix: vec!["wsl.exe".to_string(), "-e".to_string()],
            program: "/usr/local/gromacs/bin/gmx".to_string(),
        }
    }

    /// A graphene supercell large enough in-plane that a 1 nm cutoff fits its
    /// minimum image, with a generous c gap so the slab direction also clears the
    /// cutoff.
    fn graphene_sheet() -> crate::domain::Structure {
        let spec = NanosheetSpec {
            name: "graphene".to_string(),
            kind: SheetKind::Honeycomb(crate::workflows::nanosheet::HoneycombParams::graphene()),
            interlayer_spacing: 12.0,
            supercell: [11, 11, 1],
        };
        let sheet = build_nanosheet(&spec).expect("graphene builds");
        // Extend the out-of-plane gap so the c direction also clears the cutoff.
        set_slab_c_axis(&sheet, 30.0).expect("extend c")
    }

    /// Energy-minimize a flexible (bonded) graphene sheet through the real WSL
    /// GROMACS, proving the generated bonds/angles/dihedrals + `periodic-molecules`
    /// topology is grompp-valid and mdrun-runnable. Run with
    /// `cargo test --release -- --ignored wsl_gromacs_framework_flexible`.
    #[test]
    #[ignore = "requires GROMACS inside WSL (Windows acceptance environment)"]
    fn wsl_gromacs_framework_flexible_minimizes() {
        let working_dir = std::env::temp_dir().join("silicolab_gmx_framework_flexible");
        let _ = std::fs::remove_dir_all(&working_dir);

        let sheet = graphene_sheet();
        let topology =
            MdTopology::framework(&sheet, FrameworkMode::Flexible).expect("flexible topology");
        let hints = framework_run_hints(FrameworkMode::Flexible);
        assert!(hints.periodic_molecules);

        let system = prepare_system(PrepareSystemRequest {
            structure: sheet,
            topology: TopologySource::Inline(render_top(&topology)),
            working_dir,
            freeze: None,
        })
        .expect("prepare flexible framework");

        let mut settings = MdpSettings::energy_minimization();
        settings.periodic_molecules = hints.periodic_molecules;

        let result = run_stage(
            StageRequest {
                coordinate_input: system.conf_file.clone(),
                checkpoint_input: None,
                system,
                stage_name: "em".to_string(),
                settings,
                compute: wsl_gmx_launch().into(),
                max_duration: Duration::from_secs(180),
            },
            Arc::new(AtomicBool::new(false)),
            |_| {},
        )
        .expect("flexible framework EM runs");

        let energy = result
            .final_potential_energy
            .expect("a final potential energy is parsed");
        assert!(energy.is_finite(), "energy not finite: {energy}");
    }

    /// Energy-minimize a rigid (frozen) graphene sheet through the real WSL
    /// GROMACS, proving the exclusions topology + `.ndx` freeze group +
    /// `freezegrps` pipeline is grompp-valid and mdrun-runnable. Run with
    /// `cargo test --release -- --ignored wsl_gromacs_framework_rigid`.
    #[test]
    #[ignore = "requires GROMACS inside WSL (Windows acceptance environment)"]
    fn wsl_gromacs_framework_rigid_minimizes() {
        let working_dir = std::env::temp_dir().join("silicolab_gmx_framework_rigid");
        let _ = std::fs::remove_dir_all(&working_dir);

        let sheet = graphene_sheet();
        let atom_count = sheet.atoms.len();
        let topology = MdTopology::framework(&sheet, FrameworkMode::Rigid).expect("rigid topology");
        let hints = framework_run_hints(FrameworkMode::Rigid);
        let group = hints.freeze_group.clone().expect("rigid freezes");

        let system = prepare_system(PrepareSystemRequest {
            structure: sheet,
            topology: TopologySource::Inline(render_top(&topology)),
            working_dir,
            freeze: Some(framework_freeze_selection(atom_count)),
        })
        .expect("prepare rigid framework");
        assert!(system.index_file.is_some(), "rigid run needs an index file");

        let mut settings = MdpSettings::energy_minimization();
        settings.freeze = Some(FreezeGroup { group });

        let result = run_stage(
            StageRequest {
                coordinate_input: system.conf_file.clone(),
                checkpoint_input: None,
                system,
                stage_name: "em".to_string(),
                settings,
                compute: wsl_gmx_launch().into(),
                max_duration: Duration::from_secs(180),
            },
            Arc::new(AtomicBool::new(false)),
            |_| {},
        )
        .expect("rigid framework EM runs");

        assert_eq!(
            result.structure.atoms.len(),
            atom_count,
            "frozen sheet keeps all atoms"
        );
        assert!(
            result
                .final_potential_energy
                .map(|e| e.is_finite())
                .unwrap_or(true)
        );
    }

    /// Build a *solvated* rigid graphene sheet through `build_material_system`
    /// (framework topology + SOL/ion definitions + `gmx solvate`/`genion`), then
    /// energy-minimize it with the sheet frozen. Validates the whole solvation
    /// path end-to-end against the real engine. Run with
    /// `cargo test --release -- --ignored wsl_gromacs_framework_solvated`.
    #[test]
    #[ignore = "requires GROMACS inside WSL (Windows acceptance environment)"]
    fn wsl_gromacs_framework_solvated_minimizes() {
        let working_dir = std::env::temp_dir().join("silicolab_gmx_framework_solvated");
        let _ = std::fs::remove_dir_all(&working_dir);
        let build_dir = working_dir.join("build");
        let run_dir = working_dir.join("run");

        let sheet = graphene_sheet();
        // Drive the build through the explicit-cell path (as the System Builder's
        // cell editor does): the hexagonal crystal cell is used verbatim as the
        // box, proving that path is grompp-valid and mdrun-runnable end-to-end.
        let box_cell = sheet.cell.clone();
        let solvation = SolvationOptions {
            water: crate::workflows::molecular_dynamics::WaterModel::Spc,
            positive_ion: "NA".to_string(),
            negative_ion: "CL".to_string(),
            neutralize: true,
            concentration_molar: Some(0.15),
        };

        let outcome = build_material_system(
            MaterialBuildRequest {
                structure: sheet,
                mode: FrameworkMode::Rigid,
                working_dir: build_dir,
                compute: wsl_gmx_launch().into(),
                solvation: Some(solvation),
                cell_override: box_cell,
                custom_force_field: None,
                solvent_gap_angstrom: 25.0,
                cutoff_nm: 1.0,
                max_duration: Duration::from_secs(300),
            },
            Arc::new(AtomicBool::new(false)),
            |_| {},
        )
        .expect("solvated framework build runs");

        assert!(
            outcome.structure.atoms.len() > outcome.framework_atom_count,
            "solvation should add water ({} -> {})",
            outcome.framework_atom_count,
            outcome.structure.atoms.len()
        );

        // Reuse the built topology to energy-minimize, sheet frozen.
        let system = prepare_system(PrepareSystemRequest {
            structure: outcome.structure,
            topology: TopologySource::File(outcome.topology_file),
            working_dir: run_dir,
            freeze: Some(framework_freeze_selection(outcome.framework_atom_count)),
        })
        .expect("prepare solvated run");

        let mut settings = MdpSettings::energy_minimization();
        settings.freeze = outcome
            .hints
            .freeze_group
            .map(|group| FreezeGroup { group });

        let result = run_stage(
            StageRequest {
                coordinate_input: system.conf_file.clone(),
                checkpoint_input: None,
                system,
                stage_name: "em".to_string(),
                settings,
                compute: wsl_gmx_launch().into(),
                max_duration: Duration::from_secs(300),
            },
            Arc::new(AtomicBool::new(false)),
            |_| {},
        )
        .expect("solvated framework EM runs");

        let energy = result
            .final_potential_energy
            .expect("a final potential energy is parsed");
        assert!(energy.is_finite(), "energy not finite: {energy}");
    }

    /// Build and minimize a rigid sheet of an element with NO built-in
    /// parameters (platinum), supplying the missing Lennard-Jones type through a
    /// custom force field. Proves the custom-FF path reaches grompp/mdrun: the
    /// inlined `[atomtypes]` resolves the otherwise-unparameterized element. Run
    /// with `cargo test --release -- --ignored wsl_gromacs_framework_custom`.
    #[test]
    #[ignore = "requires GROMACS inside WSL (Windows acceptance environment)"]
    fn wsl_gromacs_framework_custom_force_field_minimizes() {
        let working_dir = std::env::temp_dir().join("silicolab_gmx_framework_custom");
        let _ = std::fs::remove_dir_all(&working_dir);
        let build_dir = working_dir.join("build");
        let run_dir = working_dir.join("run");

        // A platinum sheet on the graphene lattice. Platinum is not in the
        // built-in tables, so without a custom force field this cannot be built.
        let mut sheet = graphene_sheet();
        for atom in &mut sheet.atoms {
            atom.element = "Pt".to_string();
        }
        let atom_count = sheet.atoms.len();
        let box_cell = sheet.cell.clone();

        // A minimal GROMACS atomtypes block naming the type after the element.
        let custom_ff = "\
[ atomtypes ]
; name  at.num  mass     charge  ptype  sigma     epsilon
Pt      78      195.084  0.0     A      0.27540   0.33000
";

        let outcome = build_material_system(
            MaterialBuildRequest {
                structure: sheet,
                mode: FrameworkMode::Rigid,
                working_dir: build_dir,
                compute: wsl_gmx_launch().into(),
                solvation: None,
                cell_override: box_cell,
                custom_force_field: Some(custom_ff.to_string()),
                solvent_gap_angstrom: 25.0,
                cutoff_nm: 1.0,
                max_duration: Duration::from_secs(180),
            },
            Arc::new(AtomicBool::new(false)),
            |_| {},
        )
        .expect("custom-FF framework build runs");
        assert_eq!(outcome.framework_atom_count, atom_count);

        let system = prepare_system(PrepareSystemRequest {
            structure: outcome.structure,
            topology: TopologySource::File(outcome.topology_file),
            working_dir: run_dir,
            freeze: Some(framework_freeze_selection(outcome.framework_atom_count)),
        })
        .expect("prepare custom-FF run");

        let mut settings = MdpSettings::energy_minimization();
        settings.freeze = outcome
            .hints
            .freeze_group
            .map(|group| FreezeGroup { group });

        let result = run_stage(
            StageRequest {
                coordinate_input: system.conf_file.clone(),
                checkpoint_input: None,
                system,
                stage_name: "em".to_string(),
                settings,
                compute: wsl_gmx_launch().into(),
                max_duration: Duration::from_secs(180),
            },
            Arc::new(AtomicBool::new(false)),
            |_| {},
        )
        .expect("custom-FF framework EM runs");

        assert!(
            result
                .final_potential_energy
                .map(|e| e.is_finite())
                .unwrap_or(true)
        );
    }
}
