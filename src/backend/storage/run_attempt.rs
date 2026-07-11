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
                execution_state, import_state, created_at_ms, finished_at_ms
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
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let mut executions = Vec::with_capacity(rows.len());
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
        });
    }

    Ok(RunGraph::from_rows(attempts, executions))
}

/// Rewrite the attempt/execution rows inside the caller's transaction. The tables
/// are tiny (one row per attempt/job), so a full rewrite is cheap; children are
/// deleted before parents and re-inserted parent-first to satisfy the foreign key.
pub(crate) fn write_run_graph(conn: &Connection, runs: &RunGraph) -> Result<()> {
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
                 execution_state, import_state, created_at_ms, finished_at_ms)
             values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
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
        assert_eq!(local_execution.placement, Placement::Local);
        assert_eq!(local_execution.import_state, ResultImport::Applied);
        let remote_execution = loaded
            .executions()
            .iter()
            .find(|execution| execution.job_id == remote)
            .unwrap();
        assert_eq!(
            remote_execution.import_state,
            ResultImport::PendingRecovery,
            "the durable pending-recovery signal survives a restart"
        );
    }
}
