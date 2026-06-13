use std::{collections::BTreeMap, path::PathBuf};

use crate::{
    backend::{
        config::{AppConfig, RecentProject},
        entries::EntryStore,
        history::{EditSnapshot, History},
        project::WorkspaceSession,
        storage::ProjectSnapshot,
        tasks::TaskManager,
    },
    domain::Structure,
    frontend::{
        AtomSelection, BuildingBlockEditor, CommandConsoleState, NanosheetBuilderPanel,
        ReticularBuilderPanel, StructureEditor, ViewCamera, ViewportVisualState,
        jobs::JobManager,
        viewport::ViewportCache,
        viewport_defaults::{apply_entry_render_defaults, apply_solvent_render_default},
    },
    io::structure_io,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrimaryView {
    EntryList,
    Tasks,
    Style,
}

impl PrimaryView {
    pub fn all() -> &'static [Self] {
        &[Self::EntryList, Self::Tasks, Self::Style]
    }

    pub fn icon(self) -> &'static str {
        match self {
            Self::EntryList => egui_phosphor::regular::LIST,
            Self::Tasks => egui_phosphor::regular::LIGHTNING,
            Self::Style => egui_phosphor::regular::PALETTE,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::EntryList => "Entry List",
            Self::Tasks => "Tasks",
            Self::Style => "Style",
        }
    }

    /// Compact label shown inside the sidebar's segmented view switcher, where
    /// each segment is only a third of the sidebar width.
    pub fn short_label(self) -> &'static str {
        match self {
            Self::EntryList => "Entries",
            Self::Tasks => "Tasks",
            Self::Style => "Style",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelTab {
    Output,
    Console,
    TaskMonitor,
}

impl PanelTab {
    pub fn all() -> &'static [Self] {
        &[Self::Console, Self::TaskMonitor, Self::Output]
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Console => "Console",
            Self::TaskMonitor => "Task Monitor",
            Self::Output => "Output",
        }
    }
}

/// An item in the sidebar list that can be selected: either an entry or a group header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelectionItem {
    Entry(u64),
    Group(String),
}

#[derive(Debug, Clone, Default)]
pub struct EntryListState {
    pub search_query: String,
    pub search_open: bool,
    pub selected_entry_ids: std::collections::BTreeSet<u64>,
    pub selected_group_ids: std::collections::BTreeSet<String>,
    pub selection_anchor: Option<SelectionItem>,
    pub collapsed_group_ids: std::collections::BTreeSet<String>,
    pub renaming_entry_id: Option<u64>,
    pub rename_buffer: String,
    pub creating_group: bool,
    pub new_group_name: String,
    pub renaming_group_id: Option<String>,
    pub rename_group_buffer: String,
    /// Set once focus is handed to the group rename editor, so it is requested
    /// only on the first frame of a rename.
    pub rename_group_focus_requested: bool,
}

#[derive(Debug, Clone)]
pub struct LayoutState {
    pub active_primary_view: PrimaryView,
    pub active_panel_tab: PanelTab,
    pub show_primary_sidebar: bool,
    pub show_secondary_sidebar: bool,
    pub show_panel: bool,
    /// Whether the Settings modal dialog is open. Transient window chrome (it is
    /// never persisted), so — like the sidebar-visibility flags above — it is
    /// flipped directly by the UI entry points rather than through an AppAction:
    /// no persisted state changes when it toggles, so there is nothing for the
    /// dispatcher to mediate.
    pub settings_open: bool,
    pub primary_sidebar_width: f32,
    pub secondary_sidebar_width: f32,
    pub panel_height: f32,
}

pub const SIDEBAR_MIN_WIDTH_PRIMARY: f32 = 220.0;
pub const SIDEBAR_MIN_WIDTH_SECONDARY: f32 = 240.0;
pub const SIDEBAR_DEFAULT_WIDTH_PRIMARY: f32 = 240.0;
pub const SIDEBAR_DEFAULT_WIDTH_SECONDARY: f32 = 320.0;
pub const PANEL_MIN_HEIGHT: f32 = 120.0;
pub const PANEL_DEFAULT_HEIGHT: f32 = 180.0;

/// Maximum allowed width for either sidebar: half the window, capped at 480 px,
/// but never below `SIDEBAR_MIN_WIDTH_SECONDARY` so `clamp(min, max_w)` is always
/// valid (std clamp requires `min <= max`). Shared by the UI rendering pass and the
/// resize dispatcher.
pub fn sidebar_max_width(viewport_width: f32) -> f32 {
    (viewport_width * 0.5).clamp(SIDEBAR_MIN_WIDTH_SECONDARY, 480.0)
}

impl Default for LayoutState {
    fn default() -> Self {
        Self {
            active_primary_view: PrimaryView::EntryList,
            active_panel_tab: PanelTab::Console,
            show_primary_sidebar: true,
            show_secondary_sidebar: false,
            show_panel: true,
            settings_open: false,
            primary_sidebar_width: SIDEBAR_DEFAULT_WIDTH_PRIMARY,
            secondary_sidebar_width: SIDEBAR_DEFAULT_WIDTH_SECONDARY,
            panel_height: PANEL_DEFAULT_HEIGHT,
        }
    }
}

/// Which sidebar a layout resize action targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Primary,
    Secondary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoordinateOptimizationScope {
    AllAtoms,
    SelectedAtoms,
}

