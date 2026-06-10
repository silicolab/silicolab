//! The engine-neutral preset library: named, ordered sequences of [`MdStage`]s
//! covering common biomolecular and soft-matter workflows.
//!
//! A preset expresses *physical intent only*. It reads the
//! [`EffectiveContext`] to inject system-type specifics (coupling groups,
//! pressure shape, restraint targets, a smaller early timestep) but never touches
//! engine syntax — the adapter realizes each stage. [`PresetParams`] carries the
//! top-level user choices a preset honors (temperature, production length,
//! timestep); everything else is preset-defined.

use serde::{Deserialize, Serialize};

use super::stage::{
    AnnealSpec, CouplingGroups, MdStage, PressureCoupling, RestraintScheme, StageKind, StageLength,
};
use super::system_context::EffectiveContext;

/// Standard equilibration restraint force constant (kJ/mol/nm²).
const RESTRAINT_FC: f32 = 1000.0;
/// Smaller timestep (ps) for the earliest, most delicate equilibration sub-steps
/// (membrane systems).
const GENTLE_TIMESTEP_PS: f32 = 0.001;

/// Production-length quick picks. Set the production step count from
/// `length / timestep`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProductionLength {
    /// 1 ns — a quick functional test.
    Test,
    /// 100 ns — a standard run.
    Standard,
    /// 500 ns.
    Long,
    /// 1 µs.
    Extended,
}

impl ProductionLength {
    pub fn all() -> &'static [Self] {
        &[Self::Test, Self::Standard, Self::Long, Self::Extended]
    }

    pub fn picoseconds(self) -> f64 {
        match self {
            Self::Test => 1_000.0,
            Self::Standard => 100_000.0,
            Self::Long => 500_000.0,
            Self::Extended => 1_000_000.0,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Test => "Test (1 ns)",
            Self::Standard => "Standard (100 ns)",
            Self::Long => "Long (500 ns)",
            Self::Extended => "Extended (1 µs)",
        }
    }
}

/// Top-level user choices a preset honors. Everything finer is preset-defined and
/// then editable per stage.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PresetParams {
    pub temperature_k: f32,
    pub production: ProductionLength,
    /// Production/dynamics timestep (ps). The membrane preset still uses a smaller
    /// timestep for its earliest sub-steps regardless of this.
    pub timestep_ps: f32,
}

impl Default for PresetParams {
    fn default() -> Self {
        Self {
            temperature_k: 300.0,
            production: ProductionLength::Standard,
            timestep_ps: 0.002,
        }
    }
}

/// One of the presets in the library.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PresetId {
    EnergyMinimization,
    QuickRelax,
    StandardBiomolecule,
    CarefulEquilibration,
    MembraneProtein,
    ProteinLigand,
    NucleicAcid,
    FastProduction,
    SimulatedAnnealing,
    ProductionExtend,
    NvtProduction,
}

