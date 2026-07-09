use super::*;
use crate::frontend::actions::{LeaveConfirmation, LeaveIntent};

/// Flush a coalesced autosave once its debounce window has elapsed. Called every
/// frame from the app loop; a no-op when nothing is pending. While a save is
/// still pending it requests a repaint at the deadline so the flush fires even
/// if the user stops interacting.
pub fn flush_pending_autosave(state: &mut AppState, ctx: &egui::Context) {
    let Some(deadline) = state.autosave_deadline() else {
        return;
    };
    let now = ctx.input(|input| input.time);
    if now >= deadline {
        // `persist_project` clears the deadline itself.
        let _ = persist_project(state, false);
    } else {
        ctx.request_repaint_after(std::time::Duration::from_secs_f64(deadline - now));
    }
}

/// Clean-shutdown checkpoint for window close: persist the project (including
/// undo history) and release the session lock so the next launch knows the
/// session ended cleanly. Skips database compaction to keep exit responsive.
pub fn shutdown(state: &mut AppState) -> anyhow::Result<()> {
    // The workbench layout is a global preference (persisted even in a scratch
    // workspace), so flush any pending layout save before the project-only gate.
    if state.layout_save_deadline().is_some() {
        persist_layout(state);
    }
    if !state.workspace.is_project() {
        return Ok(());
    }
    persist_project(state, true)?;
    if let Some(project) = state.workspace.project() {
        housekeeping::release_lock(project);
    }
    Ok(())
}

pub(crate) fn persist_project(state: &mut AppState, persist_history: bool) -> anyhow::Result<()> {
    // Any pending coalesced autosave is subsumed by this save.
    state.clear_autosave_deadline();
    let Some(project) = state.workspace.project() else {
        return Ok(());
    };
    // Save from borrowed references into the live state rather than cloning the
    // whole workspace: in an entry-heavy project (e.g. a 20-model NMR ensemble)
    // the clone dominated and made every interaction lag. `view` is the only
    // small owned value the snapshot needs.
    let view = state.project_view_settings();
    let snapshot = ProjectSnapshotRef {
        name: project.name.as_str(),
        entries: &state.entries,
        tasks: &state.tasks,
        view: &view,
        history: &state.history,
    };
    match save_project_ref(project, &snapshot, persist_history) {
        Ok(()) => {
            state.mark_project_saved();
            Ok(())
        }
        Err(error) => {
            let message = error.to_string();
            state.mark_project_save_failed(message.clone());
            state.set_message(format!("Project save failed: {message}"));
            Err(error)
        }
    }
}

pub(crate) fn reset_transient_state(state: &mut AppState) {
    state.cancel_transient_jobs();
    state.ui.pending_optimization = None;
    state.ui.pending_supercell = None;
    state.ui.pending_md_system = None;
    state.ui.pending_md_run = None;
    state.ui.pending_disorder = None;
    state.ui.editor = None;
    state.ui.reticular_builder = None;
    state.ui.block_editor = None;
    state.edit_origin = None;
    state.builder_origin = None;
    state.optimization_origin = None;
    state.ui.hovered_atom = None;
    state.ui.viewport_cache.clear();
    state.active_task_run = None;
    // Task tabs belong to the project's task runs; drop them so a closed/switched
    // project doesn't leave stale (and unreachable) task tabs docked. Fixed-view
    // placement is untouched.
    state.ui.layout.dock.clear_task_tabs();
    // A workspace change alters which project skills apply; drop the agent's
    // skills cache so the next turn reloads them for the new `project_root`.
    state.ui.agent.invalidate_skills();
}

/// Drop the Plot panel chart and the two chart memos on a project switch.
/// These are keyed by entry / task-run ids, but the backend restarts those id
/// spaces per project, so a reopened project would otherwise render project A's
/// cached thumbnail for its task id 1 (or a stale `false` availability would
/// suppress the chart chip forever). Deliberately NOT folded into
/// `reset_transient_state`, which also fires on plain entry activation where the
/// open chart and both memos must survive within the same project.
pub(crate) fn reset_chart_caches(state: &mut AppState) {
    state.ui.chart = None;
    state.ui.chart_availability.clear();
    state.ui.task_chart_thumbnails.clear();
}

