//! Engine-neutral description of one molecular-dynamics stage.
//!
//! A [`MdStage`] carries *physical intent* — what kind of stage this is, its
//! ensemble, target temperature/pressure, restraint scheme, length, output
//! cadence, coupling groups, and a tier of finer parameters — without committing
//! to any engine's input syntax. A GROMACS (or future) adapter realizes a stage
//! into concrete engine input. Nothing here references `.mdp` keywords; the
//! `raw_passthrough` escape hatch is the one place arbitrary engine text rides
//! along, and even that is opaque to this layer.

use serde::{Deserialize, Serialize};

/// What work a stage performs. The adapter maps each kind to a concrete
/// integrator and coupling arrangement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StageKind {
    /// Energy minimization (no dynamics, no trajectory).
    Minimize,
    /// Constant-volume/temperature equilibration.
    NvtEquilibrate,
    /// Constant-pressure/temperature equilibration.
    NptEquilibrate,
    /// Production dynamics.
    Produce,
    /// A temperature-ramp (simulated annealing) dynamics stage.
    Anneal,
    /// Continue/extend an already-equilibrated run with no new equilibration.
    Extend,
}

impl StageKind {
    /// Whether this is a (trajectory-less) minimization rather than dynamics.
    pub fn is_minimization(self) -> bool {
        matches!(self, Self::Minimize)
    }

    /// Whether this stage integrates equations of motion (everything but
    /// minimization).
    pub fn is_dynamics(self) -> bool {
        !self.is_minimization()
    }

    /// Whether this is an equilibration stage (the stages that conventionally
    /// carry restraints), versus production/extend.
    pub fn is_equilibration(self) -> bool {
        matches!(self, Self::NvtEquilibrate | Self::NptEquilibrate)
    }

    /// Canonical short stage name, used as a default `name`.
    pub fn default_name(self) -> &'static str {
        match self {
            Self::Minimize => "em",
            Self::NvtEquilibrate => "nvt",
            Self::NptEquilibrate => "npt",
            Self::Produce => "md",
            Self::Anneal => "anneal",
            Self::Extend => "extend",
        }
    }

    /// Human-readable label.
    pub fn label(self) -> &'static str {
        match self {
            Self::Minimize => "Energy minimization",
            Self::NvtEquilibrate => "NVT equilibration",
            Self::NptEquilibrate => "NPT equilibration",
            Self::Produce => "Production",
            Self::Anneal => "Annealing",
            Self::Extend => "Extend",
        }
    }
}

/// Thermodynamic ensemble a dynamics stage samples.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Ensemble {
    /// Constant energy (no thermostat).
    Nve,
    /// Constant volume/temperature.
    Nvt,
    /// Constant pressure/temperature.
    Npt,
}

/// Temperature-control algorithm, named in engine-neutral terms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThermostatKind {
    /// Stochastic velocity rescaling — the robust modern default (GROMACS
    /// `V-rescale`).
    StochasticVelocityRescale,
    /// Nosé–Hoover (GROMACS `Nose-Hoover`).
    NoseHoover,
    /// Berendsen weak coupling — equilibration only.
    Berendsen,
}

/// Pressure-control algorithm, named in engine-neutral terms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BarostatKind {
    /// Stochastic cell rescaling — the modern default (GROMACS `C-rescale`,
    /// needs a recent engine version).
    StochasticCellRescale,
    /// Parrinello–Rahman — the classic production barostat; must not start from
    /// an unequilibrated configuration.
    ParrinelloRahman,
    /// Berendsen weak coupling — equilibration only.
    Berendsen,
}

impl BarostatKind {
    /// Whether this coupler is suitable only for equilibration and must never
    /// drive production (it does not sample the correct ensemble).
    pub fn is_equilibration_only(self) -> bool {
        matches!(self, Self::Berendsen)
    }
}

/// How the simulation box responds to pressure coupling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PressureShape {
    /// Uniform scaling — the standard for soluble systems.
    Isotropic,
    /// Independent in-plane (xy) and normal (z) scaling — membranes.
    SemiIsotropic,
    /// Fully independent per-axis scaling — crystals.
    Anisotropic,
}

