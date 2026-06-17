use super::*;

use std::{collections::HashSet, path::Path};

use anyhow::{Context, Result};
use rusqlite::{Connection, params};

use crate::{
    backend::project::{PROJECT_FORMAT_VERSION, ProjectSession},
    domain::Structure,
};

pub fn initialize_project_databases(session: &ProjectSession) -> Result<()> {
    std::fs::create_dir_all(&session.silicolab_dir)
        .with_context(|| format!("failed to create {}", session.silicolab_dir.display()))?;
    let project_db = Connection::open(&session.project_db)
        .with_context(|| format!("failed to open {}", session.project_db.display()))?;
    create_project_schema(&project_db)?;
    let compounds_db = Connection::open(&session.compounds_db)
        .with_context(|| format!("failed to open {}", session.compounds_db.display()))?;
    create_compounds_schema(&compounds_db)?;
    project_db.execute(
        "insert or replace into project_meta (key, value) values ('name', ?1)",
        params![session.name],
    )?;
    project_db.execute(
        "insert or replace into project_meta (key, value) values ('format_version', ?1)",
        params![PROJECT_FORMAT_VERSION.to_string()],
    )?;
    Ok(())
}

pub fn load_project_snapshot(session: &ProjectSession) -> Result<ProjectSnapshot> {
    let project_db = Connection::open(&session.project_db)
        .with_context(|| format!("failed to open {}", session.project_db.display()))?;
    let compounds_db = Connection::open(&session.compounds_db)
        .with_context(|| format!("failed to open {}", session.compounds_db.display()))?;
    create_project_schema(&project_db)?;
    create_compounds_schema(&compounds_db)?;

    let name = project_meta(&project_db, "name")?.unwrap_or_else(|| session.name.clone());
    let entries = load_entries(&project_db, &compounds_db)?;
    let tasks = load_tasks(&project_db)?;
    let view = load_project_view_settings(&project_db)?;
    let mut history = load_history(&project_db)?;
    history.set_active_entry(entries.active_entry_id());

    Ok(ProjectSnapshot {
        name,
        entries,
        tasks,
        view,
        history,
    })
}

/// Load a single compound's structure on demand (used for lazy loading entries
/// that were not materialized when the project was opened).
pub fn load_structure_for_compound(compounds_db: &Path, compound_id: i64) -> Result<Structure> {
    let db = Connection::open(compounds_db)
        .with_context(|| format!("failed to open {}", compounds_db.display()))?;
    load_structure(&db, compound_id)
}

/// Persist a project.
///
/// Geometry lives in `compounds.db` as one compressed blob per compound and is
/// written **incrementally**: only compounds whose entry revision differs from
/// the stored revision are re-encoded, and unloaded (lazily-not-yet-materialized)
/// entries are never rewritten. The small metadata tables in `project.db` are
/// cheap, so they are fully rewritten each time.
///
/// `persist_history` controls whether the (potentially large) per-entry undo/redo
/// stacks are re-serialized. Autosave after each edit passes `false`; explicit
/// save points (Save Project, opening/closing a project) pass `true`.
pub fn save_project_snapshot(
    session: &ProjectSession,
    snapshot: &ProjectSnapshot,
    persist_history: bool,
) -> Result<()> {
    save_project_snapshot_ref(session, &snapshot.borrowed(), persist_history)
}