pub(crate) fn replace_workspace_from_project(
    state: &mut AppState,
    project: ProjectSession,
    snapshot: ProjectSnapshot,
) {
    // Release the lock on the project we are leaving, then take the new one.
    if let Some(previous) = state.workspace.project() {
        housekeeping::release_lock(previous);
    }
    let recovered_from_crash = housekeeping::acquire_lock(&project);
    state.workspace = WorkspaceSession::Project(project.clone());
    state.entries = snapshot.entries;
    state.tasks = snapshot.tasks;
    state.history = snapshot.history;
    state
        .history
        .set_active_entry(state.entries.active_entry_id());
    state.ui.project_viewport = snapshot.view.viewport;
    state.ui.viewport = state.ui.project_viewport.clone();
    state.ui.entry_viewports = snapshot.view.entry_viewports;
    state.ui.entry_list.selected_entry_ids.clear();
    if let Some(id) = state.entries.active_entry_id() {
        state.ui.entry_list.selected_entry_ids.insert(id);
    }
    reset_transient_state(state);
    reset_chart_caches(state);
    state.load_viewport_for_active_entry();
    state.mark_project_saved();
    let set_current_dir_error = std::env::set_current_dir(&project.root).err();
    if let Err(error) =
        remember_opened_project(&mut state.config, &mut state.recent_projects, &project)
    {
        state.set_message(format!(
            "Opened project, but settings update failed: {error}"
        ));
    } else if let Some(error) = set_current_dir_error {
        state.set_message(format!(
            "Opened project {}, but working directory update failed: {error}",
            project.name
        ));
    } else {
        state.set_message(format!("Opened project {}", project.name));
    }
    if recovered_from_crash {
        state.set_message(format!(
            "Opened project {} (recovered: previous session did not close cleanly)",
            project.name
        ));
    }
}

pub(crate) fn create_project_action(state: &mut AppState) {
    let Some(path) = rfd::FileDialog::new()
        .set_directory(&state.config.default_project_dir)
        .set_file_name("New Project")
        .save_file()
    else {
        state.set_message("Create project canceled");
        return;
    };
    let Some(parent) = path.parent() else {
        state.set_message("Project path must have a parent directory");
        return;
    };
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        state.set_message("Project name cannot be empty");
        return;
    };

    match create_project(parent, name).and_then(|project| {
        let snapshot = state.project_snapshot().unwrap_or_else(|| ProjectSnapshot {
            name: project.name.clone(),
            entries: state.entries.clone(),
            tasks: state.tasks.clone(),
            view: state.project_view_settings(),
            history: state.history.clone(),
        });
        let snapshot = ProjectSnapshot {
            name: project.name.clone(),
            ..snapshot
        };
        save_project_session(&project, &snapshot, true)?;
        Ok((project, snapshot))
    }) {
        Ok((project, snapshot)) => replace_workspace_from_project(state, project, snapshot),
        Err(error) => state.set_message(format!("Create project failed: {error}")),
    }
}

fn pick_project_folder(state: &AppState) -> Option<PathBuf> {
    rfd::FileDialog::new()
        .set_directory(&state.config.default_project_dir)
        .pick_folder()
}

fn open_project_action_without_persist(state: &mut AppState) {
    let Some(path) = pick_project_folder(state) else {
        return;
    };
    open_project_path_without_persist(state, path);
}

fn open_project_path_without_persist(state: &mut AppState, path: PathBuf) {
    match open_project_dir(&path) {
        Ok((project, snapshot)) => replace_workspace_from_project(state, project, snapshot),
        Err(error) => state.set_message(error.to_string()),
    }
}

fn close_project_without_persist(state: &mut AppState, run_maintenance: bool) {
    // Compact the databases and release the lock now that we are leaving cleanly.
    if let Some(project) = state.workspace.project().cloned() {
        if run_maintenance && let Err(error) = housekeeping::run_maintenance(&project) {
            state.set_message(format!("Project maintenance failed: {error}"));
        }
        housekeeping::release_lock(&project);
    }
    state.workspace = WorkspaceSession::Scratch;
    state.entries = EntryStore::new_empty();
    state.tasks = TaskManager::default();
    state.ui.project_viewport = Default::default();
    state.ui.viewport = Default::default();
    state.ui.entry_viewports.clear();
    state.config.closed_to_scratch = true;
    state.config.last_project_path = None;
    if let Err(error) = save_config(&state.config) {
        state.set_message(format!(
            "Closed project, but settings update failed: {error}"
        ));
    } else {
        state.set_message("Closed project; opened Scratch");
    }
    reset_transient_state(state);
    reset_chart_caches(state);
    state.clear_history();
    state.mark_project_saved();
}