/// Per-atom drawing style, applied to a selection of atoms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum AtomStyle {
    /// Polymer-backbone ribbon. Only standard amino-acid residues actually
    /// render as cartoon; other atoms styled this way are not drawn.
    Cartoon,
    /// Not drawn at all.
    Hidden,
    /// A small flat disc per atom. Cheapest; ideal for bulk solvent and ions.
    Point,
    /// Bonds as thin lines only; atoms carry no marker. Ideal for bulk
    /// solvent — pure lines, no dots.
    Wireframe,
    /// Bonds as cylinders, no atom spheres.
    Stick,
    /// Cylinders plus small atom spheres.
    #[default]
    BallAndStick,
    /// Full van der Waals spheres, no bonds.
    Sphere,
}

impl AtomStyle {
    pub fn all() -> &'static [Self] {
        &[
            Self::Cartoon,
            Self::BallAndStick,
            Self::Stick,
            Self::Wireframe,
            Self::Sphere,
            Self::Point,
            Self::Hidden,
        ]
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Cartoon => "Cartoon",
            Self::Hidden => "Hidden",
            Self::Point => "Dots",
            Self::Wireframe => "Wireframe",
            Self::Stick => "Stick",
            Self::BallAndStick => "Ball-and-stick",
            Self::Sphere => "Sphere (VdW)",
        }
    }

    /// Stable string token for persistence and the console.
    pub fn token(self) -> &'static str {
        match self {
            Self::Cartoon => "cartoon",
            Self::Hidden => "hidden",
            Self::Point => "dots",
            Self::Wireframe => "wireframe",
            Self::Stick => "stick",
            Self::BallAndStick => "ball-stick",
            Self::Sphere => "sphere",
        }
    }

    pub fn from_token(token: &str) -> Option<Self> {
        Some(match token {
            "cartoon" => Self::Cartoon,
            "hidden" | "hide" => Self::Hidden,
            "dots" | "point" | "points" => Self::Point,
            "wireframe" | "line" | "lines" => Self::Wireframe,
            "stick" | "licorice" => Self::Stick,
            "ball-stick" | "ball_and_stick" => Self::BallAndStick,
            "sphere" | "spheres" | "vdw" => Self::Sphere,
            _ => return None,
        })
    }

    /// Whether atoms in this style draw a tessellated sphere, and at what
    /// fraction of the element's display radius. `None` means the atom is drawn
    /// as a flat point disc, via the cartoon path, or not at all.
    pub fn sphere_radius_scale(self) -> Option<f32> {
        match self {
            Self::Sphere => Some(1.0),
            Self::BallAndStick => Some(0.78),
            // A small joint so isolated atoms (lone ions / water O) stay visible.
            Self::Stick => Some(0.30),
            // Point is a flat disc; Wireframe draws only its line bonds (no atom
            // marker); Cartoon/Hidden draw no atom here.
            Self::Wireframe | Self::Point | Self::Cartoon | Self::Hidden => None,
        }
    }

    /// Whether visible atoms in this style are drawn as a flat point disc. Only
    /// `Point` (Dots) draws a disc; `Wireframe` shows bonds as lines with no
    /// per-atom marker.
    pub fn draws_point(self) -> bool {
        matches!(self, Self::Point)
    }

    /// True for styles whose per-atom geometry is heavy enough that very large
    /// selections must be downgraded to points to stay within the GPU buffer.
    pub fn is_heavy(self) -> bool {
        self.sphere_radius_scale().is_some()
    }

    /// Whether bonds touching an atom of this style are drawn as solid
    /// cylinders.
    pub fn draws_stick_bonds(self) -> bool {
        matches!(self, Self::Stick | Self::BallAndStick)
    }

    /// Whether bonds touching an atom of this style are drawn as thin lines.
    pub fn draws_line_bonds(self) -> bool {
        matches!(self, Self::Wireframe)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct OptimizationPrompt {
    pub cell: crate::engines::forcefield::CellOptimizationOptions,
    pub coordinate_scope: CoordinateOptimizationScope,
    pub allow_cell_optimization: bool,
}

impl OptimizationPrompt {
    pub fn new(allow_cell_optimization: bool, selection: &AtomSelection) -> Self {
        Self {
            cell: if allow_cell_optimization {
                crate::engines::forcefield::CellOptimizationOptions::lengths_only()
            } else {
                crate::engines::forcefield::CellOptimizationOptions::default()
            },
            coordinate_scope: if selection.is_empty() {
                CoordinateOptimizationScope::AllAtoms
            } else {
                CoordinateOptimizationScope::SelectedAtoms
            },
            allow_cell_optimization,
        }
    }

    pub fn options(
        &self,
        selection: &AtomSelection,
    ) -> crate::engines::forcefield::OptimizationOptions {
        crate::engines::forcefield::OptimizationOptions {
            atoms: match self.coordinate_scope {
                CoordinateOptimizationScope::AllAtoms => {
                    crate::engines::forcefield::AtomOptimizationScope::All
                }
                CoordinateOptimizationScope::SelectedAtoms => {
                    crate::engines::forcefield::AtomOptimizationScope::Selected(
                        selection.ordered_indices(),
                    )
                }
            },
            cell: if self.allow_cell_optimization {
                self.cell
            } else {
                crate::engines::forcefield::CellOptimizationOptions::default()
            },
            ..crate::engines::forcefield::OptimizationOptions::default()
        }
    }
}

/// User-editable configuration for a quantum-chemistry (chemx) calculation.
#[derive(Debug, Clone)]
pub struct QmPrompt {
    pub method: crate::engines::qm::QmMethod,
    /// Free-text functional name backing the "custom functional" field. When the
    /// method dropdown selects "Custom functional…", the panel reads this into
    /// [`crate::engines::qm::QmMethod::Dft`].
    pub custom_functional: String,
    pub basis: String,
    pub charge: i32,
    pub multiplicity: u32,
    pub kind: crate::engines::qm::QmKind,
    /// The calculation type the task opened with. `kind` is user-editable in the
    /// panel; this stays fixed so re-opening the panel (e.g. on an entry switch)
    /// doesn't clobber the user's choice, while switching to a different QM task
    /// re-defaults the panel.
    pub default_kind: crate::engines::qm::QmKind,
    /// All advanced chemx options (dispersion, solvation, SCF backend, …).
    pub options: crate::engines::qm::QmOptions,
}

impl QmPrompt {
    pub fn new(kind: crate::engines::qm::QmKind) -> Self {
        Self {
            // Default to r2scan-3c: a robust, batteries-included production
            // composite (functional + basis + dispersion + corrections).
            method: crate::engines::qm::QmMethod::Composite("r2scan-3c".to_string()),
            custom_functional: String::new(),
            basis: "def2-svp".to_string(),
            charge: 0,
            multiplicity: 1,
            kind,
            default_kind: kind,
            options: crate::engines::qm::QmOptions::default(),
        }
    }

    /// Build the engine request from this form against `structure`.
    pub fn to_request(&self, structure: crate::domain::Structure) -> crate::engines::qm::QmRequest {
        crate::engines::qm::QmRequest {
            structure,
            method: self.method.clone(),
            basis: self.basis.clone(),
            charge: self.charge,
            multiplicity: self.multiplicity,
            kind: self.kind,
            options: self.options.clone(),
        }
    }
}

impl Default for QmPrompt {
    fn default() -> Self {
        Self::new(crate::engines::qm::QmKind::SinglePoint)
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SupercellPrompt {
    pub repeats: [u32; 3],
}

/// User-editable configuration for the Protein Preparation task. This round
/// exposes only hydrogen completion; the other fields are placeholders for
/// future steps (protonation states, terminus patching, missing-atom repair)
/// and are not yet wired.
#[derive(Debug, Clone, Copy)]
pub struct ProteinPrepPrompt {
    /// Add missing hydrogens with chemistry heuristics.
    pub add_hydrogens: bool,
}

impl Default for ProteinPrepPrompt {
    fn default() -> Self {
        Self {
            add_hydrogens: true,
        }
    }
}

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

/// Editable draft for one engine's launch override in the Settings panel.
/// `command_prefix` is held as a single whitespace-separated line for easy
/// editing (e.g. `wsl.exe -e`); it is split on apply.
#[derive(Debug, Clone, Default)]
pub struct EngineDraft {
    pub command_prefix: String,
    pub program: String,
}

impl EngineDraft {
    pub fn from_launch(launch: &crate::engines::registry::EngineLaunch) -> Self {
        Self {
            command_prefix: launch.command_prefix.join(" "),
            program: launch.program.clone(),
        }
    }

    /// Build an [`EngineLaunch`] from the draft, or `None` if no program is
    /// set (which the dispatcher treats as "clear this override").
    pub fn to_launch(&self) -> Option<crate::engines::registry::EngineLaunch> {
        let program = self.program.trim();
        if program.is_empty() {
            return None;
        }
        Some(crate::engines::registry::EngineLaunch {
            command_prefix: self
                .command_prefix
                .split_whitespace()
                .map(str::to_string)
                .collect(),
            program: program.to_string(),
        })
    }
}

/// Editable draft for one remote host in the Settings panel. All fields are held
/// as text for direct editing and parsed/validated on save (`port`, `prelude`,
/// and `gmx_program` in particular). Mirrors [`EngineDraft`].
#[derive(Debug, Clone, Default)]
pub struct RemoteHostDraft {
    pub label: String,
    pub hostname: String,
    pub username: String,
    pub port: String,
    pub work_root: String,
    /// One shell setup line per text row (`module load gromacs`, `source GMXRC`).
    pub prelude: String,
    /// Remote path to `gmx` (or a bare name resolved via the prelude/PATH).
    pub gmx_program: String,
}

impl RemoteHostDraft {
    pub fn from_host(host: &crate::backend::config::RemoteHost) -> Self {
        let gmx_program = host
            .engines
            .get(crate::engines::registry::EngineId::GROMACS.as_str())
            .map(|launch| launch.program.clone())
            .unwrap_or_default();
        Self {
            label: host.label.clone(),
            hostname: host.hostname.clone(),
            username: host.username.clone(),
            port: host.port.to_string(),
            work_root: host.work_root.clone(),
            prelude: host.prelude.join("\n"),
            gmx_program,
        }
    }
}

/// Connection status of a remote host, shown as an indicator in the panel.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum RemoteHostStatus {
    /// Not yet probed.
    #[default]
    Unknown,
    /// A probe (passwordless check / detect) is in flight.
    Checking,
    /// Passwordless login works.
    Ready,
    /// Reachable, but passwordless login is not set up yet.
    NeedsSetup,
    /// The probe failed (unreachable / auth error). Carries a short reason.
    Unreachable(String),
}

/// State backing the Settings primary view. The engine registry is probed
/// lazily (probing spawns `--version` subprocesses) and cached here.
#[derive(Debug, Clone, Default)]
pub struct SettingsState {
    pub engine_registry: Option<crate::engines::registry::EngineRegistry>,
    pub engine_drafts: BTreeMap<String, EngineDraft>,
    /// When engine `--version` strings were last detected. Detection is slow
    /// (a WSL launch cold-starts the VM), so it runs only on explicit user
    /// request; the panel shows this so the displayed versions can be judged.
    pub engine_versions_checked_at: Option<std::time::SystemTime>,
    /// Free-text filter for the settings panel sections.
    pub search_query: String,
    /// Category selected in the Settings modal's left rail. Drives which
    /// category's groups the right pane shows while the search box is empty.
    pub selected_category: crate::frontend::ui::settings_registry::SettingCategory,
    /// Per-host editable drafts (keyed by host id) for the Remote Hosts panel.
    pub remote_host_drafts: BTreeMap<String, RemoteHostDraft>,
    /// The "add a host" form draft.
    pub new_remote_host: RemoteHostDraft,
    /// Per-host connection status (keyed by host id).
    pub remote_status: BTreeMap<String, RemoteHostStatus>,
    /// When a host's passwordless setup is being shown: `(host_id, install_cmd)`.
    pub remote_bootstrap: Option<(String, String)>,
}

/// State backing the Style primary view — the per-structure view and
/// representation properties relocated out of Settings. These belong to the
/// structure currently being viewed, not to global app preferences.
#[derive(Debug, Clone, Default)]
pub struct StyleState {
    /// Free-text filter for the Style panel sections.
    pub search_query: String,
}

pub struct UiState {
    pub layout: LayoutState,
    pub entry_list: EntryListState,
    pub settings: SettingsState,
    pub style: StyleState,
    pub camera: ViewCamera,
    pub viewport_cache: ViewportCache,
    /// Set once at startup when the GPU molecule renderer initializes
    /// successfully; gates the GPU rendering path in the viewport.
    pub gpu_ready: bool,
    /// Resolved once per frame: whether the frosted-glass material should be
    /// revealed (user enabled it, the platform supports it, and Reduce
    /// Transparency is off). Drives the transparent clear color and the
    /// semi-transparent chrome fills. See [`crate::frontend::glass`].
    pub glass_active: bool,
    /// Effective chrome-fill alpha while glass is revealed this frame, mapped
    /// from the persisted `glass_intensity`; `None` means opaque chrome (glass
    /// off, unsupported, or Reduce Transparency on). Resolved next to
    /// `glass_active` and passed to [`crate::frontend::theme::chrome_fill`].
    pub glass_alpha: Option<u8>,
    pub hovered_atom: Option<usize>,
    pub selection: AtomSelection,
    pub viewport: ViewportVisualState,
    pub project_viewport: ViewportVisualState,
    pub entry_viewports: BTreeMap<u64, ViewportVisualState>,
    pub scripted_viewport_size: [u32; 2],
    pub console: CommandConsoleState,
    pub editor: Option<StructureEditor>,
    pub sketcher: Option<crate::frontend::sketcher::SketcherState>,
    pub reticular_builder: Option<ReticularBuilderPanel>,
    pub nanosheet_builder: Option<NanosheetBuilderPanel>,
    pub block_editor: Option<BuildingBlockEditor>,
    pub pending_optimization: Option<OptimizationPrompt>,
    pub pending_qm: Option<QmPrompt>,
    pub pending_supercell: Option<SupercellPrompt>,
    pub pending_protein_prep: Option<ProteinPrepPrompt>,
    pub pending_md_system: Option<MdSystemPrompt>,
    pub pending_md_run: Option<MdRunPrompt>,
    pub pending_pdb_fetch: Option<String>,
    /// Cached solvation count preview for the System Builder panel. Recomputed
    /// (which opens the force-field DB and grid-fills the box) only when
    /// `md_solvation_preview_key` changes, so the panel stays responsive.
    pub md_solvation_preview:
        Option<Result<crate::workflows::molecular_dynamics::SolvationEstimate, String>>,
    pub md_solvation_preview_key: u64,
    /// Active trajectory playback (loaded from an MD-output entry's run
    /// directory), or `None` when nothing is playing.
    pub trajectory: Option<crate::frontend::trajectory::TrajectoryPlayback>,
    /// A newer published release found by the background update check; renders
    /// a link to the release page in the status bar.
    pub available_update: Option<crate::io::update_check::AvailableUpdate>,
    /// An open plain-text viewer window, or `None` when closed. General
    /// purpose: any tool with textual output (QM reports, future engines)
    /// shows it here rather than adding its own window.
    pub text_viewer: Option<TextViewer>,
    /// Progress of a one-click in-place self-update (the download/replace
    /// triggered from the update badge), distinct from `available_update`
    /// which only records that a newer release *exists*.
    pub self_update: SelfUpdateStatus,
}

/// Lifecycle of a user-initiated in-place update: idle until the user clicks
/// "update", downloading while the worker replaces the executable, then either
/// installed (offer a restart) or failed (show the error, leave the releases
/// link as a fallback).
#[derive(Default, Clone)]
pub enum SelfUpdateStatus {
    #[default]
    Idle,
    Downloading,
    Installed {
        version: String,
    },
    Failed {
        error: String,
    },
}

/// A read-only plain-text document shown in the shared viewer window: a
/// window title and the text to display (monospace, scrollable).
pub struct TextViewer {
    pub title: String,
    pub text: String,
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            layout: LayoutState::default(),
            entry_list: EntryListState::default(),
            settings: SettingsState::default(),
            style: StyleState::default(),
            camera: ViewCamera::default(),
            viewport_cache: ViewportCache::default(),
            gpu_ready: false,
            glass_active: false,
            glass_alpha: None,
            hovered_atom: None,
            selection: AtomSelection::default(),
            viewport: ViewportVisualState::default(),
            project_viewport: ViewportVisualState::default(),
            entry_viewports: BTreeMap::new(),
            scripted_viewport_size: [1180, 760],
            console: CommandConsoleState::default(),
            editor: None,
            sketcher: None,
            reticular_builder: None,
            nanosheet_builder: None,
            block_editor: None,
            pending_optimization: None,
            pending_qm: None,
            pending_supercell: None,
            pending_protein_prep: None,
            pending_md_system: None,
            pending_md_run: None,
            pending_pdb_fetch: None,
            md_solvation_preview: None,
            md_solvation_preview_key: 0,
            trajectory: None,
            available_update: None,
            text_viewer: None,
            self_update: SelfUpdateStatus::default(),
        }
    }
}

