//! The project-scoped materialization ledger: the durable, idempotent record of
//! which engine job produced which result entries. Keyed by the global execution
//! identity — currently the owning `TaskRun::run_uuid` value, a distinct
//! `compute_core::job::JobId` once the runtime adopts it. This is the backend's
//! authority for "has this outcome already been applied", so a remote refresh or an
//! open-project compensation never re-creates results it already imported.
//!
//! The ledger lives in the project database and is committed in the **same**
//! transaction as the entry changes it records (see `storage::materialization`), so
//! a crash can never leave geometry written without its ledger, or vice versa.

use std::collections::BTreeMap;

/// One materialized job: the result entries an outcome produced. The parent record
/// is the idempotency proof and is retained even after its entries are deleted, so
/// an old outcome is never mistaken for un-imported and re-created.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Materialization {
    pub job_id: String,
    pub applied_at_ms: u64,
    /// Entry opened by default when the user jumps to this job's result. `None` for
    /// an outcome that produced no entry (a report), or once the primary entry has
    /// been deleted.
    pub primary_entry_id: Option<u64>,
    /// Every entry the outcome produced, in application order. Empty for a
    /// report/file-only outcome; one row per pose for docking.
    pub entries: Vec<MaterializedEntry>,
}

impl Materialization {
    /// A job whose outcome produced exactly one entry (the common create-entry
    /// case): that entry is both the primary and the sole association row.
    pub fn single(job_id: String, applied_at_ms: u64, entry_id: u64, role: &str) -> Self {
        Self {
            job_id,
            applied_at_ms,
            primary_entry_id: Some(entry_id),
            entries: vec![MaterializedEntry {
                ordinal: 0,
                role: role.to_string(),
                entry_id,
            }],
        }
    }

    /// A job whose outcome produced no entry (a report/file-only run). The parent
    /// row still records that the outcome was applied, so it is idempotent.
    pub fn report(job_id: String, applied_at_ms: u64) -> Self {
        Self {
            job_id,
            applied_at_ms,
            primary_entry_id: None,
            entries: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaterializedEntry {
    pub ordinal: u32,
    pub role: String,
    pub entry_id: u64,
}

#[derive(Debug, Clone, Default)]
pub struct MaterializationLedger {
    records: BTreeMap<String, Materialization>,
    /// Records applied since the last save. The ledger must be persisted in the
    /// atomic entries+ledger transaction; this flag lets the app loop schedule that
    /// save even for a zero-entry report, which changes no entry fingerprint.
    dirty: bool,
}

impl MaterializationLedger {
    pub fn from_records(records: impl IntoIterator<Item = Materialization>) -> Self {
        Self {
            records: records
                .into_iter()
                .map(|record| (record.job_id.clone(), record))
                .collect(),
            dirty: false,
        }
    }

    /// Whether this job's outcome has already been materialized — the idempotency
    /// guard every outcome-application path checks before it creates entries.
    pub fn contains(&self, job_id: &str) -> bool {
        self.records.contains_key(job_id)
    }

    pub fn get(&self, job_id: &str) -> Option<&Materialization> {
        self.records.get(job_id)
    }

    /// Record a freshly applied outcome and mark the ledger for persistence. A
    /// re-record for the same job is the same terminal result, so it overwrites.
    pub fn record(&mut self, materialization: Materialization) {
        self.records
            .insert(materialization.job_id.clone(), materialization);
        self.dirty = true;
    }

    pub fn records(&self) -> impl Iterator<Item = &Materialization> {
        self.records.values()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// Whether a recorded materialization is not yet on disk.
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Clear the dirty flag once the atomic save has persisted every record.
    pub fn mark_saved(&mut self) {
        self.dirty = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contains_and_record_track_idempotency_and_dirty() {
        let mut ledger = MaterializationLedger::default();
        assert!(!ledger.contains("job-1"));
        assert!(!ledger.is_dirty());

        ledger.record(Materialization::single(
            "job-1".to_string(),
            10,
            42,
            "optimized",
        ));
        assert!(ledger.contains("job-1"));
        assert!(ledger.is_dirty());
        assert_eq!(ledger.get("job-1").unwrap().primary_entry_id, Some(42));

        ledger.mark_saved();
        assert!(!ledger.is_dirty());

        // A report records the parent row with no entry, still proving application.
        ledger.record(Materialization::report("job-2".to_string(), 20));
        assert!(ledger.contains("job-2"));
        assert!(ledger.get("job-2").unwrap().entries.is_empty());
    }
}