pub(crate) fn save_project(state: &mut AppState) {
    if state.workspace.is_project() {
        if persist_project(state, true).is_ok() {
            state.set_message(format!("Saved project {}", state.workspace.label()));
        }
        return;
    }
    create_project_action(state);
}

pub(crate) fn request_leave(state: &mut AppState, intent: LeaveIntent, ctx: &egui::Context) {
    if state.needs_leave_confirmation() {
        let mut confirmation = LeaveConfirmation::new(intent);
        if let Some(error) = state.project_save_error() {
            confirmation.save_error = Some(error.to_string());
        }
        state.ui.leave_confirmation = Some(confirmation);
        return;
    }

    if let Err(error) = execute_leave_intent(state, intent.clone(), ctx, true) {
        state.ui.leave_confirmation =
            Some(LeaveConfirmation::save_failed(intent, error.to_string()));
    }
}

pub(crate) fn cancel_leave(state: &mut AppState) {
    state.ui.leave_confirmation = None;
}

pub(crate) fn save_and_leave(state: &mut AppState, ctx: &egui::Context) {
    let Some(confirmation) = state.ui.leave_confirmation.take() else {
        return;
    };
    let intent = confirmation.intent;
    if let Err(error) = execute_leave_intent(state, intent.clone(), ctx, true) {
        state.ui.leave_confirmation =
            Some(LeaveConfirmation::save_failed(intent, error.to_string()));
    }
}

pub(crate) fn discard_and_leave(state: &mut AppState, ctx: &egui::Context) {
    let Some(confirmation) = state.ui.leave_confirmation.take() else {
        return;
    };
    let intent = confirmation.intent;
    if let Err(error) = execute_leave_intent(state, intent.clone(), ctx, false) {
        state.ui.leave_confirmation =
            Some(LeaveConfirmation::save_failed(intent, error.to_string()));
    }
}

fn execute_leave_intent(
    state: &mut AppState,
    intent: LeaveIntent,
    ctx: &egui::Context,
    save_current: bool,
) -> anyhow::Result<()> {
    if save_current && state.scratch_has_unsaved_content() {
        create_project_action(state);
        if state.scratch_has_unsaved_content() {
            state.ui.leave_confirmation = Some(LeaveConfirmation::new(intent));
            return Ok(());
        }
    }

    match intent {
        LeaveIntent::Quit => {
            if save_current {
                shutdown(state)?;
            } else {
                abandon_project_session(state);
                if state.layout_save_deadline().is_some() {
                    persist_layout(state);
                }
            }
            state.ui.allow_window_close = true;
            state.ui.request_window_close = true;
            ctx.request_repaint();
        }
        LeaveIntent::OpenProject => {
            if save_current {
                persist_project(state, true)?;
            }
            open_project_action_without_persist(state);
        }
        LeaveIntent::OpenRecentProject(path) => {
            if save_current {
                persist_project(state, true)?;
            }
            open_project_path_without_persist(state, path);
        }
        LeaveIntent::CloseProject => {
            if save_current {
                persist_project(state, true)?;
                close_project_without_persist(state, true);
            } else {
                close_project_without_persist(state, false);
            }
        }
    }
    Ok(())
}

fn abandon_project_session(state: &mut AppState) {
    if let Some(project) = state.workspace.project() {
        housekeeping::release_lock(project);
    }
}

pub(crate) fn load_active_entry(state: &mut AppState) {
    reset_transient_state(state);
    if let Some(active_id) = state.entries.active_entry_id() {
        state.ensure_entry_loaded(active_id);
    }
    state.sync_history_active_entry();
    if let Some(entry) = state.entries.active_entry() {
        state.ui.selection.retain_valid(entry.structure.atoms.len());
    } else {
        state.ui.selection.clear();
    }
    state.load_viewport_for_active_entry();
    state.ui.camera = crate::frontend::ViewCamera::default();
    state.ui.viewport_cache.clear();
    // The reset above wiped any transient form. If a task dashboard is still
    // open, re-initialize its form against the newly active structure so it
    // stays usable instead of rendering an empty "panel unavailable" body.
    if let Some(task_run_id) = state.tasks.active_panel {
        ensure_panel_form(state, task_run_id);
    }
    // Decide whether the freshly shown structure is heavy enough to suggest a
    // wireframe (and gate its full-detail render) instead of rendering blindly.
    maybe_gate_heavy_render(state);
}

