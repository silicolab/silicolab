use super::*;

use std::collections::BTreeMap;
use std::path::PathBuf;

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

/// Debounce before a changed workbench layout is written to `settings.json`.
/// Long enough to coalesce a divider drag or a burst of tab clicks into one save
/// once the user pauses; short enough that an isolated change persists promptly.
const LAYOUT_SAVE_DEBOUNCE_SECS: f64 = 0.6;

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
    /// Detected GPU adapter name (from the wgpu render state at startup). `None`
    /// when the renderer doesn't expose one. Display-only.
    pub gpu_name: Option<String>,
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
    pub pending_disorder: Option<DisorderedSystemPrompt>,
    pub pending_docking: Option<DockingPrompt>,
    pub pending_pdb_fetch: Option<String>,
    /// The single active non-modal notification (a message plus optional action
    /// buttons), or `None`. Posting a new one replaces any current one.
    pub notification: Option<crate::frontend::actions::Notification>,
    /// The active entry whose full-detail render is held pending the user's
    /// answer to the heavy-structure wireframe suggestion, or `None` when nothing
    /// is gated. While set (and equal to the active entry) the viewport shows the
    /// prompt instead of the molecule, rather than silently simplifying it.
    pub pending_heavy_gate: Option<u64>,
    /// Entries for which the user has already answered the heavy-structure
    /// suggestion this session (accepted wireframe or chose full detail), so the
    /// prompt is not raised again on every re-activation. Transient.
    pub heavy_render_decided: std::collections::BTreeSet<u64>,
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
    /// In-app LLM assistant session: neutral conversation history, the turn
    /// state machine, the in-flight tool batch, and the Assistant-tab transcript.
    /// Like the editor sessions above it lives across frames; only the
    /// dispatcher and the poll-driven loop mutate it.
    pub agent: crate::frontend::agent::AgentSession,
    /// Parse/image cache backing the `egui_commonmark` viewer that formats the
    /// assistant's Markdown replies. Transient render state (egui texture
    /// handles, per-frame layout): never persisted, lives across frames, and is
    /// a sibling of `agent` so the transcript can be read while the cache is
    /// mutated during rendering.
    pub markdown_cache: egui_commonmark::CommonMarkCache,
    /// Latest CPU utilization sample (0–100 %). Updated by `poll_metrics` while
    /// the sampler is running; 0.0 when the sampler is off.
    pub cpu_pct: f32,
    /// Latest per-GPU live samples (util / VRAM / temp), one per GPU the sampler
    /// could read. Empty when the sampler is off or no live backend is available
    /// (then the gauges read N/A). Joined to the GPU inventory by PCI bus id.
    pub gpus: Vec<crate::frontend::gpu_monitor::GpuSample>,
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
            gpu_name: None,
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
            pending_disorder: None,
            pending_docking: None,
            pending_pdb_fetch: None,
            notification: None,
            pending_heavy_gate: None,
            heavy_render_decided: std::collections::BTreeSet::new(),
            md_solvation_preview: None,
            md_solvation_preview_key: 0,
            trajectory: None,
            available_update: None,
            text_viewer: None,
            self_update: SelfUpdateStatus::default(),
            agent: crate::frontend::agent::AgentSession::default(),
            markdown_cache: egui_commonmark::CommonMarkCache::default(),
            cpu_pct: 0.0,
            gpus: Vec::new(),
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
    /// egui time (seconds) at which a changed workbench layout should be written
    /// to `settings.json`, or `None` when the persisted layout is up to date.
    /// Separate from `autosave_deadline` because the layout is a global
    /// preference (not project data) and so must persist even in a scratch
    /// workspace, where project autosave is a no-op.
    layout_save_deadline: Option<f64>,
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
            layout_save_deadline: None,
        };
        // Apply the persisted workbench layout (a global preference). Task tabs
        // are session state and are never restored here; they are recreated as
        // tasks open.
        state.ui.layout.dock = DockModel::from_config(&state.config.dock_layout);
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

    /// Mark the persisted workbench layout dirty, scheduling a coalesced write to
    /// `settings.json` a short while after `now_seconds` (egui clock). Repeated
    /// calls push the deadline back so a divider drag or a burst of tab clicks
    /// collapses into a single save once the user pauses — the dock itself is
    /// mutated directly for instant feedback; only the disk write is debounced.
    pub fn mark_layout_dirty(&mut self, now_seconds: f64) {
        self.layout_save_deadline = Some(now_seconds + LAYOUT_SAVE_DEBOUNCE_SECS);
    }

    pub fn layout_save_deadline(&self) -> Option<f64> {
        self.layout_save_deadline
    }

    pub fn clear_layout_save_deadline(&mut self) {
        self.layout_save_deadline = None;
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
        self.ui.pending_disorder = None;
        self.ui.pending_docking = None;
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
            && self.ui.pending_disorder.is_none()
            && !self.jobs.optimization_running()
            && !self.jobs.engine_running()
            && !self.jobs.disorder_running()
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
        self.jobs.cancel_disorder();
        self.jobs.cancel_qm();
        self.jobs.cancel_docking();
        self.jobs.cancel_engine();
    }

    pub fn reset_layout_keep_view(&mut self) {
        let active_view = self.ui.layout.active_primary_view;
        self.ui.layout = LayoutState::default();
        self.ui.layout.active_primary_view = active_view;
        // The fresh dock holds no task tabs, so drop the matching panel/form
        // state too — otherwise an open task panel would survive the reset with
        // no tab to reach it.
        self.tasks.panels.clear();
        self.tasks.active_panel = None;
    }
}