impl PresetId {
    /// Every preset, in menu order.
    pub fn all() -> &'static [Self] {
        &[
            Self::EnergyMinimization,
            Self::QuickRelax,
            Self::StandardBiomolecule,
            Self::CarefulEquilibration,
            Self::MembraneProtein,
            Self::ProteinLigand,
            Self::NucleicAcid,
            Self::FastProduction,
            Self::SimulatedAnnealing,
            Self::ProductionExtend,
            Self::NvtProduction,
        ]
    }

    /// Stable token for scripting/persistence.
    pub fn token(self) -> &'static str {
        match self {
            Self::EnergyMinimization => "minimize",
            Self::QuickRelax => "quick-relax",
            Self::StandardBiomolecule => "standard",
            Self::CarefulEquilibration => "careful",
            Self::MembraneProtein => "membrane",
            Self::ProteinLigand => "protein-ligand",
            Self::NucleicAcid => "nucleic",
            Self::FastProduction => "fast-production",
            Self::SimulatedAnnealing => "annealing",
            Self::ProductionExtend => "extend",
            Self::NvtProduction => "nvt-production",
        }
    }

    pub fn from_token(token: &str) -> Option<Self> {
        Self::all()
            .iter()
            .copied()
            .find(|p| p.token() == token.trim().to_ascii_lowercase())
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::EnergyMinimization => "Energy Minimization",
            Self::QuickRelax => "Quick Relax",
            Self::StandardBiomolecule => "Standard Biomolecule",
            Self::CarefulEquilibration => "Careful / High-Stability Equilibration",
            Self::MembraneProtein => "Membrane Protein",
            Self::ProteinLigand => "Protein–Ligand",
            Self::NucleicAcid => "Nucleic Acid",
            Self::FastProduction => "Fast Production (long timestep / HMR)",
            Self::SimulatedAnnealing => "Simulated Annealing",
            Self::ProductionExtend => "Production-only / Extend",
            Self::NvtProduction => "NVT Production",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::EnergyMinimization => "Energy minimization only.",
            Self::QuickRelax => "EM → short NVT → short NPT, for fast prep or testing.",
            Self::StandardBiomolecule => {
                "EM → restrained NVT → restrained NPT → production. The default."
            }
            Self::CarefulEquilibration => {
                "EM → multi-step NVT/NPT with a decreasing restraint force constant → \
                 production. For large, sensitive, or hard-to-converge systems."
            }
            Self::MembraneProtein => {
                "Semi-isotropic pressure, smaller early timestep, gradual release of \
                 protein and lipid-headgroup restraints → production."
            }
            Self::ProteinLigand => "Standard, with the ligand restrained during equilibration.",
            Self::NucleicAcid => {
                "Extended equilibration with nucleic coupling groups → production."
            }
            Self::FastProduction => {
                "Production at a long (4 fs) timestep. Requires HMR from Build."
            }
            Self::SimulatedAnnealing => "Temperature ramp.",
            Self::ProductionExtend => {
                "Continue/extend an equilibrated restart, no new equilibration."
            }
            Self::NvtProduction => "Constant-volume production (less common).",
        }
    }

    /// Whether this preset is applicable to the system. Used by the recommender to
    /// filter the menu; everything is still selectable manually.
    pub fn applies_to(self, ctx: &EffectiveContext) -> bool {
        match self {
            Self::MembraneProtein => ctx.has_membrane(),
            Self::ProteinLigand => ctx.has_ligand(),
            Self::NucleicAcid => ctx.has_nucleic(),
            _ => true,
        }
    }

    /// Build the stage sequence for this preset against the system context.
    pub fn build(self, ctx: &EffectiveContext, params: &PresetParams) -> Vec<MdStage> {
        let t = params.temperature_k;
        let dt = params.timestep_ps;
        let groups = coupling_groups_for(ctx);
        let production_len = StageLength::Picoseconds(params.production.picoseconds());

        match self {
            Self::EnergyMinimization => vec![MdStage::minimize()],

            Self::QuickRelax => vec![
                MdStage::minimize(),
                short_equil(MdStage::nvt(t), dt, groups, 50.0),
                short_equil(MdStage::npt(t), dt, groups, 50.0),
            ],

            Self::StandardBiomolecule => vec![
                MdStage::minimize(),
                restrained(
                    short_equil(MdStage::nvt(t), dt, groups, 100.0),
                    ctx,
                    RESTRAINT_FC,
                ),
                restrained(
                    short_equil(MdStage::npt(t), dt, groups, 100.0),
                    ctx,
                    RESTRAINT_FC,
                ),
                production(MdStage::produce(t), dt, groups, production_len),
            ],

            Self::CarefulEquilibration => {
                let mut stages = vec![
                    MdStage::minimize(),
                    restrained(
                        short_equil(MdStage::nvt(t), dt, groups, 200.0),
                        ctx,
                        RESTRAINT_FC,
                    ),
                ];
                // NPT with a decreasing restraint force constant.
                for fc in [RESTRAINT_FC, 500.0, 100.0] {
                    stages.push(restrained(
                        short_equil(MdStage::npt(t), dt, groups, 200.0),
                        ctx,
                        fc,
                    ));
                }
                stages.push(production(MdStage::produce(t), dt, groups, production_len));
                name_uniquely(stages)
            }

            Self::MembraneProtein => {
                let mut stages = vec![MdStage::minimize()];
                // Staged restraint release with a smaller early timestep and
                // semi-isotropic pressure. ~6 equilibration sub-steps.
                let releases = [
                    (GENTLE_TIMESTEP_PS, RESTRAINT_FC),
                    (GENTLE_TIMESTEP_PS, RESTRAINT_FC),
                    (dt, 500.0),
                    (dt, 200.0),
                    (dt, 50.0),
                ];
                for (i, (step, fc)) in releases.iter().enumerate() {
                    let mut stage = MdStage::npt(t);
                    stage.pressure = Some(PressureCoupling::semi_isotropic());
                    stage.coupling_groups = CouplingGroups::SoluteLipidSolvent;
                    let stage = restrained(short_equil(stage, *step, groups, 100.0), ctx, *fc);
                    // First sub-step is the only one to also do NVT-style settle;
                    // keep them all NPT here for simplicity but distinct names.
                    let mut stage = stage;
                    stage.name = format!("npt{}", i + 1);
                    stages.push(stage);
                }
                let mut prod = production(MdStage::produce(t), dt, groups, production_len);
                prod.pressure = Some(PressureCoupling::semi_isotropic());
                prod.coupling_groups = CouplingGroups::SoluteLipidSolvent;
                stages.push(prod);
                stages
            }

            Self::ProteinLigand => vec![
                MdStage::minimize(),
                restrained(
                    short_equil(MdStage::nvt(t), dt, groups, 100.0),
                    ctx,
                    RESTRAINT_FC,
                ),
                restrained(
                    short_equil(MdStage::npt(t), dt, groups, 100.0),
                    ctx,
                    RESTRAINT_FC,
                ),
                production(MdStage::produce(t), dt, groups, production_len),
            ],

            Self::NucleicAcid => vec![
                MdStage::minimize(),
                // Extended equilibration for nucleic systems.
                restrained(
                    short_equil(MdStage::nvt(t), dt, groups, 200.0),
                    ctx,
                    RESTRAINT_FC,
                ),
                restrained(
                    short_equil(MdStage::npt(t), dt, groups, 500.0),
                    ctx,
                    RESTRAINT_FC,
                ),
                production(MdStage::produce(t), dt, groups, production_len),
            ],

            Self::FastProduction => {
                // Long timestep — requires HMR (validation enforces it).
                let long_dt = (dt.max(0.004)).max(0.004);
                vec![production(
                    MdStage::produce(t),
                    long_dt,
                    groups,
                    production_len,
                )]
            }

            Self::SimulatedAnnealing => {
                let mut anneal = MdStage::nvt(t);
                anneal.kind = StageKind::Anneal;
                anneal.name = StageKind::Anneal.default_name().to_string();
                anneal.coupling_groups = groups;
                anneal.timestep_ps = dt;
                anneal.length = StageLength::Picoseconds(500.0);
                anneal.anneal = Some(AnnealSpec::ramp(t, t + 50.0, 500.0));
                anneal.trajectory_target_frames = Some(super::stage::DEFAULT_TRAJECTORY_FRAMES);
                vec![
                    MdStage::minimize(),
                    short_equil(MdStage::nvt(t), dt, groups, 100.0),
                    anneal,
                ]
            }

            Self::ProductionExtend => {
                let mut extend = production(MdStage::produce(t), dt, groups, production_len);
                extend.kind = StageKind::Extend;
                extend.name = StageKind::Extend.default_name().to_string();
                vec![extend]
            }

            Self::NvtProduction => {
                let mut prod = production(MdStage::produce(t), dt, groups, production_len);
                // Constant volume: drop the barostat.
                prod.ensemble = super::stage::Ensemble::Nvt;
                prod.pressure = None;
                vec![
                    MdStage::minimize(),
                    restrained(
                        short_equil(MdStage::nvt(t), dt, groups, 100.0),
                        ctx,
                        RESTRAINT_FC,
                    ),
                    prod,
                ]
            }
        }
    }
}

