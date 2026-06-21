use super::*;
use crate::engines::qm::{MemoryVerdict, QmScfBackend, memory_verdict};
use crate::frontend::actions::{Notification, NotificationButton, NotificationSeverity};

pub(crate) fn build_framework_task(state: &mut AppState) {
    state.cancel_transient_jobs();
    state.ui.pending_optimization = None;
    // Frameworks can be built from an empty workspace, where there is no active
    // entry to snapshot for undo; only capture an origin when one exists.
    state.builder_origin = state
        .has_active_entry()
        .then(|| state.capture_edit_snapshot());
    state.ui.reticular_builder = Some(crate::frontend::ReticularBuilderPanel::new(
        state.structure(),
    ));
}

pub(crate) fn build_block_from_current(state: &mut AppState) {
    state.cancel_transient_jobs();
    state.ui.pending_optimization = None;
    state.ui.block_editor = Some(crate::frontend::BuildingBlockEditor::new(state.structure()));
}

pub(crate) fn preview_framework_task(state: &mut AppState) {
    let Some(panel) = &state.ui.reticular_builder else {
        return;
    };
    match ReticularService::preview(&panel.spec) {
        Ok(built) => {
            state.cancel_transient_jobs();
            state.ui.pending_optimization = None;
            *state.structure_mut() = built.structure;
            state.mark_structure_changed();
            state.set_source_path(None);
            state.set_save_path(built.save_path);
            state.ui.camera = crate::frontend::ViewCamera::default();
            state.ui.selection.clear();
            state.set_message(format!(
                "Reticular structure preview generated; {}",
                built.analysis
            ));
        }
        Err(error) => state.set_message(format!("Reticular structure build failed: {error}")),
    }
}

pub(crate) fn accept_framework_task(state: &mut AppState) {
    let Some(panel) = &state.ui.reticular_builder else {
        return;
    };
    match ReticularService::build(&panel.spec) {
        Ok(built) => {
            if let Some(before) = state.builder_origin.take() {
                state.restore_edit_snapshot(before);
            }
            add_and_show_entry(state, built.structure, None, built.save_path);
            state.set_message(format!("Reticular structure built; {}", built.analysis));
            complete_active_task(
                state,
                TaskKind::BuildReticularStructure,
                TaskStatus::Completed,
            );
            close_active_task_panel(state);
        }
        Err(error) => state.set_message(format!("Reticular structure build failed: {error}")),
    }
}

pub(crate) fn cancel_framework_task(state: &mut AppState) {
    if let Some(before) = state.builder_origin.take() {
        state.restore_edit_snapshot(before);
    } else if let Some(panel) = &state.ui.reticular_builder {
        *state.structure_mut() = panel.original.clone();
        state.mark_structure_changed();
        state.ui.reticular_builder = None;
    }
    state.ui.reticular_builder = None;
    state.set_message("Reticular structure build canceled".to_string());
    complete_active_task(state, TaskKind::BuildReticularStructure, TaskStatus::Failed);
    close_active_task_panel(state);
}

pub(crate) fn build_nanosheet_task(state: &mut AppState) {
    state.cancel_transient_jobs();
    state.ui.pending_optimization = None;
    // A nanosheet is built from scratch, so the workspace is often empty (no
    // active entry to snapshot for undo); only capture an origin when one exists.
    state.builder_origin = state
        .has_active_entry()
        .then(|| state.capture_edit_snapshot());
    state.ui.nanosheet_builder = Some(crate::frontend::NanosheetBuilderPanel::new(
        state.structure(),
    ));
}

pub(crate) fn preview_nanosheet_task(state: &mut AppState) {
    let Some(panel) = &state.ui.nanosheet_builder else {
        return;
    };
    match NanosheetService::preview(&panel.spec) {
        Ok(built) => {
            state.cancel_transient_jobs();
            state.ui.pending_optimization = None;
            *state.structure_mut() = built.structure;
            state.mark_structure_changed();
            state.set_source_path(None);
            state.set_save_path(built.save_path);
            state.ui.camera = crate::frontend::ViewCamera::default();
            state.ui.selection.clear();
            state.set_message(format!("Nanosheet preview generated; {}", built.analysis));
        }
        Err(error) => state.set_message(format!("Nanosheet build failed: {error}")),
    }
}

