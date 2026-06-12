use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{anyhow, bail};
use eframe::egui;

use crate::backend::config::save_config;
use crate::{
    backend::{
        entries::{EntryOrigin, EntryStore},
        housekeeping,
        project::{
            ProjectSession, WorkspaceSession, create_project, open_project as open_project_dir,
            remember_opened_project, save_project as save_project_session, save_project_ref,
        },
        runs::ensure_run_dir,
        storage::{ProjectSnapshot, ProjectSnapshotRef},
        tasks::{TaskKind, TaskManager, TaskPanelKind, TaskStatus, task_controller_by_id},
    },
    domain::Structure,
    engines::{
        gromacs::{BuildRequest, IonOptions, TopologySource, render_top},
        registry::{EngineId, EngineRegistry},
    },
    frontend::{
        actions::AppAction,
        jobs::{
            EngineWorkerMessage, GromacsPipelineRequest, OptimizationWorkerMessage,
            QmWorkerMessage, engine_poll_frame, optimization_finished_message,
            request_next_optimization_poll, spawn_gromacs_build_job, spawn_gromacs_pipeline_job,
            spawn_optimization_job, spawn_qm_job, spawn_self_update, spawn_update_check,
        },
        md_support::{
            gromacs_topology_path_for_entry, load_md_topology_for_entry, write_md_system_context,
        },
        services::{
            BuildingBlockService, NanosheetService, ReticularService, StructureService,
            require_periodic_structure,
        },
        state::{
            AppState, PANEL_DEFAULT_HEIGHT, PANEL_MIN_HEIGHT, SIDEBAR_DEFAULT_WIDTH_PRIMARY,
            SIDEBAR_DEFAULT_WIDTH_SECONDARY, SIDEBAR_MIN_WIDTH_PRIMARY,
            SIDEBAR_MIN_WIDTH_SECONDARY, SelfUpdateStatus, Side, sidebar_max_width,
        },
        structure_import::{import_document, load_document},
        task_executor::task_executor,
        trajectory::{DEFAULT_PLAYBACK_FPS, TrajectoryPlayback, spawn_trajectory_load},
        viewport::view_center_and_radius,
    },
    io::{pdb_fetch, structure_io},
};

mod builders;
mod files;
mod jobs;
mod project;
mod selection;
mod settings;
mod simulation;
mod tasks;

pub(crate) use builders::*;
pub(crate) use files::*;
pub(crate) use jobs::*;
pub(crate) use project::*;
pub(crate) use selection::*;
pub(crate) use settings::*;
pub(crate) use simulation::*;
pub(crate) use tasks::*;