/// Coupling groups appropriate to the system's composition. Group by physical
/// phase; don't over-split.
fn coupling_groups_for(ctx: &EffectiveContext) -> CouplingGroups {
    if ctx.is_framework() {
        CouplingGroups::WholeSystem
    } else if ctx.has_membrane() {
        CouplingGroups::SoluteLipidSolvent
    } else if ctx.has_nucleic() && !ctx.has_protein() {
        CouplingGroups::NucleicSolvent
    } else if ctx.has_protein() || ctx.has_ligand() || ctx.has_nucleic() {
        CouplingGroups::SoluteSolvent
    } else {
        CouplingGroups::WholeSystem
    }
}

/// Restraint group labels for the system: the solute always, plus lipid
/// headgroups for a membrane and the ligand when present (its heavy atoms join
/// the restraint set during equilibration).
fn restraint_groups_for(ctx: &EffectiveContext) -> Vec<String> {
    let mut groups = vec!["solute".to_string()];
    if ctx.has_membrane() {
        groups.push("lipid_headgroups".to_string());
    }
    if ctx.has_ligand() {
        groups.push("ligand".to_string());
    }
    groups
}

/// Set a dynamics stage to a short equilibration of `ps` picoseconds with the
/// given coupling groups and timestep.
fn short_equil(mut stage: MdStage, dt: f32, groups: CouplingGroups, ps: f64) -> MdStage {
    stage.timestep_ps = dt;
    stage.coupling_groups = groups;
    stage.length = StageLength::Picoseconds(ps);
    stage
}

