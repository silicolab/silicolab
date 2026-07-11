use super::*;

pub(crate) fn create_task_from_template(state: &mut AppState, template_id: &'static str) {
    let Some(controller) = task_controller_by_id(template_id).copied() else {
        state.set_message(format!("Unknown task: {template_id}"));
        return;
    };

    let task_run_id = state.tasks.create_task_run(controller);
    state.ui.layout.active_primary_view = crate::frontend::state::PrimaryView::Tasks;
    state.ui.layout.show_primary_sidebar = true;
    if controller.requires_panel() {
        state.tasks.open_panel(task_run_id);
        // Task panel placement is session state; only the default preference is
        // persisted.
        state
            .ui
            .layout
            .dock
            .add_task(task_run_id, state.config.default_task_panel_placement);
    }
    state.set_message(format!(
        "Opened task #{}: {}",
        task_run_id, controller.title
    ));
    run_task(state, task_run_id);
}

pub(crate) fn run_task(state: &mut AppState, task_run_id: u64) {
    let Some(task) = state.tasks.task_run(task_run_id).cloned() else {
        state.set_message(format!("Task #{task_run_id} not found"));
        return;
    };
    // Direct (non-panel) tasks act on the active structure immediately, so they
    // require an entry up front. Interactive panel tasks only open their
    // dashboard here; their preconditions are validated when the user triggers
    // the action, so they open even on an empty workspace.
    if task.panel == TaskPanelKind::None && !state.has_active_entry() {
        state.tasks.mark_status(task_run_id, TaskStatus::Failed);
        state.set_message("Open or create an entry before running tasks".to_string());
        return;
    }

    let Some(executor) = task_executor(task.kind) else {
        state.tasks.mark_status(task_run_id, TaskStatus::Failed);
        state.set_message(format!("No executor registered for task {}", task.title));
        return;
    };
    (executor.run)(state, task_run_id);
    state.ui.layout.active_primary_view = crate::frontend::state::PrimaryView::Tasks;
}

/// The calculation type a QM task opens its shared panel with.
pub(crate) fn qm_default_kind(kind: TaskKind) -> crate::engines::qm::QmKind {
    use crate::engines::qm::QmKind;
    match kind {
        TaskKind::RunQmOptimize => QmKind::Optimize,
        TaskKind::RunQmFrequencies => QmKind::Frequencies,
        TaskKind::RunQmTransitionState => QmKind::TransitionState,
        _ => QmKind::SinglePoint,
    }
}

/// Mark whichever QM task is active as finished. The three QM tasks share one
/// panel, so we complete by trying each kind (only the active one matches).
pub(crate) fn complete_active_qm_task(state: &mut AppState, status: TaskStatus) {
    complete_active_task(state, TaskKind::RunQmEnergy, status);
    complete_active_task(state, TaskKind::RunQmOptimize, status);
    complete_active_task(state, TaskKind::RunQmFrequencies, status);
    complete_active_task(state, TaskKind::RunQmTransitionState, status);
}

pub(crate) fn complete_active_task(state: &mut AppState, kind: TaskKind, status: TaskStatus) {
    let Some(task_run_id) = state.active_task_run else {
        return;
    };
    let matches_kind = state
        .tasks
        .task_run(task_run_id)
        .map(|task| task.kind == kind)
        .unwrap_or(false);
    if matches_kind {
        mark_task_status(state, task_run_id, status);
        state.active_task_run = None;
    }
}

pub(crate) fn sync_task_manifest(state: &mut AppState, task_run_id: u64) {
    if let Err(error) = crate::frontend::task_executor::sync_task_manifest(state, task_run_id) {
        state.set_message(format!("failed to write run manifest: {error}"));
    }
}

pub(crate) fn mark_task_status(state: &mut AppState, task_run_id: u64, status: TaskStatus) {
    if let Err(error) = crate::frontend::task_executor::mark_task_status(state, task_run_id, status)
    {
        state.set_message(format!("failed to update task status: {error}"));
    }
}

pub(crate) fn ensure_active_task_run_dir(
    state: &mut AppState,
    kind: TaskKind,
    desired_name: Option<&str>,
) -> anyhow::Result<PathBuf> {
    let task_run_id = state
        .active_task_run
        .ok_or_else(|| anyhow!("no active task run"))?;
    ensure_task_run_dir(state, task_run_id, kind, desired_name)
}

