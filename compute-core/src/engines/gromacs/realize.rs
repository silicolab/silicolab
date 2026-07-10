//! Realize engine-neutral [`MdStage`]s into GROMACS [`StageSpec`]s.
//!
//! This is the GROMACS-specific half of the Run MD model: it maps each neutral
//! stage's intent (ensemble, coupling groups, restraint scheme, pressure shape,
//! thermostat/barostat kind, annealing, tiered parameters, raw passthrough) onto
//! a concrete [`MdpSettings`], and threads the mechanical stage chaining the
//! scheduler owns — velocity generation and continuation across stages, and the
//! coordinate/checkpoint links [`run_pipeline`](super::run_pipeline) resolves.
//!
//! Barostat selection is version-aware but never probes: it takes an
//! already-known GROMACS year (or `None`, meaning assume a modern engine), so the
//! modern stochastic cell-rescale barostat is the default and the legacy
//! Berendsen-equilibration / Parrinello-Rahman-production pair is used only when a
//! cached version is known to predate it.

use super::input::{
    Annealing, Barostat, ConstraintAlgorithm, ConstraintKind, MdpSettings, OutputFrequency,
    PressureCoupling, TemperatureCoupling, Thermostat, VelocityGen,
};
use super::nonbonded::NonbondedScheme;
use super::runner::{FileRef, StageFileRole, StageLinks, StageSpec};
use crate::md::run::{
    BarostatKind, ConstraintScope, CouplingGroups, ForceFieldFamily, MdStage, PressureShape,
    StageKind, ThermostatKind, family_nonbonded_intent,
};

/// First GROMACS year with the stochastic cell-rescale (`C-rescale`) barostat.
const C_RESCALE_MIN_YEAR: u32 = 2021;
/// Trajectory write-interval bounds (steps), matching the neutral protocol.
const TRAJECTORY_MIN_INTERVAL: u32 = 1;
const TRAJECTORY_MAX_INTERVAL: u32 = 5_000;

/// Build the GROMACS stage chain for a sequence of neutral stages.
///
/// `version_year` is an already-known GROMACS release year (e.g. `2023`) used
/// only for barostat selection; `None` assumes a modern engine. Stage chaining
/// follows the scheduler's rules: the first dynamics stage generates velocities
/// and does not continue; later dynamics stages continue from the previous
/// dynamics stage's checkpoint; restraints toggle per stage.
pub fn stage_specs_from_md_stages(
    stages: &[MdStage],
    family: ForceFieldFamily,
    version_year: Option<u32>,
) -> Vec<StageSpec> {
    let mut specs = Vec::with_capacity(stages.len());
    let mut first_dynamics_seen = false;
    let mut previous_stage: Option<String> = None;
    let mut last_checkpoint_stage: Option<String> = None;

    for stage in stages {
        let is_first_dynamics = stage.kind.is_dynamics() && !first_dynamics_seen;
        let settings = realize_stage(stage, family, version_year, is_first_dynamics);

        let links = match previous_stage.as_ref() {
            None => StageLinks::from_prepared(),
            Some(previous) => StageLinks {
                coordinates: FileRef::Stage {
                    stage: previous.clone(),
                    role: StageFileRole::OutputGro,
                },
                // Continue from the last dynamics stage's checkpoint, but only
                // when this stage actually continues (the first dynamics stage and
                // any stage placed straight after minimization start fresh).
                checkpoint: if settings.continuation {
                    last_checkpoint_stage.clone().map(|stage| FileRef::Stage {
                        stage,
                        role: StageFileRole::Checkpoint,
                    })
                } else {
                    None
                },
            },
        };

        specs.push(StageSpec {
            stage_name: stage.name.clone(),
            settings,
            links,
        });

        previous_stage = Some(stage.name.clone());
        if stage.kind.is_dynamics() {
            first_dynamics_seen = true;
            last_checkpoint_stage = Some(stage.name.clone());
        }
    }

    specs
}

