use super::*;

use std::{collections::HashSet, path::PathBuf};

use anyhow::Result;
use rusqlite::Connection;

use crate::{
    backend::entries::{EntryGroup, EntryOrigin, EntryRecordMetadata, EntryStore, WorkspaceTab},
    domain::Structure,
};

pub(crate) fn load_entries(
    project_db: &Connection,
    compounds_db: &Connection,
) -> Result<EntryStore> {
    let mut store = EntryStore::new_empty();
    store.groups = load_groups(project_db)?;
    store.records.clear();
    let mut statement = project_db.prepare(
        "select id, name, group_id, compound_id, source_path, save_path, revision, origin_kind, origin_trajectory from entries order by id",
    )?;
    let rows = statement.query_map([], |row| {
        let origin_kind = row.get::<_, Option<String>>(7)?;
        let origin_trajectory = row.get::<_, Option<String>>(8)?.map(PathBuf::from);
        Ok(EntryRow {
            id: row.get::<_, i64>(0)? as u64,
            name: row.get(1)?,
            group_id: row.get(2)?,
            compound_id: row.get(3)?,
            source_path: row.get::<_, Option<String>>(4)?.map(PathBuf::from),
            save_path: PathBuf::from(row.get::<_, String>(5)?),
            revision: row.get::<_, i64>(6)? as u64,
            origin: EntryOrigin::from_storage(origin_kind.as_deref(), origin_trajectory),
        })
    })?;
    let entry_rows = rows.collect::<rusqlite::Result<Vec<_>>>()?;

    let tabs = load_tabs(project_db)?;
    // Lazy loading: only materialize geometry for entries that have an open tab.
    // The rest get a placeholder structure and are loaded on demand when first
    // activated (see `AppState::ensure_entry_loaded`).
    let open_entries: HashSet<u64> = tabs.iter().map(|tab| tab.entry_id).collect();

    for row in entry_rows {
        let load_now = open_entries.contains(&row.id);
        let structure = if load_now {
            load_structure(compounds_db, row.compound_id)?
        } else {
            let mut placeholder = Structure::empty();
            placeholder.title = row.name.clone();
            placeholder
        };
        let entry_id = store.insert_entry_with_metadata(EntryRecordMetadata {
            id: row.id,
            name: row.name,
            structure,
            source_path: row.source_path,
            save_path: row.save_path,
            group_id: row.group_id,
            compound_id: Some(row.compound_id),
            revision: row.revision,
            loaded: load_now,
            origin: row.origin,
        });
        store.next_entry_id = store.next_entry_id.max(entry_id + 1);
    }

    store.tabs = tabs;
    store.active_tab = project_meta(project_db, "active_tab")?
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|index| *index < store.tabs.len())
        .unwrap_or_default();
    store.recompute_next_ids();
    Ok(store)
}

fn load_groups(db: &Connection) -> Result<Vec<EntryGroup>> {
    let mut statement = db.prepare("select id, name from groups order by sort_order, id")?;
    let rows = statement.query_map([], |row| {
        Ok(EntryGroup {
            id: row.get(0)?,
            name: row.get(1)?,
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

fn load_tabs(db: &Connection) -> Result<Vec<WorkspaceTab>> {
    let mut statement = db.prepare("select entry_id from tabs order by position")?;
    let rows = statement.query_map([], |row| {
        Ok(WorkspaceTab {
            entry_id: row.get::<_, i64>(0)? as u64,
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

struct EntryRow {
    id: u64,
    name: String,
    group_id: String,
    compound_id: i64,
    source_path: Option<PathBuf>,
    save_path: PathBuf,
    revision: u64,
    origin: EntryOrigin,
}