/// Create (once) the run directory of a specific task run and record the entry it
/// was launched from. Both are written only on first creation, so the run stays
/// anchored to the structure that was actually computed even if the user
/// activates a different entry while it is still running.
pub(crate) fn ensure_task_run_dir(
    state: &mut AppState,
    task_run_id: u64,
    kind: TaskKind,
    desired_name: Option<&str>,
) -> anyhow::Result<PathBuf> {
    let task = state
        .tasks
        .task_run(task_run_id)
        .ok_or_else(|| anyhow!("task run #{task_run_id} not found"))?
        .clone();
    if task.kind != kind {
        bail!("task run #{task_run_id} is not {kind:?}");
    }
    if let Some(run_dir) = task.run_dir {
        return Ok(run_dir);
    }
    if !task.uses_run_directory {
        bail!("task {} does not use a run directory", task.title);
    }
    // Use the user-chosen run name when supplied (and non-empty), otherwise fall
    // back to the suggested `{controller}-N`. The directory name is purely
    // human-facing; the task's durable identity is its UUID.
    let runs_dir = state.runs_dir();
    let name = desired_name
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| crate::backend::runs::default_run_name(&runs_dir, task.controller_id));
    let run_dir = ensure_run_dir(&runs_dir, &name)?;
    state.tasks.set_run_dir(task_run_id, run_dir.clone());
    state
        .tasks
        .set_source_entry_id(task_run_id, state.entries.active_entry_id());
    sync_task_manifest(state, task_run_id);
    Ok(run_dir)
}

pub(crate) fn record_task_result_entry(state: &mut AppState, task_run_id: u64, entry_id: u64) {
    if let Err(error) =
        crate::frontend::task_executor::record_task_result_entry(state, task_run_id, entry_id)
    {
        state.set_message(format!("failed to record task result entry: {error}"));
    }
}

/// Whether this job's outcome has already been materialized into the open project.
/// The idempotency guard every engine outcome path checks before it creates result
/// entries, so a remote refresh, a cancel/refresh race, or open-project
/// compensation never re-imports a result it already applied.
pub(crate) fn outcome_already_materialized(state: &AppState, job_id: &str) -> bool {
    state.materializations.contains(job_id)
}

/// Begin a job execution for `task_run_id` and return its freshly minted [`JobId`],
/// persisting the new attempt/execution promptly (so a remote job's `JobId → task`
/// resolution survives a restart). Local jobs bind the id on their `JobManager`
/// slot; a remote submission carries it as the registry `job_id`.
pub(crate) fn begin_job_execution(
    state: &mut AppState,
    task_run_id: u64,
    placement: crate::backend::run_attempt::Placement,
    job_kind: Option<String>,
) -> crate::job::JobId {
    let now = crate::backend::storage::jobs::now_ms().max(0) as u64;
    let job_id = state
        .tasks
        .runs
        .begin_execution(task_run_id, placement, job_kind, now);
    flush_dirty_run_graph(state);
    job_id
}

/// Begin a LOCAL job execution for the active task and bind its `JobId` to `slot`,
/// so the slot's poller attributes completion through the run graph. The one call
/// every local launch site makes after handing its handle to the `JobManager`; the
/// job kind is the task's controller id.
pub(crate) fn begin_local_job(
    state: &mut AppState,
    slot: crate::frontend::jobs::LocalJobSlot,
    task_run_id: u64,
) {
    let job_kind = state
        .tasks
        .task_run(task_run_id)
        .map(|task| task.controller_id.to_string());
    let job_id = begin_job_execution(
        state,
        task_run_id,
        crate::backend::run_attempt::Placement::Local,
        job_kind,
    );
    state.jobs.bind_local_execution(slot, job_id);
}

/// Attribute a finished LOCAL job to its task through the run graph, marking the
/// execution and the task terminal. Resolves `JobId → RunAttempt → TaskRun`, so two
/// concurrent local jobs never cross attribution the way the ambient active run
/// did; the active run is cleared only when it is the completed one.
pub(crate) fn complete_local_job(
    state: &mut AppState,
    job_id: Option<crate::job::JobId>,
    task_status: TaskStatus,
) {
    let Some(job_id) = job_id else {
        return;
    };
    let key = job_id.to_string();
    let now = crate::backend::storage::jobs::now_ms().max(0) as u64;
    state
        .tasks
        .runs
        .set_execution_state(&key, execution_state_for(task_status), now);
    if let Some(task_run_id) = state.tasks.runs.task_run_id_for_job(&key) {
        mark_task_status(state, task_run_id, task_status);
        if state.active_task_run == Some(task_run_id) {
            state.active_task_run = None;
        }
    }
}

