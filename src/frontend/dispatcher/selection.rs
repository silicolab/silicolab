use super::*;

pub(crate) fn select_all(state: &mut AppState) {
    state.ui.selection.select_all(state.structure().atoms.len());
    state.set_message(format!("Selected {} atom(s)", state.ui.selection.len()));
}

pub(crate) fn invert_selection(state: &mut AppState) {
    state.ui.selection.invert(state.structure().atoms.len());
    state.set_message(format!("Selected {} atom(s)", state.ui.selection.len()));
}

pub(crate) fn clear_selection(state: &mut AppState) {
    state.ui.selection.clear();
    state.set_message("Cleared atom selection".to_string());
}

pub(crate) fn select_category(state: &mut AppState, category: crate::domain::AtomCategory) {
    let indices: Vec<usize> = {
        let structure = state.structure();
        (0..structure.atoms.len())
            .filter(|index| structure.atom_category(*index) == category)
            .collect()
    };
    let count = indices.len();
    state.ui.selection.select_indices(indices);
    if count == 0 {
        state.set_message(format!("No {} atoms found", category.label()));
    } else {
        state.set_message(format!("Selected {count} {} atom(s)", category.label()));
    }
}

pub(crate) fn select_atom(state: &mut AppState, atom_index: usize, toggle: bool) {
    if toggle {
        state.ui.selection.toggle(atom_index);
    } else {
        state.ui.selection.select_only(atom_index);
    }
    state
        .ui
        .selection
        .retain_valid(state.structure().atoms.len());
    if state.ui.selection.is_empty() {
        state.set_message("Cleared atom selection".to_string());
    } else {
        state.set_message(format!("Selected {} atom(s)", state.ui.selection.len()));
    }
}

/// The atom indices a Style-panel action applies to: the current selection, or
/// — when nothing is selected — every atom in the active structure (the
/// "temporarily select all" default). The bool is `true` when the fallback
/// (whole-structure) scope was used, for messaging.
pub(crate) fn style_scope(state: &AppState) -> (Vec<usize>, bool) {
    if state.ui.selection.is_empty() {
        ((0..state.structure().atoms.len()).collect(), true)
    } else {
        (state.ui.selection.ordered_indices(), false)
    }
}

/// Pair each scope index with its chemical category (needed by the sparse
/// per-atom style/overlay maps), without holding a structure borrow.
pub(crate) fn scope_items(
    state: &AppState,
    indices: &[usize],
) -> Vec<(usize, crate::domain::AtomCategory)> {
    let structure = state.structure();
    indices
        .iter()
        .map(|&index| (index, structure.atom_category(index)))
        .collect()
}

pub(crate) fn set_selection_style(state: &mut AppState, style: crate::frontend::state::AtomStyle) {
    let (indices, all) = style_scope(state);
    if indices.is_empty() {
        state.set_message("No atoms to style".to_string());
        return;
    }
    let items = scope_items(state, &indices);
    let count = items.len();
    state.ui.viewport.apply_atom_styles(items, style);
    let scope = if all { "all" } else { "selected" };
    state.set_message(format!("Set {count} {scope} atom(s) to {}", style.label()));
}

/// Apply a visibility change to the Style panel's current scope. Visibility is a
/// per-atom override independent of style (see
/// [`crate::frontend::ViewportVisualState::atom_hidden`]).
pub(crate) fn set_selection_visibility(
    state: &mut AppState,
    command: crate::frontend::actions::VisibilityCommand,
) {
    use crate::frontend::actions::VisibilityCommand;
    let atom_count = state.structure().atoms.len();
    if atom_count == 0 {
        state.set_message("No atoms in the active entry".to_string());
        return;
    }
    let (indices, all) = style_scope(state);
    let scope = if all { "all" } else { "selected" };
    match command {
        VisibilityCommand::Show => {
            let count = indices.len();
            state.ui.viewport.set_atoms_hidden(indices, false);
            state.set_message(format!("Showed {count} {scope} atom(s)"));
        }
        VisibilityCommand::Hide => {
            let count = indices.len();
            state.ui.viewport.set_atoms_hidden(indices, true);
            state.set_message(format!("Hid {count} {scope} atom(s)"));
        }
        VisibilityCommand::ShowOnly => {
            let visible: std::collections::BTreeSet<usize> = indices.iter().copied().collect();
            let count = visible.len();
            state.ui.viewport.show_only(&visible, atom_count);
            state.set_message(format!("Showing only {count} {scope} atom(s)"));
        }
    }
}

