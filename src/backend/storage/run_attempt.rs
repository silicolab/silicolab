use std::str::FromStr;

use anyhow::Result;
use rusqlite::{Connection, params};

use crate::backend::run_attempt::{JobExecution, Placement, ResultImport, RunAttempt, RunGraph};
use crate::job::{ExecutionState, JobId};

pub(crate) fn load_run_graph(db: &Connection) -> Result<RunGraph> {
    let mut attempt_stmt = db.prepare(
        "select run_attempt_id, task_run_id, attempt_no, created_at_ms, finished_at_ms
         from run_attempts
         order by run_attempt_id",
    )?;
    let attempts = attempt_stmt
        .query_map([], |row| {
            Ok(RunAttempt {
                run_attempt_id: row.get::<_, i64>(0)? as u64,
                task_run_id: row.get::<_, i64>(1)? as u64,
                attempt_no: row.get::<_, i64>(2)? as u32,
                created_at_ms: row.get::<_, i64>(3)? as u64,
                finished_at_ms: row.get::<_, Option<i64>>(4)?.map(|value| value as u64),
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let mut execution_stmt = db.prepare(
        "select job_id, run_attempt_id, ordinal, placement, placement_host, job_kind,
                execution_state, import_state, created_at_ms, finished_at_ms, qm_result_json
         from job_executions
         order by run_attempt_id, ordinal",
    )?;
    let rows = execution_stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)? as u64,
                row.get::<_, i64>(2)? as u32,
                row.get::<_, String>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, i64>(8)? as u64,
                row.get::<_, Option<i64>>(9)?.map(|value| value as u64),
                row.get::<_, Option<String>>(10)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let mut executions = Vec::with_capacity(rows.len());
    let mut unavailable_qm_results = std::collections::BTreeMap::new();
    for (
        job_id,
        run_attempt_id,
        ordinal,
        placement,
        placement_host,
        job_kind,
        execution_state,
        import_state,
        created_at_ms,
        finished_at_ms,
        qm_result_json,
    ) in rows
    {
        // A row with an unparseable id or state predates or corrupts this schema;
        // skip it rather than fail the whole project open.
        let (Ok(job_id), Some(execution_state)) = (
            JobId::from_str(&job_id),
            ExecutionState::from_token(&execution_state),
        ) else {
            continue;
        };
        let qm_result = match qm_result_json {
            Some(json) => match serde_json::from_str(&json) {
                Ok(result) => Some(result),
                Err(_) => {
                    unavailable_qm_results.insert(job_id.to_string(), json);
                    None
                }
            },
            None => None,
        };
        executions.push(JobExecution {
            job_id,
            run_attempt_id,
            ordinal,
            placement: Placement::from_parts(&placement, placement_host),
            job_kind,
            execution_state,
            import_state: ResultImport::from_token(&import_state)
                .unwrap_or(ResultImport::NotRequired),
            created_at_ms,
            finished_at_ms,
            qm_result,
        });
    }

    let mut graph = RunGraph::from_rows(attempts, executions);
    graph.unavailable_qm_results = unavailable_qm_results;
    let mut stmt = db.prepare("select id, envelope_json from knowledge_records order by id")?;
    for row in stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
        let (id, json) = row?;
        graph.records.restore(id, json);
    }
    Ok(graph)
}

/// Rewrite the attempt/execution rows inside the caller's transaction. The tables
/// are tiny (one row per attempt/job), so a full rewrite is cheap; children are
/// deleted before parents and re-inserted parent-first to satisfy the foreign key.
pub(crate) fn write_run_graph(conn: &Connection, runs: &RunGraph) -> Result<()> {
    conn.execute("delete from knowledge_records", [])?;
    for record in runs.records.all() {
        conn.execute(
            "insert into knowledge_records values (?1, ?2)",
            params![record.id, serde_json::to_string(record)?],
        )?;
    }
    for (id, (json, _)) in &runs.records.unavailable {
        conn.execute(
            "insert into knowledge_records values (?1, ?2)",
            params![id, json],
        )?;
    }
    conn.execute("delete from job_executions", [])?;
    conn.execute("delete from run_attempts", [])?;
    for attempt in runs.attempts() {
        conn.execute(
            "insert into run_attempts
                (run_attempt_id, task_run_id, attempt_no, created_at_ms, finished_at_ms)
             values (?1, ?2, ?3, ?4, ?5)",
            params![
                attempt.run_attempt_id as i64,
                attempt.task_run_id as i64,
                attempt.attempt_no as i64,
                attempt.created_at_ms as i64,
                attempt.finished_at_ms.map(|value| value as i64),
            ],
        )?;
    }
    for execution in runs.executions() {
        conn.execute(
            "insert into job_executions
                (job_id, run_attempt_id, ordinal, placement, placement_host, job_kind,
                 execution_state, import_state, created_at_ms, finished_at_ms, qm_result_json)
             values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                execution.job_id.to_string(),
                execution.run_attempt_id as i64,
                execution.ordinal as i64,
                execution.placement.token(),
                execution.placement.host(),
                execution.job_kind.as_deref(),
                execution.execution_state.token(),
                execution.import_state.token(),
                execution.created_at_ms as i64,
                execution.finished_at_ms.map(|value| value as i64),
                execution
                    .qm_result
                    .as_ref()
                    .map(serde_json::to_string)
                    .transpose()?
                    .or_else(|| runs
                        .unavailable_qm_results
                        .get(&execution.job_id.to_string())
                        .cloned()),
            ],
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::storage::create_project_schema;

    #[test]
    fn damaged_qm_status_is_local_and_preserved_on_save() {
        let db = Connection::open_in_memory().unwrap();
        create_project_schema(&db).unwrap();
        let mut graph = RunGraph::default();
        let job = graph.begin_execution(1, Placement::Local, None, 1);
        write_run_graph(&db, &graph).unwrap();
        db.execute(
            "update job_executions set qm_result_json = 'future status'",
            [],
        )
        .unwrap();
        let loaded = load_run_graph(&db).unwrap();
        assert!(
            loaded
                .execution(&job.to_string())
                .unwrap()
                .qm_result
                .is_none()
        );
        assert_eq!(
            loaded.unavailable_qm_results[&job.to_string()],
            "future status"
        );
        write_run_graph(&db, &loaded).unwrap();
        assert_eq!(
            db.query_row("select qm_result_json from job_executions", [], |r| r
                .get::<_, String>(0))
                .unwrap(),
            "future status"
        );
    }

    #[test]
    fn record_rows_survive_job_rewrites_and_transaction_rollback() {
        use crate::backend::records::{Category, Content, Record, Scope, Source};
        let mut db = Connection::open_in_memory().unwrap();
        create_project_schema(&db).unwrap();
        db.pragma_update(None, "foreign_keys", true).unwrap();
        let mut graph = RunGraph::default();
        let job = graph.begin_execution(1, Placement::Local, None, 1);
        graph
            .records
            .insert(Record {
                id: "constraint:test".into(),
                category: Category::Memory,
                storage_version: 1,
                content_version: 1,
                revision: 1,
                source: Source::UserApproval {
                    session: 1,
                    call: "approved-click".into(),
                },
                scope: Scope {
                    task: Some(1),
                    session: Some(1),
                    run_uuid: None,
                },
                created_at_ms: 1,
                supersedes: None,
                invalidated: None,
                brief: "Keep charge zero".into(),
                input_entries: vec![],
                result_entries: vec![],
                artifacts: vec![],
                content: Content::Constraint {
                    text: "Keep charge zero".into(),
                },
            })
            .unwrap();
        graph
            .records
            .restore("unknown".into(), "{future version}".into());
        for _ in 0..3 {
            let tx = db.transaction().unwrap();
            write_run_graph(&tx, &graph).unwrap();
            tx.commit().unwrap();
            graph = load_run_graph(&db).unwrap();
            assert!(graph.execution(&job.to_string()).is_some());
            assert!(
                graph
                    .records
                    .context(Some(1), None, 1, 1000)
                    .contains("Keep charge zero")
            );
            assert_eq!(graph.records.unavailable["unknown"].0, "{future version}");
        }
        let tx = db.transaction().unwrap();
        write_run_graph(&tx, &RunGraph::default()).unwrap();
        tx.rollback().unwrap();
        assert_eq!(load_run_graph(&db).unwrap().records.all().count(), 1);
    }

    #[test]
    fn legacy_database_gains_empty_record_catalog_idempotently() {
        let db = Connection::open_in_memory().unwrap();
        create_project_schema(&db).unwrap();
        db.execute("drop table knowledge_records", []).unwrap();
        create_project_schema(&db).unwrap();
        create_project_schema(&db).unwrap();
        assert_eq!(load_run_graph(&db).unwrap().records.all().count(), 0);
    }

    #[test]
    fn existing_job_executions_gain_nullable_qm_results_idempotently() {
        use crate::backend::run_attempt::{ArtifactStatus, QmResult};

        let conn = Connection::open_in_memory().unwrap();
        create_project_schema(&conn).unwrap();
        let mut graph = RunGraph::default();
        let job = graph.begin_execution(3, Placement::Local, Some("qm-energy".into()), 100);
        graph.set_execution_state(&job.to_string(), ExecutionState::Succeeded, 200);
        write_run_graph(&conn, &graph).unwrap();
        conn.execute("alter table job_executions drop column qm_result_json", [])
            .unwrap();

        create_project_schema(&conn).unwrap();
        create_project_schema(&conn).unwrap();
        let mut loaded = load_run_graph(&conn).unwrap();
        assert_eq!(loaded.task_run_id_for_job(&job.to_string()), Some(3));
        let execution = loaded.execution(&job.to_string()).unwrap();
        assert_eq!(execution.execution_state, ExecutionState::Succeeded);
        assert_eq!(execution.finished_at_ms, Some(200));
        assert!(execution.qm_result.is_none());

        let result = QmResult {
            converged: false,
            report: ArtifactStatus::Saved,
            series: ArtifactStatus::NotApplicable,
        };
        loaded.set_qm_result(&job.to_string(), result.clone());
        write_run_graph(&conn, &loaded).unwrap();
        create_project_schema(&conn).unwrap();
        let reloaded = load_run_graph(&conn).unwrap();
        assert_eq!(
            reloaded.execution(&job.to_string()).unwrap().qm_result,
            Some(result)
        );
    }

    #[test]
    fn run_graph_round_trips_through_project_db() {
        let conn = Connection::open_in_memory().unwrap();
        create_project_schema(&conn).unwrap();

        let mut graph = RunGraph::default();
        let local = graph.begin_execution(3, Placement::Local, Some("qm-energy".into()), 100);
        let remote = graph.begin_execution(
            5,
            Placement::Remote {
                host: Some("hpc".into()),
            },
            None,
            200,
        );
        graph.set_execution_state(&local.to_string(), ExecutionState::Succeeded, 300);
        graph.set_import_state(&local.to_string(), ResultImport::Applied);
        // A remote result whose downloaded outcome went missing.
        graph.set_import_state(&remote.to_string(), ResultImport::PendingRecovery);

        let qm = crate::backend::run_attempt::QmResult {
            converged: false,
            report: crate::backend::run_attempt::ArtifactStatus::Failed("disk full".into()),
            series: crate::backend::run_attempt::ArtifactStatus::NotApplicable,
        };
        graph.set_qm_result(&local.to_string(), qm.clone());
        write_run_graph(&conn, &graph).unwrap();
        let loaded = load_run_graph(&conn).unwrap();

        assert_eq!(loaded.task_run_id_for_job(&local.to_string()), Some(3));
        assert_eq!(loaded.task_run_id_for_job(&remote.to_string()), Some(5));
        assert!(loaded.task_has_remote_execution(5));
        let local_execution = loaded
            .executions()
            .iter()
            .find(|execution| execution.job_id == local)
            .unwrap();
        assert_eq!(
            local_execution.execution_state,
            ExecutionState::Succeeded,
            "the terminal state survives the round-trip"
        );
        assert_eq!(local_execution.qm_result, Some(qm));
        assert_eq!(local_execution.placement, Placement::Local);
        assert_eq!(local_execution.import_state, ResultImport::Applied);
        let remote_execution = loaded
            .executions()
            .iter()
            .find(|execution| execution.job_id == remote)
            .unwrap();
        assert!(remote_execution.qm_result.is_none());
        assert_eq!(
            remote_execution.import_state,
            ResultImport::PendingRecovery,
            "the durable pending-recovery signal survives a restart"
        );
    }
}
