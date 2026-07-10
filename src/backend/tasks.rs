use std::{collections::HashSet, path::PathBuf, time::SystemTime};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskKind {
    BuildReticularStructure,
    BuildNanosheet,
    CreateBuildingBlock,
    OptimizeGeometry,
    OptimizeCrystalGeometry,
    RunQmEnergy,
    RunQmOptimize,
    RunQmFrequencies,
    RunQmTransitionState,
    TranslateIntoFirstUnitCell,
    ExpandSupercell,
    PrepareProtein,
    BuildMdSystem,
    BuildDisorderedSystem,
    AddHydrogens,
    RecomputeBonds,
    RunMd,
    RunDocking,
    ModifyProteinPtm,
}

impl TaskKind {
    /// Whether this task runs the QM engine, and so writes an `output.txt`
    /// report (and, when it has a trace to plot, a `series.json`) into its run
    /// directory.
    pub fn is_qm(self) -> bool {
        matches!(
            self,
            Self::RunQmEnergy
                | Self::RunQmOptimize
                | Self::RunQmFrequencies
                | Self::RunQmTransitionState
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskPanelKind {
    None,
    ReticularBuilder,
    NanosheetBuilder,
    BuildingBlockEditor,
    OptimizationPrompt,
    QmPrompt,
    SupercellPrompt,
    ProteinPrepPrompt,
    MdSystemPrompt,
    DisorderedSystemPrompt,
    MdRunPrompt,
    DockingPrompt,
    PtmPrompt,
}

#[derive(Debug, Clone, Copy)]
pub struct TaskController {
    pub id: &'static str,
    pub title: &'static str,
    pub short_title: &'static str,
    pub theme: &'static str,
    pub method: &'static str,
    pub application: &'static str,
    pub description: &'static str,
    pub kind: TaskKind,
    pub panel: TaskPanelKind,
    pub outcome: TaskOutcome,
    pub backend: TaskBackend,
    pub uses_run_directory: bool,
}

impl TaskController {
    pub fn requires_panel(self) -> bool {
        self.panel != TaskPanelKind::None
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskOutcome {
    EditInPlace,
    CreateEntry,
    FileOnly,
    /// A read-only analysis that produces a report, not a structure change
    /// (e.g. a single-point energy or frequency calculation).
    Report,
}

impl TaskOutcome {
    pub fn label(self) -> &'static str {
        match self {
            Self::EditInPlace => "edit-in-place",
            Self::CreateEntry => "create-entry",
            Self::FileOnly => "file-only",
            Self::Report => "report",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskBackend {
    InlineNative,
    BackgroundNative,
    ExternalEngine,
}

impl TaskBackend {
    pub fn label(self) -> &'static str {
        match self {
            Self::InlineNative => "inline-native",
            Self::BackgroundNative => "background-native",
            Self::ExternalEngine => "external-engine",
        }
    }
}

const TASK_CONTROLLERS: &[TaskController] = &[
    TaskController {
        id: "build-reticular",
        title: "Reticular Structure Builder",
        short_title: "Reticular Builder",
        theme: "Reticular Design",
        method: "Template Assembly",
        application: "Framework Generation",
        description: "Compose cores, linkers, and substituents into a periodic reticular structure.",
        kind: TaskKind::BuildReticularStructure,
        panel: TaskPanelKind::ReticularBuilder,
        outcome: TaskOutcome::CreateEntry,
        backend: TaskBackend::InlineNative,
        uses_run_directory: false,
    },
    TaskController {
        id: "build-nanosheet",
        title: "Nanosheet Builder",
        short_title: "Nanosheet Builder",
        theme: "2D Materials",
        method: "Lattice Generation",
        application: "Sheet Generation",
        description: "Generate periodic 2D materials (honeycomb sheets, transition-metal dichalcogenides, graphitic carbon nitrides) from parametrized lattice families.",
        kind: TaskKind::BuildNanosheet,
        panel: TaskPanelKind::NanosheetBuilder,
        outcome: TaskOutcome::CreateEntry,
        backend: TaskBackend::InlineNative,
        uses_run_directory: false,
    },
    TaskController {
        id: "building-block",
        title: "Building Block Authoring",
        short_title: "Building Block Editor",
        theme: "Reticular Design",
        method: "Fragment Authoring",
        application: "Component Library",
        description: "Extract and annotate a reusable building block from the current structure.",
        kind: TaskKind::CreateBuildingBlock,
        panel: TaskPanelKind::BuildingBlockEditor,
        outcome: TaskOutcome::FileOnly,
        backend: TaskBackend::InlineNative,
        uses_run_directory: false,
    },
    TaskController {
        id: "optimize-geometry",
        title: "Molecular Geometry Optimization",
        short_title: "Geometry Optimization",
        theme: "Geometry",
        method: "Forcefield Relaxation",
        application: "Local Structure Cleanup",
        description: "Relax the current molecular geometry with the forcefield optimizer.",
        kind: TaskKind::OptimizeGeometry,
        panel: TaskPanelKind::OptimizationPrompt,
        outcome: TaskOutcome::EditInPlace,
        backend: TaskBackend::BackgroundNative,
        uses_run_directory: false,
    },
    TaskController {
        id: "optimize-crystal",
        title: "Periodic Geometry Optimization",
        short_title: "Crystal Optimization",
        theme: "Geometry",
        method: "Cell-Constrained Relaxation",
        application: "Crystal Refinement",
        description: "Relax atomic coordinates and optional cell parameters for periodic structures.",
        kind: TaskKind::OptimizeCrystalGeometry,
        panel: TaskPanelKind::OptimizationPrompt,
        outcome: TaskOutcome::EditInPlace,
        backend: TaskBackend::BackgroundNative,
        uses_run_directory: false,
    },
    // The three QM tasks share the QmPrompt panel; each opens it with a
    // different default calculation type (the panel still lets you switch).
    TaskController {
        id: "qm-energy",
        title: "Single-Point Energy",
        short_title: "QM Energy",
        theme: "Electronic Structure",
        method: "HF / DFT / Post-HF",
        application: "Energy, Dipole, Charges",
        description: "Compute the energy (and optional dipole and atomic charges) at the current geometry with the hartree quantum-chemistry engine.",
        kind: TaskKind::RunQmEnergy,
        panel: TaskPanelKind::QmPrompt,
        outcome: TaskOutcome::Report,
        backend: TaskBackend::BackgroundNative,
        uses_run_directory: true,
    },
    TaskController {
        id: "qm-optimize",
        title: "Geometry Optimization (QM)",
        short_title: "QM Optimization",
        theme: "Electronic Structure",
        method: "HF / DFT Gradients",
        application: "Quantum Geometry Relaxation",
        description: "Relax the geometry on the quantum-chemical energy surface; the optimized structure is added as a new entry.",
        kind: TaskKind::RunQmOptimize,
        panel: TaskPanelKind::QmPrompt,
        outcome: TaskOutcome::CreateEntry,
        backend: TaskBackend::BackgroundNative,
        uses_run_directory: true,
    },
    TaskController {
        id: "qm-frequencies",
        title: "Vibrational Frequencies",
        short_title: "QM Frequencies",
        theme: "Electronic Structure",
        method: "Harmonic Hessian",
        application: "IR Modes, Thermochemistry",
        description: "Compute harmonic vibrational frequencies and thermochemistry at the current geometry with the hartree quantum-chemistry engine.",
        kind: TaskKind::RunQmFrequencies,
        panel: TaskPanelKind::QmPrompt,
        outcome: TaskOutcome::Report,
        backend: TaskBackend::BackgroundNative,
        uses_run_directory: true,
    },
    TaskController {
        id: "qm-transition-state",
        title: "Transition-State Search",
        short_title: "QM Transition State",
        theme: "Electronic Structure",
        method: "HF / DFT Saddle Search",
        application: "Reaction Barriers, Mechanisms",
        description: "Climb to a first-order saddle point (transition state) on the quantum-chemical \
                      energy surface — from the current geometry, between a reactant and a product, or \
                      along a driven coordinate. The saddle structure is added as a new entry.",
        kind: TaskKind::RunQmTransitionState,
        panel: TaskPanelKind::QmPrompt,
        outcome: TaskOutcome::CreateEntry,
        backend: TaskBackend::BackgroundNative,
        uses_run_directory: true,
    },
    TaskController {
        id: "translate-into-cell",
        title: "Wrap Atoms Into First Cell",
        short_title: "Wrap Into Cell",
        theme: "Crystal Editing",
        method: "Periodic Normalization",
        application: "Cell Cleanup",
        description: "Wrap the current periodic structure into the first unit cell while preserving bonds.",
        kind: TaskKind::TranslateIntoFirstUnitCell,
        panel: TaskPanelKind::None,
        outcome: TaskOutcome::EditInPlace,
        backend: TaskBackend::InlineNative,
        uses_run_directory: false,
    },
    TaskController {
        id: "expand-supercell",
        title: "Supercell Expansion",
        short_title: "Expand Supercell",
        theme: "Crystal Editing",
        method: "Cell Replication",
        application: "Periodic Model Scaling",
        description: "Replicate the active periodic structure along the lattice vectors.",
        kind: TaskKind::ExpandSupercell,
        panel: TaskPanelKind::SupercellPrompt,
        outcome: TaskOutcome::EditInPlace,
        backend: TaskBackend::InlineNative,
        uses_run_directory: false,
    },
    TaskController {
        id: "prepare-protein",
        title: "Protein Preparation",
        short_title: "Prep Protein",
        theme: "Molecular Dynamics",
        method: "Structure Cleanup",
        application: "MD Preparation",
        description: "Prepare a biomolecule for simulation (currently: add missing hydrogens). \
                      The prepared structure is added as a new entry.",
        kind: TaskKind::PrepareProtein,
        panel: TaskPanelKind::ProteinPrepPrompt,
        outcome: TaskOutcome::CreateEntry,
        backend: TaskBackend::InlineNative,
        uses_run_directory: false,
    },
    TaskController {
        id: "build-md-system",
        title: "MD System Builder",
        short_title: "Build MD System",
        theme: "Molecular Dynamics",
        method: "Vacuum Box Packing",
        application: "MD Preparation",
        description: "Pad a non-periodic structure with vacuum into an orthorhombic simulation box.",
        kind: TaskKind::BuildMdSystem,
        panel: TaskPanelKind::MdSystemPrompt,
        outcome: TaskOutcome::CreateEntry,
        backend: TaskBackend::InlineNative,
        uses_run_directory: true,
    },
    TaskController {
        id: "build-disordered-system",
        title: "Build Disordered System",
        short_title: "Disordered System",
        theme: "Molecular Dynamics",
        method: "Randomized Packing",
        application: "System Building",
        description: "Pack copies of one or more molecules into a box, sphere, or cylinder without \
                      clashes — liquids, mixtures, droplets, pores, or packing around a solute.",
        kind: TaskKind::BuildDisorderedSystem,
        panel: TaskPanelKind::DisorderedSystemPrompt,
        outcome: TaskOutcome::CreateEntry,
        backend: TaskBackend::BackgroundNative,
        uses_run_directory: false,
    },
    TaskController {
        id: "add-hydrogens",
        title: "Hydrogen Completion",
        short_title: "Add Hydrogens",
        theme: "Structure Editing",
        method: "Valence Completion",
        application: "Model Cleanup",
        description: "Add missing hydrogens to the current structure using chemistry heuristics.",
        kind: TaskKind::AddHydrogens,
        panel: TaskPanelKind::None,
        outcome: TaskOutcome::EditInPlace,
        backend: TaskBackend::InlineNative,
        uses_run_directory: false,
    },
    TaskController {
        id: "recompute-bonds",
        title: "Bond Topology Rebuild",
        short_title: "Rebuild Bonds",
        theme: "Structure Editing",
        method: "Geometric Inference",
        application: "Connectivity Repair",
        description: "Rebuild bond topology from atomic positions and periodic boundary conditions.",
        kind: TaskKind::RecomputeBonds,
        panel: TaskPanelKind::None,
        outcome: TaskOutcome::EditInPlace,
        backend: TaskBackend::InlineNative,
        uses_run_directory: false,
    },
    TaskController {
        id: "run-md",
        title: "Run MD",
        short_title: "Run MD",
        theme: "Molecular Dynamics",
        method: "Multi-step Engine Workflow",
        application: "Simulation",
        description: "Run a configurable multi-step MD workflow.",
        kind: TaskKind::RunMd,
        panel: TaskPanelKind::MdRunPrompt,
        outcome: TaskOutcome::CreateEntry,
        backend: TaskBackend::ExternalEngine,
        uses_run_directory: true,
    },
    TaskController {
        id: "dock-ligand",
        title: "Molecular Docking",
        short_title: "Dock Ligand",
        theme: "Molecular Docking",
        method: "AutoDock Vina Search",
        application: "Ligand-Receptor Binding",
        description: "Dock a ligand into a receptor within a search box and rank the binding poses by \
                      affinity, using the built-in pure-Rust Vina engine. Each pose is added as a new entry.",
        kind: TaskKind::RunDocking,
        panel: TaskPanelKind::DockingPrompt,
        outcome: TaskOutcome::CreateEntry,
        backend: TaskBackend::BackgroundNative,
        uses_run_directory: true,
    },
    TaskController {
        id: "modify-ptm",
        title: "Modify Protein (PTM)",
        short_title: "Modify PTM",
        theme: "Structure Editing",
        method: "Side-Chain Conjugation",
        application: "Post-Translational Modification",
        description: "Attach a post-translational modification — phosphorylation, acetylation, \
                      methylation, lipidation, or ubiquitination — to a protein residue. The \
                      modified structure is added as a new entry.",
        kind: TaskKind::ModifyProteinPtm,
        panel: TaskPanelKind::PtmPrompt,
        outcome: TaskOutcome::CreateEntry,
        backend: TaskBackend::InlineNative,
        uses_run_directory: false,
    },
];

pub fn task_controllers() -> &'static [TaskController] {
    TASK_CONTROLLERS
}

pub fn task_controller_by_id(id: &str) -> Option<&'static TaskController> {
    TASK_CONTROLLERS
        .iter()
        .find(|controller| controller.id == id)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskStatus {
    Ready,
    WaitingInput,
    Running,
    Cancelling,
    Completed,
    Failed,
    Cancelled,
}

impl TaskStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Ready => "Ready",
            Self::WaitingInput => "Waiting Input",
            Self::Running => "Running",
            Self::Cancelling => "Cancelling",
            Self::Completed => "Completed",
            Self::Failed => "Failed",
            Self::Cancelled => "Cancelled",
        }
    }
}

#[derive(Debug, Clone)]
pub struct TaskRun {
    /// In-memory handle used by the UI and actions. Stable within a project, but
    /// not the durable identity — that is [`Self::run_uuid`].
    pub id: u64,
    /// Random, globally-unique identifier for this task run. Decoupled from both
    /// the [`Self::id`] handle and the on-disk run-directory name, so two runs
    /// never collide regardless of session, project, or directory numbering.
    pub run_uuid: String,
    pub controller_id: &'static str,
    pub title: String,
    pub theme: String,
    pub method: String,
    pub application: String,
    pub kind: TaskKind,
    pub panel: TaskPanelKind,
    pub outcome: TaskOutcome,
    pub backend: TaskBackend,
    pub uses_run_directory: bool,
    pub status: TaskStatus,
    pub run_dir: Option<PathBuf>,
    pub source_entry_id: Option<u64>,
    pub result_entry_id: Option<u64>,
    pub engine_label: Option<String>,
    pub created_at_ms: u64,
    pub finished_at_ms: Option<u64>,
}

impl TaskRun {
    /// The entry this run's artifacts describe: the entry it produced, or — for a
    /// run that yields no new geometry, such as a single-point energy or a
    /// frequency calculation — the entry it was launched from. This is what makes
    /// a report reachable from the structure it belongs to without duplicating
    /// that structure into a second entry.
    pub fn anchor_entry_id(&self) -> Option<u64> {
        self.result_entry_id.or(self.source_entry_id)
    }

    pub fn from_controller(id: u64, controller: TaskController) -> Self {
        Self {
            id,
            run_uuid: uuid::Uuid::new_v4().to_string(),
            controller_id: controller.id,
            title: controller.title.to_string(),
            theme: controller.theme.to_string(),
            method: controller.method.to_string(),
            application: controller.application.to_string(),
            kind: controller.kind,
            panel: controller.panel,
            outcome: controller.outcome,
            backend: controller.backend,
            uses_run_directory: controller.uses_run_directory,
            status: TaskStatus::Ready,
            run_dir: None,
            source_entry_id: None,
            result_entry_id: None,
            engine_label: None,
            created_at_ms: now_unix_ms(),
            finished_at_ms: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct TaskDetailPanel {
    pub task_run_id: u64,
}

#[derive(Debug, Clone, Default)]
pub struct TaskListState {
    pub search_query: String,
    pub collapsed_themes: HashSet<String>,
}

#[derive(Debug, Clone)]
pub struct TaskManager {
    pub task_list: TaskListState,
    pub tasks: Vec<TaskRun>,
    pub panels: Vec<TaskDetailPanel>,
    pub active_panel: Option<u64>,
    pub(crate) next_task_run_id: u64,
}

impl Default for TaskManager {
    fn default() -> Self {
        Self {
            task_list: TaskListState::default(),
            tasks: Vec::new(),
            panels: Vec::new(),
            active_panel: None,
            next_task_run_id: 1,
        }
    }
}

impl TaskManager {
    pub fn create_task_run(&mut self, controller: TaskController) -> u64 {
        let task_id = self.next_task_run_id;
        self.next_task_run_id += 1;
        self.tasks
            .push(TaskRun::from_controller(task_id, controller));
        task_id
    }

    pub fn task_run(&self, task_run_id: u64) -> Option<&TaskRun> {
        self.tasks.iter().find(|task| task.id == task_run_id)
    }

    pub fn task_run_mut(&mut self, task_run_id: u64) -> Option<&mut TaskRun> {
        self.tasks.iter_mut().find(|task| task.id == task_run_id)
    }

    /// Find a run by its durable `run_uuid` (the key a detached remote job is
    /// reconnected through), rather than the in-memory `id` handle.
    pub fn task_run_by_uuid(&self, run_uuid: &str) -> Option<&TaskRun> {
        self.tasks.iter().find(|task| task.run_uuid == run_uuid)
    }

    /// The newest completed QM run whose artifacts belong to `entry_id` (see
    /// [`TaskRun::anchor_entry_id`]), or `None` when the entry has no QM results.
    /// An entry that was the input to several runs surfaces the most recent one.
    pub fn latest_qm_run_for_entry(&self, entry_id: u64) -> Option<&TaskRun> {
        self.tasks
            .iter()
            .filter(|task| {
                task.kind.is_qm()
                    && task.status == TaskStatus::Completed
                    && task.run_dir.is_some()
                    && task.anchor_entry_id() == Some(entry_id)
            })
            .max_by_key(|task| task.finished_at_ms.unwrap_or(task.created_at_ms))
    }

    pub fn mark_status(&mut self, task_run_id: u64, status: TaskStatus) {
        if let Some(task) = self.task_run_mut(task_run_id) {
            task.status = status;
            if matches!(
                status,
                TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Cancelled
            ) {
                task.finished_at_ms = Some(now_unix_ms());
            }
        }
    }

    pub fn set_run_dir(&mut self, task_run_id: u64, run_dir: PathBuf) {
        if let Some(task) = self.task_run_mut(task_run_id) {
            task.run_dir = Some(run_dir);
        }
    }

    pub fn set_source_entry_id(&mut self, task_run_id: u64, entry_id: Option<u64>) {
        if let Some(task) = self.task_run_mut(task_run_id) {
            task.source_entry_id = entry_id;
        }
    }

    pub fn set_result_entry_id(&mut self, task_run_id: u64, entry_id: Option<u64>) {
        if let Some(task) = self.task_run_mut(task_run_id) {
            task.result_entry_id = entry_id;
        }
    }

    pub fn set_engine_label(&mut self, task_run_id: u64, engine_label: Option<String>) {
        if let Some(task) = self.task_run_mut(task_run_id) {
            task.engine_label = engine_label;
        }
    }

    pub fn latest_completed_run_for_result(
        &self,
        kind: TaskKind,
        entry_id: u64,
    ) -> Option<&TaskRun> {
        self.tasks.iter().rev().find(|task| {
            task.kind == kind
                && task.status == TaskStatus::Completed
                && task.result_entry_id == Some(entry_id)
        })
    }

    pub fn open_panel(&mut self, task_run_id: u64) {
        self.active_panel = Some(task_run_id);
        if self
            .panels
            .iter()
            .all(|panel| panel.task_run_id != task_run_id)
        {
            self.panels.push(TaskDetailPanel { task_run_id });
        }
    }

    pub fn close_panel(&mut self, task_run_id: u64) {
        self.panels.retain(|panel| panel.task_run_id != task_run_id);
        if self.panels.is_empty() {
            self.active_panel = None;
        } else if self.active_panel == Some(task_run_id) {
            self.active_panel = self.panels.last().map(|panel| panel.task_run_id);
        }
    }

    pub fn activate_panel(&mut self, task_run_id: u64) {
        if self
            .panels
            .iter()
            .any(|panel| panel.task_run_id == task_run_id)
        {
            self.active_panel = Some(task_run_id);
        }
    }

    pub fn running_task_runs(&self) -> Vec<&TaskRun> {
        self.tasks
            .iter()
            .filter(|task| matches!(task.status, TaskStatus::Running | TaskStatus::Cancelling))
            .collect()
    }
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::{TaskManager, TaskStatus, task_controller_by_id};

    #[test]
    fn task_manager_creates_and_tracks_runs() {
        let mut manager = TaskManager::default();
        let controller = *task_controller_by_id("optimize-geometry").unwrap();

        let run_id = manager.create_task_run(controller);
        manager.open_panel(run_id);
        manager.mark_status(run_id, TaskStatus::WaitingInput);

        let run = manager.task_run(run_id).unwrap();
        assert_eq!(run.controller_id, "optimize-geometry");
        assert_eq!(run.title, "Molecular Geometry Optimization");
        assert_eq!(run.status, TaskStatus::WaitingInput);
        assert_eq!(manager.active_panel, Some(run_id));
        assert!(matches!(run.backend, super::TaskBackend::BackgroundNative));
    }

    #[test]
    fn recording_a_result_entry_sets_the_field() {
        let mut tasks = TaskManager::default();
        let controller = task_controller_by_id("qm-optimize").copied().unwrap();
        let id = tasks.create_task_run(controller);
        assert_eq!(tasks.task_run(id).unwrap().result_entry_id, None);
        tasks.set_result_entry_id(id, Some(42));
        assert_eq!(tasks.task_run(id).unwrap().result_entry_id, Some(42));
    }

    #[test]
    fn running_task_runs_lists_only_running() {
        let mut tasks = TaskManager::default();
        let a = tasks.create_task_run(task_controller_by_id("qm-energy").copied().unwrap());
        let b = tasks.create_task_run(task_controller_by_id("run-md").copied().unwrap());
        tasks.mark_status(a, TaskStatus::Running);
        tasks.mark_status(b, TaskStatus::Completed);
        let running: Vec<u64> = tasks.running_task_runs().iter().map(|t| t.id).collect();
        assert_eq!(running, vec![a]);
    }
}