pub struct AppState {
    pub workspace: WorkspaceSession,
    pub config: AppConfig,
    pub recent_projects: Vec<RecentProject>,
    pub entries: EntryStore,
    pub history: History,
    pub tasks: TaskManager,
    pub jobs: JobManager,
    pub ui: UiState,
    pub message: String,
    pub output_log: Vec<String>,
    pub active_task_run: Option<u64>,
    pub edit_origin: Option<EditSnapshot>,
    pub builder_origin: Option<EditSnapshot>,
    pub optimization_origin: Option<EditSnapshot>,
    workspace_structure: Structure,
    workspace_save_path: PathBuf,
    last_logged_message: String,
    /// egui time (seconds) at which a coalesced autosave should flush, or `None`
    /// when no project change is pending. Set by the dispatcher after a
    /// persist-worthy action and drained on the UI thread once the debounce
    /// window elapses, so rapid interactions don't each pay a full project save.
    autosave_deadline: Option<f64>,
}

impl AppState {
    pub fn new(
        structure: Structure,
        source_path: Option<PathBuf>,
        workspace: WorkspaceSession,
        config: AppConfig,
        recent_projects: Vec<RecentProject>,
        project_snapshot: Option<ProjectSnapshot>,
    ) -> Self {
        let save_path =
            structure_io::default_structure_save_path(&structure, source_path.as_deref());
        let has_initial_entry = source_path.is_some()
            || !structure.atoms.is_empty()
            || !structure.bonds.is_empty()
            || structure.cell.is_some()
            || {
                let trimmed_title = structure.title.trim();
                !trimmed_title.is_empty() && trimmed_title != "Untitled"
            };
        let message = "Ready to open or edit a structure".to_string();
        let entries = if let Some(snapshot) = project_snapshot.as_ref() {
            snapshot.entries.clone()
        } else if has_initial_entry {
            EntryStore::with_initial(structure.clone(), source_path, save_path.clone())
        } else {
            EntryStore::new_empty()
        };
        let tasks = project_snapshot
            .as_ref()
            .map(|snapshot| snapshot.tasks.clone())
            .unwrap_or_default();
        let mut state = Self {
            workspace,
            config,
            recent_projects,
            entries,
            history: History::default(),
            tasks,
            jobs: JobManager::default(),
            ui: UiState::default(),
            message: message.clone(),
            output_log: vec![message.clone()],
            active_task_run: None,
            edit_origin: None,
            builder_origin: None,
            optimization_origin: None,
            workspace_structure: structure,
            workspace_save_path: save_path,
            last_logged_message: message,
            autosave_deadline: None,
        };
        if let Some(snapshot) = project_snapshot.as_ref() {
            state.ui.project_viewport = snapshot.view.viewport.clone();
            state.ui.viewport = state.ui.project_viewport.clone();
            state.ui.entry_viewports = snapshot.view.entry_viewports.clone();
            if let Some(entry_id) = state.entries.active_entry_id() {
                state
                    .ui
                    .entry_viewports
                    .entry(entry_id)
                    .or_insert_with(|| state.ui.project_viewport.clone());
            }
            state.history = snapshot.history.clone();
            state
                .history
                .set_active_entry(state.entries.active_entry_id());
        }
        state.load_viewport_for_active_entry();
        state.ui.entry_list.selected_entry_ids.clear();
        if let Some(id) = state.entries.active_entry_id() {
            state.ui.entry_list.selected_entry_ids.insert(id);
        }
        state
            .history
            .set_active_entry(state.entries.active_entry_id());
        state
    }

