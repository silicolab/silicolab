use std::path::PathBuf;

/// Which sizing strategy the MD system panel is currently editing. Both sets of
/// values are retained so toggling between modes does not lose the user's input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MdSystemSizingMode {
    #[default]
    Padding,
    Absolute,
}

/// User-editable configuration for the MD system builder. Padding and absolute
/// edge lengths are both held (per-axis, in angstroms); `mode` selects which
/// drives the build, and `shape` selects the lattice geometry.
///
/// The solvation fields mirror [`SolvationOptions`](crate::workflows::molecular_dynamics::SolvationOptions)
/// so the System Builder can box, solvate, and ionize in one step.
/// When `solvate` is false the box is built empty and the remaining fields are ignored.
#[derive(Debug, Clone)]
pub struct MdSystemPrompt {
    /// Human-readable run name; becomes the run directory's name. Seeded with a
    /// suggested `{kind}-N` when the panel opens, but freely editable.
    pub run_name: String,
    /// Which engine assembles the system. GROMACS (the default) produces a
    /// force-field topology a run reuses; the built-in path is geometry only.
    pub engine: MdBuildEngine,
    /// For a periodic framework (nanosheet) built with GROMACS, whether the
    /// sheet is modeled rigidly (frozen) or flexibly (bonded). Ignored for
    /// non-framework structures.
    pub framework_mode: crate::workflows::molecular_dynamics::FrameworkMode,
    /// For a periodic framework (nanosheet), the simulation cell's lattice
    /// parameters `[a, b, c, α, β, γ]` (lengths in A, angles in degrees), seeded
    /// from the input crystal cell when the panel opens and freely editable. The
    /// build uses this cell verbatim, preserving its shape (e.g. hexagonal), so
    /// the box matches the material rather than a generic cuboid. `None` until
    /// seeded / for non-framework structures.
    pub framework_cell: Option<[f32; 6]>,
    /// Name of the custom force field (from the reusable library) merged into a
    /// framework build, or `None` for built-in parameters only. Used to cover
    /// elements the built-in tables lack, or to override built-in types.
    pub custom_force_field: Option<String>,
    /// Cached `.itp` text of the selected `custom_force_field`, loaded when the
    /// selection changes so the panel and build don't re-read it each frame.
    pub custom_force_field_text: Option<String>,
    /// Draft name and `.itp` text for composing/importing a new custom force
    /// field before saving it to the library.
    pub custom_ff_draft_name: String,
    pub custom_ff_draft: String,
    pub mode: MdSystemSizingMode,
    pub padding_angstrom: [f32; 3],
    pub absolute_angstrom: [f32; 3],
    pub shape: crate::workflows::molecular_dynamics::BoxShape,
    /// Fill the box with explicit water and ions after building it.
    pub solvate: bool,
    pub water: crate::workflows::molecular_dynamics::WaterModel,
    pub force_field: String,
    /// Add the minimum ions needed to make the system net-neutral.
    pub neutralize: bool,
    /// Add a background salt bath at `salt_concentration_molar`.
    pub add_salt: bool,
    pub salt_concentration_molar: f32,
    pub positive_ion: String,
    pub negative_ion: String,
    /// Where the build executes: locally or on a configured remote host. Seeded
    /// from `config.default_compute_target` when the panel opens.
    pub target: crate::backend::config::ComputeTarget,
}