/// Map a task's terminal (or transitional) status to the execution state recorded
/// on its job, keeping the two projections consistent.
fn execution_state_for(status: TaskStatus) -> crate::job::ExecutionState {
    use crate::job::ExecutionState;
    match status {
        TaskStatus::Completed => ExecutionState::Succeeded,
        TaskStatus::Failed => ExecutionState::Failed,
        TaskStatus::Cancelled => ExecutionState::Cancelled,
        TaskStatus::Interrupted => ExecutionState::Interrupted,
        TaskStatus::Cancelling => ExecutionState::Cancelling,
        TaskStatus::Ready | TaskStatus::WaitingInput | TaskStatus::Running => {
            ExecutionState::Running
        }
    }
}

/// Record that `job_id`'s outcome produced `entry_ids` (in application order), with
/// `primary_entry_id` opened by default and each entry labelled `role`. The record
/// is persisted in the atomic entries+ledger save. An empty `entry_ids` records a
/// report — proof the outcome was applied, so it is never re-imported.
pub(crate) fn record_materialization(
    state: &mut AppState,
    job_id: &str,
    role: &str,
    primary_entry_id: Option<u64>,
    entry_ids: &[u64],
) {
    let applied_at_ms = crate::backend::storage::jobs::now_ms().max(0) as u64;
    let entries = entry_ids
        .iter()
        .enumerate()
        .map(
            |(ordinal, &entry_id)| crate::backend::materialization::MaterializedEntry {
                ordinal: ordinal as u32,
                role: role.to_string(),
                entry_id,
            },
        )
        .collect();
    state
        .materializations
        .record(crate::backend::materialization::Materialization {
            job_id: job_id.to_string(),
            applied_at_ms,
            primary_entry_id,
            entries,
        });
    // The ledger is the idempotency authority; mark the execution's import state
    // Applied so the run graph carries the durable "imported" signal too (a remote
    // execution starts Pending). A no-op when the job is not in this project's graph.
    state
        .tasks
        .runs
        .set_import_state(job_id, crate::backend::run_attempt::ResultImport::Applied);
}

pub(crate) fn open_task_panel(state: &mut AppState, task_run_id: u64) {
    state.tasks.open_panel(task_run_id);
    state
        .ui
        .layout
        .dock
        .add_task(task_run_id, state.config.default_task_panel_placement);
    ensure_panel_form(state, task_run_id);
}

pub(crate) fn close_task_panel(state: &mut AppState, task_run_id: u64) {
    // The dominant caller is `close_active_task_panel` on task completion/cancel,
    // so this must stay cheap: drop the tab (the area auto-hides when it was the
    // last tab) and never touch the disk — task tabs are not persisted.
    state.tasks.close_panel(task_run_id);
    state.ui.layout.dock.remove_task(task_run_id);
}

pub(crate) fn activate_task_panel(state: &mut AppState, task_run_id: u64) {
    use crate::frontend::state::DockTab;
    state.tasks.activate_panel(task_run_id);
    if let Some(area) = state.ui.layout.dock.area_of(DockTab::Task(task_run_id)) {
        state
            .ui
            .layout
            .dock
            .activate(area, DockTab::Task(task_run_id));
    }
    ensure_panel_form(state, task_run_id);
}

