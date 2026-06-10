use crate::{
    backend::{
        runs::write_manifest,
        tasks::{TaskKind, TaskStatus},
    },
    frontend::{dispatcher, state::AppState},
};

type TaskRunFn = fn(&mut AppState, u64);

pub(super) struct TaskExecutor {
    pub kind: TaskKind,
    pub run: TaskRunFn,
}

pub(crate) fn sync_task_manifest(state: &AppState, task_run_id: u64) -> anyhow::Result<()> {
    let Some(task) = state.tasks.task_run(task_run_id) else {
        return Ok(());
    };
    if !task.uses_run_directory || task.run_dir.is_none() {
        return Ok(());
    }
    write_manifest(task)
}

pub(crate) fn mark_task_status(
    state: &mut AppState,
    task_run_id: u64,
    status: TaskStatus,
) -> anyhow::Result<()> {
    state.tasks.mark_status(task_run_id, status);
    sync_task_manifest(state, task_run_id)
}

pub(crate) fn record_task_result_entry(
    state: &mut AppState,
    task_run_id: u64,
    entry_id: u64,
) -> anyhow::Result<()> {
    state.tasks.set_result_entry_id(task_run_id, Some(entry_id));
    sync_task_manifest(state, task_run_id)
}

const TASK_EXECUTORS: &[TaskExecutor] = &[
    TaskExecutor {
        kind: TaskKind::BuildReticularStructure,
        run: run_reticular_builder,
    },
    TaskExecutor {
        kind: TaskKind::BuildNanosheet,
        run: run_nanosheet_builder,
    },
    TaskExecutor {
        kind: TaskKind::CreateBuildingBlock,
        run: run_building_block_editor,
    },
    TaskExecutor {
        kind: TaskKind::OptimizeGeometry,
        run: run_geometry_optimization,
    },
    TaskExecutor {
        kind: TaskKind::OptimizeCrystalGeometry,
        run: run_crystal_optimization,
    },
    TaskExecutor {
        kind: TaskKind::RunQmEnergy,
        run: run_qm_panel,
    },
    TaskExecutor {
        kind: TaskKind::RunQmOptimize,
        run: run_qm_panel,
    },
    TaskExecutor {
        kind: TaskKind::RunQmFrequencies,
        run: run_qm_panel,
    },
    TaskExecutor {
        kind: TaskKind::TranslateIntoFirstUnitCell,
        run: run_translate_into_first_cell,
    },
    TaskExecutor {
        kind: TaskKind::ExpandSupercell,
        run: run_supercell_expansion,
    },
    TaskExecutor {
        kind: TaskKind::PrepareProtein,
        run: run_prepare_protein,
    },
    TaskExecutor {
        kind: TaskKind::BuildMdSystem,
        run: run_build_md_system,
    },
    TaskExecutor {
        kind: TaskKind::AddHydrogens,
        run: run_add_hydrogens,
    },
    TaskExecutor {
        kind: TaskKind::RecomputeBonds,
        run: run_recompute_bonds,
    },
    TaskExecutor {
        kind: TaskKind::RunMd,
        run: run_md,
    },
];

pub(super) fn task_executor(kind: TaskKind) -> Option<&'static TaskExecutor> {
    TASK_EXECUTORS.iter().find(|executor| executor.kind == kind)
}

fn run_reticular_builder(state: &mut AppState, task_run_id: u64) {
    wait_for_input(state, task_run_id);
    dispatcher::build_framework_task(state);
}

fn run_nanosheet_builder(state: &mut AppState, task_run_id: u64) {
    wait_for_input(state, task_run_id);
    dispatcher::build_nanosheet_task(state);
}

fn run_building_block_editor(state: &mut AppState, task_run_id: u64) {
    wait_for_input(state, task_run_id);
    dispatcher::build_block_from_current(state);
}

fn run_geometry_optimization(state: &mut AppState, task_run_id: u64) {
    open_panel_task(state, task_run_id);
}

fn run_crystal_optimization(state: &mut AppState, task_run_id: u64) {
    open_panel_task(state, task_run_id);
}

fn run_qm_panel(state: &mut AppState, task_run_id: u64) {
    open_panel_task(state, task_run_id);
}

fn run_translate_into_first_cell(state: &mut AppState, task_run_id: u64) {
    dispatcher::translate_atoms_into_first_unit_cell(state);
    state.tasks.mark_status(task_run_id, TaskStatus::Completed);
}

fn run_supercell_expansion(state: &mut AppState, task_run_id: u64) {
    open_panel_task(state, task_run_id);
}

fn run_prepare_protein(state: &mut AppState, task_run_id: u64) {
    open_panel_task(state, task_run_id);
}

fn run_build_md_system(state: &mut AppState, task_run_id: u64) {
    open_panel_task(state, task_run_id);
}

fn run_add_hydrogens(state: &mut AppState, task_run_id: u64) {
    dispatcher::add_hydrogens(state);
    state.tasks.mark_status(task_run_id, TaskStatus::Completed);
}

fn run_recompute_bonds(state: &mut AppState, task_run_id: u64) {
    dispatcher::recompute_bonds(state);
    state.tasks.mark_status(task_run_id, TaskStatus::Completed);
}

fn run_md(state: &mut AppState, task_run_id: u64) {
    open_panel_task(state, task_run_id);
}

/// Open an interactive task's dashboard: bind it as the active run, mark it as
/// waiting for input, and ensure its form is initialized. Preconditions are
/// checked later, when the user triggers the action from the panel.
fn open_panel_task(state: &mut AppState, task_run_id: u64) {
    wait_for_input(state, task_run_id);
    dispatcher::ensure_panel_form(state, task_run_id);
}

fn wait_for_input(state: &mut AppState, task_run_id: u64) {
    state.active_task_run = Some(task_run_id);
    state
        .tasks
        .mark_status(task_run_id, TaskStatus::WaitingInput);
}

#[cfg(test)]
mod tests {
    use crate::backend::tasks::task_controllers;

    #[test]
    fn every_task_controller_has_frontend_executor() {
        for controller in task_controllers() {
            assert!(
                super::task_executor(controller.kind).is_some(),
                "missing executor for {}",
                controller.id
            );
        }
    }
}