pub(crate) fn accept_nanosheet_task(state: &mut AppState) {
    let Some(panel) = &state.ui.nanosheet_builder else {
        return;
    };
    match NanosheetService::build(&panel.spec) {
        Ok(built) => {
            if let Some(before) = state.builder_origin.take() {
                state.restore_edit_snapshot(before);
            }
            add_and_show_entry(state, built.structure, None, built.save_path);
            state.set_message(format!("Nanosheet built; {}", built.analysis));
            complete_active_task(state, TaskKind::BuildNanosheet, TaskStatus::Completed);
            close_active_task_panel(state);
        }
        Err(error) => state.set_message(format!("Nanosheet build failed: {error}")),
    }
}

pub(crate) fn cancel_nanosheet_task(state: &mut AppState) {
    if let Some(before) = state.builder_origin.take() {
        state.restore_edit_snapshot(before);
    } else if let Some(panel) = &state.ui.nanosheet_builder {
        *state.structure_mut() = panel.original.clone();
        state.mark_structure_changed();
        state.ui.nanosheet_builder = None;
    }
    state.ui.nanosheet_builder = None;
    state.set_message("Nanosheet build canceled".to_string());
    complete_active_task(state, TaskKind::BuildNanosheet, TaskStatus::Failed);
    close_active_task_panel(state);
}

pub(crate) fn save_block_editor_task(state: &mut AppState) {
    let Some(editor) = &state.ui.block_editor else {
        return;
    };
    match BuildingBlockService::save(editor, state.structure()) {
        Ok((path, source)) => {
            let current_structure = state.structure().clone();
            state.set_message(format!("Building block saved {}", path.display()));
            state
                .ui
                .reticular_builder
                .get_or_insert_with(|| {
                    crate::frontend::ReticularBuilderPanel::new(&current_structure)
                })
                .spec
                .custom_components
                .push(source);
            state.ui.block_editor = None;
            complete_active_task(state, TaskKind::CreateBuildingBlock, TaskStatus::Completed);
            close_active_task_panel(state);
        }
        Err(error) => state.set_message(format!("Building block save failed: {error}")),
    }
}

pub(crate) fn cancel_block_editor_task(state: &mut AppState) {
    state.ui.block_editor = None;
    state.set_message("Building block creation canceled".to_string());
    complete_active_task(state, TaskKind::CreateBuildingBlock, TaskStatus::Failed);
    close_active_task_panel(state);
}

pub(crate) fn start_pending_optimization(state: &mut AppState) {
    bind_active_panel_task(state, TaskPanelKind::OptimizationPrompt);
    let Some(prompt) = state.ui.pending_optimization else {
        return;
    };
    if state.jobs.optimization_running() {
        state.set_message(
            "forcefield optimization is already running; press Esc to stop".to_string(),
        );
        return;
    }
    if prompt.allow_cell_optimization && state.structure().cell.is_none() {
        state
            .set_message("crystal geometry optimization requires a periodic structure".to_string());
        return;
    }
    let options = prompt.options(&state.ui.selection);
    match spawn_optimization_job(state.structure().clone(), options) {
        Ok(job) => {
            state.optimization_origin = Some(state.capture_edit_snapshot());
            state.set_source_path(None);
            state.ui.editor = None;
            state.ui.pending_optimization = None;
            state.jobs.set_optimizer(job);
            if let Some(task_run_id) = state.active_task_run {
                state.tasks.mark_status(task_run_id, TaskStatus::Running);
            }
            state.set_message("forcefield optimization running; press Esc to stop".to_string());
        }
        Err(error) => {
            state.set_message(format!("forcefield optimization failed to start: {error}"));
            complete_active_task(state, TaskKind::OptimizeGeometry, TaskStatus::Failed);
            complete_active_task(state, TaskKind::OptimizeCrystalGeometry, TaskStatus::Failed);
        }
    }
}

