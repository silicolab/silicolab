//! CLI-side Task plumbing for the `md` commands: create a task run, materialize
//! its run directory, sync its manifest, and mark terminal status. These mirror
//! the GUI's Task lifecycle so headless `md` runs show up in the same machinery.

use std::path::PathBuf;

use anyhow::{Result, anyhow, bail};

use crate::{
    backend::{
        runs::ensure_run_dir,
        tasks::{TaskStatus, task_controller_by_id},
    },
    frontend::state::AppState,
};

pub fn create_cli_task_run(state: &mut AppState, template_id: &'static str) -> Result<u64> {
    let controller = task_controller_by_id(template_id)
        .copied()
        .ok_or_else(|| anyhow!("unknown task template `{template_id}`"))?;
    let task_run_id = state.tasks.create_task_run(controller);
    state
        .tasks
        .set_source_entry_id(task_run_id, state.entries.active_entry_id());
    sync_cli_task_manifest(state, task_run_id)?;
    Ok(task_run_id)
}

pub fn ensure_cli_task_run_dir(state: &mut AppState, task_run_id: u64) -> Result<PathBuf> {
    let task = state
        .tasks
        .task_run(task_run_id)
        .ok_or_else(|| anyhow!("task run #{task_run_id} not found"))?
        .clone();
    if let Some(run_dir) = task.run_dir {
        return Ok(run_dir);
    }
    if !task.uses_run_directory {
        bail!("task {} does not use a run directory", task.title);
    }
    let runs_dir = state.runs_dir();
    let name = crate::backend::runs::default_run_name(&runs_dir, task.controller_id);
    let run_dir = ensure_run_dir(&runs_dir, &name)?;
    state.tasks.set_run_dir(task_run_id, run_dir.clone());
    sync_cli_task_manifest(state, task_run_id)?;
    Ok(run_dir)
}

pub fn sync_cli_task_manifest(state: &AppState, task_run_id: u64) -> Result<()> {
    super::super::task_executor::sync_task_manifest(state, task_run_id)
}

pub fn mark_cli_task_status(
    state: &mut AppState,
    task_run_id: u64,
    status: TaskStatus,
) -> Result<()> {
    super::super::task_executor::mark_task_status(state, task_run_id, status)
}

pub fn record_cli_task_result_entry(
    state: &mut AppState,
    task_run_id: u64,
    entry_id: u64,
) -> Result<()> {
    super::super::task_executor::record_task_result_entry(state, task_run_id, entry_id)
}

pub fn finish_cli_task(
    state: &mut AppState,
    task_run_id: u64,
    result: Result<String>,
) -> Result<String> {
    match result {
        Ok(message) => {
            mark_cli_task_status(state, task_run_id, TaskStatus::Completed)?;
            Ok(message)
        }
        Err(error) => {
            mark_cli_task_status(state, task_run_id, TaskStatus::Failed)?;
            Err(error)
        }
    }
}