impl Default for MdSystemPrompt {
    fn default() -> Self {
        // Seed the solvation fields from the engine-neutral defaults so the GUI
        // and the `md solvate` console command start from the same place.
        let solv = crate::workflows::molecular_dynamics::SolvationOptions::default();
        Self {
            run_name: String::new(),
            engine: MdBuildEngine::default(),
            framework_mode: crate::workflows::molecular_dynamics::FrameworkMode::Rigid,
            framework_cell: None,
            custom_force_field: None,
            custom_force_field_text: None,
            custom_ff_draft_name: String::new(),
            custom_ff_draft: String::new(),
            mode: MdSystemSizingMode::Padding,
            padding_angstrom: [crate::workflows::molecular_dynamics::DEFAULT_PADDING_ANGSTROM; 3],
            absolute_angstrom: [30.0; 3],
            shape: crate::workflows::molecular_dynamics::BoxShape::default(),
            solvate: false,
            water: solv.water,
            force_field: crate::workflows::molecular_dynamics::DEFAULT_FORCE_FIELD.to_string(),
            neutralize: solv.neutralize,
            add_salt: false,
            salt_concentration_molar: 0.15,
            positive_ion: solv.positive_ion,
            negative_ion: solv.negative_ion,
            target: crate::backend::config::ComputeTarget::Local,
        }
    }
}

impl MdSystemPrompt {
    pub fn config(&self) -> crate::workflows::molecular_dynamics::MdSystemConfig {
        use crate::workflows::molecular_dynamics::{BoxSizing, MdSystemConfig};
        let sizing = match self.mode {
            MdSystemSizingMode::Padding => BoxSizing::Padding {
                padding_angstrom: self.padding_angstrom,
            },
            MdSystemSizingMode::Absolute => BoxSizing::Absolute {
                edges_angstrom: self.absolute_angstrom,
            },
        };
        MdSystemConfig {
            sizing,
            shape: self.shape,
        }
    }

    /// The solvation request this prompt describes, or `None` when solvation is
    /// disabled. Folds the `add_salt` toggle and concentration into the engine's
    /// `Option<f32>` concentration field.
    pub fn solvation_options(
        &self,
    ) -> Option<crate::workflows::molecular_dynamics::SolvationOptions> {
        if !self.solvate {
            return None;
        }
        Some(crate::workflows::molecular_dynamics::SolvationOptions {
            water: self.water,
            positive_ion: self.positive_ion.clone(),
            negative_ion: self.negative_ion.clone(),
            neutralize: self.neutralize,
            concentration_molar: self.add_salt.then_some(self.salt_concentration_molar),
        })
    }
}

/// Which engine the MD System Builder uses to assemble the system.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MdBuildEngine {
    /// Run GROMACS' pdb2gmx → editconf → solvate → genion pipeline. Assigns a
    /// force field and writes a `topol.top` an MD run reuses directly.
    #[default]
    Gromacs,
    /// Built-in geometry-only build: periodic box plus solvation coordinates,
    /// with no force field or topology. A run still needs a topology supplied
    /// separately.
    BuiltIn,
}

impl MdBuildEngine {
    pub fn all() -> &'static [Self] {
        &[Self::Gromacs, Self::BuiltIn]
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Gromacs => "GROMACS",
            Self::BuiltIn => "Built-in",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MdEngineChoice {
    #[default]
    Gromacs,
}

impl MdEngineChoice {
    pub fn all() -> &'static [Self] {
        &[Self::Gromacs]
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Gromacs => "GROMACS",
        }
    }
}

/// Which detected system-type flag a Run MD override toggle edits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MdSystemAxis {
    Membrane,
    Ligand,
    Nucleic,
}

