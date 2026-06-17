use std::path::PathBuf;

use anyhow::Result;
use rusqlite::Connection;

use crate::backend::tasks::{TaskManager, TaskRun, TaskStatus, task_controller_by_id};

pub(crate) fn load_tasks(db: &Connection) -> Result<TaskManager> {
    let mut manager = TaskManager::default();
    let mut statement = db.prepare(
        "select
            id,
            run_uuid,
            controller_id,
            status,
            run_dir,
            source_entry_id,
            result_entry_id,
            engine_label,
            created_at_ms,
            finished_at_ms
         from task_runs
         order by id",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)? as u64,
            row.get::<_, Option<String>>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, Option<String>>(4)?,
            row.get::<_, Option<i64>>(5)?,
            row.get::<_, Option<i64>>(6)?,
            row.get::<_, Option<String>>(7)?,
            row.get::<_, i64>(8)? as u64,
            row.get::<_, Option<i64>>(9)?,
        ))
    })?;
    for row in rows {
        let (
            id,
            run_uuid,
            controller_id,
            status,
            run_dir,
            source_entry_id,
            result_entry_id,
            engine_label,
            created_at_ms,
            finished_at_ms,
        ) = row?;
        let Some(controller) = task_controller_by_id(&controller_id).copied() else {
            continue;
        };
        let mut run = TaskRun::from_controller(id, controller);
        // Preserve the persisted UUID; rows written before this column existed
        // keep the freshly generated one.
        if let Some(run_uuid) = run_uuid {
            run.run_uuid = run_uuid;
        }
        run.status = parse_task_status(&status);
        run.run_dir = run_dir.map(PathBuf::from);
        run.source_entry_id = source_entry_id.map(|value| value as u64);
        run.result_entry_id = result_entry_id.map(|value| value as u64);
        run.engine_label = engine_label;
        run.created_at_ms = created_at_ms;
        run.finished_at_ms = finished_at_ms.map(|value| value as u64);
        manager.tasks.push(run);
        manager.next_task_run_id = manager.next_task_run_id.max(id + 1);
    }
    Ok(manager)
}

pub(crate) fn task_status_token(status: TaskStatus) -> &'static str {
    match status {
        TaskStatus::Ready => "ready",
        TaskStatus::WaitingInput => "waiting_input",
        TaskStatus::Running => "running",
        TaskStatus::Completed => "completed",
        TaskStatus::Failed => "failed",
    }
}

fn parse_task_status(token: &str) -> TaskStatus {
    match token {
        "waiting_input" => TaskStatus::WaitingInput,
        "running" => TaskStatus::Running,
        "completed" => TaskStatus::Completed,
        "failed" => TaskStatus::Failed,
        _ => TaskStatus::Ready,
    }
}
