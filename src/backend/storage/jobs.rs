//! Global registry of detached remote compute jobs.
//!
//! Remote jobs outlive the app: a submitted job keeps running on the cluster
//! while the laptop is closed. Their identity and launch handle therefore live
//! in a **global** SQLite database (`jobs.db` in the app config dir), not in
//! per-project state. On a fresh session the non-terminal rows are listed, each
//! `RemoteTarget` is rebuilt deterministically from `host_id` + `job_id`, and
//! liveness is probed again.
//!
//! The access pattern mirrors the per-project store verbatim: `Connection::open`
//! then `create table if not exists`, with forward-compatible `pragma
//! table_info` / `alter table add column` migrations. WAL is enabled so the
//! off-thread liveness probes can read while the UI thread writes.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rusqlite::{Connection, params};

use crate::backend::config::config_dir;

/// Lifecycle of a remote job. `queued`/`running` are non-terminal (reconnect
/// probes them); `done`/`failed`/`lost` are terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteJobStatus {
    Queued,
    Running,
    Cancelling,
    Done,
    Failed,
    Lost,
    Cancelled,
}

impl RemoteJobStatus {
    pub fn token(self) -> &'static str {
        match self {
            RemoteJobStatus::Queued => "queued",
            RemoteJobStatus::Running => "running",
            RemoteJobStatus::Cancelling => "cancelling",
            RemoteJobStatus::Done => "done",
            RemoteJobStatus::Failed => "failed",
            RemoteJobStatus::Lost => "lost",
            RemoteJobStatus::Cancelled => "cancelled",
        }
    }

    pub fn from_token(token: &str) -> Option<Self> {
        Some(match token {
            "queued" => RemoteJobStatus::Queued,
            "running" => RemoteJobStatus::Running,
            "cancelling" => RemoteJobStatus::Cancelling,
            "done" => RemoteJobStatus::Done,
            "failed" => RemoteJobStatus::Failed,
            "lost" => RemoteJobStatus::Lost,
            "cancelled" => RemoteJobStatus::Cancelled,
            _ => return None,
        })
    }

    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            RemoteJobStatus::Done
                | RemoteJobStatus::Failed
                | RemoteJobStatus::Lost
                | RemoteJobStatus::Cancelled
        )
    }
}

/// One row of the `remote_jobs` table — the minimal durable state for a detached
/// remote run. Everything about the remote path is reconstructable from
/// `host_id` + `job_id` via `RemoteTarget::for_run`.
#[derive(Debug, Clone)]
pub struct RemoteJob {
    /// Global execution identity (primary key). Currently equal to the owning
    /// `TaskRun::run_uuid` value, which the reverse lookups still key on; it
    /// becomes a distinct [`compute_core::job::JobId`] when the runtime adopts it.
    pub job_id: String,
    /// `RemoteHost::id`; indexed so a reconnect lists a host's rows cheaply.
    pub host_id: String,
    /// Denormalized label for display without a config lookup.
    pub host_label: String,
    /// The run's shared work dir (`work_root/runs/<uuid>`).
    pub remote_dir: String,
    /// Scheduler/launcher token at launch (`direct`).
    pub scheduler: String,
    /// The PGID (Direct) or JobID (scheduler) the job launched under.
    pub launch_handle: String,
    pub cluster: Option<String>,
    /// The `EngineId` string (`hartree`).
    pub engine_id: String,
    /// Engine-specific job kind (e.g. the task controller id).
    pub job_kind: String,
    /// Stable owning-project id, the authority for re-association on reopen.
    pub project_id: Option<String>,
    /// Owning project's root path — only a hint (a moved project keeps its id but
    /// not its path).
    pub project_root_hint: Option<String>,
    /// Local dir `outcome.json`/logs retrieve to.
    pub local_run_dir: String,
    pub status: RemoteJobStatus,
    pub submitted_at_ms: i64,
    pub last_polled_at_ms: Option<i64>,
    /// From `.exit`; `None` until terminal.
    pub exit_code: Option<i64>,
    pub scheduler_state: Option<String>,
    pub reason: Option<String>,
    pub console_offset: u64,
    pub unknown_since_ms: Option<i64>,
}