/// A single edit to one stage of a Run MD draft. Each variant targets one field
/// of the neutral [`MdStage`](crate::workflows::molecular_dynamics::MdStage); the
/// detail-view widgets emit these and the dispatcher applies them through
/// [`MdRunPrompt::edit_stage`], keeping the dispatcher the sole mutator. The
/// resolved stage realizes through the same adapter as the headless
/// `md run --set/--raw` path, so the two stay one source of truth.
#[derive(Debug, Clone)]
pub enum MdStageEdit {
    // --- Inline (Basic) fields, also reachable in the detail view ---
    Temperature(f32),
    /// Reference pressure (bar) for a pressure-coupled stage.
    PressureBar(f32),
    Length(crate::workflows::molecular_dynamics::StageLength),
    // --- Detail-view structural fields ---
    Timestep(f32),
    Thermostat(Option<crate::workflows::molecular_dynamics::run::ThermostatKind>),
    ThermostatTau(Option<f32>),
    Barostat(crate::workflows::molecular_dynamics::run::BarostatKind),
    BarostatTau(f32),
    CouplingGroups(crate::workflows::molecular_dynamics::run::CouplingGroups),
    Constraints(Option<crate::workflows::molecular_dynamics::run::ConstraintScope>),
    /// Restraint force constant (kJ/mol/nm²); only meaningful on a restrained stage.
    RestraintForceConstant(f32),
    /// A single-ramp annealing schedule (start K, end K, duration ps).
    Anneal {
        start_k: f32,
        end_k: f32,
        duration_ps: f32,
    },
    // --- Detail-view tiered parameters (the `ParamId` table) ---
    CoulombCutoff(Option<f32>),
    VdwCutoff(Option<f32>),
    PmeSpacing(Option<f32>),
    PmeOrder(Option<u32>),
    ConstraintOrder(Option<u32>),
    ConstraintIterations(Option<u32>),
    DispersionCorrection(Option<bool>),
    RemoveComMotion(Option<bool>),
    NeighborListSteps(Option<u32>),
    RandomSeed(Option<i64>),
    // --- Per-stage raw passthrough ---
    AddRawLine,
    SetRawLine {
        line: usize,
        key: String,
        value: String,
    },
    RemoveRawLine(usize),
}

/// Recommendation-led draft for a Run MD launch. Holds the inherited build-time
/// detection (`context`, read-only) strictly separate from the user's
/// per-run corrections (`overrides`), so an override never writes back into the
/// persisted context. The editable `stages` are the engine-neutral
/// [`MdStage`](crate::workflows::molecular_dynamics::MdStage) sequence; changing
/// the preset or an override rebuilds them, while Basic-parameter edits and
/// add/remove/reorder mutate them in place.
#[derive(Debug, Clone)]
pub struct MdRunPrompt {
    /// Human-readable run name; becomes the run directory's name.
    pub run_name: String,
    pub engine: MdEngineChoice,
    /// Inherited build-time detection record (read-only). `None` until loaded
    /// when the panel opens.
    pub context: Option<crate::workflows::molecular_dynamics::MdSystemContext>,
    /// Per-run user corrections to the detected system types; never written back
    /// into `context`.
    pub overrides: crate::workflows::molecular_dynamics::SystemTypeOverrides,
    pub preset: crate::workflows::molecular_dynamics::PresetId,
    pub params: crate::workflows::molecular_dynamics::PresetParams,
    /// The editable stage sequence.
    pub stages: Vec<crate::workflows::molecular_dynamics::MdStage>,
    /// Save a compressed trajectory for each dynamics stage. On by default.
    pub save_trajectory: bool,
    pub topology_override_path: Option<PathBuf>,
    pub show_advanced: bool,
    /// Which stage's detail view is currently expanded (one at a time).
    pub expanded_stage: Option<usize>,
    /// Where the run executes: locally or on a configured remote host. Seeded from
    /// `config.default_compute_target` when the panel opens.
    pub target: crate::backend::config::ComputeTarget,
}

impl Default for MdRunPrompt {
    fn default() -> Self {
        Self {
            run_name: String::new(),
            engine: MdEngineChoice::Gromacs,
            context: None,
            overrides: Default::default(),
            preset: crate::workflows::molecular_dynamics::PresetId::StandardBiomolecule,
            params: crate::workflows::molecular_dynamics::PresetParams::default(),
            stages: Vec::new(),
            save_trajectory: true,
            topology_override_path: None,
            show_advanced: false,
            expanded_stage: None,
            target: crate::backend::config::ComputeTarget::Local,
        }
    }
}