pub(crate) fn cancel_pending_optimization_request(state: &mut AppState) {
    bind_active_panel_task(state, TaskPanelKind::OptimizationPrompt);
    state.ui.pending_optimization = None;
    state.set_message("forcefield optimization canceled".to_string());
    complete_active_task(state, TaskKind::OptimizeGeometry, TaskStatus::Failed);
    complete_active_task(state, TaskKind::OptimizeCrystalGeometry, TaskStatus::Failed);
    close_active_task_panel(state);
}

pub(crate) fn start_pending_qm(state: &mut AppState) {
    bind_active_panel_task(state, TaskPanelKind::QmPrompt);
    let Some(prompt) = state.ui.pending_qm.clone() else {
        return;
    };
    if state.jobs.qm_running() {
        state.set_message("a QM calculation is already running; press Esc to stop".to_string());
        return;
    }
    if state.structure().atoms.is_empty() {
        state.set_message("open a structure before running a QM calculation".to_string());
        return;
    }
    // A periodic run needs a real unit cell; reject early with a clear message
    // rather than letting the worker fail (the panel only offers the periodic
    // mode when a cell is present, but the prompt can outlive an entry switch).
    if prompt.periodic
        && state
            .structure()
            .cell
            .as_ref()
            .filter(|cell| !cell.is_placeholder())
            .is_none()
    {
        state
            .set_message("periodic QM needs a real unit cell; this structure has none".to_string());
        return;
    }
    // Memory guard: estimate the in-core ERI allocation for a molecular job and
    // refuse (or offer integral-direct) before we spawn the worker and clear the
    // prompt. Periodic jobs are exempt (no nao⁴ in-core tensor). A LOCAL job is
    // judged here against this machine's RAM; a REMOTE job defers to the off-thread
    // submit, which probes the host and judges against ITS RAM (this machine's
    // budget would be the wrong yardstick), so it is not pre-flighted on this path.
    if !prompt.periodic && resolve_qm_remote_host(state).is_none() {
        let request = prompt.to_request(state.structure().clone());
        let verdict = memory_verdict(&request, crate::backend::hardware::qm_incore_budget_bytes());
        if let Some(notification) = qm_memory_notification(&verdict, "this machine") {
            state.ui.notification = Some(notification);
            return; // leave pending_qm intact so the prompt stays open
        }
    }
    let job = prompt.to_job(state.structure().clone());
    let remote_host = resolve_qm_remote_host(state);
    state.set_source_path(None);
    state.ui.editor = None;
    state.ui.pending_qm = None;
    match remote_host {
        // A configured remote target: deploy + submit detached, tracked via the
        // job registry and the opt-in refresh — not the in-process worker.
        Some(host) => start_remote_qm(state, job, host),
        None => {
            let running = spawn_qm_job(job, Some(qm_thread_count(state)));
            state.jobs.set_qm(running);
            if let Some(task_run_id) = state.active_task_run {
                state.tasks.mark_status(task_run_id, TaskStatus::Running);
            }
            state.set_message("QM calculation running; press Esc to stop".to_string());
        }
    }
}

fn qm_thread_count(state: &AppState) -> usize {
    state
        .config
        .compute_core_count
        .clamp(1, crate::backend::hardware::info().logical_cores)
}

/// The remote host a QM job should run on, or `None` for local. Resolves the
/// app-wide compute target leniently: a dangling host id falls back to local,
/// mirroring the MD path's `resolve_md_compute`.
fn resolve_qm_remote_host(state: &AppState) -> Option<crate::backend::config::RemoteHost> {
    use crate::backend::config::ComputeTarget;
    match &state.config.default_compute_target {
        ComputeTarget::Local => None,
        ComputeTarget::Remote(host_id) => state.config.remote_hosts.get(host_id).cloned(),
    }
}