    pub fn scratch(config: AppConfig, recent_projects: Vec<RecentProject>) -> Self {
        Self::new(
            Structure::empty(),
            None,
            WorkspaceSession::Scratch,
            config,
            recent_projects,
            None,
        )
    }

    pub fn has_active_entry(&self) -> bool {
        self.entries.active_entry().is_some()
    }

    pub fn structure(&self) -> &Structure {
        self.entries
            .active_entry()
            .map(|entry| &entry.structure)
            .unwrap_or(&self.workspace_structure)
    }

    pub fn structure_mut(&mut self) -> &mut Structure {
        if let Some(entry) = self.entries.active_entry_mut() {
            &mut entry.structure
        } else {
            &mut self.workspace_structure
        }
    }

    /// Make `entry_id` the active, loaded, and selected entry, persisting the
    /// previously active entry's viewport first. Used by console commands that
    /// add a new entry (an imported structure, a QM-optimized geometry, …).
    pub fn show_entry(&mut self, entry_id: u64) {
        self.save_viewport_for_active_entry();
        self.entries.activate_entry(entry_id);
        self.ensure_entry_loaded(entry_id);
        self.history.set_active_entry(Some(entry_id));
        self.ui.entry_list.selected_entry_ids.clear();
        self.ui.entry_list.selected_entry_ids.insert(entry_id);
        self.load_viewport_for_active_entry();
    }