/// Pressure-coupling intent for an NPT stage.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PressureCoupling {
    pub shape: PressureShape,
    pub barostat: BarostatKind,
    /// Reference pressure (bar).
    pub ref_bar: f32,
    /// Coupling time constant (ps).
    pub tau_ps: f32,
}

impl PressureCoupling {
    /// Isotropic 1 bar with the modern stochastic cell-rescale barostat.
    pub fn isotropic() -> Self {
        Self {
            shape: PressureShape::Isotropic,
            barostat: BarostatKind::StochasticCellRescale,
            ref_bar: 1.0,
            tau_ps: 2.0,
        }
    }

    /// Semi-isotropic 1 bar (membrane systems: in-plane and normal couple
    /// independently).
    pub fn semi_isotropic() -> Self {
        Self {
            shape: PressureShape::SemiIsotropic,
            ..Self::isotropic()
        }
    }
}

/// Which physical phases are coupled separately to the thermostat. Engine-neutral
/// — the adapter resolves these to concrete index-group names. The guidance is to
/// group by physical phase (solute / lipid / solvent), not to over-split.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CouplingGroups {
    /// One group for the whole system (homogeneous LJ / framework).
    WholeSystem,
    /// Solute vs. everything else (soluble protein, protein–ligand).
    SoluteSolvent,
    /// Solute / lipid / solvent (membrane systems).
    SoluteLipidSolvent,
    /// Nucleic acid vs. solvent.
    NucleicSolvent,
}

/// Position-restraint scheme applied during a stage.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum RestraintScheme {
    /// No restraints (production).
    None,
    /// Restrain the named groups' heavy atoms at the given force constant.
    Posres {
        /// Restraint force constant (kJ/mol/nm²).
        fc_kj_mol_nm2: f32,
        /// Engine-neutral group labels to restrain (e.g. "solute", "lipid").
        groups: Vec<String>,
    },
}

impl RestraintScheme {
    pub fn is_restrained(&self) -> bool {
        matches!(self, Self::Posres { .. })
    }

    /// The force constant, if restrained.
    pub fn force_constant(&self) -> Option<f32> {
        match self {
            Self::Posres { fc_kj_mol_nm2, .. } => Some(*fc_kj_mol_nm2),
            Self::None => None,
        }
    }
}

/// A temperature ramp for a simulated-annealing stage: monotonic `(time_ps,
/// temperature_k)` control points.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AnnealSpec {
    pub points: Vec<(f32, f32)>,
}

impl AnnealSpec {
    /// A single ramp from `start_k` to `end_k` over `duration_ps`.
    pub fn ramp(start_k: f32, end_k: f32, duration_ps: f32) -> Self {
        Self {
            points: vec![(0.0, start_k), (duration_ps, end_k)],
        }
    }
}

/// Stage length expressed in steps or picoseconds. Steps are exact; a duration in
/// ps derives a step count from the stage's timestep.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum StageLength {
    Steps(u64),
    Picoseconds(f64),
}

impl StageLength {
    /// Resolve to a concrete step count for the given timestep (ps).
    pub fn steps(self, timestep_ps: f32) -> u64 {
        match self {
            Self::Steps(n) => n,
            Self::Picoseconds(ps) => {
                if timestep_ps <= 0.0 {
                    0
                } else {
                    (ps / timestep_ps as f64).round() as u64
                }
            }
        }
    }
}

/// Bonds an engine converts to holonomic constraints (engine-neutral).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConstraintScope {
    /// Leave all bonds flexible.
    None,
    /// Constrain bonds to hydrogen (enables a 2 fs timestep).
    HBonds,
    /// Constrain all bonds.
    AllBonds,
}

/// Default-visibility tier for a parameter in the GUI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ParamTier {
    /// Always visible.
    Basic,
    /// One expand away.
    Standard,
    /// Collapsed by default.
    Advanced,
}