/// Borrowed-input variant of [`save_project_snapshot`]. The autosave path calls
/// this directly with references into the live `AppState`, avoiding an owned
/// clone of the workspace on every save.
pub fn save_project_snapshot_ref(
    session: &ProjectSession,
    snapshot: &ProjectSnapshotRef<'_>,
    persist_history: bool,
) -> Result<()> {
    let mut project_db = Connection::open(&session.project_db)
        .with_context(|| format!("failed to open {}", session.project_db.display()))?;
    let mut compounds_db = Connection::open(&session.compounds_db)
        .with_context(|| format!("failed to open {}", session.compounds_db.display()))?;
    create_project_schema(&project_db)?;
    create_compounds_schema(&compounds_db)?;

    let project_tx = project_db.transaction()?;
    project_tx.execute("delete from project_meta", [])?;
    project_tx.execute(
        "insert into project_meta (key, value) values ('name', ?1)",
        params![snapshot.name],
    )?;
    project_tx.execute(
        "insert into project_meta (key, value) values ('format_version', ?1)",
        params![PROJECT_FORMAT_VERSION.to_string()],
    )?;

    project_tx.execute("delete from groups", [])?;
    for (index, group) in snapshot.entries.groups.iter().enumerate() {
        project_tx.execute(
            "insert into groups (id, name, sort_order) values (?1, ?2, ?3)",
            params![group.id, group.name, index as i64],
        )?;
    }

    project_tx.execute("delete from entries", [])?;
    let compound_tx = compounds_db.transaction()?;
    let stored_revisions = load_compound_revisions(&compound_tx)?;
    let mut referenced_compounds = HashSet::new();
    for entry in &snapshot.entries.records {
        let compound_id = entry.compound_id.unwrap_or(entry.id as i64);
        referenced_compounds.insert(compound_id);
        // Only re-encode a compound when its geometry actually changed. Unloaded
        // entries keep their (unchanged) revision, so they are always skipped and
        // never overwritten with their placeholder structure.
        let needs_write = entry.loaded
            && stored_revisions
                .get(&compound_id)
                .is_none_or(|stored| *stored != entry.revision as i64);
        if needs_write {
            save_structure(
                &compound_tx,
                compound_id,
                entry.revision as i64,
                &entry.structure,
            )?;
        }
        project_tx.execute(
            "insert into entries (id, name, group_id, compound_id, source_path, save_path, revision, origin_kind, origin_trajectory) values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                entry.id as i64,
                entry.name,
                entry.group_id,
                compound_id,
                entry.source_path.as_ref().map(path_to_string),
                path_to_string(&entry.save_path),
                entry.revision as i64,
                entry.origin.kind_token(),
                entry.origin.stored_path().map(path_to_string),
            ],
        )?;
    }
    // Drop geometry for entries that no longer exist.
    for stored_id in stored_revisions.keys() {
        if !referenced_compounds.contains(stored_id) {
            compound_tx.execute("delete from compounds where id = ?1", params![stored_id])?;
        }
    }

    project_tx.execute("delete from tabs", [])?;
    for (index, tab) in snapshot.entries.tabs.iter().enumerate() {
        project_tx.execute(
            "insert into tabs (position, entry_id) values (?1, ?2)",
            params![index as i64, tab.entry_id as i64],
        )?;
    }
    project_tx.execute(
        "insert into project_meta (key, value) values ('active_tab', ?1)",
        params![snapshot.entries.active_tab.to_string()],
    )?;

    project_tx.execute("delete from task_runs", [])?;
    for task in &snapshot.tasks.tasks {
        project_tx.execute(
            "insert into task_runs (
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
            ) values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                task.id as i64,
                task.run_uuid,
                task.controller_id,
                task_status_token(task.status),
                task.run_dir.as_ref().map(path_to_string),
                task.source_entry_id.map(|id| id as i64),
                task.result_entry_id.map(|id| id as i64),
                task.engine_label.as_deref(),
                task.created_at_ms as i64,
                task.finished_at_ms.map(|value| value as i64),
            ],
        )?;
    }
    save_project_view_settings(&project_tx, snapshot.view)?;

    if persist_history {
        save_history(&project_tx, snapshot.history)?;
    }

    compound_tx.commit()?;
    project_tx.commit()?;
    Ok(())
}

fn path_to_string(path: impl AsRef<std::path::Path>) -> String {
    path.as_ref().to_string_lossy().to_string()
}
