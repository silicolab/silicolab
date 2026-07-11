use std::collections::{HashMap, HashSet};

use anyhow::Result;
use rusqlite::{Connection, params};

use crate::backend::materialization::{Materialization, MaterializationLedger, MaterializedEntry};

pub(crate) fn load_materializations(db: &Connection) -> Result<MaterializationLedger> {
    let mut children: HashMap<String, Vec<MaterializedEntry>> = HashMap::new();
    {
        let mut statement = db.prepare(
            "select job_id, ordinal, role, entry_id
             from job_materialization_entries
             order by job_id, ordinal",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)? as u32,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)? as u64,
            ))
        })?;
        for row in rows {
            let (job_id, ordinal, role, entry_id) = row?;
            children.entry(job_id).or_default().push(MaterializedEntry {
                ordinal,
                role,
                entry_id,
            });
        }
    }

    let mut statement = db.prepare(
        "select job_id, applied_at_ms, primary_entry_id
         from job_materializations
         order by job_id",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, i64>(1)? as u64,
            row.get::<_, Option<i64>>(2)?.map(|value| value as u64),
        ))
    })?;
    let mut records = Vec::new();
    for row in rows {
        let (job_id, applied_at_ms, primary_entry_id) = row?;
        let entries = children.remove(&job_id).unwrap_or_default();
        records.push(Materialization {
            job_id,
            applied_at_ms,
            primary_entry_id,
            entries,
        });
    }
    Ok(MaterializationLedger::from_records(records))
}

/// Rewrite both ledger tables inside the caller's atomic transaction, reconciled
/// against the entry ids that survive this save. A parent row is always kept (the
/// idempotency proof), but a `primary_entry_id` or association row pointing at a
/// since-deleted entry is nulled/dropped — the same effect the schema's
/// `on delete set null` / `on delete cascade` foreign keys would produce, done
/// explicitly because the whole project is rewritten each save. Children are
/// deleted before parents to satisfy the `on delete restrict` back-reference.
pub(crate) fn write_materializations(
    tx: &Connection,
    ledger: &MaterializationLedger,
    live_entry_ids: &HashSet<u64>,
) -> Result<()> {
    tx.execute("delete from job_materialization_entries", [])?;
    tx.execute("delete from job_materializations", [])?;
    for record in ledger.records() {
        let primary = record
            .primary_entry_id
            .filter(|id| live_entry_ids.contains(id));
        tx.execute(
            "insert into job_materializations (job_id, applied_at_ms, primary_entry_id)
             values (?1, ?2, ?3)",
            params![
                record.job_id,
                record.applied_at_ms as i64,
                primary.map(|id| id as i64),
            ],
        )?;
        for entry in &record.entries {
            if !live_entry_ids.contains(&entry.entry_id) {
                continue;
            }
            tx.execute(
                "insert into job_materialization_entries (job_id, ordinal, role, entry_id)
                 values (?1, ?2, ?3, ?4)",
                params![
                    record.job_id,
                    entry.ordinal as i64,
                    entry.role,
                    entry.entry_id as i64,
                ],
            )?;
        }
    }
    Ok(())
}
