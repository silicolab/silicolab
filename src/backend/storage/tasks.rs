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
                created_at_ms, finished_at_ms, inputs_json
            ) values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
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
                task.inputs
                    .as_ref()
                    .map(serde_json::to_string)
                    .transpose()?,
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
            finished_at_ms, inputs_json
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
            row.get::<_, Option<String>>(10)?,
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
            inputs_json,
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
        run.inputs = inputs_json
            .as_deref()
            .map(serde_json::from_str)
            .transpose()?;
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

#[cfg(test)]
mod provenance_tests {
    use super::*;
    use crate::backend::tasks::TaskInput;

    #[test]
    fn old_schema_migration_and_incremental_input_roundtrip() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("create table task_runs (id integer primary key, controller_id text not null, status text not null); insert into task_runs values (1, 'qm-energy', 'completed');").unwrap();
        crate::backend::storage::create_project_schema(&db).unwrap();
        let old = load_tasks(&db).unwrap();
        assert!(old.task_run(1).unwrap().inputs.is_none());
        let mut run = TaskRun::from_controller(2, *task_controller_by_id("dock-ligand").unwrap());
        run.source_entry_id = Some(9);
        run.inputs = Some(vec![
            TaskInput {
                role: "receptor".into(),
                entry_id: 7,
                revision: 3,
            },
            TaskInput {
                role: "ligand".into(),
                entry_id: 9,
                revision: 5,
            },
        ]);
        upsert_task_runs(&db, &[&run]).unwrap();
        let loaded = load_tasks(&db).unwrap();
        assert_eq!(loaded.task_run(2).unwrap().inputs, run.inputs);
        assert_eq!(loaded.task_run(2).unwrap().source_entry_id, Some(9));
        assert!(loaded.task_run(1).unwrap().inputs.is_none());
    }

    #[test]
    fn full_project_save_preserves_inputs_and_manifest() {
        use crate::backend::{
            entries::EntryStore, history::History, project::ProjectSession, storage::*,
        };
        let root =
            std::env::temp_dir().join(format!("silicolab-provenance-{}", uuid::Uuid::new_v4()));
        let session = ProjectSession::from_root(root.clone(), "Inputs".into());
        initialize_project_databases(&session).unwrap();
        let mut tasks = TaskManager::default();
        let id = tasks.create_task_run(*task_controller_by_id("qm-transition-state").unwrap());
        let task = tasks.task_run_mut(id).unwrap();
        task.source_entry_id = Some(2);
        task.inputs = Some(vec![
            TaskInput {
                role: "primary".into(),
                entry_id: 2,
                revision: 8,
            },
            TaskInput {
                role: "product".into(),
                entry_id: 4,
                revision: 12,
            },
        ]);
        task.run_dir = Some(root.join("runs/ts"));
        crate::backend::runs::write_manifest(task).unwrap();
        let expected = task.inputs.clone();
        let manifest: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(root.join("runs/ts/manifest.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(manifest["inputs"], serde_json::to_value(&expected).unwrap());
        let snapshot = ProjectSnapshot {
            name: "Inputs".into(),
            project_id: String::new(),
            entries: EntryStore::new_empty(),
            tasks,
            materializations: Default::default(),
            view: Default::default(),
            history: History::default(),
            assistant: Default::default(),
            warnings: Vec::new(),
        };
        save_project_snapshot(&session, &snapshot, true).unwrap();
        let loaded = load_project_snapshot(&session).unwrap();
        assert_eq!(loaded.tasks.task_run(id).unwrap().inputs, expected);
        assert_eq!(loaded.tasks.task_run(id).unwrap().source_entry_id, Some(2));
        std::fs::remove_dir_all(root).unwrap();
    }
}
