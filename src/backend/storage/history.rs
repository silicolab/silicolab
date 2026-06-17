use std::collections::BTreeMap;

use anyhow::Result;
use rusqlite::{Connection, params};

use crate::backend::{
    history::{EditSnapshot, EntryHistory, History},
    structure_codec::{decode_snapshot, encode_snapshot},
};

pub(crate) fn save_history(db: &Connection, history: &History) -> Result<()> {
    db.execute("delete from undo_history", [])?;
    for (entry_id, entry_history) in history.iter_entries() {
        write_history_stack(db, entry_id, "undo", &entry_history.undo_stack)?;
        write_history_stack(db, entry_id, "redo", &entry_history.redo_stack)?;
    }
    Ok(())
}

fn write_history_stack(
    db: &Connection,
    entry_id: u64,
    stack: &str,
    snapshots: &[EditSnapshot],
) -> Result<()> {
    for (position, snapshot) in snapshots.iter().enumerate() {
        let blob = encode_snapshot(snapshot)?;
        db.execute(
            "insert into undo_history (entry_id, stack, position, payload, uncompressed_len) values (?1, ?2, ?3, ?4, ?5)",
            params![
                entry_id as i64,
                stack,
                position as i64,
                blob.bytes,
                blob.uncompressed_len as i64,
            ],
        )?;
    }
    Ok(())
}

pub(crate) fn load_history(db: &Connection) -> Result<History> {
    let mut history = History::default();
    let mut statement = db.prepare(
        "select entry_id, stack, position, payload, uncompressed_len from undo_history order by entry_id, stack, position",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)? as u64,
            row.get::<_, String>(1)?,
            row.get::<_, Vec<u8>>(3)?,
            row.get::<_, i64>(4)? as usize,
        ))
    })?;
    let mut per_entry: BTreeMap<u64, EntryHistory> = BTreeMap::new();
    for row in rows {
        let (entry_id, stack, payload, uncompressed_len) = row?;
        let snapshot = decode_snapshot(&payload, uncompressed_len)?;
        let entry = per_entry.entry(entry_id).or_default();
        if stack == "redo" {
            entry.redo_stack.push(snapshot);
        } else {
            entry.undo_stack.push(snapshot);
        }
    }
    for (entry_id, entry_history) in per_entry {
        history.set_entry_history(entry_id, entry_history);
    }
    Ok(history)
}
