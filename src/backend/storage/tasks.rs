use std::path::PathBuf;

use anyhow::Result;
use rusqlite::{Connection, params};

use crate::backend::tasks::{TaskManager, TaskRun, TaskStatus, task_controller_by_id};

/// Write specific task rows to `project.db` without rewriting the whole table —
/// the narrow persist that lets a status change reach disk immediately. Keyed by
/// `id`, so it inserts a new row or replaces an existing one.
pub(crate) fn upsert_task_runs(conn: &Connection, tasks: &[&TaskRun]) -> Result<()> {
    for task in tasks {
        conn.execute(
            "insert or replace into task_runs (
                id, run_uuid, controller_id, status, run_dir,
                source_entry_id, result_entry_id, engine_label,
                created_at_ms, finished_at_ms
            ) values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                task.id as i64,
                task.run_uuid,
                task.controller_id,
                task_status_token(task.status),
                task.run_dir
                    .as_ref()
                    .map(|dir| dir.to_string_lossy().to_string()),
                task.source_entry_id.map(|value| value as i64),
                task.result_entry_id.map(|value| value as i64),
                task.engine_label.as_deref(),
                task.created_at_ms as i64,
                task.finished_at_ms.map(|value| value as i64),
            ],
        )?;
    }
    Ok(())
}

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
        TaskStatus::Cancelling => "cancelling",
        TaskStatus::Completed => "completed",
        TaskStatus::Failed => "failed",
        TaskStatus::Cancelled => "cancelled",
        TaskStatus::Interrupted => "interrupted",
    }
}

fn parse_task_status(token: &str) -> TaskStatus {
    match token {
        "waiting_input" => TaskStatus::WaitingInput,
        "running" => TaskStatus::Running,
        "cancelling" => TaskStatus::Cancelling,
        "completed" => TaskStatus::Completed,
        "failed" => TaskStatus::Failed,
        "cancelled" => TaskStatus::Cancelled,
        "interrupted" => TaskStatus::Interrupted,
        _ => TaskStatus::Ready,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::storage::create_project_schema;
    use crate::backend::tasks::TaskRun;

    #[test]
    fn interrupted_status_round_trips_through_upsert_and_load() {
        let conn = Connection::open_in_memory().unwrap();
        create_project_schema(&conn).unwrap();
        let controller = task_controller_by_id("qm-energy").copied().unwrap();

        let mut run = TaskRun::from_controller(7, controller);
        run.status = TaskStatus::Running;
        upsert_task_runs(&conn, &[&run]).unwrap();
        assert_eq!(
            load_tasks(&conn).unwrap().task_run(7).unwrap().status,
            TaskStatus::Running
        );

        // The narrow persist overwrites the single row in place.
        run.status = TaskStatus::Interrupted;
        upsert_task_runs(&conn, &[&run]).unwrap();
        let loaded = load_tasks(&conn).unwrap();
        let task = loaded.task_run(7).unwrap();
        assert_eq!(task.status, TaskStatus::Interrupted);
        assert_eq!(task.controller_id, "qm-energy");
    }
}