pub fn dispatch(state: &mut AppState, action: AppAction, ctx: &egui::Context) {
    // Project lifecycle actions persist themselves (open/create/close/save), so
    // they opt out of change-detected autosave to avoid a redundant save.
    let manages_own_persistence = matches!(
        action,
        AppAction::OpenProject
            | AppAction::OpenRecentProject(_)
            | AppAction::CreateProject
            | AppAction::CloseProject
            | AppAction::SaveProject
    );
    // Autosave only when the persisted entry state actually changes — an entry
    // added, removed, or edited. View-only changes (camera, render styles,
    // selection, active tab) don't move this fingerprint and are saved at exit
    // instead, so navigating or restyling never schedules a save.
    let fingerprint_before = (!manages_own_persistence).then(|| state.entries_fingerprint());
    match action {
        AppAction::CreateProject => create_project_action(state),
        AppAction::OpenProject => open_project_action(state),
        AppAction::OpenRecentProject(path) => open_project_path(state, path),
        AppAction::CloseProject => close_project(state),
        AppAction::SaveProject => save_project(state),
        AppAction::NewEmptyEntry => new_empty_entry(state),
        AppAction::OpenFile => open_file(state),
        AppAction::OpenPdbFetchDialog => open_pdb_fetch_dialog(state),
        AppAction::FetchPdb => fetch_pdb(state),
        AppAction::CancelPdbFetch => state.ui.pending_pdb_fetch = None,
        AppAction::Save => save(state),
        AppAction::SaveAs => save_as(state),
        AppAction::Undo => undo(state),
        AppAction::Redo => redo(state),
        AppAction::EditStructure => edit_structure(state),
        AppAction::ApplyStructureEdits => apply_structure_edits(state),
        AppAction::CancelStructureEdits => cancel_structure_edits(state),
        AppAction::SelectAll => select_all(state),
        AppAction::InvertSelection => invert_selection(state),
        AppAction::ClearSelection => clear_selection(state),
        AppAction::SelectCategory(category) => select_category(state, category),
        AppAction::SelectAtom { atom_index, toggle } => select_atom(state, atom_index, toggle),
        AppAction::SetSelectionStyle(style) => set_selection_style(state, style),
        AppAction::SetCartoonOverlay(on) => set_overlay(state, OverlayKind::Cartoon, on),
        AppAction::SetSurfaceOverlay(on) => set_overlay(state, OverlayKind::Surface, on),
        AppAction::ResetSelectionStyle => reset_selection_style(state),
        AppAction::SetSelectionVisibility(command) => set_selection_visibility(state, command),
        AppAction::SetHydrogenDisplay(mode) => set_hydrogen_display(state, mode),
        AppAction::ActivateEntry(entry_id) => activate_entry(state, entry_id),
        AppAction::DeleteEntry(entry_id) => delete_entry(state, entry_id),
        AppAction::DeleteEntries(ids) => delete_entries(state, ids),
        AppAction::RenameEntry { entry_id, new_name } => {
            state.entries.rename_entry(entry_id, new_name)
        }
        AppAction::CreateGroup { name } => create_group(state, name),
        AppAction::RenameGroup { group_id, new_name } => rename_group(state, &group_id, &new_name),
        AppAction::DeleteGroup(group_id) => delete_group(state, &group_id),
        AppAction::DeleteGroupWithEntries(group_id) => delete_group_with_entries(state, &group_id),
        AppAction::MoveEntryToGroup { entry_id, group_id } => {
            move_entry_to_group(state, entry_id, &group_id)
        }
        AppAction::CreateTask(template_id) => create_task_from_template(state, template_id),
        AppAction::RunTask(task_run_id) => run_task(state, task_run_id),
        AppAction::OpenTaskPanel(task_run_id) => open_task_panel(state, task_run_id),
        AppAction::CloseTaskPanel(task_run_id) => close_task_panel(state, task_run_id),
        AppAction::ActivateTaskPanel(task_run_id) => activate_task_panel(state, task_run_id),
        AppAction::PreviewFramework => preview_framework_task(state),
        AppAction::BuildFramework => accept_framework_task(state),
        AppAction::CancelFramework => cancel_framework_task(state),
        AppAction::PreviewNanosheet => preview_nanosheet_task(state),
        AppAction::BuildNanosheet => accept_nanosheet_task(state),
        AppAction::CancelNanosheet => cancel_nanosheet_task(state),
        AppAction::SaveBuildingBlock => save_block_editor_task(state),
        AppAction::CancelBuildingBlock => cancel_block_editor_task(state),
        AppAction::StartOptimization => start_pending_optimization(state),
        AppAction::CancelOptimizationPrompt => cancel_pending_optimization_request(state),
        AppAction::StartQmCalculation => start_pending_qm(state),
        AppAction::CancelQmPrompt => cancel_pending_qm_request(state),
        AppAction::ConfirmSupercell => confirm_pending_supercell(state),
        AppAction::CancelSupercellPrompt => cancel_pending_supercell_request(state),
        AppAction::ConfirmProteinPrep => confirm_pending_protein_prep(state),
        AppAction::CancelProteinPrepPrompt => cancel_pending_protein_prep_request(state),
        AppAction::ConfirmMdSystem => confirm_pending_md_system(state),
        AppAction::CancelMdSystemPrompt => cancel_pending_md_system_request(state),
        AppAction::PickMdTopologyOverride => pick_md_topology_override(state),
        AppAction::SelectCustomForceField(name) => select_custom_force_field(state, name.clone()),
        AppAction::SaveCustomForceField => save_custom_force_field(state),
        AppAction::DeleteCustomForceField(name) => delete_custom_force_field(state, name.as_str()),
        AppAction::ImportCustomForceFieldFile => import_custom_force_field_file(state),
        AppAction::StartMdRun => start_pending_md_run(state),
        AppAction::CancelMdRunPrompt => cancel_pending_md_run_request(state),
        AppAction::SetMdRunPreset(preset) => set_md_run_preset(state, preset),
        AppAction::SetMdRunOverride(axis, value) => set_md_run_override(state, axis, value),
        AppAction::SetMdRunTemperature(temperature) => {
            with_md_run_prompt(state, |prompt| prompt.apply_temperature(temperature))
        }
        AppAction::SetMdRunProduction(production) => {
            with_md_run_prompt(state, |prompt| prompt.apply_production(production))
        }
        AppAction::SetMdRunTimestep(timestep) => {
            with_md_run_prompt(state, |prompt| prompt.apply_timestep(timestep))
        }
        AppAction::SetMdRunSaveTrajectory(save) => {
            with_md_run_prompt(state, |prompt| prompt.set_save_trajectory(save))
        }
        AppAction::AddMdRunStage(kind) => {
            with_md_run_prompt(state, |prompt| prompt.add_stage(kind))
        }
        AppAction::RemoveMdRunStage(index) => {
            with_md_run_prompt(state, |prompt| prompt.remove_stage(index))
        }
        AppAction::MoveMdRunStage { index, up } => {
            with_md_run_prompt(state, |prompt| prompt.move_stage(index, up))
        }
        AppAction::EditMdRunStage { index, edit } => {
            with_md_run_prompt(state, |prompt| prompt.edit_stage(index, edit))
        }
        AppAction::ToggleMdRunStageExpanded(index) => {
            with_md_run_prompt(state, |prompt| prompt.toggle_stage_expanded(index))
        }
        AppAction::RefreshEngineRegistry => reprobe_engines(state),
        AppAction::DetectEngineVersions => detect_engine_versions(state),
        AppAction::ApplyEngineOverride(id) => apply_engine_override(state, id),
        AppAction::ClearEngineOverride(id) => clear_engine_override(state, id),
        AppAction::BrowseEngineProgram(id) => browse_engine_program(state, id),
        AppAction::RunConsoleCommand(command) => run_console_command(state, &command),
        AppAction::SetThemeMode(mode) => set_theme_mode(state, mode, ctx),
        AppAction::SetColorScheme(scheme) => set_color_scheme(state, scheme, ctx),
        AppAction::SetRepresentation(edit) => set_representation(state, edit),
        AppAction::ResetRepresentationGroup(group) => reset_representation_group(state, group),
        AppAction::ResetRepresentationDefaults => reset_representation_defaults(state),
        AppAction::SetGlass(on) => set_glass(state, on),
        AppAction::SetGlassIntensity { value, commit } => set_glass_intensity(state, value, commit),
        AppAction::SetCheckUpdates(on) => set_check_updates(state, on),
        AppAction::SetAutoInstallUpdates(on) => set_auto_install_updates(state, on),
        AppAction::SetReopenLastProject(on) => set_reopen_last_project(state, on),
        AppAction::PickDefaultProjectDir => pick_default_project_dir(state),
        AppAction::RevealSettingsFile => reveal_settings_file(state),
        AppAction::ResetAllSettings => reset_all_settings(state, ctx),
        AppAction::ExportSettings => export_settings(state),
        AppAction::ImportSettings => import_settings(state, ctx),
        AppAction::LoadTrajectory(entry_id, trajectory) => {
            load_trajectory(state, entry_id, trajectory, ctx)
        }
        AppAction::ToggleTrajectoryPlay => toggle_trajectory_play(state, ctx),
        AppAction::SetTrajectoryFrame(frame) => set_trajectory_frame(state, frame),
        AppAction::StopTrajectory => stop_trajectory(state),
        AppAction::ShowQmOutput(entry_id) => show_qm_output(state, entry_id),
        AppAction::ResizeSidebar(side, delta) => resize_sidebar(state, side, delta, ctx),
        AppAction::ResetSidebar(side) => reset_sidebar(state, side, ctx),
        AppAction::ResizePanel(delta) => resize_panel(state, delta, ctx),
        AppAction::ResetPanel => reset_panel(state),
    }
    if let Some(before) = fingerprint_before
        && state.entries_fingerprint() != before
    {
        // Entries changed (add/remove/edit). Coalesce rather than save
        // synchronously: a burst of edits collapses into one save once the user
        // pauses (see `flush_pending_autosave`). The flush still skips
        // re-serializing the (large) undo/redo history; that is persisted only at
        // explicit checkpoints (save, open, close, shutdown).
        let now = ctx.input(|input| input.time);
        state.request_autosave(now, AUTOSAVE_DEBOUNCE_SECS);
    }
}