/// Fine hydrogen-visibility control over the current scope. Polar-hydrogen
/// identification is not implemented yet, so that mode reports unavailable.
pub(crate) fn set_hydrogen_display(
    state: &mut AppState,
    mode: crate::frontend::actions::HydrogenDisplay,
) {
    use crate::frontend::actions::HydrogenDisplay;
    if matches!(mode, HydrogenDisplay::PolarOnly) {
        state.set_message("Polar-hydrogen detection is not yet implemented".to_string());
        return;
    }
    let (indices, _) = style_scope(state);
    let hydrogens: Vec<usize> = {
        let structure = state.structure();
        indices
            .into_iter()
            .filter(|&index| structure.atoms[index].element.eq_ignore_ascii_case("H"))
            .collect()
    };
    if hydrogens.is_empty() {
        state.set_message("No hydrogen atoms in scope".to_string());
        return;
    }
    let count = hydrogens.len();
    match mode {
        HydrogenDisplay::All => {
            state.ui.viewport.set_atoms_hidden(hydrogens, false);
            state.set_message(format!("Showing {count} hydrogen(s)"));
        }
        HydrogenDisplay::None => {
            state.ui.viewport.set_atoms_hidden(hydrogens, true);
            state.set_message(format!("Hid {count} hydrogen(s)"));
        }
        HydrogenDisplay::PolarOnly => unreachable!("handled above"),
    }
}

/// Which additive representation overlay an [`AppAction`] targets.
pub(crate) enum OverlayKind {
    Cartoon,
    Surface,
}

pub(crate) fn set_overlay(state: &mut AppState, kind: OverlayKind, on: bool) {
    let (indices, _) = style_scope(state);
    if indices.is_empty() {
        state.set_message("No atoms in the active entry".to_string());
        return;
    }
    let items = scope_items(state, &indices);
    let count = items.len();

    // Whether this entry had *any* surface overlay before this edit. Used below
    // to detect the "first surface on this entry" transition that seeds the
    // default surface appearance.
    let surface_was_empty = state.ui.viewport.surface_overlay.is_empty();

    let overlay = match kind {
        OverlayKind::Cartoon => &mut state.ui.viewport.cartoon_overlay,
        OverlayKind::Surface => &mut state.ui.viewport.surface_overlay,
    };
    for (index, category) in items {
        let default_on = match kind {
            OverlayKind::Cartoon => {
                crate::frontend::viewport::software_default_style(category)
                    == crate::frontend::state::AtomStyle::Cartoon
            }
            OverlayKind::Surface => false,
        };
        overlay.set_atom(index, on, default_on);
    }

    // Seed the default surface appearance the first time a surface is enabled on
    // an entry whose surface style is still factory-default. Surfaces aren't
    // discrete objects (there is no volume import), so "a surface is created" is
    // the surface overlay going from empty → non-empty. We only touch an
    // untouched surface state, so a user who already restyled this entry's
    // surface keeps their choice; later toggles never re-seed.
    if matches!(kind, OverlayKind::Surface)
        && on
        && surface_was_empty
        && !state.ui.viewport.surface_overlay.is_empty()
    {
        let factory = crate::frontend::ViewportSurfaceState::default();
        let untouched = state.ui.viewport.surface.style == factory.style
            && state.ui.viewport.surface.transparency == factory.transparency;
        if untouched {
            let prefs = &state.config.representation.surface;
            state.ui.viewport.surface.style = match prefs.style {
                crate::backend::representation::SurfaceStylePref::Solid => {
                    crate::frontend::SurfaceStyle::Fill
                }
                crate::backend::representation::SurfaceStylePref::Mesh => {
                    crate::frontend::SurfaceStyle::Mesh
                }
            };
            state.ui.viewport.surface.transparency = prefs.transparency_percent as f32 / 100.0;
        }
    }

    let label = match kind {
        OverlayKind::Cartoon => "Cartoon",
        OverlayKind::Surface => "Surface",
    };
    let verb = if on { "Enabled" } else { "Disabled" };
    state.set_message(format!("{verb} {label} overlay for {count} atom(s)"));
}