impl MdRunPrompt {
    /// The effective context (detection overlaid with overrides) used for
    /// recommendation, preset building, and validation.
    pub fn effective(&self) -> Option<crate::workflows::molecular_dynamics::EffectiveContext<'_>> {
        self.context
            .as_ref()
            .map(|context| context.with_overrides(self.overrides))
    }

    /// The force-field family the run realizes against (generic if no context).
    pub fn force_field_family(&self) -> crate::workflows::molecular_dynamics::ForceFieldFamily {
        self.context.as_ref().map_or(
            crate::workflows::molecular_dynamics::ForceFieldFamily::Other,
            |context| context.force_field_family,
        )
    }

    /// Rebuild the stage list from the current preset, params, and effective
    /// context. Called when the preset or an override changes.
    pub fn rebuild_stages(&mut self) {
        if let Some(context) = &self.context {
            let eff = context.with_overrides(self.overrides);
            self.stages = self.preset.build(&eff, &self.params);
            self.apply_trajectory_flag();
        }
    }

    /// Apply the run-level temperature to every stage, preserving the stage list.
    pub fn apply_temperature(&mut self, temperature_k: f32) {
        self.params.temperature_k = temperature_k;
        for stage in &mut self.stages {
            stage.temperature_k = temperature_k;
        }
    }

    /// Apply the run-level timestep to every dynamics stage.
    pub fn apply_timestep(&mut self, timestep_ps: f32) {
        self.params.timestep_ps = timestep_ps;
        for stage in &mut self.stages {
            if stage.kind.is_dynamics() {
                stage.timestep_ps = timestep_ps;
            }
        }
    }

    /// Apply the run-level production length to the production/extend stage(s).
    pub fn apply_production(
        &mut self,
        production: crate::workflows::molecular_dynamics::ProductionLength,
    ) {
        use crate::workflows::molecular_dynamics::{StageKind, StageLength};
        self.params.production = production;
        for stage in &mut self.stages {
            if matches!(stage.kind, StageKind::Produce | StageKind::Extend) {
                stage.length = StageLength::Picoseconds(production.picoseconds());
            }
        }
    }

    /// Toggle whether dynamics stages write a trajectory.
    pub fn set_save_trajectory(&mut self, save: bool) {
        self.save_trajectory = save;
        self.apply_trajectory_flag();
    }

    fn apply_trajectory_flag(&mut self) {
        let frames = self
            .save_trajectory
            .then_some(crate::workflows::molecular_dynamics::DEFAULT_TRAJECTORY_FRAMES);
        for stage in &mut self.stages {
            if stage.kind.is_dynamics() {
                stage.trajectory_target_frames = frames;
            }
        }
    }

    /// Append a stage of the given kind, with a name made unique against the
    /// existing stages (stage names key the run's file chaining).
    pub fn add_stage(&mut self, kind: crate::workflows::molecular_dynamics::StageKind) {
        use crate::workflows::molecular_dynamics::{AnnealSpec, MdStage, StageKind};
        let t = self.params.temperature_k;
        let mut stage = match kind {
            StageKind::Minimize => MdStage::minimize(),
            StageKind::NvtEquilibrate => MdStage::nvt(t),
            StageKind::NptEquilibrate => MdStage::npt(t),
            StageKind::Produce => MdStage::produce(t),
            StageKind::Anneal => {
                let mut stage = MdStage::nvt(t);
                stage.kind = StageKind::Anneal;
                stage.name = StageKind::Anneal.default_name().to_string();
                stage.anneal = Some(AnnealSpec::ramp(t, t + 50.0, 500.0));
                stage
            }
            StageKind::Extend => {
                let mut stage = MdStage::produce(t);
                stage.kind = StageKind::Extend;
                stage.name = StageKind::Extend.default_name().to_string();
                stage
            }
        };
        if stage.kind.is_dynamics() {
            stage.timestep_ps = self.params.timestep_ps;
        }
        self.assign_unique_name(&mut stage);
        self.stages.push(stage);
        self.apply_trajectory_flag();
    }

    fn assign_unique_name(&self, stage: &mut crate::workflows::molecular_dynamics::MdStage) {
        let base = stage.name.clone();
        let mut name = base.clone();
        let mut suffix = 1;
        while self.stages.iter().any(|existing| existing.name == name) {
            suffix += 1;
            name = format!("{base}{suffix}");
        }
        stage.name = name;
    }

    pub fn remove_stage(&mut self, index: usize) {
        if index < self.stages.len() {
            self.stages.remove(index);
        }
    }

    pub fn move_stage(&mut self, index: usize, up: bool) {
        if up && index > 0 {
            self.stages.swap(index, index - 1);
        } else if !up && index + 1 < self.stages.len() {
            self.stages.swap(index, index + 1);
        }
    }

    /// Toggle the detail view of the stage at `index` (only one open at a time).
    pub fn toggle_stage_expanded(&mut self, index: usize) {
        self.expanded_stage = if self.expanded_stage == Some(index) {
            None
        } else {
            Some(index)
        };
    }

    /// Apply one detail/inline edit to the stage at `index`. Mutates the stage in
    /// place (preserving the rest of the sequence and any add/remove/reorder), so
    /// preset-filled defaults remain the starting point and only the touched field
    /// changes.
    pub fn edit_stage(&mut self, index: usize, edit: MdStageEdit) {
        use crate::workflows::molecular_dynamics::{AnnealSpec, RestraintScheme};
        let Some(stage) = self.stages.get_mut(index) else {
            return;
        };
        match edit {
            MdStageEdit::Temperature(t) => stage.temperature_k = t,
            MdStageEdit::PressureBar(p) => {
                if let Some(pressure) = stage.pressure.as_mut() {
                    pressure.ref_bar = p;
                }
            }
            MdStageEdit::Length(length) => stage.length = length,
            MdStageEdit::Timestep(dt) => stage.timestep_ps = dt,
            MdStageEdit::Thermostat(kind) => stage.params.thermostat = kind,
            MdStageEdit::ThermostatTau(tau) => stage.params.thermostat_tau_ps = tau,
            MdStageEdit::Barostat(kind) => {
                if let Some(pressure) = stage.pressure.as_mut() {
                    pressure.barostat = kind;
                }
            }
            MdStageEdit::BarostatTau(tau) => {
                if let Some(pressure) = stage.pressure.as_mut() {
                    pressure.tau_ps = tau;
                }
            }
            MdStageEdit::CouplingGroups(groups) => stage.coupling_groups = groups,
            MdStageEdit::Constraints(scope) => stage.params.constraints = scope,
            MdStageEdit::RestraintForceConstant(fc) => {
                if let RestraintScheme::Posres { fc_kj_mol_nm2, .. } = &mut stage.restraint {
                    *fc_kj_mol_nm2 = fc;
                }
            }
            MdStageEdit::Anneal {
                start_k,
                end_k,
                duration_ps,
            } => stage.anneal = Some(AnnealSpec::ramp(start_k, end_k, duration_ps)),
            MdStageEdit::CoulombCutoff(v) => stage.params.coulomb_cutoff_nm = v,
            MdStageEdit::VdwCutoff(v) => stage.params.vdw_cutoff_nm = v,
            MdStageEdit::PmeSpacing(v) => stage.params.pme_spacing_nm = v,
            MdStageEdit::PmeOrder(v) => stage.params.pme_order = v,
            MdStageEdit::ConstraintOrder(v) => stage.params.constraint_order = v,
            MdStageEdit::ConstraintIterations(v) => stage.params.constraint_iterations = v,
            MdStageEdit::DispersionCorrection(v) => stage.params.dispersion_correction = v,
            MdStageEdit::RemoveComMotion(v) => stage.params.remove_com_motion = v,
            MdStageEdit::NeighborListSteps(v) => stage.params.neighbor_list_steps = v,
            MdStageEdit::RandomSeed(v) => stage.params.random_seed = v,
            MdStageEdit::AddRawLine => stage.raw_passthrough.push((String::new(), String::new())),
            MdStageEdit::SetRawLine { line, key, value } => {
                if let Some(slot) = stage.raw_passthrough.get_mut(line) {
                    *slot = (key, value);
                }
            }
            MdStageEdit::RemoveRawLine(line) => {
                if line < stage.raw_passthrough.len() {
                    stage.raw_passthrough.remove(line);
                }
            }
        }
    }
}