/// Tiered finer parameters. `None` means "unset — use the engine/preset default".
/// The Basic-tier choices (temperature, pressure, length, timestep, trajectory
/// cadence) are first-class fields on [`MdStage`]; everything finer lives here.
/// [`MdParameters::tiers`] maps each field to a [`ParamTier`] for GUI grouping.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MdParameters {
    // --- Standard tier ---
    /// Electrostatic real-space cutoff (nm).
    pub coulomb_cutoff_nm: Option<f32>,
    /// Van der Waals cutoff (nm).
    pub vdw_cutoff_nm: Option<f32>,
    pub thermostat: Option<ThermostatKind>,
    /// Thermostat coupling time (ps).
    pub thermostat_tau_ps: Option<f32>,
    pub constraints: Option<ConstraintScope>,
    // --- Advanced tier ---
    /// PME real-space grid spacing (nm).
    pub pme_spacing_nm: Option<f32>,
    /// PME interpolation order.
    pub pme_order: Option<u32>,
    /// Constraint solver order (e.g. LINCS order).
    pub constraint_order: Option<u32>,
    /// Constraint solver iterations.
    pub constraint_iterations: Option<u32>,
    /// Apply a long-range dispersion correction.
    pub dispersion_correction: Option<bool>,
    /// Remove center-of-mass motion.
    pub remove_com_motion: Option<bool>,
    /// Neighbor-list rebuild interval (steps).
    pub neighbor_list_steps: Option<u32>,
    /// Random seed for velocity generation; negative lets the engine pick.
    pub random_seed: Option<i64>,
}

/// A finer-parameter slot, paired with its tier and label, for GUI grouping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamId {
    CoulombCutoff,
    VdwCutoff,
    Thermostat,
    ThermostatTau,
    Constraints,
    PmeSpacing,
    PmeOrder,
    ConstraintOrder,
    ConstraintIterations,
    DispersionCorrection,
    RemoveComMotion,
    NeighborListSteps,
    RandomSeed,
}

impl ParamId {
    pub fn tier(self) -> ParamTier {
        match self {
            Self::CoulombCutoff
            | Self::VdwCutoff
            | Self::Thermostat
            | Self::ThermostatTau
            | Self::Constraints => ParamTier::Standard,
            Self::PmeSpacing
            | Self::PmeOrder
            | Self::ConstraintOrder
            | Self::ConstraintIterations
            | Self::DispersionCorrection
            | Self::RemoveComMotion
            | Self::NeighborListSteps
            | Self::RandomSeed => ParamTier::Advanced,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::CoulombCutoff => "Coulomb cutoff (nm)",
            Self::VdwCutoff => "VdW cutoff (nm)",
            Self::Thermostat => "Thermostat",
            Self::ThermostatTau => "Thermostat coupling time (ps)",
            Self::Constraints => "Constraints",
            Self::PmeSpacing => "PME grid spacing (nm)",
            Self::PmeOrder => "PME order",
            Self::ConstraintOrder => "Constraint solver order",
            Self::ConstraintIterations => "Constraint solver iterations",
            Self::DispersionCorrection => "Dispersion correction",
            Self::RemoveComMotion => "Remove COM motion",
            Self::NeighborListSteps => "Neighbor-list interval (steps)",
            Self::RandomSeed => "Random seed",
        }
    }
}

