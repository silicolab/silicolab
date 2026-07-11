//! Atomicity and fault-injection tests for the two-database (`project.db` +
//! `compounds.db`) plus materialization-ledger save transaction — the storage
//! invariants that disk never shows geometry written without its ledger, or the
//! reverse.

use std::path::PathBuf;

use nalgebra::Point3;
use rusqlite::Connection;

use crate::{
    backend::{
        entries::EntryStore,
        history::History,
        materialization::{Materialization, MaterializationLedger, MaterializedEntry},
        project::ProjectSession,
        storage::{
            ProjectAssistantSnapshot, ProjectSnapshot, ProjectViewSettings,
            initialize_project_databases, load_project_snapshot, save_project_snapshot,
        },
        tasks::TaskManager,
    },
    domain::{Atom, Structure},
};

fn carbon(title: &str) -> Structure {
    Structure::new(
        title,
        vec![Atom {
            element: "C".to_string(),
            position: Point3::new(0.0, 0.0, 0.0),
            charge: 0.0,
        }],
    )
}

fn ledger_snapshot(
    name: &str,
    entries: EntryStore,
    materializations: MaterializationLedger,
) -> ProjectSnapshot {
    ProjectSnapshot {
        name: name.to_string(),
        project_id: String::new(),
        entries,
        tasks: TaskManager::default(),
        materializations,
        view: ProjectViewSettings::default(),
        history: History::default(),
        assistant: ProjectAssistantSnapshot::default(),
        warnings: Vec::new(),
    }
}

#[test]
fn ledger_and_geometry_commit_together() {
    let root = PathBuf::from("target/test-project-ledger-atomic");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join(".silicolab")).unwrap();
    let session = ProjectSession::from_root(root, "Ledger".to_string());
    initialize_project_databases(&session).unwrap();

    let mut entries = EntryStore::new_empty();
    let entry_id = entries.add_entry(carbon("optimized"), None, PathBuf::from("opt.xyz"));
    let mut ledger = MaterializationLedger::default();
    ledger.record(Materialization::single(
        "job-xyz".to_string(),
        123,
        entry_id,
        "optimized",
    ));

    save_project_snapshot(&session, &ledger_snapshot("Ledger", entries, ledger), true).unwrap();

    let loaded = load_project_snapshot(&session).unwrap();
    let record = loaded
        .materializations
        .get("job-xyz")
        .expect("ledger row survives the round-trip");
    assert_eq!(record.primary_entry_id, Some(entry_id));
    assert_eq!(record.entries.len(), 1);
    assert_eq!(record.entries[0].role, "optimized");
    assert_eq!(record.entries[0].entry_id, entry_id);
    assert!(
        loaded.entries.entry(entry_id).is_some(),
        "the entry the ledger references is present in the same store"
    );
}

#[test]
fn failed_ledger_write_rolls_back_the_whole_transaction() {
    // fault injection: a ledger write that fails partway must roll back the
    // entry + geometry writes committed in the SAME transaction, so disk never
    // shows "geometry written but ledger not written" (or the reverse).
    let root = PathBuf::from("target/test-project-ledger-rollback");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join(".silicolab")).unwrap();
    let session = ProjectSession::from_root(root, "Rollback".to_string());
    initialize_project_databases(&session).unwrap();

    let mut baseline = EntryStore::new_empty();
    baseline.add_entry(carbon("kept"), None, PathBuf::from("kept.xyz"));
    save_project_snapshot(
        &session,
        &ledger_snapshot("Rollback", baseline, MaterializationLedger::default()),
        true,
    )
    .unwrap();

    // A second save that adds a new entry AND a malformed ledger: two association
    // rows share the same (job_id, ordinal) primary key, so the second ledger
    // insert violates the constraint and aborts the transaction.
    let mut entries = EntryStore::new_empty();
    entries.add_entry(carbon("kept"), None, PathBuf::from("kept.xyz"));
    let added = entries.add_entry(carbon("added"), None, PathBuf::from("added.xyz"));
    let mut ledger = MaterializationLedger::default();
    ledger.record(Materialization {
        job_id: "dup".to_string(),
        applied_at_ms: 1,
        primary_entry_id: Some(added),
        entries: vec![
            MaterializedEntry {
                ordinal: 0,
                role: "a".to_string(),
                entry_id: added,
            },
            MaterializedEntry {
                ordinal: 0,
                role: "b".to_string(),
                entry_id: added,
            },
        ],
    });

    let result = save_project_snapshot(
        &session,
        &ledger_snapshot("Rollback", entries, ledger),
        true,
    );
    assert!(result.is_err(), "a malformed ledger must abort the save");

    // Nothing from the failed save survives: the added entry and its geometry are
    // absent, the ledger is empty, and the baseline entry is intact.
    let project_db = Connection::open(&session.project_db).unwrap();
    let entry_count: i64 = project_db
        .query_row("select count(*) from entries", [], |row| row.get(0))
        .unwrap();
    assert_eq!(entry_count, 1, "the new entry rolled back with the ledger");
    let ledger_count: i64 = project_db
        .query_row("select count(*) from job_materializations", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(ledger_count, 0);
    let compounds_db = Connection::open(&session.compounds_db).unwrap();
    let compound_count: i64 = compounds_db
        .query_row("select count(*) from compounds", [], |row| row.get(0))
        .unwrap();
    assert_eq!(
        compound_count, 1,
        "the new compound geometry rolled back too"
    );
}

#[test]
fn project_databases_stay_rollback_journal_after_save() {
    let root = PathBuf::from("target/test-project-journal-mode");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join(".silicolab")).unwrap();
    let session = ProjectSession::from_root(root, "Journal".to_string());
    initialize_project_databases(&session).unwrap();

    let mut entries = EntryStore::new_empty();
    entries.add_entry(carbon("mol"), None, PathBuf::from("a.xyz"));
    save_project_snapshot(
        &session,
        &ledger_snapshot("Journal", entries, MaterializationLedger::default()),
        true,
    )
    .unwrap();

    // Multi-database atomic commit relies on rollback-journal; WAL would silently
    // break the cross-database guarantee, so neither project DB may be WAL.
    for path in [&session.project_db, &session.compounds_db] {
        let db = Connection::open(path).unwrap();
        let mode: String = db
            .query_row("pragma journal_mode", [], |row| row.get(0))
            .unwrap();
        assert_ne!(
            mode.to_lowercase(),
            "wal",
            "{} must not be WAL",
            path.display()
        );
    }
}