/// Path to the global registry database, alongside `settings.json`.
pub fn jobs_db_path() -> PathBuf {
    config_dir().join("jobs.db")
}

/// Open (creating if needed) the global registry at its standard location.
pub fn open() -> Result<Connection> {
    open_at(&jobs_db_path())
}

/// Open the registry at an explicit path (used by tests). Enables WAL and
/// ensures the schema.
pub fn open_at(path: &Path) -> Result<Connection> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let conn =
        Connection::open(path).with_context(|| format!("failed to open {}", path.display()))?;
    // WAL: off-thread liveness probes read while the UI thread writes.
    let _ = conn.pragma_update(None, "journal_mode", "WAL");
    create_schema(&conn)?;
    Ok(conn)
}

fn create_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "
        create table if not exists remote_jobs (
            job_id text primary key,
            host_id text not null,
            host_label text,
            remote_dir text,
            scheduler text,
            launch_handle text,
            cluster text,
            engine_id text,
            job_kind text,
            project_root_hint text,
            local_run_dir text,
            status text not null default 'queued',
            submitted_at_ms integer not null default 0,
            last_polled_at_ms integer,
            exit_code integer
            ,scheduler_state text
            ,reason text
            ,console_offset integer not null default 0
            ,unknown_since_ms integer
            ,project_id text
        );
        create index if not exists remote_jobs_host_idx on remote_jobs (host_id);
        ",
    )?;
    ensure_columns(conn)?;
    Ok(())
}

/// Forward-compatible migration: add any column missing from an older `jobs.db`,
/// the same `pragma table_info` + `alter table add column` idiom the task-run
/// schema uses. A future column is appended here; existing data is never dropped.
fn ensure_columns(conn: &Connection) -> Result<()> {
    let mut statement = conn.prepare("pragma table_info(remote_jobs)")?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let add_column = |name: &str, ddl: &str| -> Result<()> {
        if columns.iter().any(|column| column == name) {
            return Ok(());
        }
        conn.execute(ddl, [])?;
        Ok(())
    };

    add_column(
        "host_label",
        "alter table remote_jobs add column host_label text",
    )?;
    add_column(
        "remote_dir",
        "alter table remote_jobs add column remote_dir text",
    )?;
    add_column(
        "scheduler",
        "alter table remote_jobs add column scheduler text",
    )?;
    add_column(
        "launch_handle",
        "alter table remote_jobs add column launch_handle text",
    )?;
    add_column(
        "engine_id",
        "alter table remote_jobs add column engine_id text",
    )?;
    add_column(
        "job_kind",
        "alter table remote_jobs add column job_kind text",
    )?;
    add_column(
        "project_root_hint",
        "alter table remote_jobs add column project_root_hint text",
    )?;
    add_column(
        "project_id",
        "alter table remote_jobs add column project_id text",
    )?;
    add_column(
        "local_run_dir",
        "alter table remote_jobs add column local_run_dir text",
    )?;
    add_column(
        "last_polled_at_ms",
        "alter table remote_jobs add column last_polled_at_ms integer",
    )?;
    add_column(
        "exit_code",
        "alter table remote_jobs add column exit_code integer",
    )?;
    add_column("cluster", "alter table remote_jobs add column cluster text")?;
    add_column(
        "scheduler_state",
        "alter table remote_jobs add column scheduler_state text",
    )?;
    add_column("reason", "alter table remote_jobs add column reason text")?;
    add_column(
        "console_offset",
        "alter table remote_jobs add column console_offset integer not null default 0",
    )?;
    add_column(
        "unknown_since_ms",
        "alter table remote_jobs add column unknown_since_ms integer",
    )?;
    Ok(())
}