impl MdParameters {
    /// Every parameter slot with its tier, in display order (drives GUI grouping).
    pub fn tiers() -> &'static [ParamId] {
        &[
            ParamId::CoulombCutoff,
            ParamId::VdwCutoff,
            ParamId::Thermostat,
            ParamId::ThermostatTau,
            ParamId::Constraints,
            ParamId::PmeSpacing,
            ParamId::PmeOrder,
            ParamId::ConstraintOrder,
            ParamId::ConstraintIterations,
            ParamId::DispersionCorrection,
            ParamId::RemoveComMotion,
            ParamId::NeighborListSteps,
            ParamId::RandomSeed,
        ]
    }

    /// Overlay `other` onto `self`: any `Some` field in `other` wins. Used by the
    /// layered merge so user edits override preset defaults.
    pub fn overlay(&mut self, other: &MdParameters) {
        macro_rules! take {
            ($field:ident) => {
                if other.$field.is_some() {
                    self.$field = other.$field;
                }
            };
        }
        take!(coulomb_cutoff_nm);
        take!(vdw_cutoff_nm);
        take!(thermostat);
        take!(thermostat_tau_ps);
        take!(constraints);
        take!(pme_spacing_nm);
        take!(pme_order);
        take!(constraint_order);
        take!(constraint_iterations);
        take!(dispersion_correction);
        take!(remove_com_motion);
        take!(neighbor_list_steps);
        take!(random_seed);
    }
}

/// Default trajectory frame target: roughly how many frames a dynamics stage
/// should write so even a short stage yields a watchable track.
pub const DEFAULT_TRAJECTORY_FRAMES: u64 = 250;

/// One engine-neutral molecular-dynamics stage.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MdStage {
    pub kind: StageKind,
    /// Short stage name (becomes the run's `-deffnm` basename in GROMACS).
    pub name: String,
    pub ensemble: Ensemble,
    /// Target temperature (K). Ignored by pure minimization.
    pub temperature_k: f32,
    /// MD integration timestep (ps). Ignored by minimization.
    pub timestep_ps: f32,
    pub length: StageLength,
    /// Trajectory frame target. `None` => write no trajectory for this stage.
    pub trajectory_target_frames: Option<u64>,
    /// Pressure coupling (NPT only). `None` for NVE/NVT/minimization.
    pub pressure: Option<PressureCoupling>,
    pub coupling_groups: CouplingGroups,
    pub restraint: RestraintScheme,
    /// Temperature ramp, for an annealing stage.
    pub anneal: Option<AnnealSpec>,
    pub params: MdParameters,
    /// Free-form engine passthrough merged last; may introduce *any* key. Written
    /// verbatim by the adapter into the generated engine input.
    pub raw_passthrough: Vec<(String, String)>,
}

impl MdStage {
    /// A steepest-descent energy-minimization stage.
    pub fn minimize() -> Self {
        Self {
            kind: StageKind::Minimize,
            name: StageKind::Minimize.default_name().to_string(),
            ensemble: Ensemble::Nve,
            temperature_k: 300.0,
            timestep_ps: 0.0,
            length: StageLength::Steps(50_000),
            trajectory_target_frames: None,
            pressure: None,
            coupling_groups: CouplingGroups::WholeSystem,
            restraint: RestraintScheme::None,
            anneal: None,
            params: MdParameters::default(),
            raw_passthrough: Vec::new(),
        }
    }

    /// An NVT equilibration stage at `temperature_k`.
    pub fn nvt(temperature_k: f32) -> Self {
        Self {
            kind: StageKind::NvtEquilibrate,
            name: StageKind::NvtEquilibrate.default_name().to_string(),
            ensemble: Ensemble::Nvt,
            temperature_k,
            timestep_ps: 0.002,
            length: StageLength::Picoseconds(100.0),
            trajectory_target_frames: Some(DEFAULT_TRAJECTORY_FRAMES),
            pressure: None,
            coupling_groups: CouplingGroups::WholeSystem,
            restraint: RestraintScheme::None,
            anneal: None,
            params: MdParameters::default(),
            raw_passthrough: Vec::new(),
        }
    }

    /// An NPT equilibration stage at `temperature_k`, isotropic 1 bar.
    pub fn npt(temperature_k: f32) -> Self {
        Self {
            kind: StageKind::NptEquilibrate,
            name: StageKind::NptEquilibrate.default_name().to_string(),
            ensemble: Ensemble::Npt,
            pressure: Some(PressureCoupling::isotropic()),
            length: StageLength::Picoseconds(100.0),
            ..Self::nvt(temperature_k)
        }
    }

