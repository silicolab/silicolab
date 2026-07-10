#[derive(Debug, Clone)]
pub enum AppAction {
    CreateProject,
    OpenProject,
    OpenRecentProject(std::path::PathBuf),
    CloseProject,
    SaveProject,
    RequestLeave(LeaveIntent),
    SaveAndLeave,
    DiscardAndLeave,
    CancelLeave,
    NewEmptyEntry,
    OpenFile,
    OpenPdbFetchDialog,
    FetchPdb,
    CancelPdbFetch,
    /// Open the Export dialog, which defaults to the entries selected in the
    /// sidebar. `entry_id` is set when the action comes from an entry's context
    /// menu: right-clicking outside the current selection exports just that
    /// entry, as a file manager would.
    OpenExportDialog {
        entry_id: Option<u64>,
    },
    /// Write the export the open dialog describes, then close it.
    RunExport,
    CancelExport,
    Undo,
    Redo,
    EditStructure,
    ApplyStructureEdits,
    CancelStructureEdits,
    /// Open the 2D molecule sketcher.
    SketchMolecule,
    /// Build the current sketch into a new workspace entry.
    CommitSketch,
    /// Discard the sketch and close the sketcher.
    CancelSketch,
    SelectAll,
    InvertSelection,
    ClearSelection,
    /// Replace the selection with every atom of a chemical category (protein,
    /// solvent, ligand, …).
    SelectCategory(crate::domain::AtomCategory),
    SelectAtom {
        atom_index: usize,
        toggle: bool,
    },
    SelectResidue {
        residue_index: usize,
        toggle: bool,
    },
    SelectResidueRange {
        chain_id: char,
        start: usize,
        end: usize,
        toggle: bool,
    },
    SelectResidues {
        residue_indices: Vec<usize>,
        mode: ResidueSelectionMode,
    },
    /// Apply a per-atom *base* drawing style to the current selection (or, when
    /// the selection is empty, to every atom as the new default).
    SetSelectionStyle(crate::frontend::state::AtomStyle),
    /// Toggle the cartoon ribbon overlay for the current selection (or all atoms
    /// when none is selected). Additive — combines with the base style.
    SetCartoonOverlay(bool),
    /// Toggle the molecular-surface overlay for the current selection (or all
    /// atoms when none is selected). Additive — combines with the base style and
    /// works for any molecule.
    SetSurfaceOverlay(bool),
    /// Remove per-atom style overrides and overlays for the current selection
    /// (or all atoms when the selection is empty), reverting to category
    /// defaults.
    ResetSelectionStyle,
    /// Change the visibility of the current selection (or all atoms when nothing
    /// is selected) via the per-atom visibility override, independent of style.
    SetSelectionVisibility(VisibilityCommand),
    /// Fine control of hydrogen-atom visibility within the current scope. Polar
    /// detection is not yet implemented (the variant is reserved).
    SetHydrogenDisplay(HydrogenDisplay),
    ActivateEntry(u64),
    DeleteEntry(u64),
    DeleteEntries(Vec<u64>),
    RenameEntry {
        entry_id: u64,
        new_name: String,
    },
    CreateGroup {
        name: String,
    },
    RenameGroup {
        group_id: String,
        new_name: String,
    },
    DeleteGroup(String),
    DeleteGroupWithEntries(String),
    MoveEntryToGroup {
        entry_id: u64,
        group_id: String,
    },
    CreateTask(&'static str),
    RunTask(u64),
    OpenTaskPanel(u64),
    CloseTaskPanel(u64),
    ActivateTaskPanel(u64),
    PreviewFramework,
    BuildFramework,
    CancelFramework,
    PreviewNanosheet,
    BuildNanosheet,
    CancelNanosheet,
    SaveBuildingBlock,
    CancelBuildingBlock,
    StartOptimization,
    CancelOptimizationPrompt,
    StartQmCalculation,
    /// Re-run the pending QM job after switching its SCF backend to
    /// integral-direct (the memory-guard escape hatch).
    StartQmWithDirectBackend,
    /// Estimate the peak memory the pending molecular QM job would use and stash
    /// the result on the prompt for display (the panel's "Estimate memory" button).
    EstimateQmMemory,
    CancelQmPrompt,
    StartDocking,
    CancelDockingPrompt,
    // --- Modify protein (PTM) ---
    /// Choose the modification family (phosphorylate/acetylate/…).
    SetPtmFamily(crate::frontend::state::PtmUiKind),
    /// Set the anchor residue chain id.
    SetPtmChain(String),
    /// Set the anchor residue sequence number.
    SetPtmResSeq(i32),
    /// Set the methylation degree (Methylate family).
    SetPtmDegree(crate::domain::modification::MethylDegree),
    /// Set the lipid (Lipidate family).
    SetPtmLipid(crate::frontend::ptm_commands::LipidKind),
    /// Set the ubiquitin-like modifier (Ubiquitinate family).
    SetPtmUbl(crate::domain::modification::UblKind),
    /// Choose an open entry to supply the UBL template (`None` uses the bundled
    /// one).
    SetPtmUblOverride(Option<u64>),
    /// Acetylate the chain N-terminus instead of the Lys side-chain NZ.
    SetPtmNTerminal(bool),
    /// Set the IUPAC-condensed glycan notation (Glycosylate family).
    SetPtmGlycanIupac(String),
    /// Assert the glycosylation junction, or `None` to derive it from the anchor
    /// residue (Glycosylate family).
    SetPtmGlycoKind(Option<crate::workflows::glycan::GlycosylationKind>),
    /// Override the reducing end's anomer, or `None` to derive it (Glycosylate).
    SetPtmGlycoRootAnomer(Option<crate::domain::glycan::Anomer>),
    /// Set the result entry's name.
    SetPtmName(String),
    /// Apply the modification to the active protein, creating a new entry.
    StartPtm,
    /// Discard the PTM draft and close the panel.
    CancelPtmPrompt,
    ConfirmSupercell,
    CancelSupercellPrompt,
    ConfirmProteinPrep,
    CancelProteinPrepPrompt,
    ConfirmMdSystem,
    CancelMdSystemPrompt,
    PickMdTopologyOverride,
    /// Select a custom force field from the library by name (or `None` for
    /// built-in only) for the MD System Builder; loads and caches its text.
    SelectCustomForceField(Option<String>),
    /// Save the MD System Builder's draft custom force field to the library under
    /// its draft name, then select it.
    SaveCustomForceField,
    /// Delete the named custom force field from the library.
    DeleteCustomForceField(String),
    /// Open a file picker and load a `.itp` into the draft custom force field.
    ImportCustomForceFieldFile,
    StartMdRun,
    CancelMdRunPrompt,
    // --- Build Disordered System (molecular packing) ---
    /// Launch the packing run from the current draft.
    StartDisorder,
    /// Cancel an in-flight packing run (or discard the draft if none is running).
    CancelDisorderPrompt,
    /// Set the result entry's name.
    SetDisorderName(String),
    /// Add a molecule row, optionally seeded with a specific entry id.
    AddDisorderComponent(Option<u64>),
    /// Remove the molecule row at this index.
    RemoveDisorderComponent(usize),
    /// Point the molecule row at a workspace entry.
    SetDisorderComponentEntry {
        index: usize,
        entry_id: u64,
    },
    /// Set a molecule row's copy count (Count mode).
    SetDisorderComponentCount {
        index: usize,
        count: u32,
    },
    /// Set a molecule row's density/concentration value (non-Count modes).
    SetDisorderComponentAmount {
        index: usize,
        value: f32,
    },
    /// Switch how component amounts are specified (count / density / molarity).
    SetDisorderAmountMode(crate::frontend::state::DisorderAmount),
    /// Choose the region shape (box / sphere / cylinder).
    SetDisorderRegionKind(crate::frontend::state::DisorderRegionKind),
    /// Set one box edge length (axis 0/1/2), in Å.
    SetDisorderBoxLength {
        axis: usize,
        value: f32,
    },
    /// Set the sphere radius (Å).
    SetDisorderSphereRadius(f32),
    /// Set the cylinder radius and length (Å).
    SetDisorderCylinder {
        radius: f32,
        length: f32,
    },
    /// Pack outside the region (`true`) rather than inside.
    SetDisorderSense(bool),
    /// Set the minimum inter-molecular spacing (Å).
    SetDisorderTolerance(f32),
    /// Set the RNG seed.
    SetDisorderSeed(u64),
    /// Pick a fresh random seed.
    RandomizeDisorderSeed,
    /// Choose an existing entry to pack around (`None` clears it).
    SetDisorderObstacle(Option<u64>),
    /// Stamp the region as the result's simulation cell.
    SetDisorderSetCell(bool),
    /// Pack periodically (no clashes across box edges).
    SetDisorderPeriodic(bool),
    /// Show or hide the advanced packing controls.
    SetDisorderShowAdvanced(bool),
    /// Set the optimizer limits (advanced).
    SetDisorderLimits {
        max_restarts: u32,
        max_steps: u32,
    },
    /// Select the Run MD preset; rebuilds the stage sequence for the system.
    SetMdRunPreset(crate::workflows::molecular_dynamics::PresetId),
    /// Set a system-type override (membrane/ligand/nucleic) for the run. Edits
    /// the separate per-run overrides, never the persisted detection context, and
    /// rebuilds the stages. `None` reverts that axis to "trust detection".
    SetMdRunOverride(crate::frontend::state::MdSystemAxis, Option<bool>),
    /// Set the run-level target temperature (K), applied to every stage.
    SetMdRunTemperature(f32),
    /// Set the production-length quick pick, applied to the production stage(s).
    SetMdRunProduction(crate::workflows::molecular_dynamics::ProductionLength),
    /// Set the run-level MD timestep (ps), applied to every dynamics stage.
    SetMdRunTimestep(f32),
    /// Toggle whether dynamics stages write a playable trajectory.
    SetMdRunSaveTrajectory(bool),
    /// Append a stage of the given kind to the run's sequence.
    AddMdRunStage(crate::workflows::molecular_dynamics::StageKind),
    /// Remove the stage at the given index from the run's sequence.
    RemoveMdRunStage(usize),
    /// Move the stage at the given index up (`true`) or down (`false`).
    MoveMdRunStage {
        index: usize,
        up: bool,
    },
    /// Apply one inline/detail edit to the stage at `index`. The detail-view
    /// widgets emit these; the dispatcher applies them in place so preset-filled
    /// defaults stay the starting point and only the touched field changes.
    EditMdRunStage {
        index: usize,
        edit: crate::frontend::state::MdStageEdit,
    },
    /// Open or close the detail view of the stage at the given index.
    ToggleMdRunStageExpanded(usize),
    RefreshEngineRegistry,
    DetectEngineVersions,
    ApplyEngineOverride(crate::engines::registry::EngineId),
    ClearEngineOverride(crate::engines::registry::EngineId),
    BrowseEngineProgram(crate::engines::registry::EngineId),
    /// Add a new remote host from the "add host" draft.
    AddRemoteHost,
    /// Persist edits to the host with this id from its draft.
    SaveRemoteHost(String),
    /// Remove the host with this id.
    RemoveRemoteHost(String),
    /// Detect GROMACS on the host with this id (worker thread).
    DetectRemoteGromacs(String),
    DetectRemoteSlurm(String),
    RefreshSlurmCapabilities(String),
    TestRemoteSlurm(String),
    /// Test passwordless login to the host with this id (worker thread).
    CheckRemoteHost(String),
    /// Generate the dedicated key (if needed) and show the one-line install
    /// command for the host with this id.
    SetupRemoteHostKey(String),
    /// Open the Settings dialog at the Remote Hosts section (from the per-task
    /// target picker's "Add host…" button).
    OpenRemoteHostsSettings,
    /// Set the global default compute target that new task panels seed from (each
    /// panel can still override it per run). Written to the settings file by the
    /// dispatcher.
    SetDefaultComputeTarget(crate::backend::config::ComputeTarget),
    /// Set where newly opened task panels appear by default. Users can still drag
    /// an open task panel into another dock host.
    SetDefaultTaskPanelPlacement(crate::backend::config::TaskPanelPlacement),
    /// Fetch the static hardware inventory (CPU/memory/GPU) of the host with this
    /// id over SSH, for the Hardware ▸ Remote settings panel (worker thread).
    FetchRemoteHardware(String),
    /// Opt-in refresh of every detached remote job: probe liveness and retrieve
    /// any finished outcome over SSH (off the UI thread). Never automatic.
    RefreshRemoteJobs,
    /// Request cancellation for a job through the unified control plane.
    CancelControlledJob(crate::frontend::jobs::JobControlId),
    /// Remove the remote scratch directory of the remote job with this `run_uuid`
    /// and drop its registry row.
    RemoveRemoteScratch(String),
    /// Set which machine the sidebar system monitor shows (Local or a remote host).
    /// Reconciles the live remote-GPU SSH sampler to match: starts it for a remote
    /// host, stops it on Local or a host change. At most one sampler runs at a time.
    SetMonitorSource(crate::frontend::state::MonitorSource),
    RunConsoleCommand(String),
    /// Send a message to the in-app assistant, kicking off an agent turn.
    SendAgentMessage(String),
    /// Create and switch to a new in-memory assistant conversation.
    NewAssistantConversation,
    /// Switch the Assistant panel to another in-memory conversation.
    SwitchAssistantConversation(crate::frontend::agent::AssistantConversationId),
    /// Rename one in-memory assistant conversation.
    RenameAssistantConversation {
        id: crate::frontend::agent::AssistantConversationId,
        title: String,
    },
    /// Delete one in-memory assistant conversation.
    DeleteAssistantConversation(crate::frontend::agent::AssistantConversationId),
    /// Cancel the in-flight assistant turn and any pending tool batch.
    CancelAgent,
    /// Approve the gated (compute/destructive) tool call with this id.
    ApproveToolCall(String),
    /// Reject the gated tool call with this id (the model gets an error result).
    RejectToolCall(String),
    /// Approve this id and auto-allow its command verb for the rest of the chat.
    AlwaysAllowCommand(String),
    /// Approve this id and auto-allow its whole risk level for the rest of the chat.
    AlwaysAllowRisk(String),
    /// Set the assistant's command-approval mode and persist.
    SetApprovalMode(crate::backend::config::ApprovalMode),
    /// Drop the queued (type-ahead) assistant follow-up message at this index.
    RemoveQueuedAgentInput(usize),
    /// Switch the active assistant provider + model and persist.
    SwitchProviderModel {
        provider: String,
        model: String,
    },
    /// Enable or disable the assistant and persist.
    SetAssistantEnabled(bool),
    /// Set the assistant's reasoning effort and persist.
    SetAssistantEffort(crate::io::llm::types::Effort),
    /// Pin whether the active OpenAI-compatible model accepts a reasoning-effort
    /// knob, overriding the registry heuristic. Persists.
    SetAssistantEffortSupported(bool),
    /// Set (blank clears) the base-URL override for an OpenAI-compatible
    /// assistant provider and persist.
    SetAssistantBaseUrl(String),
    /// Store the entered API key for the active provider in the app key store.
    SetAssistantApiKey(String),
    /// Remove the stored key for the provider with this id (active "Clear" button
    /// and the per-row Remove in the keys overview).
    ClearStoredKey(String),
    /// Fetch the active provider's live model list from its `/models` endpoint.
    RefreshModels,
    /// Set the number of CPU cores QM jobs may use and persist it.
    SetComputeCoreCount(usize),
    /// Set the light/dark appearance preference and persist it.
    SetThemeMode(crate::backend::config::ThemeMode),
    /// Set the accent + neutral color scheme and persist it.
    SetColorScheme(crate::backend::config::ColorScheme),
    /// Apply one edit to the global Representation defaults (base style, cartoon
    /// geometry, surface style/transparency) and persist. One parameterized
    /// action covers every live field, mirroring `EditMdRunStage`.
    SetRepresentation(crate::backend::representation::RepresentationEdit),
    /// Restore one Representation group (Base / Cartoon / Surface) to its
    /// defaults and persist.
    ResetRepresentationGroup(crate::backend::representation::RepresentationGroup),
    /// Restore every Representation default and persist.
    ResetRepresentationDefaults,
    /// Toggle the Apple-style frosted-glass material and persist it.
    SetGlass(bool),
    /// Set the Liquid Glass tint intensity (0..=1, clear → tinted). `commit`
    /// persists to disk — false while the settings slider is mid-drag (live
    /// preview only), true on release or a discrete change.
    SetGlassIntensity {
        value: f32,
        commit: bool,
    },
    /// Whether to check GitHub Releases for a newer version once per launch.
    /// Persisted; switching it on also runs a check right away so the toggle
    /// gives immediate feedback.
    SetCheckUpdates(bool),
    /// Show live CPU/GPU utilization gauges in the status bar. Persisted;
    /// turning on starts the sampler immediately, turning off stops it.
    SetShowUtilizationBars(bool),
    /// Set how often the system monitor samples (or pause it). Persisted; the
    /// running sampler picks up the new cadence on the next poll.
    SetMonitorRefresh(crate::backend::config::MonitorRefresh),
    /// Whether a discovered update downloads and installs itself automatically
    /// (`true`) or waits for a one-click "Update" (`false`). Persisted; only
    /// acts when update checks are on and the install is writable.
    SetAutoInstallUpdates(bool),
    /// Whether to reopen the last project on launch (`true`) or start in a blank
    /// scratch workspace (`false`). Maps to the inverse of
    /// `AppConfig::closed_to_scratch`; persisted.
    SetReopenLastProject(bool),
    /// Open a folder picker to choose the default project directory (where new
    /// projects are created and file dialogs start). Persisted on selection.
    PickDefaultProjectDir,
    /// Reveal the global settings.json in the OS file manager (Advanced ▸
    /// Configuration). Falls back to surfacing the path in a message if the
    /// shell-out fails.
    RevealSettingsFile,
    /// Restore every setting to its default value and persist (Advanced ▸
    /// Configuration). Gated behind an explicit confirmation in the UI.
    ResetAllSettings,
    /// Export the current settings to a user-chosen JSON file via a native save
    /// dialog (run in the dispatcher).
    ExportSettings,
    /// Import settings from a user-chosen JSON file via a native open dialog.
    /// Malformed input is reported non-fatally and leaves settings untouched.
    ImportSettings,
    /// Decode an MD trajectory for the given entry (from its run directory) in
    /// the background and begin playback once it is ready. The optional path
    /// selects a specific stage's trajectory (project-root-relative, as stored);
    /// `None` plays the entry's default (production) trajectory.
    LoadTrajectory(u64, Option<std::path::PathBuf>),
    /// Toggle play/pause of the active trajectory.
    ToggleTrajectoryPlay,
    /// Jump the active trajectory to a specific frame (pauses playback).
    SetTrajectoryFrame(usize),
    /// Close trajectory playback, returning the viewport to the static entry.
    StopTrajectory,
    /// Open the saved QM output report of the given QM-produced entry in a
    /// viewer window (triggered by clicking the entry's "QM" badge).
    ShowQmOutput(u64),
    /// Load a run's saved series data into the Plot panel and reveal it. The
    /// target is an entry (chart button on a QM-produced entry row) or a task
    /// run (completed-task panels; single-point runs produce no entry).
    OpenChart(ChartTarget),
    /// Switch the Plot panel to another dataset of the loaded run.
    SelectChartDataset(usize),
    /// Rename an axis of the Plot panel's active dataset.
    SetChartAxisLabel {
        axis: ChartAxis,
        label: String,
    },
    /// Export the Plot panel's active dataset using the dialog's draft
    /// choices: asks for a save path (rfd), writes the file, and persists the
    /// choices as the new defaults.
    ExportChart,
    /// Resize the primary (left) sidebar by a signed delta (drag direction
    /// already applied). The right sidebar and bottom panel resize via
    /// `ResizeArea`.
    ResizeSidebar(f32),
    /// Reset the primary sidebar to its default width.
    ResetSidebar,
    /// Resize a dock area (right sidebar width / bottom panel height) by a signed
    /// delta. Clamped by the dispatcher against the viewport size.
    ResizeArea(crate::frontend::state::DockArea, f32),
    /// Reset a dock area to its default size.
    ResetArea(crate::frontend::state::DockArea),
    TogglePrimarySidebar,
    ToggleAtomLabels,
    /// Move a dock tab into `to` at `index` (`None` appends) — the single
    /// drag-and-drop handler for both reorder-within-area and move-across-area.
    MoveDockTab {
        tab: crate::frontend::state::DockTab,
        to: crate::frontend::state::DockArea,
        index: Option<usize>,
    },
    /// Toggle a dock area's visibility (View / native menu). Revealing an empty
    /// area restores a default view so the toggle is never a dead no-op.
    ToggleDockArea(crate::frontend::state::DockArea),
    /// Reset the entire workbench layout to defaults (and persist).
    ResetWorkbenchLayout,
    /// Dismiss the active [`Notification`] without taking any of its actions.
    DismissNotification,
    /// Accept the heavy-structure suggestion for an entry: switch all of its
    /// atoms to wireframe, then render it.
    UseWireframeForHeavyEntry(u64),
    /// Decline the heavy-structure suggestion for an entry and render it at full
    /// detail (no silent simplification).
    RenderHeavyEntryAtFull(u64),
}

/// A user request that leaves the current workspace or exits the app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LeaveIntent {
    Quit,
    OpenProject,
    OpenRecentProject(std::path::PathBuf),
    CloseProject,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResidueSelectionMode {
    Replace,
    Add,
    Toggle,
    Remove,
}

impl LeaveIntent {
    pub fn action_label(&self) -> &'static str {
        match self {
            Self::Quit => "quit",
            Self::OpenProject | Self::OpenRecentProject(_) => "open another project",
            Self::CloseProject => "close this project",
        }
    }