/// Map one neutral stage to a GROMACS [`MdpSettings`].
pub fn realize_stage(
    stage: &MdStage,
    family: ForceFieldFamily,
    version_year: Option<u32>,
    is_first_dynamics: bool,
) -> MdpSettings {
    let minimization = stage.kind.is_minimization();
    let (fallback_rc, fallback_rv) = family_nonbonded_intent(family);
    let coulomb_cutoff_nm = stage.params.coulomb_cutoff_nm.unwrap_or(fallback_rc);
    let vdw_cutoff_nm = stage.params.vdw_cutoff_nm.unwrap_or(fallback_rv);

    let nonbonded = if family.is_biomolecular() {
        NonbondedScheme::ForceField(family)
    } else {
        NonbondedScheme::Cutoff
    };

    let temperature_coupling = (!minimization).then(|| TemperatureCoupling {
        tc_grps: coupling_group_names(stage.coupling_groups),
        tau_t: vec![
            stage.params.thermostat_tau_ps.unwrap_or(0.1);
            coupling_group_count(stage.coupling_groups)
        ],
        ref_t: vec![stage.temperature_k; coupling_group_count(stage.coupling_groups)],
    });

    let pressure_coupling = stage
        .pressure
        .map(|pressure| realize_pressure(stage.kind, pressure, version_year));

    // Velocity generation and continuation are scheduler-owned: the first
    // dynamics stage seeds velocities and does not continue; later ones continue.
    let velocity_generation = (is_first_dynamics).then(|| VelocityGen {
        gen_temp: stage.temperature_k,
        gen_seed: stage.params.random_seed.unwrap_or(-1),
    });
    let continuation = stage.kind.is_dynamics() && !is_first_dynamics;

    let constraints = if minimization {
        None
    } else {
        match stage.params.constraints {
            Some(ConstraintScope::None) => None,
            Some(ConstraintScope::AllBonds) => Some(ConstraintKind::AllBonds),
            // Default dynamics to h-bond constraints (needed for a 2 fs step).
            Some(ConstraintScope::HBonds) | None => Some(ConstraintKind::HBonds),
        }
    };

    // Position restraints switch on via `-DPOSRES`; production drops the define.
    let define = stage
        .restraint
        .is_restrained()
        .then(|| "-DPOSRES".to_string());

    let annealing = stage.anneal.as_ref().map(|spec| Annealing {
        points: spec.points.clone(),
    });

    MdpSettings {
        integrator: if minimization {
            super::input::Integrator::SteepestDescent
        } else {
            super::input::Integrator::Leapfrog
        },
        nsteps: stage.steps(),
        timestep_ps: stage.timestep_ps,
        coulomb_cutoff_nm,
        vdw_cutoff_nm,
        emtol: 1_000.0,
        emstep: 0.01,
        continuation,
        temperature_coupling,
        pressure_coupling,
        velocity_generation,
        output: stage_output(stage),
        constraints,
        constraint_algorithm: ConstraintAlgorithm::Lincs,
        periodic_molecules: false,
        freeze: None,
        nonbonded,
        define,
        thermostat: map_thermostat(stage.params.thermostat),
        annealing,
        raw_lines: advanced_raw_lines(stage),
    }
}

/// The GROMACS index-group names for a neutral coupling arrangement. `System`,
/// `Protein`, `Non-Protein`, and `Water_and_ions` are default groups grompp
/// generates; `Lipid` and `Nucleic` require an index file (a later enhancement).
fn coupling_group_names(groups: CouplingGroups) -> Vec<String> {
    let names: &[&str] = match groups {
        CouplingGroups::WholeSystem => &["System"],
        CouplingGroups::SoluteSolvent => &["Protein", "Non-Protein"],
        CouplingGroups::SoluteLipidSolvent => &["Protein", "Lipid", "Water_and_ions"],
        CouplingGroups::NucleicSolvent => &["Nucleic", "Water_and_ions"],
    };
    names.iter().map(|name| name.to_string()).collect()
}

fn coupling_group_count(groups: CouplingGroups) -> usize {
    match groups {
        CouplingGroups::WholeSystem => 1,
        CouplingGroups::SoluteSolvent | CouplingGroups::NucleicSolvent => 2,
        CouplingGroups::SoluteLipidSolvent => 3,
    }
}

fn map_thermostat(kind: Option<ThermostatKind>) -> Thermostat {
    match kind {
        Some(ThermostatKind::NoseHoover) => Thermostat::NoseHoover,
        Some(ThermostatKind::Berendsen) => Thermostat::Berendsen,
        Some(ThermostatKind::StochasticVelocityRescale) | None => Thermostat::VRescale,
    }
}