    pub fn mark_structure_changed(&mut self) {
        self.entries.bump_active_revision();
        self.ui.hovered_atom = None;
        self.ui.viewport_cache.clear();
        let atom_count = self.structure().atoms.len();
        self.ui.viewport.retain_atom_styles(atom_count);
    }

    pub fn runs_dir(&self) -> std::path::PathBuf {
        self.workspace
            .project()
            .map(|project| project.root.join("runs"))
            .unwrap_or_else(|| std::env::temp_dir().join("silicolab").join("runs"))
    }

    pub fn apply_render_defaults_for_active_entry(&mut self) {
        let structure = self.structure().clone();
        apply_entry_render_defaults(
            &mut self.ui.viewport,
            &structure,
            &self.config.representation,
        );
    }

    pub fn save_viewport_for_active_entry(&mut self) {
        let Some(entry_id) = self.entries.active_entry_id() else {
            return;
        };
        self.ui
            .entry_viewports
            .insert(entry_id, self.ui.viewport.clone());
    }

    pub fn load_viewport_for_active_entry(&mut self) {
        let Some(entry_id) = self.entries.active_entry_id() else {
            self.ui.viewport = ViewportVisualState::default();
            return;
        };
        if let Some(viewport) = self.ui.entry_viewports.get(&entry_id).cloned() {
            self.ui.viewport = viewport;
            // Migrate entries saved before the bulk-solvent wireframe default: if
            // no per-atom style was ever stored for this entry, apply the default
            // now. A non-empty map means the user (or a newer build) already
            // configured atoms, so we leave their choices untouched.
            if self.ui.viewport.atom_styles.is_empty() {
                let structure = self.structure().clone();
                apply_solvent_render_default(&mut self.ui.viewport, &structure);
            }
        } else {
            self.ui.viewport = self.ui.project_viewport.clone();
            self.apply_render_defaults_for_active_entry();
        }
        // Category styles are project-level: every entry shows the project's
        // current category defaults, regardless of what was stored per entry.
        self.ui.viewport.category_styles = self.ui.project_viewport.category_styles.clone();
    }