    /// A production stage at `temperature_k`, NPT isotropic 1 bar.
    pub fn produce(temperature_k: f32) -> Self {
        Self {
            kind: StageKind::Produce,
            name: StageKind::Produce.default_name().to_string(),
            length: StageLength::Picoseconds(100_000.0),
            ..Self::npt(temperature_k)
        }
    }

    /// Resolve this stage's length to a step count.
    pub fn steps(&self) -> u64 {
        self.length.steps(self.timestep_ps)
    }

    /// Whether this stage writes a trajectory (dynamics with a frame target).
    pub fn writes_trajectory(&self) -> bool {
        self.kind.is_dynamics() && self.trajectory_target_frames.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stage_length_derives_steps_from_picoseconds() {
        assert_eq!(StageLength::Picoseconds(100.0).steps(0.002), 50_000);
        assert_eq!(StageLength::Steps(1234).steps(0.002), 1234);
        // Guards against division by a zero timestep.
        assert_eq!(StageLength::Picoseconds(100.0).steps(0.0), 0);
    }

    #[test]
    fn minimize_stage_writes_no_trajectory_and_has_no_coupling() {
        let em = MdStage::minimize();
        assert!(em.kind.is_minimization());
        assert!(!em.writes_trajectory());
        assert!(em.pressure.is_none());
        assert_eq!(em.restraint, RestraintScheme::None);
    }

    #[test]
    fn npt_adds_pressure_over_nvt() {
        let nvt = MdStage::nvt(310.0);
        let npt = MdStage::npt(310.0);
        assert!(nvt.pressure.is_none());
        assert_eq!(
            npt.pressure.map(|p| p.shape),
            Some(PressureShape::Isotropic)
        );
        assert_eq!(npt.temperature_k, 310.0);
        assert!(npt.writes_trajectory());
    }

    #[test]
    fn berendsen_is_equilibration_only() {
        assert!(BarostatKind::Berendsen.is_equilibration_only());
        assert!(!BarostatKind::StochasticCellRescale.is_equilibration_only());
        assert!(!BarostatKind::ParrinelloRahman.is_equilibration_only());
    }

    #[test]
    fn parameters_overlay_keeps_existing_where_other_is_unset() {
        let mut base = MdParameters {
            coulomb_cutoff_nm: Some(1.0),
            vdw_cutoff_nm: Some(1.0),
            ..Default::default()
        };
        let edits = MdParameters {
            vdw_cutoff_nm: Some(1.2),
            pme_order: Some(6),
            ..Default::default()
        };
        base.overlay(&edits);
        assert_eq!(base.coulomb_cutoff_nm, Some(1.0)); // untouched
        assert_eq!(base.vdw_cutoff_nm, Some(1.2)); // overridden
        assert_eq!(base.pme_order, Some(6)); // introduced
    }

    #[test]
    fn param_tiers_are_stable_and_classified() {
        assert_eq!(ParamId::CoulombCutoff.tier(), ParamTier::Standard);
        assert_eq!(ParamId::PmeOrder.tier(), ParamTier::Advanced);
        // Every slot is enumerated exactly once.
        assert_eq!(MdParameters::tiers().len(), 13);
    }

    #[test]
    fn descriptor_table_drives_the_inline_detail_split() {
        // The GUI partitions the detail view straight off this table: nothing in
        // `MdParameters` is Basic (the Basic/inline set — temperature, pressure,
        // length, timestep — is first-class on `MdStage`), so every table entry
        // falls into the Standard (shown) or Advanced (collapsed) detail tier.
        let count = |tier: ParamTier| {
            MdParameters::tiers()
                .iter()
                .filter(|pid| pid.tier() == tier)
                .count()
        };
        assert_eq!(count(ParamTier::Basic), 0);
        assert_eq!(count(ParamTier::Standard), 5);
        assert_eq!(count(ParamTier::Advanced), 8);
        assert_eq!(
            count(ParamTier::Basic) + count(ParamTier::Standard) + count(ParamTier::Advanced),
            MdParameters::tiers().len()
        );
    }
}