/// How long after an entry change a coalesced autosave waits before flushing.
/// Long enough to absorb a burst of edits, short enough that an isolated change
/// is saved promptly.
const AUTOSAVE_DEBOUNCE_SECS: f64 = 0.5;

pub(crate) fn run_console_command(state: &mut AppState, command: &str) {
    let prompt = format!("sls> {command}");
    state.output_log.push(prompt);
    state.ui.console.history.push(command.to_string());
    match crate::frontend::console::execute_console_line(state, command) {
        Ok(message) => {
            if !message.is_empty() {
                state.set_message(message);
            }
        }
        Err(error) => state.set_message(format!("command failed: {error}")),
    }
}

pub fn handle_history_shortcuts(state: &mut AppState, ctx: &egui::Context) {
    if !state.history_navigation_enabled() || ctx.egui_wants_keyboard_input() {
        return;
    }

    let (undo_pressed, redo_pressed) = ctx.input(|input| {
        let command = input.modifiers.command || input.modifiers.ctrl;
        (
            command && input.key_pressed(egui::Key::Z) && !input.modifiers.shift,
            command
                && (input.key_pressed(egui::Key::Y)
                    || (input.modifiers.shift && input.key_pressed(egui::Key::Z))),
        )
    });

    if undo_pressed {
        dispatch(state, AppAction::Undo, ctx);
    } else if redo_pressed {
        dispatch(state, AppAction::Redo, ctx);
    }
}