/// Apply position restraints (force constant `fc`) on the system's restraint
/// groups to an equilibration stage.
fn restrained(mut stage: MdStage, ctx: &EffectiveContext, fc: f32) -> MdStage {
    stage.restraint = RestraintScheme::Posres {
        fc_kj_mol_nm2: fc,
        groups: restraint_groups_for(ctx),
    };
    stage
}

/// Configure a production stage: length, timestep, coupling groups, no restraints.
fn production(mut stage: MdStage, dt: f32, groups: CouplingGroups, length: StageLength) -> MdStage {
    stage.timestep_ps = dt;
    stage.coupling_groups = groups;
    stage.length = length;
    stage.restraint = RestraintScheme::None;
    stage
}

/// Give NPT sub-steps distinct names so stage-chaining keys stay unique.
fn name_uniquely(mut stages: Vec<MdStage>) -> Vec<MdStage> {
    let mut npt_index = 0;
    for stage in &mut stages {
        if stage.kind == StageKind::NptEquilibrate {
            npt_index += 1;
            stage.name = format!("npt{npt_index}");
        }
    }
    stages
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflows::molecular_dynamics::run::stage::PressureShape;
    use crate::workflows::molecular_dynamics::run::system_context::{
        ForceFieldFamily, MdSystemContext, SystemTypeOverrides,
    };

    fn context(protein: bool, membrane: bool, ligand: bool, nucleic: bool) -> MdSystemContext {
        MdSystemContext {
            force_field_token: "amber14sb".to_string(),
            force_field_family: ForceFieldFamily::Amber,
            water_token: Some("tip3p".to_string()),
            detected_protein: protein,
            detected_nucleic: nucleic,
            detected_membrane: membrane,
            detected_ligand: ligand,
            is_framework: false,
            net_charge: 0.0,
            atom_count: 10_000,
            restraint_groups: vec![],
            hmr_applied: false,
        }
    }

    #[test]
    fn standard_preset_is_em_nvt_npt_production_with_restrained_equilibration() {
        let ctx = context(true, false, false, false);
        let eff = ctx.with_overrides(SystemTypeOverrides::default());
        let stages = PresetId::StandardBiomolecule.build(&eff, &PresetParams::default());
        let kinds: Vec<StageKind> = stages.iter().map(|s| s.kind).collect();
        assert_eq!(
            kinds,
            vec![
                StageKind::Minimize,
                StageKind::NvtEquilibrate,
                StageKind::NptEquilibrate,
                StageKind::Produce,
            ]
        );
        // Equilibration restrained, production free.
        assert!(stages[1].restraint.is_restrained());
        assert!(stages[2].restraint.is_restrained());
        assert!(!stages[3].restraint.is_restrained());
    }

    #[test]
    fn careful_preset_releases_restraints_monotonically() {
        let ctx = context(true, false, false, false);
        let eff = ctx.with_overrides(SystemTypeOverrides::default());
        let stages = PresetId::CarefulEquilibration.build(&eff, &PresetParams::default());
        let fcs: Vec<f32> = stages
            .iter()
            .filter_map(|s| s.restraint.force_constant())
            .collect();
        // Decreasing (or equal) force constants across the restrained stages.
        assert!(
            fcs.windows(2).all(|w| w[0] >= w[1]),
            "force constants {fcs:?} not decreasing"
        );
        // Stage names are unique (chaining keys must not collide).
        let mut names: Vec<&str> = stages.iter().map(|s| s.name.as_str()).collect();
        names.sort_unstable();
        let unique = names.len();
        names.dedup();
        assert_eq!(names.len(), unique, "stage names must be unique");
    }

    #[test]
    fn membrane_preset_uses_semi_isotropic_and_three_group_coupling() {
        let ctx = context(true, true, false, false);
        let eff = ctx.with_overrides(SystemTypeOverrides::default());
        let stages = PresetId::MembraneProtein.build(&eff, &PresetParams::default());
        let prod = stages.last().unwrap();
        assert_eq!(prod.coupling_groups, CouplingGroups::SoluteLipidSolvent);
        assert_eq!(
            prod.pressure.map(|p| p.shape),
            Some(PressureShape::SemiIsotropic)
        );
        // The earliest equilibration sub-step uses the gentler timestep.
        let first_npt = stages
            .iter()
            .find(|s| s.kind == StageKind::NptEquilibrate)
            .unwrap();
        assert_eq!(first_npt.timestep_ps, GENTLE_TIMESTEP_PS);
    }

    #[test]
    fn applicability_filters_by_system_type() {
        let plain = context(true, false, false, false);
        let plain = plain.with_overrides(SystemTypeOverrides::default());
        assert!(PresetId::StandardBiomolecule.applies_to(&plain));
        assert!(!PresetId::MembraneProtein.applies_to(&plain));
        assert!(!PresetId::ProteinLigand.applies_to(&plain));

        let mem = context(true, true, true, false);
        let mem = mem.with_overrides(SystemTypeOverrides::default());
        assert!(PresetId::MembraneProtein.applies_to(&mem));
        assert!(PresetId::ProteinLigand.applies_to(&mem));
    }

    #[test]
    fn production_length_sets_step_count() {
        let ctx = context(true, false, false, false);
        let eff = ctx.with_overrides(SystemTypeOverrides::default());
        let params = PresetParams {
            production: ProductionLength::Test,
            ..PresetParams::default()
        };
        let stages = PresetId::StandardBiomolecule.build(&eff, &params);
        let prod = stages.last().unwrap();
        // 1 ns / 2 fs = 500,000 steps.
        assert_eq!(prod.steps(), 500_000);
    }

    #[test]
    fn token_round_trips() {
        for preset in PresetId::all() {
            assert_eq!(PresetId::from_token(preset.token()), Some(*preset));
        }
        assert_eq!(PresetId::from_token("bogus"), None);
    }
}