/// Make a task's dashboard renderable on demand: initialize its form state if
/// it is not already present, so every panel shows its controls immediately
/// (whether freshly created, re-opened, or re-activated) without requiring a
/// run first. Preconditions are deferred to the action handlers, which validate
/// when the user actually triggers the work.
pub(crate) fn ensure_panel_form(state: &mut AppState, task_run_id: u64) {
    let Some(task) = state.tasks.task_run(task_run_id).cloned() else {
        return;
    };
    match task.panel {
        TaskPanelKind::OptimizationPrompt => {
            let allow_cell = task.kind == TaskKind::OptimizeCrystalGeometry;
            // Re-init when absent, or when switching between the geometry and
            // crystal tasks that share this panel (they differ by cell scope).
            let stale = state
                .ui
                .pending_optimization
                .as_ref()
                .map(|prompt| prompt.allow_cell_optimization != allow_cell)
                .unwrap_or(true);
            if stale {
                state.ui.pending_optimization =
                    Some(crate::frontend::state::OptimizationPrompt::new(
                        allow_cell,
                        &state.ui.selection,
                    ));
            }
        }
        TaskPanelKind::QmPrompt => {
            // Each QM task opens this shared panel with its own default
            // calculation type. Re-default only when the task type differs from
            // the one the current form was opened for, so an entry switch (which
            // re-runs this) keeps any choice the user already made.
            let default_kind = qm_default_kind(task.kind);
            let stale = state
                .ui
                .pending_qm
                .as_ref()
                .map(|prompt| prompt.default_kind != default_kind)
                .unwrap_or(true);
            if stale {
                let mut prompt = crate::frontend::state::QmPrompt::new(default_kind);
                prompt.prefs = crate::frontend::state::ExecutionPrefs::seeded(&state.config);
                state.ui.pending_qm = Some(prompt);
            }
        }
        TaskPanelKind::SupercellPrompt => {
            state
                .ui
                .pending_supercell
                .get_or_insert_with(Default::default);
        }
        TaskPanelKind::ProteinPrepPrompt => {
            state
                .ui
                .pending_protein_prep
                .get_or_insert_with(Default::default);
        }
        TaskPanelKind::DisorderedSystemPrompt => {
            let active_entry = state.entries.active_entry_id();
            let prompt = state
                .ui
                .pending_disorder
                .get_or_insert_with(Default::default);
            // Seed the first molecule from the active entry on first open.
            if prompt.components.is_empty()
                && let Some(entry_id) = active_entry
            {
                prompt
                    .components
                    .push(crate::frontend::state::DisorderComponentDraft {
                        entry_id,
                        ..Default::default()
                    });
            }
        }
        TaskPanelKind::MdSystemPrompt => {
            let default_name =
                crate::backend::runs::default_run_name(&state.runs_dir(), task.controller_id);
            // On first open, default the force field to the best fit for the
            // structure's content (protein/nucleic vs. crystal/small molecule).
            if state.ui.pending_md_system.is_none() {
                let force_field = crate::workflows::molecular_dynamics::recommended_force_field(
                    state.structure(),
                )
                .to_string();
                state.ui.pending_md_system = Some(crate::frontend::state::MdSystemPrompt {
                    force_field,
                    prefs: crate::frontend::state::ExecutionPrefs::seeded(&state.config),
                    ..Default::default()
                });
            }
            // A periodic framework keeps its crystal cell as the MD box; seed the
            // editable lattice parameters from it, opening the out-of-plane axis to
            // a cutoff-safe floor so the default just runs. The in-plane lattice is
            // taken verbatim — it defines how the sheet tiles across the boundary.
            let framework_cell =
                crate::workflows::molecular_dynamics::is_framework(state.structure())
                    .then(|| {
                        state.structure().cell.as_ref().map(|cell| {
                            let c = cell.c.max(FRAMEWORK_C_FLOOR_ANGSTROM);
                            [cell.a, cell.b, c, cell.alpha, cell.beta, cell.gamma]
                        })
                    })
                    .flatten();
            let prompt = state
                .ui
                .pending_md_system
                .get_or_insert_with(Default::default);
            if prompt.run_name.trim().is_empty() {
                prompt.run_name = default_name;
            }
            if prompt.framework_cell.is_none() {
                prompt.framework_cell = framework_cell;
            }
        }
        TaskPanelKind::MdRunPrompt => {
            let default_name =
                crate::backend::runs::default_run_name(&state.runs_dir(), task.controller_id);
            let default_prefs = crate::frontend::state::ExecutionPrefs::seeded(&state.config);
            // Load the inherited build-time context (or derive a minimal one) and
            // run the recommendation once, before borrowing the prompt mutably.
            let needs_init = state
                .ui
                .pending_md_run
                .as_ref()
                .is_none_or(|prompt| prompt.context.is_none());
            let context = needs_init.then(|| load_or_derive_md_context(state));

            let prompt = state.ui.pending_md_run.get_or_insert_with(Default::default);
            if prompt.run_name.trim().is_empty() {
                prompt.run_name = default_name;
            }
            if needs_init {
                prompt.prefs = default_prefs;
            }
            if let Some(context) = context {
                let recommendation = crate::workflows::molecular_dynamics::run::recommend(
                    &context.with_overrides(prompt.overrides),
                );
                prompt.preset = recommendation.preset;
                prompt.params = recommendation.prefill;
                prompt.context = Some(context);
                prompt.rebuild_stages();
            }
        }
        TaskPanelKind::ReticularBuilder => {
            if state.ui.reticular_builder.is_none() {
                if state.builder_origin.is_none() && state.has_active_entry() {
                    state.builder_origin = Some(state.capture_edit_snapshot());
                }
                let panel = crate::frontend::ReticularBuilderPanel::new(state.structure());
                state.ui.reticular_builder = Some(panel);
            }
        }
        TaskPanelKind::NanosheetBuilder => {
            if state.ui.nanosheet_builder.is_none() {
                if state.builder_origin.is_none() && state.has_active_entry() {
                    state.builder_origin = Some(state.capture_edit_snapshot());
                }
                let panel = crate::frontend::NanosheetBuilderPanel::new(state.structure());
                state.ui.nanosheet_builder = Some(panel);
            }
        }
        TaskPanelKind::BuildingBlockEditor => {
            if state.ui.block_editor.is_none() {
                let editor = crate::frontend::BuildingBlockEditor::new(state.structure());
                state.ui.block_editor = Some(editor);
            }
        }
        TaskPanelKind::DockingPrompt => {
            let active_entry = state.entries.active_entry_id();
            if state.ui.pending_docking.is_none() {
                let mut prompt = crate::frontend::state::DockingPrompt::default();
                // Seed the receptor and search box from the active entry so the
                // box starts centered on something the user can see.
                if let Some(entry_id) = active_entry {
                    prompt.receptor_entry = Some(entry_id);
                    let center = state.structure().center();
                    prompt.box_center = [center.x, center.y, center.z];
                }
                prompt.prefs = crate::frontend::state::ExecutionPrefs::seeded(&state.config);
                state.ui.pending_docking = Some(prompt);
            }
        }
        TaskPanelKind::PtmPrompt => {
            state.ui.pending_ptm.get_or_insert_with(Default::default);
        }
        TaskPanelKind::None => {}
    }
}