/// The in-core RAM budget the panel's "Estimate memory" reports against, and a
/// label naming the host it belongs to. A remote target with a detected inventory
/// uses that host's RAM and label; otherwise this machine's RAM. The detected
/// inventory is best-effort (the settings "Detect" action fills it); the off-thread
/// submit re-probes and re-checks against the real host before launch regardless.
fn qm_incore_budget_and_location(state: &AppState) -> (u64, String) {
    use crate::backend::hardware::{qm_incore_budget_bytes, qm_incore_budget_for};
    if let Some(host) = resolve_qm_remote_host(state)
        && let Some(ram) = state
            .ui
            .settings
            .remote_hardware
            .get(&host.id)
            .and_then(|info| info.ram_bytes)
    {
        return (qm_incore_budget_for(ram), host.label);
    }
    (qm_incore_budget_bytes(), "this machine".to_string())
}

/// Launch a detached remote QM job: ensure the worker is deployed, stage the
/// bundle, and submit — all off the UI thread. The durable handle is recorded in
/// the registry when the submission returns; status is tracked via the opt-in
/// refresh, so the run survives an app restart.
fn start_remote_qm(
    state: &mut AppState,
    job: crate::engines::qm::QmJob,
    host: crate::backend::config::RemoteHost,
) {
    let Some(task_run_id) = state.active_task_run else {
        state.set_message("no active task to run remotely".to_string());
        return;
    };
    let Some(task) = state.tasks.task_run(task_run_id).cloned() else {
        return;
    };
    if let Err(error) = crate::engines::remote::ensure_ssh_available() {
        state.set_message(format!("remote QM unavailable: {error}"));
        complete_active_qm_task(state, TaskStatus::Failed);
        return;
    }
    let local_run_dir = match ensure_active_task_run_dir(state, task.kind, None) {
        Ok(dir) => dir,
        Err(error) => {
            state.set_message(format!("could not create run directory: {error}"));
            complete_active_qm_task(state, TaskStatus::Failed);
            return;
        }
    };
    let project_root = state
        .workspace
        .project()
        .map(|project| project.root.to_string_lossy().to_string());
    // Per-job override → per-host default → app-wide count; the off-thread submit
    // then clamps this to the remote host's probed CPU inventory before staging.
    let requested_cores = crate::frontend::remote_jobs::resolve_requested_cores(
        None,
        &host,
        state.config.compute_core_count,
    );
    let handle = crate::frontend::remote_jobs::spawn_remote_submit(
        host.clone(),
        crate::wire::Engine::Qm(job),
        Some(requested_cores),
        task.run_uuid.clone(),
        Some(task_run_id),
        task.controller_id.to_string(),
        project_root,
        local_run_dir,
    );
    state.jobs.remote_submit = Some(handle);
    mark_task_status(state, task_run_id, TaskStatus::Running);
    state.set_message(format!(
        "Deploying & submitting QM to {} (use Refresh Remote to track it)…",
        host.label
    ));
}

pub(crate) fn cancel_pending_qm_request(state: &mut AppState) {
    bind_active_panel_task(state, TaskPanelKind::QmPrompt);
    state.ui.pending_qm = None;
    state.set_message("QM calculation canceled".to_string());
    complete_active_qm_task(state, TaskStatus::Failed);
    close_active_task_panel(state);
}

pub(crate) fn confirm_pending_supercell(state: &mut AppState) {
    if state.ui.pending_supercell.is_none() {
        return;
    }
    bind_active_panel_task(state, TaskPanelKind::SupercellPrompt);
    if let Err(error) = require_periodic_structure(
        state.structure(),
        "supercell expansion requires a periodic structure",
    ) {
        state.set_message(error.to_string());
        return;
    }
    let prompt = state
        .ui
        .pending_supercell
        .take()
        .expect("checked is_some above");
    expand_supercell(state, prompt.repeats);
    close_active_task_panel(state);
}

pub(crate) fn cancel_pending_supercell_request(state: &mut AppState) {
    bind_active_panel_task(state, TaskPanelKind::SupercellPrompt);
    state.ui.pending_supercell = None;
    state.set_message("Supercell expansion canceled".to_string());
    complete_active_task(state, TaskKind::ExpandSupercell, TaskStatus::Failed);
    close_active_task_panel(state);
}