    pub fn project_view_settings(&self) -> crate::backend::storage::ProjectViewSettings {
        let mut entry_viewports = self.ui.entry_viewports.clone();
        if let Some(entry_id) = self.entries.active_entry_id() {
            entry_viewports.insert(entry_id, self.ui.viewport.clone());
        }
        crate::backend::storage::ProjectViewSettings {
            viewport: self.ui.project_viewport.clone(),
            entry_viewports,
        }
    }

    pub fn save_path(&self) -> &PathBuf {
        self.entries
            .active_entry()
            .map(|entry| &entry.save_path)
            .unwrap_or(&self.workspace_save_path)
    }

    pub fn set_source_path(&mut self, source_path: Option<PathBuf>) {
        if let Some(entry) = self.entries.active_entry_mut() {
            entry.source_path = source_path;
        }
    }

    pub fn set_save_path(&mut self, save_path: PathBuf) {
        if let Some(entry) = self.entries.active_entry_mut() {
            entry.save_path = save_path;
        } else {
            self.workspace_save_path = save_path;
        }
    }

    pub fn current_entry_label(&self) -> String {
        self.entries
            .active_entry()
            .map(|entry| entry.name.clone())
            .unwrap_or_else(|| self.workspace.label())
    }

    pub fn workspace_label(&self) -> String {
        self.workspace.label()
    }

    /// Directory where downloaded structures (e.g. fetched PDB files) are kept.
    /// Anchored at the project root when a project is open, otherwise relative
    /// to the current working directory.
    pub fn structures_dir(&self) -> std::path::PathBuf {
        let subdir = crate::io::pdb_fetch::DOWNLOAD_SUBDIR;
        match self.workspace.project() {
            Some(project) => project.root.join(subdir),
            None => std::path::PathBuf::from(subdir),
        }
    }