/// Realize a neutral pressure coupling, choosing the barostat with version
/// awareness. Modern engines use stochastic cell rescaling; on a known-old engine
/// it downgrades to Berendsen for equilibration and Parrinello-Rahman for
/// production (Appendix D).
fn realize_pressure(
    kind: StageKind,
    pressure: crate::md::run::PressureCoupling,
    version_year: Option<u32>,
) -> PressureCoupling {
    let is_production = matches!(kind, StageKind::Produce | StageKind::Extend);
    let barostat = match pressure.barostat {
        BarostatKind::StochasticCellRescale => {
            if version_year.is_none_or(|year| year >= C_RESCALE_MIN_YEAR) {
                Barostat::CRescale
            } else if is_production {
                Barostat::ParrinelloRahman
            } else {
                Barostat::Berendsen
            }
        }
        BarostatKind::ParrinelloRahman => Barostat::ParrinelloRahman,
        BarostatKind::Berendsen => Barostat::Berendsen,
    };

    let p = pressure.ref_bar;
    let c = 4.5e-5_f32;
    let (pcoupltype, ref_p, compressibility) = match pressure.shape {
        PressureShape::Isotropic => ("isotropic", vec![p], vec![c]),
        PressureShape::SemiIsotropic => ("semiisotropic", vec![p, p], vec![c, c]),
        // Anisotropic takes the full 6-component tensor (xx yy zz xy xz yz); the
        // off-diagonal references are zero.
        PressureShape::Anisotropic => (
            "anisotropic",
            vec![p, p, p, 0.0, 0.0, 0.0],
            vec![c, c, c, 0.0, 0.0, 0.0],
        ),
    };

    PressureCoupling {
        barostat,
        pcoupltype: pcoupltype.to_string(),
        tau_p: pressure.tau_ps,
        ref_p,
        compressibility,
    }
}

/// The output cadence for a stage: a compressed-trajectory interval targeting the
/// stage's frame goal, or no trajectory. Minimization writes none.
fn stage_output(stage: &MdStage) -> Option<OutputFrequency> {
    if stage.kind.is_minimization() {
        return None;
    }
    let Some(frames) = stage.trajectory_target_frames.filter(|&f| f > 0) else {
        // A dynamics stage that writes no trajectory still logs energy.
        return Some(OutputFrequency::equilibration());
    };
    let nsteps = stage.steps().max(1);
    let interval =
        ((nsteps / frames).max(1) as u32).clamp(TRAJECTORY_MIN_INTERVAL, TRAJECTORY_MAX_INTERVAL);
    Some(OutputFrequency {
        nstxout: 0,
        nstvout: 0,
        nstenergy: interval,
        nstlog: interval,
        nstxout_compressed: interval,
    })
}