/// Point `active_task_run` at the active panel's task (matching `panel`) when no
/// run is currently bound. Lets action handlers report task status correctly
/// even when the dashboard was opened directly rather than via "Run".
pub(crate) fn bind_active_panel_task(state: &mut AppState, panel: TaskPanelKind) {
    if state.active_task_run.is_some() {
        return;
    }
    if let Some(task_run_id) = state.tasks.active_panel {
        let matches = state
            .tasks
            .task_run(task_run_id)
            .map(|task| task.panel == panel)
            .unwrap_or(false);
        if matches {
            state.active_task_run = Some(task_run_id);
        }
    }
}

pub(crate) fn close_active_task_panel(state: &mut AppState) {
    if let Some(task_run_id) = state.tasks.active_panel {
        close_task_panel(state, task_run_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::run_attempt::Placement;
    use crate::backend::tasks::task_controller_by_id;

    #[test]
    fn concurrent_local_jobs_attribute_by_job_id_not_active_run() {
        // attribution/concurrency: two different-kind local jobs run at once;
        // each completes its own task through `JobId → RunAttempt → TaskRun`, never
        // through the ambient active run (which points at the last-launched task —
        // the condition that would otherwise cross attribution).
        let mut state = AppState::scratch(Default::default(), Vec::new());
        let qm = state
            .tasks
            .create_task_run(*task_controller_by_id("qm-energy").unwrap());
        let optimize = state
            .tasks
            .create_task_run(*task_controller_by_id("optimize-geometry").unwrap());
        state.tasks.mark_status(qm, TaskStatus::Running);
        state.tasks.mark_status(optimize, TaskStatus::Running);

        let qm_job = state
            .tasks
            .runs
            .begin_execution(qm, Placement::Local, None, 0);
        let _optimize_job = state
            .tasks
            .runs
            .begin_execution(optimize, Placement::Local, None, 0);
        state.active_task_run = Some(optimize);

        complete_local_job(&mut state, Some(qm_job), TaskStatus::Completed);

        assert_eq!(
            state.tasks.task_run(qm).unwrap().status,
            TaskStatus::Completed,
            "the QM job completes its own task"
        );
        assert_eq!(
            state.tasks.task_run(optimize).unwrap().status,
            TaskStatus::Running,
            "the concurrent optimization task is untouched"
        );
        assert_eq!(
            state.active_task_run,
            Some(optimize),
            "the active run was not the completed job, so it is left intact"
        );
    }
}
