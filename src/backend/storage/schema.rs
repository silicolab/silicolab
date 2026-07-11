use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};

pub(crate) fn create_project_schema(db: &Connection) -> Result<()> {
    db.execute_batch(
        "
        create table if not exists project_meta (
            key text primary key,
            value text not null
        );
        create table if not exists groups (
            id text primary key,
            name text not null,
            sort_order integer not null
        );
        create table if not exists entries (
            id integer primary key,
            name text not null,
            group_id text not null,
            compound_id integer not null,
            source_path text,
            save_path text not null,
            revision integer not null default 0,
            origin_kind text not null default 'user',
            origin_trajectory text
        );
        create table if not exists tabs (
            position integer primary key,
            entry_id integer not null
        );
        create table if not exists task_runs (
            id integer primary key,
            run_uuid text,
            controller_id text not null,
            status text not null,
            run_dir text,
            source_entry_id integer,
            result_entry_id integer,
            engine_label text,
            created_at_ms integer not null default 0,
            finished_at_ms integer
        );
        create table if not exists render_overrides (
            id integer primary key,
            scope_type text not null,
            scope_id text not null,
            target_type text not null,
            target_id text not null,
            property text not null,
            value_type text not null,
            value_text text,
            value_real real,
            value_integer integer,
            value_json text,
            priority integer not null default 0
        );
        create index if not exists render_overrides_lookup_idx on render_overrides (
            scope_type, scope_id, target_type, target_id, property, priority
        );
        create table if not exists undo_history (
            entry_id integer not null,
            stack text not null,
            position integer not null,
            payload blob not null,
            uncompressed_len integer not null,
            primary key (entry_id, stack, position)
        );
        create table if not exists assistant_state (
            id integer primary key check (id = 1),
            format integer not null,
            payload blob not null,
            uncompressed_len integer not null,
            updated_at_ms integer not null
        );
        create table if not exists run_attempts (
            run_attempt_id  integer primary key,
            task_run_id     integer not null,
            attempt_no      integer not null,
            run_name        text,
            run_dir         text,
            execution_state text,
            import_state    text,
            created_at_ms   integer not null default 0,
            finished_at_ms  integer,
            unique (task_run_id, attempt_no)
        );
        create table if not exists job_executions (
            job_id            text primary key,
            run_attempt_id    integer not null references run_attempts(run_attempt_id),
            ordinal           integer not null,
            stage             text,
            placement         text not null,
            placement_host    text,
            job_kind          text,
            required          integer not null default 1,
            execution_state   text not null,
            observation_state text,
            cancel_capability text,
            import_state      text not null default 'not_required',
            exit_code         integer,
            error             text,
            created_at_ms     integer not null default 0,
            finished_at_ms    integer,
            unique (run_attempt_id, ordinal)
        );
        create table if not exists job_materializations (
            job_id            text primary key,
            applied_at_ms     integer not null,
            primary_entry_id  integer references entries(id) on delete set null
        );
        create table if not exists job_materialization_entries (
            job_id    text    not null,
            ordinal   integer not null,
            role      text    not null,
            entry_id  integer not null references entries(id) on delete cascade,
            primary key (job_id, ordinal),
            foreign key (job_id) references job_materializations(job_id) on delete restrict
        );
        ",
    )?;
    ensure_task_run_columns(db)?;
    ensure_entry_columns(db)?;
    Ok(())
}

/// Geometry is stored as one compressed blob per compound rather than spread
/// across many normalized rows. A handful of columns (title, kind, counts) are
/// duplicated out of the blob so the entry list and queries can read them
/// without decompressing, and `revision` drives incremental saves.
pub(crate) fn create_compounds_schema(db: &Connection) -> Result<()> {
    db.execute_batch(
        "
        create table if not exists compounds (
            id integer primary key,
            title text not null,
            kind text not null default 'structure',
            atom_count integer not null default 0,
            bond_count integer not null default 0,
            revision integer not null default 0,
            format integer not null default 1,
            payload blob not null,
            uncompressed_len integer not null
        );
        ",
    )?;
    Ok(())
}

pub(crate) fn project_meta(db: &Connection, key: &str) -> Result<Option<String>> {
    db.query_row(
        "select value from project_meta where key = ?1",
        params![key],
        |row| row.get(0),
    )
    .optional()
    .map_err(Into::into)
}

fn ensure_task_run_columns(db: &Connection) -> Result<()> {
    let mut statement = db.prepare("pragma table_info(task_runs)")?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let add_column = |name: &str, ddl: &str| -> Result<()> {
        if columns.iter().any(|column| column == name) {
            return Ok(());
        }
        db.execute(ddl, [])?;
        Ok(())
    };

    add_column("run_uuid", "alter table task_runs add column run_uuid text")?;
    add_column("run_dir", "alter table task_runs add column run_dir text")?;
    add_column(
        "source_entry_id",
        "alter table task_runs add column source_entry_id integer",
    )?;
    add_column(
        "result_entry_id",
        "alter table task_runs add column result_entry_id integer",
    )?;
    add_column(
        "engine_label",
        "alter table task_runs add column engine_label text",
    )?;
    add_column(
        "created_at_ms",
        "alter table task_runs add column created_at_ms integer not null default 0",
    )?;
    add_column(
        "finished_at_ms",
        "alter table task_runs add column finished_at_ms integer",
    )?;
    Ok(())
}

fn ensure_entry_columns(db: &Connection) -> Result<()> {
    let mut statement = db.prepare("pragma table_info(entries)")?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let add_column = |name: &str, ddl: &str| -> Result<()> {
        if columns.iter().any(|column| column == name) {
            return Ok(());
        }
        db.execute(ddl, [])?;
        Ok(())
    };

    add_column(
        "origin_kind",
        "alter table entries add column origin_kind text not null default 'user'",
    )?;
    add_column(
        "origin_trajectory",
        "alter table entries add column origin_trajectory text",
    )?;
    Ok(())
}