/// Translate the set advanced/standard tiered parameters that have no dedicated
/// [`MdpSettings`] field into raw `.mdp` lines, then append the stage's own raw
/// passthrough last (so a user's explicit key wins). Nothing the user set is
/// silently dropped.
fn advanced_raw_lines(stage: &MdStage) -> Vec<(String, String)> {
    let mut lines = Vec::new();
    let params = &stage.params;
    let mut push = |key: &str, value: String| lines.push((key.to_string(), value));

    if let Some(spacing) = params.pme_spacing_nm {
        push("fourierspacing", format!("{spacing}"));
    }
    if let Some(order) = params.pme_order {
        push("pme-order", format!("{order}"));
    }
    if let Some(order) = params.constraint_order {
        push("lincs-order", format!("{order}"));
    }
    if let Some(iter) = params.constraint_iterations {
        push("lincs-iter", format!("{iter}"));
    }
    if let Some(dispersion) = params.dispersion_correction {
        push(
            "DispCorr",
            if dispersion { "EnerPres" } else { "no" }.to_string(),
        );
    }
    if let Some(remove) = params.remove_com_motion {
        push(
            "comm-mode",
            if remove { "Linear" } else { "None" }.to_string(),
        );
    }
    if let Some(nstlist) = params.neighbor_list_steps {
        push("nstlist", format!("{nstlist}"));
    }

    // The stage's explicit raw passthrough is appended last so it overrides both
    // the translated parameters above and any generated directive.
    lines.extend(stage.raw_passthrough.iter().cloned());
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engines::gromacs::input::render_mdp;
    use crate::md::run::system_context::MdSystemContext;
    use crate::md::run::{MdStage, PresetId, PresetParams, RestraintScheme, SystemTypeOverrides};

    fn amber_protein_context() -> MdSystemContext {
        MdSystemContext {
            force_field_token: "amber99sb-ildn".to_string(),
            force_field_family: ForceFieldFamily::Amber,
            water_token: Some("tip3p".to_string()),
            detected_protein: true,
            detected_nucleic: false,
            detected_membrane: false,
            detected_ligand: false,
            is_framework: false,
            net_charge: 0.0,
            atom_count: 10_000,
            restraint_groups: vec!["solute".to_string()],
            hmr_applied: false,
        }
    }

    /// The Phase-C gate, "ran correctly" portion that does NOT need a real engine:
    /// the biomolecular path must render PME + the force-field nonbonded block, and
    /// `-DPOSRES` must be present on restrained equilibration stages AND absent on
    /// production — both directions.
    #[test]
    fn biomolecular_standard_preset_renders_pme_and_posres_on_restrained_only() {
        let ctx = amber_protein_context();
        let eff = ctx.with_overrides(SystemTypeOverrides::default());
        let stages = PresetId::StandardBiomolecule.build(&eff, &PresetParams::default());
        let specs = stage_specs_from_md_stages(&stages, ForceFieldFamily::Amber, None);

        for spec in &specs {
            let mdp = render_mdp(&spec.settings);
            // PME + the AMBER nonbonded block on every stage (incl. minimization).
            assert!(
                mdp.contains("coulombtype              = PME"),
                "stage '{}' should use PME, got:\n{mdp}",
                spec.stage_name
            );
            assert!(
                mdp.contains("vdw-modifier             = potential-shift")
                    && mdp.contains("DispCorr                 = EnerPres"),
                "stage '{}' missing the AMBER nonbonded block",
                spec.stage_name
            );
            assert!(!mdp.contains("coulombtype              = cutoff"));

            let restrained = matches!(spec.stage_name.as_str(), "nvt" | "npt");
            if restrained {
                // Direction 1: restraints ON for equilibration.
                assert!(
                    mdp.contains("define                   = -DPOSRES"),
                    "restrained stage '{}' must apply -DPOSRES",
                    spec.stage_name
                );
            } else if spec.stage_name == "md" {
                // Direction 2: restraints OFF for production.
                assert!(
                    !mdp.contains("-DPOSRES") && !mdp.contains("define "),
                    "production stage must NOT apply -DPOSRES, got:\n{mdp}"
                );
            }
        }
    }

    #[test]
    fn dry_biomolecular_preset_renders_only_the_system_coupling_group() {
        let mut ctx = amber_protein_context();
        ctx.water_token = None;
        let eff = ctx.with_overrides(SystemTypeOverrides::default());
        let stages = PresetId::StandardBiomolecule.build(&eff, &PresetParams::default());
        let specs = stage_specs_from_md_stages(&stages, ForceFieldFamily::Amber, None);

        for spec in specs
            .iter()
            .filter(|spec| !spec.settings.integrator.is_minimization())
        {
            let mdp = render_mdp(&spec.settings);
            assert!(
                mdp.contains("tc-grps                  = System"),
                "dry stage '{}' must couple the whole system, got:\n{mdp}",
                spec.stage_name
            );
            assert!(!mdp.contains("Non-Protein"));
        }
    }

    /// The legacy path is preserved: a non-biomolecular (framework / generic)
    /// system still realizes to plain cut-off, never PME. PME is a new path, not a
    /// rewrite of the old one.
    #[test]
    fn non_biomolecular_family_keeps_the_cutoff_path() {
        let stage = MdStage::nvt(300.0);
        let settings = realize_stage(&stage, ForceFieldFamily::Other, None, true);
        let mdp = render_mdp(&settings);
        assert!(mdp.contains("coulombtype              = cutoff"));
        assert!(!mdp.contains("PME"));
    }

    #[test]
    fn first_dynamics_seeds_velocities_then_later_stages_continue() {
        let ctx = amber_protein_context();
        let eff = ctx.with_overrides(SystemTypeOverrides::default());
        let stages = PresetId::StandardBiomolecule.build(&eff, &PresetParams::default());
        let specs = stage_specs_from_md_stages(&stages, ForceFieldFamily::Amber, None);

        let nvt = specs.iter().find(|s| s.stage_name == "nvt").unwrap();
        assert!(nvt.settings.velocity_generation.is_some());
        assert!(!nvt.settings.continuation);
        assert!(nvt.links.checkpoint.is_none());

        let npt = specs.iter().find(|s| s.stage_name == "npt").unwrap();
        assert!(npt.settings.velocity_generation.is_none());
        assert!(npt.settings.continuation);
        assert!(npt.links.checkpoint.is_some());
    }

    #[test]
    fn barostat_selection_is_version_aware() {
        let mut prod = MdStage::produce(300.0); // NPT, C-rescale intent
        prod.restraint = RestraintScheme::None;

        // Modern (None or >= 2021): stochastic cell rescaling.
        let modern = realize_stage(&prod, ForceFieldFamily::Amber, None, false);
        assert!(render_mdp(&modern).contains("pcoupl                   = C-rescale"));

        // Known-old engine on production: Parrinello-Rahman.
        let legacy_prod = realize_stage(&prod, ForceFieldFamily::Amber, Some(2018), false);
        assert!(render_mdp(&legacy_prod).contains("pcoupl                   = Parrinello-Rahman"));

        // Known-old engine on equilibration: Berendsen.
        let npt_equil = MdStage::npt(300.0);
        let legacy_equil = realize_stage(&npt_equil, ForceFieldFamily::Amber, Some(2018), false);
        assert!(render_mdp(&legacy_equil).contains("pcoupl                   = Berendsen"));
    }

    #[test]
    fn membrane_pressure_renders_semiisotropic_arrays() {
        let mut stage = MdStage::npt(300.0);
        stage.pressure = Some(crate::md::run::PressureCoupling::semi_isotropic());
        let mdp = render_mdp(&realize_stage(
            &stage,
            ForceFieldFamily::Charmm,
            None,
            false,
        ));
        assert!(mdp.contains("pcoupltype               = semiisotropic"));
        // Two parallel reference-pressure entries (xy, z).
        assert!(mdp.contains("ref-p                    = 1 1"));
    }

    #[test]
    fn raw_passthrough_and_advanced_params_reach_the_mdp() {
        let mut stage = MdStage::produce(300.0);
        stage.params.pme_order = Some(6);
        stage
            .raw_passthrough
            .push(("nstcomm".to_string(), "50".to_string()));
        let mdp = render_mdp(&realize_stage(&stage, ForceFieldFamily::Amber, None, false));
        assert!(mdp.contains("pme-order                = 6"));
        assert!(mdp.contains("nstcomm                  = 50"));
    }

    /// The Phase-C integration gate: build a real solvated peptide and run the
    /// Standard Biomolecule preset (EM -> restrained NVT -> restrained NPT ->
    /// production) end-to-end through WSL GROMACS with PME and `-DPOSRES`. The
    /// assertions check "ran correctly", not merely "ran": a finite production
    /// potential energy, a production temperature near the 300 K target, no LINCS
    /// warnings in any stage's log, and a decodable trajectory per dynamics stage.
    /// The stage lengths are shortened so the acceptance run completes quickly.
    /// Run with `cargo test --release -- --ignored wsl_gromacs_standard_preset`.
    #[test]
    #[ignore = "requires GROMACS inside WSL (Windows acceptance environment)"]
    fn wsl_gromacs_standard_preset_runs_end_to_end() {
        use std::sync::{Arc, atomic::AtomicBool};
        use std::time::Duration;

        use crate::engines::gromacs::analysis::{AnalysisContext, gmx_energy, parse_xvg};
        use crate::engines::gromacs::build::{BuildRequest, IonOptions, build_system};
        use crate::engines::gromacs::runner::{PrepareSystemRequest, prepare_system, run_pipeline};
        use crate::engines::gromacs::topology::TopologySource;
        use crate::engines::registry::EngineLaunch;
        use crate::io::formats::pdb::parse_pdb;
        use crate::md::run::{StageKind, StageLength};
        use crate::md::{BoxShape, MdSystemConfig, WaterModel};

        let root = std::env::temp_dir().join("silicolab_gmx_standard_preset");
        let _ = std::fs::remove_dir_all(&root);
        let build_dir = root.join("build");
        let run_dir = root.join("run");

        let launch = EngineLaunch {
            command_prefix: vec!["wsl.exe".to_string(), "-e".to_string()],
            program: "/usr/local/gromacs/bin/gmx".to_string(),
        };

        let pdb = include_str!("../../md/fixtures/capped_ala.pdb");
        let structure = parse_pdb(pdb).expect("fixture parses");

        // Build a solvated, neutralized peptide with AMBER + TIP3P (also writes
        // posre.itp, which the restrained stages enable via -DPOSRES).
        let outcome = build_system(
            BuildRequest {
                structure,
                working_dir: build_dir,
                compute: launch.clone().into(),
                force_field: "amber99sb-ildn".to_string(),
                water: WaterModel::Tip3p,
                box_config: MdSystemConfig::with_uniform_padding(10.0, BoxShape::Cubic),
                solvate: true,
                ions: Some(IonOptions {
                    neutralize: true,
                    concentration_molar: None,
                    positive_ion: "NA".to_string(),
                    negative_ion: "CL".to_string(),
                }),
                max_duration: Duration::from_secs(600),
            },
            Arc::new(AtomicBool::new(false)),
            |_| {},
        )
        .expect("GROMACS build should complete");

        // The Standard preset for an AMBER protein, shortened for the test.
        let ctx = amber_protein_context();
        let eff = ctx.with_overrides(SystemTypeOverrides::default());
        let mut stages = PresetId::StandardBiomolecule.build(&eff, &PresetParams::default());
        for stage in &mut stages {
            stage.length = match stage.kind {
                StageKind::Minimize => StageLength::Steps(500),
                _ => StageLength::Picoseconds(4.0),
            };
        }
        let specs = stage_specs_from_md_stages(&stages, ForceFieldFamily::Amber, None);

        let system = prepare_system(PrepareSystemRequest {
            structure: outcome.structure,
            topology: TopologySource::File(outcome.topology_file),
            working_dir: run_dir.clone(),
            freeze: None,
        })
        .expect("preparing the run from the built topology should succeed");

        let cancel = Arc::new(AtomicBool::new(false));
        let results = run_pipeline(
            system,
            specs,
            launch.clone().into(),
            Duration::from_secs(900),
            Arc::clone(&cancel),
            |_| {},
        )
        .expect("the Standard preset pipeline should run to completion");

        assert_eq!(results.len(), 4, "EM, NVT, NPT, production");
        let production = results.last().expect("production stage present");

        // --- "Ran correctly", not just "ran" -------------------------------

        // No LINCS warnings in any stage's log (an instability signal).
        for stage in &results {
            let log = std::fs::read_to_string(&stage.log).unwrap_or_default();
            let combined = format!("{}{}{log}", stage.mdrun_stdout, stage.mdrun_stderr);
            assert!(
                !combined.contains("LINCS WARNING"),
                "stage '{}' produced a LINCS warning (instability)",
                stage.stage_name
            );
        }

        // Finite production potential energy and a temperature near 300 K,
        // extracted from the production energy file. The band is generous because
        // the test's equilibration is intentionally short.
        let analysis = AnalysisContext {
            working_dir: run_dir.clone(),
            gmx_launch: launch,
            max_duration: Duration::from_secs(120),
        };
        // Extract one term per call so the value is unambiguously column 1 (time
        // is column 0), avoiding any dependence on how gmx energy labels or orders
        // a multi-term selection.
        let mean_energy_term = |term: &str, out: &str| -> f64 {
            gmx_energy(
                &analysis,
                &production.edr,
                out,
                &[term],
                Arc::clone(&cancel),
                |_| {},
            )
            .unwrap_or_else(|error| panic!("gmx energy {term} failed: {error}"));
            let xvg = parse_xvg(
                &std::fs::read_to_string(run_dir.join(out))
                    .unwrap_or_else(|_| panic!("read {out}")),
            )
            .unwrap_or_else(|_| panic!("parse {out}"));
            xvg.mean(1)
                .unwrap_or_else(|| panic!("{term} has no data rows"))
        };

        // A physically meaningful band: the thermostat holds the mean within a few
        // kelvin of target, so a 20 K window still catches a mis-wired thermostat,
        // wrong coupling groups, or an unequilibrated run while tolerating
        // short-run noise. A looser band would not distinguish "ran correctly" from
        // merely "ran".
        let temperature = mean_energy_term("Temperature", "temp.xvg");
        assert!(
            (temperature - 300.0).abs() < 20.0,
            "production temperature {temperature} K is not within 20 K of the 300 K target"
        );
        let potential = mean_energy_term("Potential", "pot.xvg");
        assert!(
            potential.is_finite(),
            "production potential energy should be finite, got {potential}"
        );

        // Each dynamics stage wrote a decodable trajectory.
        for stage in &results {
            if stage.stage_name == "em" {
                continue;
            }
            let trajectory = stage.trajectory.as_ref().unwrap_or_else(|| {
                panic!("stage '{}' should write a trajectory", stage.stage_name)
            });
            let decoded = crate::io::trajectory::read_xtc(trajectory)
                .unwrap_or_else(|_| panic!("decode '{}' trajectory", stage.stage_name));
            assert!(
                decoded.frame_count() >= 1,
                "stage '{}' trajectory should have at least one frame",
                stage.stage_name
            );
        }
    }
}