/// Insert or replace a job row (a full-row upsert keyed by `job_id`).
pub fn upsert(conn: &Connection, job: &RemoteJob) -> Result<()> {
    conn.execute(
        "insert or replace into remote_jobs (
            job_id, host_id, host_label, remote_dir, scheduler, launch_handle, cluster,
            engine_id, job_kind, project_root_hint, local_run_dir, status,
            submitted_at_ms, last_polled_at_ms, exit_code, scheduler_state, reason,
            console_offset, unknown_since_ms, project_id
        ) values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20)",
        params![
            job.job_id,
            job.host_id,
            job.host_label,
            job.remote_dir,
            job.scheduler,
            job.launch_handle,
            job.cluster,
            job.engine_id,
            job.job_kind,
            job.project_root_hint,
            job.local_run_dir,
            job.status.token(),
            job.submitted_at_ms,
            job.last_polled_at_ms,
            job.exit_code,
            job.scheduler_state,
            job.reason,
            job.console_offset as i64,
            job.unknown_since_ms,
            job.project_id,
        ],
    )?;
    Ok(())
}

pub struct RemoteObservationUpdate<'a> {
    pub status: RemoteJobStatus,
    pub exit_code: Option<i64>,
    pub scheduler_state: Option<&'a str>,
    pub reason: Option<&'a str>,
    pub console_offset: u64,
    pub unknown_since_ms: Option<i64>,
    pub polled_at_ms: i64,
}

pub fn record_observation(
    conn: &Connection,
    job_id: &str,
    update: RemoteObservationUpdate<'_>,
) -> Result<()> {
    conn.execute(
        "update remote_jobs set status = ?2, exit_code = ?3, scheduler_state = ?4, reason = ?5, console_offset = ?6, unknown_since_ms = ?7, last_polled_at_ms = ?8 where job_id = ?1",
        params![job_id, update.status.token(), update.exit_code, update.scheduler_state, update.reason, update.console_offset as i64, update.unknown_since_ms, update.polled_at_ms],
    )?;
    Ok(())
}

/// All non-terminal rows (`queued`/`running`/`cancelling`), oldest first — the
/// reconnect set.
pub fn list_non_terminal(conn: &Connection) -> Result<Vec<RemoteJob>> {
    query(
        conn,
        "where status in ('queued', 'running', 'cancelling') order by submitted_at_ms",
        [],
    )
}

/// All rows for a project, newest first (for the per-project task surface).
pub fn list_for_project(conn: &Connection, project_root_hint: &str) -> Result<Vec<RemoteJob>> {
    query(
        conn,
        "where project_root_hint = ?1 order by submitted_at_ms desc",
        params![project_root_hint],
    )
}

/// Successfully-finished rows owned by a project (matched by the path-independent
/// `project_id`), oldest first — the set open-project compensation checks against
/// the ledger to import any result that finished while another project was open.
pub fn list_completed_for_project_id(
    conn: &Connection,
    project_id: &str,
) -> Result<Vec<RemoteJob>> {
    query(
        conn,
        "where project_id = ?1 and status = 'done' order by submitted_at_ms",
        params![project_id],
    )
}

/// One row by `job_id`.
pub fn get(conn: &Connection, job_id: &str) -> Result<Option<RemoteJob>> {
    Ok(query(conn, "where job_id = ?1", params![job_id])?
        .into_iter()
        .next())
}

/// Drop a row (after the remote scratch is removed, or the run is forgotten).
pub fn remove(conn: &Connection, job_id: &str) -> Result<()> {
    conn.execute("delete from remote_jobs where job_id = ?1", params![job_id])?;
    Ok(())
}

const COLUMNS: &str = "job_id, host_id, host_label, remote_dir, scheduler, \
     launch_handle, cluster, engine_id, job_kind, project_root_hint, local_run_dir, status, \
     submitted_at_ms, last_polled_at_ms, exit_code, scheduler_state, reason, console_offset, \
     unknown_since_ms, project_id";

