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
        actions::{AppAction, LeaveIntent},
        jobs::{
            EngineWorkerMessage, GromacsPipelineRequest, OptimizationWorkerMessage,
            QmWorkerMessage, engine_poll_frame, optimization_finished_message,
            request_next_optimization_poll, spawn_gromacs_build_job, spawn_gromacs_pipeline_job,
            spawn_optimization_job, spawn_self_update, spawn_update_check,
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
            SIDEBAR_MIN_WIDTH_SECONDARY, SelfUpdateStatus,
        },
        structure_import::{import_document, load_document},
        task_executor::task_executor,
        trajectory::{DEFAULT_PLAYBACK_FPS, TrajectoryPlayback, spawn_trajectory_load},
        viewport::view_center_and_radius,
    },
    io::{pdb_fetch, structure_io},
};

mod builders;
mod chart;
mod disorder;
mod dock;
mod docking;
mod export;
#[cfg(test)]
mod feedback_tests;
mod files;
mod gromacs;
mod heavy_render;
mod jobs;
mod project;
mod ptm;
mod remote_jobs;
mod selection;
mod settings;
mod simulation;
mod sketch;
mod tasks;

pub(crate) use builders::*;
pub(crate) use chart::*;
pub(crate) use disorder::*;
pub(crate) use dock::*;
pub(crate) use docking::*;
pub(crate) use export::*;
pub(crate) use files::*;
pub(crate) use gromacs::*;
pub(crate) use heavy_render::*;
pub(crate) use jobs::*;
pub(crate) use project::*;
pub(crate) use ptm::*;
pub(crate) use remote_jobs::*;
pub(crate) use selection::*;
pub(crate) use settings::*;
pub(crate) use simulation::*;
pub(crate) use sketch::*;
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
            | AppAction::RequestLeave(_)
            | AppAction::SaveAndLeave
            | AppAction::DiscardAndLeave
            | AppAction::CancelLeave
    );
    // Autosave only when the persisted entry state actually changes — an entry
    // added, removed, or edited. View-only changes (camera, render styles,
    // selection, active tab) don't move this fingerprint and are saved at exit
    // instead, so navigating or restyling never schedules a save.
    let fingerprint_before = (!manages_own_persistence).then(|| state.entries_fingerprint());
    let assistant_fingerprint_before = (!manages_own_persistence && state.workspace.is_project())
        .then(|| state.assistant_fingerprint());
    match action {
        AppAction::CreateProject => create_project_action(state),
        AppAction::OpenProject => request_leave(state, LeaveIntent::OpenProject, ctx),
        AppAction::OpenRecentProject(path) => {
            request_leave(state, LeaveIntent::OpenRecentProject(path), ctx)
        }
        AppAction::CloseProject => request_leave(state, LeaveIntent::CloseProject, ctx),
        AppAction::SaveProject => save_project(state),
        AppAction::RequestLeave(intent) => request_leave(state, intent, ctx),
        AppAction::SaveAndLeave => save_and_leave(state, ctx),
        AppAction::DiscardAndLeave => discard_and_leave(state, ctx),
        AppAction::CancelLeave => cancel_leave(state),
        AppAction::NewEmptyEntry => new_empty_entry(state),
        AppAction::OpenFile => open_file(state),
        AppAction::OpenPdbFetchDialog => open_pdb_fetch_dialog(state),
        AppAction::FetchPdb => fetch_pdb(state),
        AppAction::CancelPdbFetch => state.ui.pending_pdb_fetch = None,
        AppAction::OpenExportDialog { entry_id } => open_export_dialog(state, entry_id),
        AppAction::RunExport => run_export(state),
        AppAction::CancelExport => cancel_export(state),
        AppAction::Undo => undo(state),
        AppAction::Redo => redo(state),
        AppAction::EditStructure => edit_structure(state),
        AppAction::ApplyStructureEdits => apply_structure_edits(state),
        AppAction::CancelStructureEdits => cancel_structure_edits(state),
        AppAction::SketchMolecule => sketch_molecule(state),
        AppAction::CommitSketch => commit_sketch(state),
        AppAction::CancelSketch => cancel_sketch(state),
        AppAction::SelectAll => select_all(state),
        AppAction::InvertSelection => invert_selection(state),
        AppAction::ClearSelection => clear_selection(state),
        AppAction::SelectCategory(category) => select_category(state, category),
        AppAction::SelectAtom { atom_index, toggle } => select_atom(state, atom_index, toggle),
        AppAction::SelectResidue {
            residue_index,
            toggle,
        } => select_residue(state, residue_index, toggle),
        AppAction::SelectResidueRange {
            chain_id,
            start,
            end,
            toggle,
        } => select_residue_range(state, chain_id, start, end, toggle),
        AppAction::SelectResidues {
            residue_indices,
            mode,
        } => select_residues(state, residue_indices, mode),
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
        AppAction::StartQmWithDirectBackend => start_qm_with_direct_backend(state),
        AppAction::EstimateQmMemory => estimate_qm_memory(state),
        AppAction::CancelQmPrompt => cancel_pending_qm_request(state),
        AppAction::StartDocking => start_pending_docking(state),
        AppAction::CancelDockingPrompt => cancel_pending_docking_request(state),
        AppAction::SetPtmFamily(family) => with_ptm_prompt(state, |p| p.family = family),
        AppAction::SetPtmChain(chain) => with_ptm_prompt(state, |p| p.chain = chain),
        AppAction::SetPtmResSeq(res_seq) => with_ptm_prompt(state, |p| p.res_seq = res_seq),
        AppAction::SetPtmDegree(degree) => with_ptm_prompt(state, |p| p.degree = degree),
        AppAction::SetPtmLipid(lipid) => with_ptm_prompt(state, |p| p.lipid = lipid),
        AppAction::SetPtmUbl(ubl) => with_ptm_prompt(state, |p| p.ubl = ubl),
        AppAction::SetPtmUblOverride(entry) => with_ptm_prompt(state, |p| p.ubl_override = entry),
        AppAction::SetPtmNTerminal(on) => with_ptm_prompt(state, |p| p.n_terminal = on),
        AppAction::SetPtmGlycanIupac(iupac) => with_ptm_prompt(state, |p| p.glycan_iupac = iupac),
        AppAction::SetPtmGlycoKind(kind) => with_ptm_prompt(state, |p| p.glyco_kind = kind),
        AppAction::SetPtmGlycoRootAnomer(anomer) => {
            with_ptm_prompt(state, |p| p.glyco_root_anomer = anomer)
        }
        AppAction::SetPtmName(name) => with_ptm_prompt(state, |p| p.output_name = name),
        AppAction::StartPtm => start_pending_ptm(state),
        AppAction::CancelPtmPrompt => cancel_pending_ptm_request(state),
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
        AppAction::StartDisorder => start_pending_disorder(state),
        AppAction::CancelDisorderPrompt => cancel_pending_disorder_request(state),
        AppAction::SetDisorderName(name) => with_disorder_prompt(state, |p| p.output_name = name),
        AppAction::AddDisorderComponent(entry) => add_disorder_component(state, entry),
        AppAction::RemoveDisorderComponent(index) => with_disorder_prompt(state, |p| {
            if index < p.components.len() {
                p.components.remove(index);
            }
        }),
        AppAction::SetDisorderComponentEntry { index, entry_id } => {
            with_disorder_prompt(state, |p| {
                if let Some(component) = p.components.get_mut(index) {
                    component.entry_id = entry_id;
                }
            })
        }
        AppAction::SetDisorderComponentCount { index, count } => with_disorder_prompt(state, |p| {
            if let Some(component) = p.components.get_mut(index) {
                component.count = count;
            }
        }),
        AppAction::SetDisorderComponentAmount { index, value } => {
            with_disorder_prompt(state, |p| {
                if let Some(component) = p.components.get_mut(index) {
                    component.amount_value = value;
                }
            })
        }
        AppAction::SetDisorderAmountMode(mode) => {
            with_disorder_prompt(state, |p| p.amount_mode = mode)
        }
        AppAction::SetDisorderRegionKind(kind) => {
            with_disorder_prompt(state, |p| p.region_kind = kind)
        }
        AppAction::SetDisorderBoxLength { axis, value } => with_disorder_prompt(state, |p| {
            if axis < 3 {
                p.box_lengths[axis] = value;
            }
        }),
        AppAction::SetDisorderSphereRadius(radius) => {
            with_disorder_prompt(state, |p| p.sphere_radius = radius)
        }
        AppAction::SetDisorderCylinder { radius, length } => with_disorder_prompt(state, |p| {
            p.cyl_radius = radius;
            p.cyl_length = length;
        }),
        AppAction::SetDisorderSense(outside) => {
            with_disorder_prompt(state, |p| p.sense_outside = outside)
        }
        AppAction::SetDisorderTolerance(tolerance) => {
            with_disorder_prompt(state, |p| p.tolerance_angstrom = tolerance)
        }
        AppAction::SetDisorderSeed(seed) => with_disorder_prompt(state, |p| p.seed = seed),
        AppAction::RandomizeDisorderSeed => randomize_disorder_seed(state),
        AppAction::SetDisorderObstacle(entry) => {
            with_disorder_prompt(state, |p| p.obstacle_entry_id = entry)
        }
        AppAction::SetDisorderSetCell(on) => {
            with_disorder_prompt(state, |p| p.set_cell_from_region = on)
        }
        AppAction::SetDisorderPeriodic(on) => with_disorder_prompt(state, |p| p.periodic = on),
        AppAction::SetDisorderShowAdvanced(on) => {
            with_disorder_prompt(state, |p| p.show_advanced = on)
        }
        AppAction::SetDisorderLimits {
            max_restarts,
            max_steps,
        } => with_disorder_prompt(state, |p| {
            p.max_restarts = max_restarts;
            p.max_steps = max_steps;
        }),
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
        AppAction::VerifyEngine { target, engine } => verify_engine(state, target, engine),
        AppAction::ClearEngineLaunch { target, engine } => {
            clear_engine_launch(state, target, engine)
        }
        AppAction::BrowseEngineProgram(id) => browse_engine_program(state, id),
        AppAction::AddRemoteHost => add_remote_host(state),
        AppAction::SaveRemoteHost(id) => save_remote_host(state, id),
        AppAction::RemoveRemoteHost(id) => remove_remote_host(state, id),
        AppAction::DetectRemoteSlurm(id) => detect_remote_slurm(state, id),
        AppAction::RefreshSlurmCapabilities(id) => refresh_slurm_capabilities(state, id),
        AppAction::TestRemoteSlurm(id) => test_remote_slurm(state, id),
        AppAction::CheckRemoteHost(id) => check_remote_host(state, id),
        AppAction::SetupRemoteHostKey(id) => setup_remote_host_key(state, id),
        AppAction::BeginAddRemoteHost => begin_add_remote_host(state),
        AppAction::CancelAddRemoteHost => cancel_add_remote_host(state),
        AppAction::SetDefaultComputeTarget(target) => set_default_compute_target(state, target),
        AppAction::SetDefaultTaskPanelPlacement(placement) => {
            set_default_task_panel_placement(state, placement)
        }
        AppAction::FetchRemoteHardware(id) => fetch_remote_hardware(state, id),
        AppAction::RefreshRemoteJobs => refresh_remote_jobs(state),
        AppAction::CancelControlledJob(id) => cancel_controlled_job_action(state, &id),
        AppAction::RemoveRemoteScratch(run_uuid) => remove_remote_job_scratch(state, &run_uuid),
        AppAction::SetMonitorSource(src) => set_monitor_source(state, src),
        AppAction::RunConsoleCommand(command) => run_console_command(state, &command),
        AppAction::SendAgentMessage(text) => {
            crate::frontend::agent::send_agent_message(state, &text, ctx)
        }
        AppAction::NewAssistantConversation => {
            crate::frontend::agent::new_assistant_conversation(state)
        }
        AppAction::SwitchAssistantConversation(id) => {
            crate::frontend::agent::switch_assistant_conversation(state, id, ctx)
        }
        AppAction::RenameAssistantConversation { id, title } => {
            crate::frontend::agent::rename_assistant_conversation(state, id, &title)
        }
        AppAction::DeleteAssistantConversation(id) => {
            crate::frontend::agent::delete_assistant_conversation(state, id)
        }
        AppAction::SwitchAssistantConversationModel { provider, model } => {
            crate::frontend::agent::switch_assistant_conversation_model(state, &provider, &model)
        }
        AppAction::CancelAgent => crate::frontend::agent::cancel_agent(state, ctx),
        AppAction::ApproveToolCall(id) => {
            crate::frontend::agent::approve_tool_call(state, &id, ctx)
        }
        AppAction::RejectToolCall(id) => crate::frontend::agent::reject_tool_call(state, &id, ctx),
        AppAction::AlwaysAllowCommand(id) => {
            crate::frontend::agent::always_allow_command(state, &id, ctx)
        }
        AppAction::AlwaysAllowRisk(id) => {
            crate::frontend::agent::always_allow_risk(state, &id, ctx)
        }
        AppAction::SetApprovalMode(mode) => crate::frontend::agent::set_approval_mode(state, mode),
        AppAction::RemoveQueuedAgentInput(index) => {
            crate::frontend::agent::remove_queued_agent_input(state, index)
        }
        AppAction::SwitchProviderModel { provider, model } => {
            crate::frontend::agent::switch_provider_model(state, &provider, &model)
        }
        AppAction::SetAssistantEnabled(on) => {
            crate::frontend::agent::set_assistant_enabled(state, on)
        }
        AppAction::SetAssistantEffort(effort) => {
            crate::frontend::agent::set_assistant_effort(state, effort)
        }
        AppAction::SetAssistantEffortSupported(supported) => {
            crate::frontend::agent::set_assistant_effort_supported(state, supported)
        }
        AppAction::SetAssistantBaseUrl(url) => {
            crate::frontend::agent::set_assistant_base_url(state, &url)
        }
        AppAction::SetAssistantApiKey(key) => {
            crate::frontend::agent::set_assistant_api_key(state, &key)
        }
        AppAction::ClearStoredKey(id) => crate::frontend::agent::clear_stored_key(state, &id),
        AppAction::RefreshModels => crate::frontend::agent::fetch_models(state, ctx),
        AppAction::SetComputeCoreCount(cores) => set_compute_core_count(state, cores),
        AppAction::SetThemeMode(mode) => set_theme_mode(state, mode, ctx),
        AppAction::SetColorScheme(scheme) => set_color_scheme(state, scheme, ctx),
        AppAction::SetRepresentation(edit) => set_representation(state, edit),
        AppAction::ResetRepresentationGroup(group) => reset_representation_group(state, group),
        AppAction::ResetRepresentationDefaults => reset_representation_defaults(state),
        AppAction::SetGlass(on) => set_glass(state, on),
        AppAction::SetGlassIntensity { value, commit } => set_glass_intensity(state, value, commit),
        AppAction::SetCheckUpdates(on) => set_check_updates(state, on),
        AppAction::SetShowUtilizationBars(on) => set_show_utilization_bars(state, on),
        AppAction::SetMonitorRefresh(rate) => set_monitor_refresh(state, rate, ctx),
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
        AppAction::ShowDockPoses(entry_id) => show_dock_poses(state, entry_id),
        AppAction::OpenChart(target) => open_chart(state, target, ctx),
        AppAction::SelectChartDataset(index) => select_chart_dataset(state, index),
        AppAction::SetChartAxisLabel { axis, label } => set_chart_axis_label(state, axis, label),
        AppAction::ExportChart => export_chart(state),
        AppAction::ResizeSidebar(delta) => resize_sidebar(state, delta, ctx),
        AppAction::ResetSidebar => reset_sidebar(state, ctx),
        AppAction::ResizeArea(area, delta) => resize_area(state, area, delta, ctx),
        AppAction::ResetArea(area) => reset_area(state, area, ctx),
        AppAction::TogglePrimarySidebar => toggle_primary_sidebar(state, ctx),
        AppAction::ToggleAtomLabels => toggle_atom_labels(state),
        AppAction::MoveDockTab { tab, to, index } => move_dock_tab(state, tab, to, index, ctx),
        AppAction::ToggleDockArea(area) => toggle_dock_area(state, area, ctx),
        AppAction::ResetWorkbenchLayout => reset_workbench_layout(state),
        AppAction::DismissNotification => state.ui.notification = None,
        AppAction::RevealOutput(target) => reveal_output(state, target),
        AppAction::OpenDetailTarget(target) => open_detail_target(state, target),
        AppAction::AcknowledgeStatus => state.acknowledge_status(),
        AppAction::PostStatusNeutral(text) => state.status_neutral(text),
        AppAction::ReportSystemError { subsystem, text } => {
            state.report_system_error(subsystem, text)
        }
        AppAction::UseWireframeForHeavyEntry(entry_id) => {
            use_wireframe_for_heavy_entry(state, entry_id)
        }
        AppAction::RenderHeavyEntryAtFull(entry_id) => render_heavy_entry_at_full(state, entry_id),
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
    if let Some(before) = assistant_fingerprint_before
        && state.assistant_fingerprint() != before
    {
        let now = ctx.input(|input| input.time);
        state.request_autosave(now, AUTOSAVE_DEBOUNCE_SECS);
    }
}