    pub fn save_button_label(&self) -> &'static str {
        match self {
            Self::Quit => "Save and Quit",
            Self::OpenProject | Self::OpenRecentProject(_) => "Save and Open",
            Self::CloseProject => "Save and Close",
        }
    }
}

/// State for the modal that guards leaving with unsaved project or scratch data.
#[derive(Debug, Clone)]
pub struct LeaveConfirmation {
    pub intent: LeaveIntent,
    pub save_error: Option<String>,
}

impl LeaveConfirmation {
    pub fn new(intent: LeaveIntent) -> Self {
        Self {
            intent,
            save_error: None,
        }
    }

    pub fn save_failed(intent: LeaveIntent, error: impl Into<String>) -> Self {
        Self {
            intent,
            save_error: Some(error.into()),
        }
    }
}

/// A non-modal notification surfaced over the workspace: a short message that,
/// unlike [`crate::frontend::state::AppState::set_message`]'s plain status-bar
/// text, can offer the user a choice through action buttons. One notification is
/// shown at a time; posting a new one replaces any current one. Clicking a button
/// dismisses the notification and then dispatches the button's action, so a
/// button may itself post a follow-up notification.
#[derive(Debug, Clone)]
pub struct Notification {
    pub severity: NotificationSeverity,
    pub title: String,
    pub body: String,
    /// Buttons drawn left-to-right. Empty means the only way out is the dismiss
    /// affordance (the "×").
    pub buttons: Vec<NotificationButton>,
}