pub(crate) fn require_active_entry(state: &mut AppState, action_label: &str) -> bool {
    if state.has_active_entry() {
        true
    } else {
        state.set_message(format!("{action_label} requires an open entry"));
        false
    }
}

pub(crate) fn new_empty_entry(state: &mut AppState) {
    let structure = Structure::empty();
    let save_path = structure_io::default_structure_save_path(&structure, None);
    let entry_id = add_and_show_entry(state, structure, None, save_path);
    state.set_message(format!("Created empty entry #{entry_id}"));
}

/// Insert a freshly produced structure as a new entry and switch to it, running
/// the full app-level load (first-load render defaults, transient reset, camera
/// recenter). Returns the new entry id.
///
/// `EntryStore::add_entry` already marks the entry active in the store, so this
/// must NOT route through [`activate_entry`]: its "already active" early-return
/// would skip [`load_active_entry`], leaving the new structure rendered with the
/// previous entry's styles — which is why a freshly built MD system showed its
/// bulk solvent as ball-and-stick instead of the wireframe default. Mirrors the
/// save → add → load sequence of [`new_empty_entry`].
pub(crate) fn add_and_show_entry(
    state: &mut AppState,
    structure: Structure,
    source_path: Option<PathBuf>,
    save_path: PathBuf,
) -> u64 {
    state.save_viewport_for_active_entry();
    let entry_id = state.entries.add_entry(structure, source_path, save_path);
    state.ui.entry_list.selected_entry_ids.clear();
    state.ui.entry_list.selected_entry_ids.insert(entry_id);
    // `load_active_entry` resets transient state, which includes the active task
    // run. When a task (e.g. an MD system build) produces and shows its result
    // entry, that task context must survive so the caller can still mark the run
    // complete and record this entry as its result — otherwise the run is never
    // marked completed and lookups like the GROMACS topology for the entry fail.
    let active_task_run = state.active_task_run;
    load_active_entry(state);
    state.active_task_run = active_task_run;
    entry_id
}

pub(crate) fn activate_entry(state: &mut AppState, entry_id: u64) {
    if state.entries.active_entry_id() == Some(entry_id) {
        return;
    }
    state.save_viewport_for_active_entry();
    state.entries.activate_entry(entry_id);
    state.ui.entry_list.selected_entry_ids.insert(entry_id);
    load_active_entry(state);
    state.set_message(format!("Loaded entry {}", state.current_entry_label()));
}

pub(crate) fn delete_entry(state: &mut AppState, entry_id: u64) {
    let Some(name) = state
        .entries
        .entry(entry_id)
        .map(|entry| entry.name.clone())
    else {
        state.set_message("Cannot delete entry".to_string());
        return;
    };
    let active_before = state.entries.active_entry_id();
    state.save_viewport_for_active_entry();

    if state.entries.delete_entry(entry_id) {
        state.ui.entry_viewports.remove(&entry_id);
        state.history.forget_entry(entry_id);
        state.ui.entry_list.selected_entry_ids.remove(&entry_id);
        if state.ui.entry_list.renaming_entry_id == Some(entry_id) {
            state.ui.entry_list.renaming_entry_id = None;
            state.ui.entry_list.rename_buffer.clear();
        }
        if active_before == Some(entry_id) {
            reset_transient_state(state);
            load_active_entry(state);
        }
        state.set_message(format!("Deleted entry {name}"));
    } else {
        state.set_message("Cannot delete entry".to_string());
    }
}

pub(crate) fn delete_entries(state: &mut AppState, ids: Vec<u64>) {
    for id in ids {
        delete_entry(state, id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::state::ChartState;

    #[test]
    fn reset_chart_caches_clears_chart_and_both_memos() {
        let mut state = AppState::scratch(Default::default(), Vec::new());
        state.ui.chart = Some(ChartState::new("project-a run".to_string()));
        state.ui.chart_availability.insert(1, true);
        state.ui.chart_availability.insert(2, false);
        state.ui.task_chart_thumbnails.insert(1, None);

        reset_chart_caches(&mut state);

        assert!(state.ui.chart.is_none());
        assert!(state.ui.chart_availability.is_empty());
        assert!(state.ui.task_chart_thumbnails.is_empty());
    }
}