fn query(conn: &Connection, tail: &str, params: impl rusqlite::Params) -> Result<Vec<RemoteJob>> {
    let sql = format!("select {COLUMNS} from remote_jobs {tail}");
    let mut statement = conn.prepare(&sql)?;
    let rows = statement
        .query_map(params, row_to_job)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

fn row_to_job(row: &rusqlite::Row<'_>) -> rusqlite::Result<RemoteJob> {
    let status: String = row.get(11)?;
    Ok(RemoteJob {
        job_id: row.get(0)?,
        host_id: row.get(1)?,
        host_label: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
        remote_dir: row.get::<_, Option<String>>(3)?.unwrap_or_default(),
        scheduler: row.get::<_, Option<String>>(4)?.unwrap_or_default(),
        launch_handle: row.get::<_, Option<String>>(5)?.unwrap_or_default(),
        cluster: row.get(6)?,
        engine_id: row.get::<_, Option<String>>(7)?.unwrap_or_default(),
        job_kind: row.get::<_, Option<String>>(8)?.unwrap_or_default(),
        project_root_hint: row.get(9)?,
        local_run_dir: row.get::<_, Option<String>>(10)?.unwrap_or_default(),
        // An unknown token degrades to Lost rather than failing the load.
        status: RemoteJobStatus::from_token(&status).unwrap_or(RemoteJobStatus::Lost),
        submitted_at_ms: row.get(12)?,
        last_polled_at_ms: row.get(13)?,
        exit_code: row.get(14)?,
        scheduler_state: row.get(15)?,
        reason: row.get(16)?,
        console_offset: row.get::<_, i64>(17)?.max(0) as u64,
        unknown_since_ms: row.get(18)?,
        project_id: row.get(19)?,
    })
}

/// Current wall-clock in epoch milliseconds, matching the task-run convention.
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(job_id: &str) -> RemoteJob {
        RemoteJob {
            job_id: job_id.to_string(),
            host_id: "hpc".to_string(),
            host_label: "Cluster".to_string(),
            remote_dir: format!("~/.silicolab/runs/{job_id}"),
            scheduler: "direct".to_string(),
            launch_handle: "12345".to_string(),
            cluster: None,
            engine_id: "hartree".to_string(),
            job_kind: "qm-energy".to_string(),
            project_id: Some("proj-id".to_string()),
            project_root_hint: Some("/work/proj".to_string()),
            local_run_dir: "/tmp/run".to_string(),
            status: RemoteJobStatus::Running,
            submitted_at_ms: 1000,
            last_polled_at_ms: None,
            exit_code: None,
            scheduler_state: None,
            reason: None,
            console_offset: 0,
            unknown_since_ms: None,
        }
    }

    fn temp_db() -> (Connection, std::path::PathBuf) {
        let path = std::env::temp_dir().join(format!(
            "silicolab-jobs-test-{}.db",
            uuid::Uuid::new_v4().simple()
        ));
        let _ = std::fs::remove_file(&path);
        (open_at(&path).expect("open registry"), path)
    }

    #[test]
    fn status_tokens_round_trip() {
        for status in [
            RemoteJobStatus::Queued,
            RemoteJobStatus::Running,
            RemoteJobStatus::Cancelling,
            RemoteJobStatus::Done,
            RemoteJobStatus::Failed,
            RemoteJobStatus::Lost,
            RemoteJobStatus::Cancelled,
        ] {
            assert_eq!(RemoteJobStatus::from_token(status.token()), Some(status));
        }
        assert_eq!(RemoteJobStatus::from_token("bogus"), None);
        assert!(RemoteJobStatus::Done.is_terminal());
        assert!(!RemoteJobStatus::Running.is_terminal());
    }

    #[test]
    fn upsert_get_and_list_round_trip() {
        let (conn, path) = temp_db();
        upsert(&conn, &sample("a")).expect("insert a");
        upsert(&conn, &sample("b")).expect("insert b");

        let got = get(&conn, "a").expect("query").expect("row a present");
        assert_eq!(got.host_id, "hpc");
        assert_eq!(got.launch_handle, "12345");
        assert_eq!(got.status, RemoteJobStatus::Running);
        assert_eq!(got.exit_code, None);

        let non_terminal = list_non_terminal(&conn).expect("list");
        assert_eq!(non_terminal.len(), 2);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn reconnect_then_record_terminal_drops_from_non_terminal() {
        // Simulate a restart: a non-terminal row persists, a fresh connection
        // lists it, a refresh transitions it to terminal, and it then leaves the
        // reconnect set.
        let (conn, path) = temp_db();
        upsert(&conn, &sample("a")).expect("insert");
        drop(conn);

        let conn = open_at(&path).expect("reopen");
        let pending = list_non_terminal(&conn).expect("list");
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].job_id, "a");

        record_observation(
            &conn,
            "a",
            RemoteObservationUpdate {
                status: RemoteJobStatus::Done,
                exit_code: Some(0),
                scheduler_state: None,
                reason: None,
                console_offset: 0,
                unknown_since_ms: None,
                polled_at_ms: 2000,
            },
        )
        .expect("record");
        assert!(list_non_terminal(&conn).expect("list").is_empty());
        let done = get(&conn, "a").expect("query").expect("present");
        assert_eq!(done.status, RemoteJobStatus::Done);
        assert_eq!(done.exit_code, Some(0));
        assert_eq!(done.last_polled_at_ms, Some(2000));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn remove_deletes_the_row() {
        let (conn, path) = temp_db();
        upsert(&conn, &sample("a")).expect("insert");
        remove(&conn, "a").expect("remove");
        assert!(get(&conn, "a").expect("query").is_none());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn list_for_project_filters() {
        let (conn, path) = temp_db();
        upsert(&conn, &sample("a")).expect("insert a");
        let mut other = sample("b");
        other.project_root_hint = Some("/work/other".to_string());
        upsert(&conn, &other).expect("insert b");

        let proj = list_for_project(&conn, "/work/proj").expect("list");
        assert_eq!(proj.len(), 1);
        assert_eq!(proj[0].job_id, "a");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn older_registry_gains_scheduler_observation_columns() {
        let path = std::env::temp_dir().join(format!(
            "silicolab-jobs-old-{}.db",
            uuid::Uuid::new_v4().simple()
        ));
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "create table remote_jobs (job_id text primary key, host_id text not null, status text not null);",
        )
        .unwrap();
        drop(conn);
        let conn = open_at(&path).unwrap();
        let columns = conn
            .prepare("pragma table_info(remote_jobs)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        for column in [
            "cluster",
            "project_root_hint",
            "project_id",
            "scheduler_state",
            "reason",
            "console_offset",
            "unknown_since_ms",
        ] {
            assert!(columns.iter().any(|item| item == column));
        }
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn scheduler_observation_round_trips() {
        let (conn, path) = temp_db();
        let mut job = sample("a");
        job.scheduler = "slurm".to_string();
        job.cluster = Some("alpha".to_string());
        upsert(&conn, &job).unwrap();
        record_observation(
            &conn,
            "a",
            RemoteObservationUpdate {
                status: RemoteJobStatus::Queued,
                exit_code: None,
                scheduler_state: Some("PENDING"),
                reason: Some("Resources"),
                console_offset: 128,
                unknown_since_ms: Some(2000),
                polled_at_ms: 2500,
            },
        )
        .unwrap();
        let loaded = get(&conn, "a").unwrap().unwrap();
        assert_eq!(loaded.cluster.as_deref(), Some("alpha"));
        assert_eq!(loaded.scheduler_state.as_deref(), Some("PENDING"));
        assert_eq!(loaded.reason.as_deref(), Some("Resources"));
        assert_eq!(loaded.console_offset, 128);
        assert_eq!(loaded.unknown_since_ms, Some(2000));
        let _ = std::fs::remove_file(path);
    }
}