pub(crate) fn confirm_pending_protein_prep(state: &mut AppState) {
    let Some(prompt) = state.ui.pending_protein_prep else {
        return;
    };
    bind_active_panel_task(state, TaskPanelKind::ProteinPrepPrompt);
    if prepare_protein(state, prompt) {
        state.ui.pending_protein_prep = None;
        close_active_task_panel(state);
    }
}

pub(crate) fn cancel_pending_protein_prep_request(state: &mut AppState) {
    bind_active_panel_task(state, TaskPanelKind::ProteinPrepPrompt);
    state.ui.pending_protein_prep = None;
    state.set_message("Protein preparation canceled".to_string());
    complete_active_task(state, TaskKind::PrepareProtein, TaskStatus::Failed);
    close_active_task_panel(state);
}

/// Prepare the active structure for simulation and add the result as a new
/// entry. This round only completes hydrogens; future steps (protonation states,
/// terminus patching, missing-atom repair) will extend the same prompt. Returns
/// `false` (keeping the panel open) on failure.
pub(crate) fn prepare_protein(
    state: &mut AppState,
    prompt: crate::frontend::state::ProteinPrepPrompt,
) -> bool {
    if state.structure().atoms.is_empty() {
        state.set_message("no active structure to prepare".to_string());
        return false;
    }
    if let Some(task_run_id) = state.active_task_run {
        mark_task_status(state, task_run_id, TaskStatus::Running);
    }
    state.cancel_transient_jobs();
    state.ui.pending_optimization = None;
    state.ui.editor = None;
    state.ui.selection.clear();

    let mut prepared = state.structure().clone();
    let mut added_hydrogens = 0usize;
    if prompt.add_hydrogens {
        added_hydrogens = prepared.add_missing_hydrogens();
    }

    let save_path = structure_io::default_structure_save_path(&prepared, None);
    let entry_id = add_and_show_entry(state, prepared, None, save_path);
    if let Some(task_run_id) = state.active_task_run {
        record_task_result_entry(state, task_run_id, entry_id);
    }
    state.set_message(format!(
        "Protein prepared: added {added_hydrogens} hydrogen(s) (new entry)"
    ));
    complete_active_task(state, TaskKind::PrepareProtein, TaskStatus::Completed);
    true
}

pub(crate) fn confirm_pending_md_system(state: &mut AppState) {
    let Some(prompt) = state.ui.pending_md_system.clone() else {
        return;
    };
    bind_active_panel_task(state, TaskPanelKind::MdSystemPrompt);
    if build_md_system(state, &prompt) {
        state.ui.pending_md_system = None;
        close_active_task_panel(state);
    }
}

pub(crate) fn cancel_pending_md_system_request(state: &mut AppState) {
    bind_active_panel_task(state, TaskPanelKind::MdSystemPrompt);
    state.ui.pending_md_system = None;
    state.set_message("MD system build canceled".to_string());
    complete_active_task(state, TaskKind::BuildMdSystem, TaskStatus::Failed);
    close_active_task_panel(state);
}

pub(crate) fn pick_md_topology_override(state: &mut AppState) {
    let Some(prompt) = state.ui.pending_md_run.as_mut() else {
        return;
    };
    let starting_dir = prompt
        .topology_override_path
        .as_ref()
        .and_then(|path| path.parent().map(PathBuf::from))
        .unwrap_or_else(|| state.config.default_project_dir.clone());
    let picked = rfd::FileDialog::new()
        .set_directory(starting_dir)
        .add_filter("GROMACS topology", &["top", "itp"])
        .pick_file();
    if let Some(path) = picked {
        prompt.topology_override_path = Some(path);
    }
}