    /// A cheap hash of the persisted entry/group state — entry set, per-entry
    /// revision (bumped on every edit), names, and grouping. Deliberately
    /// excludes transient/view state (active tab, selection, camera, render
    /// styles): the autosave policy only saves when entries are added, removed,
    /// or edited, leaving view-only changes to be persisted at exit. Touches no
    /// geometry, so it is fast even for entry-heavy projects.
    pub fn entries_fingerprint(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.entries.records.len().hash(&mut hasher);
        for record in &self.entries.records {
            record.id.hash(&mut hasher);
            record.revision.hash(&mut hasher);
            record.name.hash(&mut hasher);
            record.group_id.hash(&mut hasher);
            // Provenance (e.g. an entry becoming an MD-run output) is persisted,
            // so a change to it must trigger an autosave too.
            record.origin.kind_token().hash(&mut hasher);
            record.origin.trajectory().hash(&mut hasher);
        }
        for group in &self.entries.groups {
            group.id.hash(&mut hasher);
            group.name.hash(&mut hasher);
        }
        hasher.finish()
    }

    /// Schedule a coalesced autosave to flush `delay_seconds` after `now_seconds`
    /// (both egui clock seconds). Repeated calls push the deadline back so a burst
    /// of actions collapses into a single save once the user pauses.
    pub fn request_autosave(&mut self, now_seconds: f64, delay_seconds: f64) {
        self.autosave_deadline = Some(now_seconds + delay_seconds);
    }

    pub fn autosave_deadline(&self) -> Option<f64> {
        self.autosave_deadline
    }

    pub fn clear_autosave_deadline(&mut self) {
        self.autosave_deadline = None;
    }

    pub fn project_snapshot(&self) -> Option<ProjectSnapshot> {
        let project = self.workspace.project()?;
        Some(ProjectSnapshot {
            name: project.name.clone(),
            entries: self.entries.clone(),
            tasks: self.tasks.clone(),
            view: self.project_view_settings(),
            history: self.history.clone(),
        })
    }

    /// Materialize an entry's geometry if it was lazily left unloaded when the
    /// project was opened. No-op for already-loaded entries and scratch sessions.
    pub fn ensure_entry_loaded(&mut self, entry_id: u64) {
        let Some(project) = self.workspace.project().cloned() else {
            return;
        };
        let Some(entry) = self.entries.entry(entry_id) else {
            return;
        };
        if entry.loaded {
            return;
        }
        let compound_id = entry.compound_id.unwrap_or(entry.id as i64);
        match crate::backend::storage::load_structure_for_compound(
            &project.compounds_db,
            compound_id,
        ) {
            Ok(structure) => {
                if let Some(entry) = self.entries.entry_mut(entry_id) {
                    entry.structure = structure;
                    entry.loaded = true;
                }
            }
            Err(error) => self.set_message(format!("Failed to load entry #{entry_id}: {error}")),
        }
    }

    pub fn capture_edit_snapshot(&self) -> EditSnapshot {
        let entry = self
            .entries
            .active_entry()
            .expect("active entry must exist");
        EditSnapshot {
            structure: entry.structure.clone(),
            source_path: entry.source_path.clone(),
            save_path: entry.save_path.clone(),
            selection: self.ui.selection.clone(),
        }
    }

    pub fn restore_edit_snapshot(&mut self, snapshot: EditSnapshot) {
        self.cancel_transient_jobs();
        self.ui.pending_optimization = None;
        self.ui.pending_qm = None;
        self.ui.pending_supercell = None;
        self.ui.pending_md_system = None;
        self.ui.pending_md_run = None;
        self.ui.editor = None;
        self.ui.reticular_builder = None;
        self.ui.nanosheet_builder = None;
        self.ui.block_editor = None;
        self.edit_origin = None;
        self.builder_origin = None;
        self.optimization_origin = None;
        self.ui.hovered_atom = None;

        if let Some(entry) = self.entries.active_entry_mut() {
            entry.structure = snapshot.structure;
            entry.source_path = snapshot.source_path;
            entry.save_path = snapshot.save_path;
        }
        self.mark_structure_changed();
        self.ui.selection = snapshot.selection;
        self.ui.selection.retain_valid(self.structure().atoms.len());
    }

    /// Forget every entry's undo/redo history (e.g. when closing a project).
    pub fn clear_history(&mut self) {
        self.history.clear();
        self.reset_edit_origins();
    }

    fn reset_edit_origins(&mut self) {
        self.edit_origin = None;
        self.builder_origin = None;
        self.optimization_origin = None;
    }

    /// Point the (per-entry) undo/redo history at the currently active entry
    /// without discarding any entry's stacks. Each entry keeps its own history,
    /// so switching between entries — or reopening a project — preserves undo.
    pub fn sync_history_active_entry(&mut self) {
        let active = self.entries.active_entry_id();
        self.history.set_active_entry(active);
        self.reset_edit_origins();
    }

    pub fn history_navigation_enabled(&self) -> bool {
        self.ui.editor.is_none()
            && self.ui.reticular_builder.is_none()
            && self.ui.nanosheet_builder.is_none()
            && self.ui.block_editor.is_none()
            && self.ui.pending_optimization.is_none()
            && self.ui.pending_md_system.is_none()
            && self.ui.pending_md_run.is_none()
            && !self.jobs.optimization_running()
            && !self.jobs.engine_running()
    }

    pub fn can_undo(&self) -> bool {
        self.history_navigation_enabled() && self.history.can_undo()
    }

    pub fn can_redo(&self) -> bool {
        self.history_navigation_enabled() && self.history.can_redo()
    }

    pub fn set_message(&mut self, message: impl Into<String>) {
        self.message = message.into();
        self.record_message_change();
    }