pub(crate) fn reset_selection_style(state: &mut AppState) {
    let (indices, all) = style_scope(state);
    if indices.is_empty() {
        state.set_message("No atoms to reset".to_string());
        return;
    }
    let count = indices.len();
    for index in &indices {
        state.ui.viewport.cartoon_overlay.atoms.remove(index);
        state.ui.viewport.surface_overlay.atoms.remove(index);
    }
    // Reset also clears any visibility override so the atoms come back.
    state
        .ui
        .viewport
        .set_atoms_hidden(indices.iter().copied(), false);
    state.ui.viewport.clear_atom_styles(indices);
    let scope = if all { "all" } else { "selected" };
    state.set_message(format!("Reset style for {count} {scope} atom(s)"));
}

pub(crate) fn undo(state: &mut AppState) {
    let Some(previous) = state.history.take_undo() else {
        return;
    };
    let current = state.capture_edit_snapshot();
    state.history.push_redo(current);
    state.restore_edit_snapshot(previous);
    state.set_message("Undid last change".to_string());
}

pub(crate) fn redo(state: &mut AppState) {
    let Some(next) = state.history.take_redo() else {
        return;
    };
    let current = state.capture_edit_snapshot();
    state.history.push_undo(current);
    state.restore_edit_snapshot(next);
    state.set_message("Redid last change".to_string());
}

pub(crate) fn create_group(state: &mut AppState, name: String) {
    match state.entries.create_group(&name) {
        Some(group_id) => {
            state.ui.entry_list.creating_group = false;
            state.ui.entry_list.new_group_name.clear();
            state.ui.entry_list.collapsed_group_ids.remove(&group_id);
            state.set_message(format!("Created group {}", name.trim()));
        }
        None => state.set_message("Group name cannot be empty".to_string()),
    }
}

pub(crate) fn rename_group(state: &mut AppState, group_id: &str, new_name: &str) {
    state.entries.rename_group(group_id, new_name);
    state.ui.entry_list.renaming_group_id = None;
    state.ui.entry_list.rename_group_buffer.clear();
}

pub(crate) fn delete_group(state: &mut AppState, group_id: &str) {
    if state.entries.delete_group(group_id) {
        state.ui.entry_list.collapsed_group_ids.remove(group_id);
        if state.ui.entry_list.renaming_group_id.as_deref() == Some(group_id) {
            state.ui.entry_list.renaming_group_id = None;
            state.ui.entry_list.rename_group_buffer.clear();
        }
        state.set_message("Deleted group".to_string());
    } else {
        state.set_message("Cannot delete group".to_string());
    }
}

pub(crate) fn delete_group_with_entries(state: &mut AppState, group_id: &str) {
    let ids: Vec<u64> = state
        .entries
        .records
        .iter()
        .filter(|e| e.group_id == group_id)
        .map(|e| e.id)
        .collect();
    for id in ids {
        delete_entry(state, id);
    }
    delete_group(state, group_id);
}

/// The group's display name and how many entries would be destroyed alongside
/// it, for a delete confirmation prompt. `None` if the group id is unknown.
pub(crate) fn group_delete_summary(state: &AppState, group_id: &str) -> Option<(String, usize)> {
    let name = state.entries.group(group_id)?.name.clone();
    let count = state
        .entries
        .records
        .iter()
        .filter(|entry| entry.group_id == group_id)
        .count();
    Some((name, count))
}