/// Select (or clear) the framework build's custom force field, caching the
/// library entry's `.itp` text so the panel and build need not re-read it.
pub(crate) fn select_custom_force_field(state: &mut AppState, name: Option<String>) {
    let Some(prompt) = state.ui.pending_md_system.as_mut() else {
        return;
    };
    match name {
        None => {
            prompt.custom_force_field = None;
            prompt.custom_force_field_text = None;
        }
        Some(name) => match crate::backend::force_fields::load_force_field(&name) {
            Ok(text) => {
                prompt.custom_force_field = Some(name);
                prompt.custom_force_field_text = Some(text);
            }
            Err(error) => state.set_message(format!("failed to load force field: {error}")),
        },
    }
}

/// Save the draft custom force field to the reusable library, then select it.
pub(crate) fn save_custom_force_field(state: &mut AppState) {
    let Some(prompt) = state.ui.pending_md_system.as_ref() else {
        return;
    };
    let name = prompt.custom_ff_draft_name.trim().to_string();
    let text = prompt.custom_ff_draft.clone();
    if name.is_empty() {
        state.set_message("enter a name for the force field before saving".to_string());
        return;
    }
    if text.trim().is_empty() {
        state.set_message("the force field is empty".to_string());
        return;
    }
    match crate::backend::force_fields::save_force_field(&name, &text) {
        Ok(()) => {
            if let Some(prompt) = state.ui.pending_md_system.as_mut() {
                prompt.custom_force_field = Some(name.clone());
                prompt.custom_force_field_text = Some(text);
                prompt.custom_ff_draft.clear();
                prompt.custom_ff_draft_name.clear();
            }
            state.set_message(format!("saved force field `{name}`"));
        }
        Err(error) => state.set_message(format!("failed to save force field: {error}")),
    }
}

/// Delete a custom force field from the library; clear the selection if it was
/// the one in use.
pub(crate) fn delete_custom_force_field(state: &mut AppState, name: &str) {
    match crate::backend::force_fields::delete_force_field(name) {
        Ok(()) => {
            if let Some(prompt) = state.ui.pending_md_system.as_mut()
                && prompt.custom_force_field.as_deref() == Some(name)
            {
                prompt.custom_force_field = None;
                prompt.custom_force_field_text = None;
            }
            state.set_message(format!("deleted force field `{name}`"));
        }
        Err(error) => state.set_message(format!("failed to delete force field: {error}")),
    }
}

/// Open a file picker and load a `.itp`/`.top` into the draft custom force field,
/// suggesting a name from the file stem.
pub(crate) fn import_custom_force_field_file(state: &mut AppState) {
    let Some(path) = rfd::FileDialog::new()
        .set_directory(&state.config.default_project_dir)
        .add_filter("GROMACS force field", &["itp", "top"])
        .pick_file()
    else {
        return;
    };
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) => {
            state.set_message(format!("failed to read {}: {error}", path.display()));
            return;
        }
    };
    if let Some(prompt) = state.ui.pending_md_system.as_mut() {
        if prompt.custom_ff_draft_name.trim().is_empty()
            && let Some(stem) = path.file_stem().and_then(|s| s.to_str())
        {
            prompt.custom_ff_draft_name = stem.to_string();
        }
        prompt.custom_ff_draft = text;
    }
}

/// Build the warning shown when a pending QM job would exceed the RAM budget.
/// `ExceedsCanDirect` offers a one-click switch to integral-direct; otherwise the
/// only path forward is editing the job, so the warning is acknowledge-only.
pub(crate) fn qm_memory_notification(
    verdict: &MemoryVerdict,
    location: &str,
) -> Option<Notification> {
    let detail = verdict.detail(location)?;
    let title = "This calculation may run out of memory";
    match verdict {
        // Unreachable: detail()? above already returned None for Ok; arm kept for exhaustiveness.
        MemoryVerdict::Ok => None,
        MemoryVerdict::ExceedsCanDirect { .. } => Some(Notification {
            severity: NotificationSeverity::Warning,
            title: title.into(),
            body: format!(
                "{detail} Integral-direct SCF runs the same single point with far less memory."
            ),
            buttons: vec![
                NotificationButton {
                    label: "Run with integral-direct".into(),
                    action: AppAction::StartQmWithDirectBackend,
                    primary: true,
                },
                NotificationButton {
                    label: "Cancel".into(),
                    action: AppAction::DismissNotification,
                    primary: false,
                },
            ],
        }),
        MemoryVerdict::ExceedsMustReduce { .. } => Some(Notification {
            severity: NotificationSeverity::Warning,
            title: title.into(),
            body: format!(
                "{detail} This calculation type needs in-core integrals — choose a smaller basis set or a smaller system."
            ),
            buttons: vec![NotificationButton {
                label: "OK".into(),
                action: AppAction::DismissNotification,
                primary: true,
            }],
        }),
    }
}

