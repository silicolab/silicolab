mod monitor;
mod ui_state;

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
        ViewportVisualState,
        jobs::JobManager,
        viewport_defaults::{apply_entry_render_defaults, apply_solvent_render_default},
    },
    io::structure_io,
};

pub use monitor::RemoteGpuLive;
// Only the monitor panel's unit tests name this type via `state::RemoteGpuView`;
// non-test code reaches it through `RemoteGpuLive::gpus`, so gate the re-export.
#[cfg(test)]
pub use monitor::RemoteGpuView;
pub use ui_state::{SelfUpdateStatus, SequenceViewerState, TextViewer, UiState};

use super::*;

/// Debounce before a changed workbench layout is written to `settings.json`.
/// Long enough to coalesce a divider drag or a burst of tab clicks into one save
/// once the user pauses; short enough that an isolated change persists promptly.
const LAYOUT_SAVE_DEBOUNCE_SECS: f64 = 0.6;

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
    last_saved_entries_fingerprint: u64,
    last_saved_assistant_fingerprint: u64,
    project_save_error: Option<String>,
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
            last_saved_entries_fingerprint: 0,
            last_saved_assistant_fingerprint: 0,
            project_save_error: None,
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
            state.ui.agent = crate::frontend::agent::AgentSession::from_project_snapshot(
                snapshot.assistant.clone(),
            );
        }
        state.load_viewport_for_active_entry();
        state.ui.entry_list.selected_entry_ids.clear();
        if let Some(id) = state.entries.active_entry_id() {
            state.ui.entry_list.selected_entry_ids.insert(id);
        }
        state
            .history
            .set_active_entry(state.entries.active_entry_id());
        state.mark_project_saved();
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

    pub fn mark_project_saved(&mut self) {
        self.last_saved_entries_fingerprint = self.entries_fingerprint();
        self.last_saved_assistant_fingerprint = self.assistant_fingerprint();
        self.project_save_error = None;
    }

    pub fn mark_project_save_failed(&mut self, error: impl Into<String>) {
        self.project_save_error = Some(error.into());
    }

    pub fn project_save_error(&self) -> Option<&str> {
        self.project_save_error.as_deref()
    }

    pub fn has_project_changes_to_save(&self) -> bool {
        self.workspace.is_project()
            && (self.autosave_deadline.is_some()
                || self.entries_fingerprint() != self.last_saved_entries_fingerprint
                || self.assistant_fingerprint() != self.last_saved_assistant_fingerprint
                || self.project_save_error.is_some())
    }

    pub fn has_unsaved_workspace_drafts(&self) -> bool {
        self.ui.editor.is_some()
            || self.ui.sketcher.is_some()
            || self.ui.reticular_builder.is_some()
            || self.ui.nanosheet_builder.is_some()
            || self.ui.block_editor.is_some()
            || self.ui.pending_optimization.is_some()
            || self.ui.pending_qm.is_some()
            || self.ui.pending_supercell.is_some()
            || self.ui.pending_protein_prep.is_some()
            || self.ui.pending_md_system.is_some()
            || self.ui.pending_md_run.is_some()
            || self.ui.pending_disorder.is_some()
            || self.ui.pending_docking.is_some()
            || self.ui.pending_ptm.is_some()
            || self.ui.pending_pdb_fetch.is_some()
            || self.ui.pending_export.is_some()
    }

    pub fn scratch_has_unsaved_content(&self) -> bool {
        !self.workspace.is_project()
            && (!self.entries.records.is_empty()
                || !self.tasks.tasks.is_empty()
                || self.has_unsaved_workspace_drafts())
    }

    pub fn needs_leave_confirmation(&self) -> bool {
        self.scratch_has_unsaved_content()
            || self.has_project_changes_to_save()
            || self.has_unsaved_workspace_drafts()
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
            assistant: self.ui.agent.project_snapshot(),
            warnings: Vec::new(),
        })
    }

    pub fn assistant_fingerprint(&self) -> u64 {
        self.ui.agent.project_snapshot().fingerprint()
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
        use crate::frontend::jobs::{JobControlId, LocalJobSlot, cancel_controlled_job};
        for slot in [
            LocalJobSlot::Optimizer,
            LocalJobSlot::Disorder,
            LocalJobSlot::Qm,
            LocalJobSlot::Docking,
            LocalJobSlot::Engine,
        ] {
            let _ = cancel_controlled_job(self, &JobControlId::Local(slot));
        }
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

#[cfg(test)]
mod tests;