pub(crate) fn move_entry_to_group(state: &mut AppState, entry_id: u64, group_id: &str) {
    if state.entries.move_entry_to_group(entry_id, group_id) {
        if group_id.is_empty() {
            state.set_message("Removed entry from group".to_string());
        } else {
            state.set_message("Moved entry to group".to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use eframe::egui::Context;
    use nalgebra::Point3;

    use crate::{
        backend::project::WorkspaceSession,
        domain::{Atom, Structure, UnitCell},
        frontend::{actions::AppAction, state::AppState},
    };

    fn test_structure(title: &str) -> Structure {
        Structure::new(
            title,
            vec![Atom {
                element: "C".to_string(),
                position: Point3::new(0.0, 0.0, 0.0),
                charge: 0.0,
            }],
        )
    }

    /// An absolute project root that is valid on the platform running the
    /// tests — `md_run_origin` relativizes against a real absolute path.
    fn project_root() -> &'static std::path::Path {
        if cfg!(windows) {
            std::path::Path::new(r"C:\proj")
        } else {
            std::path::Path::new("/proj")
        }
    }

    #[test]
    fn md_run_origin_marks_run_even_without_a_trajectory() {
        use crate::backend::entries::EntryOrigin;

        // A relax-only run writes no `.xtc`: still an MD-run output (so the
        // badge shows), just without a playable trajectory.
        let origin = super::md_run_origin(None, Some(project_root()));
        assert!(origin.is_md_run());
        assert_eq!(origin.trajectory(), None);
        assert_eq!(origin, EntryOrigin::MdRun { trajectory: None });
    }

    #[test]
    fn md_run_origin_stores_trajectory_relative_to_project_root() {
        use std::path::Path;

        let root = project_root();
        let absolute = root.join("runs").join("run-md-1").join("md.xtc");
        let origin = super::md_run_origin(Some(absolute), Some(root));
        assert_eq!(origin.trajectory(), Some(Path::new("runs/run-md-1/md.xtc")));
    }

    #[test]
    fn md_run_origin_keeps_trajectory_absolute_when_outside_the_project() {
        // Run directory outside the project root: the path can't be made
        // relative, so it stays absolute rather than being dropped.
        let outside = if cfg!(windows) {
            PathBuf::from(r"D:\scratch\md.xtc")
        } else {
            PathBuf::from("/scratch/md.xtc")
        };
        let origin = super::md_run_origin(Some(outside.clone()), Some(project_root()));
        assert_eq!(origin.trajectory(), Some(outside.as_path()));
    }

    #[test]
    fn undo_and_redo_restore_edit_snapshot_metadata() {
        let ctx = Context::default();
        let mut state = AppState::new(
            test_structure("original"),
            Some(PathBuf::from(r"C:\tmp\original.xyz")),
            WorkspaceSession::Scratch,
            Default::default(),
            Vec::new(),
            None,
        );
        state.ui.selection.select_only(0);

        let before = state.capture_edit_snapshot();
        *state.structure_mut() = test_structure("edited");
        state.set_source_path(None);
        state.set_save_path(PathBuf::from(r"C:\tmp\edited.cif"));
        state.ui.selection.clear();
        state.history.push_undo(before);

        super::dispatch(&mut state, AppAction::Undo, &ctx);
        assert_eq!(state.structure().title, "original");
        assert_eq!(
            state
                .entries
                .active_entry()
                .and_then(|entry| entry.source_path.as_ref()),
            Some(&PathBuf::from(r"C:\tmp\original.xyz"))
        );
        assert_eq!(state.save_path(), &PathBuf::from(r"C:\tmp\original.xyz"));
        assert_eq!(state.ui.selection.ordered_indices(), vec![0]);

        super::dispatch(&mut state, AppAction::Redo, &ctx);
        assert_eq!(state.structure().title, "edited");
        assert_eq!(
            state
                .entries
                .active_entry()
                .and_then(|entry| entry.source_path.as_ref()),
            None
        );
        assert_eq!(state.save_path(), &PathBuf::from(r"C:\tmp\edited.cif"));
        assert!(state.ui.selection.is_empty());
    }

    #[test]
    fn entry_changes_move_the_fingerprint_but_view_changes_do_not() {
        let ctx = Context::default();
        let mut state = scratch_state(test_structure("mol"));
        let fingerprint = state.entries_fingerprint();

        // View-only interactions (selection, restyle) must not change the
        // fingerprint, so they never schedule a save.
        super::dispatch(&mut state, AppAction::SelectAll, &ctx);
        assert_eq!(
            state.entries_fingerprint(),
            fingerprint,
            "selection is view-only and must not move the fingerprint"
        );
        super::dispatch(&mut state, AppAction::ResetSelectionStyle, &ctx);
        assert_eq!(
            state.entries_fingerprint(),
            fingerprint,
            "restyling is view-only and must not move the fingerprint"
        );

        // Adding an entry is a persisted change and must move the fingerprint.
        super::dispatch(&mut state, AppAction::NewEmptyEntry, &ctx);
        assert_ne!(
            state.entries_fingerprint(),
            fingerprint,
            "adding an entry must move the fingerprint"
        );
    }

    #[test]
    fn autosave_deadline_is_scheduled_and_cleared() {
        let mut state = scratch_state(test_structure("mol"));
        assert_eq!(state.autosave_deadline(), None);
        state.request_autosave(10.0, 0.5);
        assert_eq!(state.autosave_deadline(), Some(10.5));
        // A later request pushes the deadline back (debounce coalescing).
        state.request_autosave(10.4, 0.5);
        assert_eq!(state.autosave_deadline(), Some(10.9));
        state.clear_autosave_deadline();
        assert_eq!(state.autosave_deadline(), None);
    }

    #[test]
    fn run_task_wraps_periodic_structure() {
        let ctx = Context::default();
        let structure = Structure::with_cell(
            "cell",
            vec![Atom {
                element: "C".to_string(),
                position: Point3::new(12.0, -1.0, 0.0),
                charge: 0.0,
            }],
            UnitCell::from_parameters(10.0, 10.0, 10.0, 90.0, 90.0, 90.0),
        );
        let mut state = AppState::new(
            structure,
            None,
            WorkspaceSession::Scratch,
            Default::default(),
            Vec::new(),
            None,
        );

        super::dispatch(
            &mut state,
            AppAction::CreateTask("translate-into-cell"),
            &ctx,
        );

        let atom = &state.structure().atoms[0];
        assert!(atom.position.x >= 0.0 && atom.position.x < 10.0);
        assert!(atom.position.y >= 0.0 && atom.position.y < 10.0);
    }

    #[test]
    fn nanosheet_task_opens_and_builds_on_empty_workspace() {
        // A nanosheet is the natural first thing to build with nothing loaded;
        // opening and building it must not require (or panic without) an entry.
        let ctx = Context::default();
        let mut state = scratch_state(Structure::empty());
        assert!(!state.has_active_entry());

        super::dispatch(&mut state, AppAction::CreateTask("build-nanosheet"), &ctx);
        assert!(state.ui.nanosheet_builder.is_some(), "panel should open");

        super::dispatch(&mut state, AppAction::BuildNanosheet, &ctx);
        assert!(state.has_active_entry(), "build should create an entry");
        assert!(state.structure().cell.is_some());
        assert!(state.structure().atoms.len() > 2);
    }

    fn scratch_state(structure: Structure) -> AppState {
        AppState::new(
            structure,
            None,
            WorkspaceSession::Scratch,
            Default::default(),
            Vec::new(),
            None,
        )
    }

    #[test]
    fn group_delete_summary_counts_members() {
        let mut state = scratch_state(test_structure("mol"));
        let gid = state
            .entries
            .create_group("Proteins")
            .expect("group created");
        state.entries.add_entry_to_group(
            test_structure("1abc"),
            None,
            PathBuf::from("1abc.xyz"),
            gid.clone(),
            None,
            false,
        );
        state.entries.add_entry_to_group(
            test_structure("2xyz"),
            None,
            PathBuf::from("2xyz.xyz"),
            gid.clone(),
            None,
            false,
        );

        let (name, count) = super::group_delete_summary(&state, &gid).expect("group exists");
        assert_eq!(name, "Proteins");
        assert_eq!(count, 2);
    }

    #[test]
    fn panel_dashboard_opens_on_empty_workspace() {
        let ctx = Context::default();
        // No atoms/title => no active entry at all.
        let mut state = scratch_state(Structure::empty());
        assert!(!state.has_active_entry());

        super::dispatch(&mut state, AppAction::CreateTask("build-md-system"), &ctx);
        assert!(
            state.ui.pending_md_system.is_some(),
            "interactive dashboard should open even with an empty workspace"
        );
    }

    #[test]
    fn switching_entries_keeps_open_dashboard_populated() {
        let ctx = Context::default();
        let mut state = scratch_state(test_structure("mol"));

        super::dispatch(&mut state, AppAction::CreateTask("build-md-system"), &ctx);
        assert!(state.ui.pending_md_system.is_some());

        // Creating + switching to another entry resets transient state; the
        // open dashboard must re-populate against the new structure.
        super::dispatch(&mut state, AppAction::NewEmptyEntry, &ctx);
        assert!(
            state.ui.pending_md_system.is_some(),
            "dashboard should survive an entry switch"
        );
    }

    #[test]
    fn md_system_confirm_defers_box_fit_check_and_keeps_panel() {
        use crate::frontend::state::{MdBuildEngine, MdSystemSizingMode};
        let ctx = Context::default();
        // Two atoms 2 A apart: a 0.5 A absolute box cannot contain them.
        let structure = Structure::new(
            "mol",
            vec![
                Atom {
                    element: "C".to_string(),
                    position: Point3::new(0.0, 0.0, 0.0),
                    charge: 0.0,
                },
                Atom {
                    element: "C".to_string(),
                    position: Point3::new(2.0, 0.0, 0.0),
                    charge: 0.0,
                },
            ],
        );
        let mut state = scratch_state(structure);

        super::dispatch(&mut state, AppAction::CreateTask("build-md-system"), &ctx);
        let prompt = state
            .ui
            .pending_md_system
            .as_mut()
            .expect("dashboard renders immediately");
        // The synchronous box-fit check is the built-in engine's; GROMACS boxes
        // via a subprocess job that unit tests can't run.
        prompt.engine = MdBuildEngine::BuiltIn;
        prompt.mode = MdSystemSizingMode::Absolute;
        prompt.absolute_angstrom = [0.5, 0.5, 0.5];

        // The undersized-box check runs at confirm time, not at open time: it
        // rejects gracefully and leaves the panel open for correction.
        super::dispatch(&mut state, AppAction::ConfirmMdSystem, &ctx);
        assert!(state.ui.pending_md_system.is_some());
        assert!(state.structure().cell.is_none());
    }

    #[test]
    fn confirm_md_system_boxes_structure_and_completes_task() {
        use crate::frontend::state::MdBuildEngine;
        let ctx = Context::default();
        let mut state = scratch_state(test_structure("mol"));

        super::dispatch(&mut state, AppAction::CreateTask("build-md-system"), &ctx);
        // The built-in engine boxes synchronously; GROMACS (the default) would
        // need a real subprocess this unit test can't run.
        state
            .ui
            .pending_md_system
            .as_mut()
            .expect("panel open after create")
            .engine = MdBuildEngine::BuiltIn;
        super::dispatch(&mut state, AppAction::ConfirmMdSystem, &ctx);

        assert!(state.structure().cell.is_some());
        assert!(state.ui.pending_md_system.is_none());
    }

    #[test]
    fn reopening_md_panel_reinitializes_dashboard() {
        let ctx = Context::default();
        let mut state = scratch_state(test_structure("mol"));

        super::dispatch(&mut state, AppAction::CreateTask("build-md-system"), &ctx);
        let task_id = state.tasks.active_panel.expect("panel open after create");

        // Canceling consumes the form and closes the panel.
        super::dispatch(&mut state, AppAction::CancelMdSystemPrompt, &ctx);
        assert!(state.ui.pending_md_system.is_none());

        // Re-opening restores the dashboard without re-running the task.
        super::dispatch(&mut state, AppAction::OpenTaskPanel(task_id), &ctx);
        assert!(state.ui.pending_md_system.is_some());
    }

    #[test]
    fn supercell_dashboard_defers_periodic_check_to_confirm() {
        let ctx = Context::default();
        // Non-periodic structure: the dashboard still opens.
        let mut state = scratch_state(test_structure("mol"));

        super::dispatch(&mut state, AppAction::CreateTask("expand-supercell"), &ctx);
        assert!(state.ui.pending_supercell.is_some());

        // Confirming without a cell is rejected, leaving the panel open.
        super::dispatch(&mut state, AppAction::ConfirmSupercell, &ctx);
        assert!(state.ui.pending_supercell.is_some());
        assert!(state.structure().cell.is_none());
    }

    #[test]
    fn caching_a_detected_launch_inserts_once_and_never_clobbers() {
        use crate::engines::registry::{EngineId, EngineLaunch};
        use std::collections::HashMap;

        let mut overrides: HashMap<String, EngineLaunch> = HashMap::new();
        let detected = EngineLaunch {
            command_prefix: vec!["wsl.exe".to_string(), "-e".to_string()],
            program: "/usr/local/gromacs/bin/gmx".to_string(),
        };

        // First detection caches the launch.
        assert!(super::cache_engine_override(
            &mut overrides,
            EngineId::GROMACS,
            detected.clone()
        ));
        assert_eq!(
            overrides.get("gromacs").map(|l| l.program.as_str()),
            Some("/usr/local/gromacs/bin/gmx")
        );

        // A later detection must not overwrite a launch already configured.
        let other = EngineLaunch {
            command_prefix: vec!["wsl.exe".to_string(), "-e".to_string()],
            program: "gmx".to_string(),
        };
        assert!(!super::cache_engine_override(
            &mut overrides,
            EngineId::GROMACS,
            other
        ));
        assert_eq!(
            overrides.get("gromacs").map(|l| l.program.as_str()),
            Some("/usr/local/gromacs/bin/gmx")
        );
    }
}