/// Memory-guard escape hatch: flip the pending job to integral-direct and re-run.
pub(crate) fn start_qm_with_direct_backend(state: &mut AppState) {
    if let Some(prompt) = state.ui.pending_qm.as_mut() {
        prompt.options.scf_backend = QmScfBackend::Direct;
    }
    start_pending_qm(state);
}

/// Estimate the pending molecular QM job's peak memory and stash it on the prompt
/// for the panel to display. Periodic jobs have no in-core ERI tensor to model,
/// so the panel hides the button for them and this no-ops if one slips through.
pub(crate) fn estimate_qm_memory(state: &mut AppState) {
    let Some(prompt) = state.ui.pending_qm.as_ref() else {
        return;
    };
    if prompt.periodic {
        return;
    }
    if state.structure().atoms.is_empty() {
        state.set_message("open a structure before estimating QM memory".to_string());
        return;
    }
    let request = prompt.to_request(state.structure().clone());
    let signature = prompt.memory_signature(state.structure());
    let (budget, location) = qm_incore_budget_and_location(state);
    match crate::engines::qm::estimate_request_memory(&request, budget) {
        Ok(report) => {
            if let Some(prompt) = state.ui.pending_qm.as_mut() {
                prompt.memory_report = Some(crate::frontend::state::QmMemoryEstimate {
                    report,
                    signature,
                    location,
                });
            }
        }
        Err(error) => {
            if let Some(prompt) = state.ui.pending_qm.as_mut() {
                prompt.memory_report = None;
            }
            state.set_message(format!("could not estimate QM memory: {error}"));
        }
    }
}

#[cfg(test)]
mod memory_guard_tests {
    use super::*;
    use crate::engines::qm::MemoryVerdict;

    #[test]
    fn notification_offers_direct_for_can_direct_only() {
        let can = MemoryVerdict::ExceedsCanDirect {
            estimate: 20_000_000_000,
            budget: 16_000_000_000,
        };
        let n = qm_memory_notification(&can, "this machine").expect("should warn");
        assert_eq!(n.buttons.len(), 2);
        assert!(matches!(
            n.buttons[0].action,
            AppAction::StartQmWithDirectBackend
        ));
        assert!(n.buttons[0].primary);

        let must = MemoryVerdict::ExceedsMustReduce {
            estimate: 20_000_000_000,
            budget: 16_000_000_000,
        };
        let n = qm_memory_notification(&must, "this machine").expect("should warn");
        assert!(
            !n.buttons
                .iter()
                .any(|b| matches!(b.action, AppAction::StartQmWithDirectBackend))
        );

        assert!(qm_memory_notification(&MemoryVerdict::Ok, "this machine").is_none());
    }

    #[test]
    fn start_with_direct_flips_backend_and_reruns() {
        let mut state = AppState::scratch(Default::default(), Vec::new());
        let mut prompt = crate::frontend::state::QmPrompt::default();
        prompt.options.scf_backend = crate::engines::qm::QmScfBackend::InCore;
        state.ui.pending_qm = Some(prompt);
        start_qm_with_direct_backend(&mut state);
        // The handler flips the backend before re-running; with no atoms the
        // re-run no-ops, but the backend choice must have changed.
        // (pending_qm is cleared on a successful spawn; with an empty structure
        // start_pending_qm returns early, leaving pending_qm intact.)
        assert_eq!(
            state.ui.pending_qm.as_ref().unwrap().options.scf_backend,
            crate::engines::qm::QmScfBackend::Direct
        );
    }
}