/// How long after an entry change a coalesced autosave waits before flushing.
/// Long enough to absorb a burst of edits, short enough that an isolated change
/// is saved promptly.
const AUTOSAVE_DEBOUNCE_SECS: f64 = 0.5;

fn toggle_atom_labels(state: &mut AppState) {
    state.ui.viewport.show_atom_labels = !state.ui.viewport.show_atom_labels;
}

fn cancel_controlled_job_action(state: &mut AppState, id: &crate::frontend::jobs::JobControlId) {
    let job = crate::frontend::jobs::list_controlled_jobs(state)
        .into_iter()
        .find(|job| job.id == *id);
    match crate::frontend::jobs::cancel_controlled_job(state, id) {
        Ok(outcome) => state.status_neutral(crate::frontend::jobs::format_cancel_outcome_for_job(
            &outcome,
            job.as_ref(),
        )),
        Err(error) => state.status_error(format!("Could not cancel job: {error}")),
    }
}

pub(crate) fn run_console_command(state: &mut AppState, command: &str) {
    state.ui.console.history.push(command.to_string());
    // The prompt and its result/error are recorded to the Console transcript; the
    // transcript the user is looking at is the feedback, so no status is posted.
    let _ = crate::frontend::console::record_console_command(
        state,
        command,
        crate::frontend::state::CommandActor::User,
    );
}