    pub fn record_message_change(&mut self) {
        if self.message == self.last_logged_message {
            return;
        }
        self.output_log.push(self.message.clone());
        self.last_logged_message = self.message.clone();
        if self.output_log.len() > 400 {
            let excess = self.output_log.len() - 400;
            self.output_log.drain(0..excess);
        }
    }

    pub fn cancel_transient_jobs(&mut self) {
        self.jobs.cancel_optimization();
        self.jobs.cancel_qm();
        self.jobs.cancel_engine();
    }

    pub fn reset_layout_keep_view(&mut self) {
        let active_view = self.ui.layout.active_primary_view;
        self.ui.layout = LayoutState::default();
        self.ui.layout.active_primary_view = active_view;
    }
}

#[cfg(test)]
mod tests {
    use super::{AppState, MdRunPrompt, MdStageEdit};
    use crate::workflows::molecular_dynamics::{MdStage, StageLength};

    #[test]
    fn empty_startup_does_not_create_initial_entry() {
        let state = AppState::scratch(Default::default(), Vec::new());

        assert!(!state.has_active_entry());
        assert_eq!(state.entries.records.len(), 0);
        assert_eq!(state.entries.tabs.len(), 0);
        assert_eq!(state.current_entry_label(), "Scratch");
    }

    fn prompt_with_one_produce_stage() -> MdRunPrompt {
        MdRunPrompt {
            stages: vec![MdStage::produce(300.0)],
            ..Default::default()
        }
    }

    #[test]
    fn edit_stage_sets_and_reverts_tiered_parameter() {
        let mut prompt = prompt_with_one_produce_stage();
        // Setting and clearing an Advanced-tier parameter round-trips through the
        // Option model (set -> Some, revert -> None).
        prompt.edit_stage(0, MdStageEdit::PmeOrder(Some(6)));
        assert_eq!(prompt.stages[0].params.pme_order, Some(6));
        prompt.edit_stage(0, MdStageEdit::PmeOrder(None));
        assert_eq!(prompt.stages[0].params.pme_order, None);
    }

    #[test]
    fn edit_stage_inline_fields_mutate_in_place() {
        let mut prompt = prompt_with_one_produce_stage();
        prompt.edit_stage(0, MdStageEdit::Temperature(287.0));
        prompt.edit_stage(0, MdStageEdit::Length(StageLength::Steps(1234)));
        prompt.edit_stage(0, MdStageEdit::PressureBar(1.5));
        assert_eq!(prompt.stages[0].temperature_k, 287.0);
        assert_eq!(prompt.stages[0].length, StageLength::Steps(1234));
        assert_eq!(prompt.stages[0].pressure.unwrap().ref_bar, 1.5);
    }

    #[test]
    fn edit_stage_raw_lines_add_set_and_remove() {
        let mut prompt = prompt_with_one_produce_stage();
        prompt.edit_stage(0, MdStageEdit::AddRawLine);
        assert_eq!(prompt.stages[0].raw_passthrough.len(), 1);
        prompt.edit_stage(
            0,
            MdStageEdit::SetRawLine {
                line: 0,
                key: "nstcomm".to_string(),
                value: "50".to_string(),
            },
        );
        assert_eq!(
            prompt.stages[0].raw_passthrough[0],
            ("nstcomm".to_string(), "50".to_string())
        );
        prompt.edit_stage(0, MdStageEdit::RemoveRawLine(0));
        assert!(prompt.stages[0].raw_passthrough.is_empty());
    }

    #[test]
    fn edit_stage_ignores_out_of_range_index() {
        let mut prompt = prompt_with_one_produce_stage();
        // Must not panic on a stale index (e.g. a removed stage).
        prompt.edit_stage(9, MdStageEdit::Temperature(123.0));
        assert_eq!(prompt.stages[0].temperature_k, 300.0);
    }

    #[test]
    fn toggle_stage_expanded_opens_one_at_a_time() {
        let mut prompt = prompt_with_one_produce_stage();
        assert_eq!(prompt.expanded_stage, None);
        prompt.toggle_stage_expanded(0);
        assert_eq!(prompt.expanded_stage, Some(0));
        prompt.toggle_stage_expanded(0);
        assert_eq!(prompt.expanded_stage, None);
    }

    #[test]
    fn inline_and_detail_edits_reach_the_realized_mdp() {
        use crate::engines::gromacs::input::render_mdp;
        use crate::engines::gromacs::stage_specs_from_md_stages;
        use crate::workflows::molecular_dynamics::ForceFieldFamily;

        // The merge of an inline (temperature) and a detail (PME order) edit must
        // resolve into the realized stage exactly as the run will see it.
        let mut prompt = prompt_with_one_produce_stage();
        prompt.edit_stage(0, MdStageEdit::Temperature(310.0));
        prompt.edit_stage(0, MdStageEdit::PmeOrder(Some(6)));

        let specs = stage_specs_from_md_stages(&prompt.stages, ForceFieldFamily::Amber, None);
        let mdp = render_mdp(&specs[0].settings);
        assert!(
            mdp.lines()
                .any(|line| line.starts_with("ref-t") && line.trim_end().ends_with("= 310")),
            "edited temperature should reach ref-t:\n{mdp}"
        );
        assert!(
            mdp.lines()
                .any(|line| line.starts_with("pme-order") && line.trim_end().ends_with("= 6")),
            "edited PME order should reach the mdp:\n{mdp}"
        );
    }
}