/// Accent/intent of a [`Notification`], driving only its color — not behaviour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationSeverity {
    /// Reserved — no producer posts an informational notification yet (the
    /// renderer still handles it, so a future caller needs no UI changes).
    #[allow(dead_code)]
    Info,
    Warning,
}

/// One [`Notification`] button: a label plus the action it dispatches.
#[derive(Debug, Clone)]
pub struct NotificationButton {
    pub label: String,
    pub action: AppAction,
    /// The recommended choice is drawn as the filled, primary-styled button.
    pub primary: bool,
}

impl Notification {
    pub fn new(
        severity: NotificationSeverity,
        title: impl Into<String>,
        body: impl Into<String>,
    ) -> Self {
        Self {
            severity,
            title: title.into(),
            body: body.into(),
            buttons: Vec::new(),
        }
    }

    /// Append a button. `primary` marks the recommended choice (filled style).
    pub fn button(mut self, label: impl Into<String>, primary: bool, action: AppAction) -> Self {
        self.buttons.push(NotificationButton {
            label: label.into(),
            action,
            primary,
        });
        self
    }
}

/// What a chart-open request points at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChartTarget {
    Entry(u64),
    TaskRun(u64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChartAxis {
    X,
    Y,
}

/// A visibility change applied to the Style panel's current scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VisibilityCommand {
    /// Make the scope atoms visible (clear their visibility override).
    Show,
    /// Hide the scope atoms (independent of their style).
    Hide,
    /// Show only the scope atoms; hide every other atom in the structure.
    ShowOnly,
}

/// Fine control of hydrogen visibility within the Style panel's current scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HydrogenDisplay {
    /// Show only polar hydrogens. **Reserved** — polar-hydrogen identification
    /// is not yet implemented, so the dispatcher reports it as unavailable and
    /// the panel control is disabled (hence no producer yet).
    #[allow(dead_code)]
    PolarOnly,
    /// Show every hydrogen in the scope.
    All,
    /// Hide every hydrogen in the scope.
    None,
}
