use std::path::PathBuf;

use nalgebra::Point3;
use rusqlite::{Connection, OptionalExtension, params};

use crate::{
    backend::{
        entries::{EntryOrigin, EntryStore},
        history::{EditSnapshot, History},
        project::ProjectSession,
        storage::{
            ProjectSnapshot, ProjectViewSettings, initialize_project_databases,
            load_project_snapshot, load_structure_for_compound, save_project_snapshot,
        },
        tasks::TaskManager,
    },
    domain::{Atom, AtomCategory, Bond, BondType, Structure, UnitCell},
    frontend::{AtomStyle, SurfaceStyle, ViewportSurfaceState, ViewportVisualState},
};

#[test]
fn structure_roundtrips_through_project_databases() {
    let root = PathBuf::from("target/test-project-storage");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join(".silicolab")).unwrap();
    let session = ProjectSession::from_root(root, "Test".to_string());
    initialize_project_databases(&session).unwrap();

    let structure = Structure::with_cell_and_bonds(
        "ethene",
        vec![
            Atom {
                element: "C".to_string(),
                position: Point3::new(0.0, 0.0, 0.0),
                charge: 0.0,
            },
            Atom {
                element: "C".to_string(),
                position: Point3::new(1.34, 0.0, 0.0),
                charge: 0.0,
            },
        ],
        vec![Bond::with_type(0, 1, BondType::Double)],
        UnitCell::from_parameters(10.0, 11.0, 12.0, 90.0, 91.0, 92.0),
    );
    let mut entries = EntryStore::new_empty();
    entries.add_entry(structure, None, PathBuf::from("ethene.cif"));
    let snapshot = ProjectSnapshot {
        name: "Test".to_string(),
        entries,
        tasks: TaskManager::default(),
        view: ProjectViewSettings::default(),
        history: History::default(),
    };

    save_project_snapshot(&session, &snapshot, true).unwrap();
    let loaded = load_project_snapshot(&session).unwrap();
    let entry = loaded.entries.records.first().unwrap();

    assert_eq!(entry.structure.title, "ethene");
    assert_eq!(entry.structure.atoms.len(), 2);
    assert_eq!(entry.structure.bonds[0].bond_type, BondType::Double);
    assert!(entry.structure.cell.is_some());
    // Default provenance survives a round-trip.
    assert_eq!(entry.origin, EntryOrigin::User);
}

#[test]
fn entry_origin_roundtrips_through_project_databases() {
    let root = PathBuf::from("target/test-project-origin-storage");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join(".silicolab")).unwrap();
    let session = ProjectSession::from_root(root, "Origin".to_string());
    initialize_project_databases(&session).unwrap();

    let mut entries = EntryStore::new_empty();
    let entry_id = entries.add_entry(
        Structure::new("md-output", Vec::new()),
        None,
        PathBuf::from("md-output.xyz"),
    );
    let trajectory = PathBuf::from(".silicolab/runs/run-md-1/prod.xtc");
    entries.set_entry_origin(
        entry_id,
        EntryOrigin::MdRun {
            trajectory: Some(trajectory.clone()),
        },
    );
    let snapshot = ProjectSnapshot {
        name: "Origin".to_string(),
        entries,
        tasks: TaskManager::default(),
        view: ProjectViewSettings::default(),
        history: History::default(),
    };

    save_project_snapshot(&session, &snapshot, true).unwrap();
    let loaded = load_project_snapshot(&session).unwrap();
    let entry = loaded.entries.records.first().unwrap();

    assert_eq!(
        entry.origin,
        EntryOrigin::MdRun {
            trajectory: Some(trajectory),
        }
    );
    assert!(entry.origin.is_md_run());
}

#[test]
fn biopolymer_metadata_roundtrips_through_project_databases() {
    let root = PathBuf::from("target/test-project-biopolymer-storage");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join(".silicolab")).unwrap();
    let session = ProjectSession::from_root(root, "Protein".to_string());
    initialize_project_databases(&session).unwrap();

    let pdb = "\
TITLE     tiny protein
ATOM      1  N   ALA A   1       0.000   0.000   0.000  1.00  0.00           N
ATOM      2  CA  ALA A   1       1.400   0.000   0.000  1.00  0.00           C
ATOM      3  C   ALA A   1       2.000   1.200   0.000  1.00  0.00           C
END
";
    let structure = crate::io::formats::pdb::parse_pdb(pdb).unwrap();
    assert!(structure.biopolymer.is_some());

    let mut entries = EntryStore::new_empty();
    entries.add_entry(structure, None, PathBuf::from("protein.pdb"));
    let snapshot = ProjectSnapshot {
        name: "Protein".to_string(),
        entries,
        tasks: TaskManager::default(),
        view: ProjectViewSettings::default(),
        history: History::default(),
    };

    save_project_snapshot(&session, &snapshot, true).unwrap();
    let loaded = load_project_snapshot(&session).unwrap();

    let loaded_biopolymer = loaded.entries.records[0]
        .structure
        .biopolymer
        .as_ref()
        .expect("biopolymer survives round-trip");
    assert!(loaded_biopolymer.residues[0].is_standard_amino_acid);
    // Per-atom PDB names survive the save/load so RTP matching still works.
    assert_eq!(loaded_biopolymer.atom_name(0), Some("N"));
    assert_eq!(loaded_biopolymer.atom_name(1), Some("CA"));
    assert_eq!(loaded_biopolymer.atom_name(2), Some("C"));
}

#[test]
fn compounds_schema_stores_geometry_as_a_single_blob() {
    let root = PathBuf::from("target/test-project-blob-schema");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join(".silicolab")).unwrap();
    let session = ProjectSession::from_root(root, "Schema".to_string());
    initialize_project_databases(&session).unwrap();

    let db = rusqlite::Connection::open(&session.compounds_db).unwrap();
    let mut columns = db.prepare("pragma table_info(compounds)").unwrap();
    let column_names = columns
        .query_map([], |row| row.get::<_, String>(1))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();

    // Geometry now lives in a single blob column with a revision for
    // incremental saves; the old normalized per-atom tables are gone.
    for column in ["payload", "uncompressed_len", "revision", "format"] {
        assert!(
            column_names.iter().any(|name| name == column),
            "missing column {column}"
        );
    }
    for removed_table in ["atoms", "bonds", "biopolymers", "secondary_structures"] {
        let exists = db
            .query_row(
                "select 1 from sqlite_master where type = 'table' and name = ?1",
                rusqlite::params![removed_table],
                |_| Ok(()),
            )
            .optional()
            .unwrap()
            .is_some();
        assert!(!exists, "obsolete table still exists: {removed_table}");
    }

    let project_db = rusqlite::Connection::open(&session.project_db).unwrap();
    for table in ["render_overrides", "undo_history"] {
        let exists = project_db
            .query_row(
                "select 1 from sqlite_master where type = 'table' and name = ?1",
                rusqlite::params![table],
                |_| Ok(()),
            )
            .optional()
            .unwrap()
            .is_some();
        assert!(exists, "missing table {table}");
    }
}

#[test]
fn project_view_settings_roundtrip_surface_overrides() {
    let root = PathBuf::from("target/test-project-view-settings");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join(".silicolab")).unwrap();
    let session = ProjectSession::from_root(root, "View".to_string());
    initialize_project_databases(&session).unwrap();

    let mut viewport = ViewportVisualState {
        // Non-default style (default is Mesh) so it persists as a genuine
        // surface view-override; round-tripped and asserted below.
        surface: ViewportSurfaceState {
            style: SurfaceStyle::Fill,
            ..Default::default()
        },
        ..ViewportVisualState::default()
    };
    viewport.surface.chains.insert('A');
    viewport
        .chain_colors
        .insert('A', eframe::egui::Color32::from_rgb(100, 149, 237));
    viewport.ions.show_within = Some(3.5);
    // A non-default view-level flag (default is true).
    viewport.show_cell = false;
    // Project-level category style override.
    viewport
        .category_styles
        .insert(AtomCategory::Solvent, AtomStyle::Wireframe);
    let view = ProjectViewSettings {
        viewport,
        entry_viewports: Default::default(),
    };

    save_project_snapshot(
        &session,
        &ProjectSnapshot {
            name: "View".to_string(),
            entries: EntryStore::new_empty(),
            tasks: TaskManager::default(),
            view,
            history: History::default(),
        },
        true,
    )
    .unwrap();
    let loaded = load_project_snapshot(&session).unwrap();

    assert_eq!(
        loaded
            .view
            .viewport
            .category_styles
            .get(&AtomCategory::Solvent),
        Some(&AtomStyle::Wireframe)
    );
    assert_eq!(loaded.view.viewport.surface.style, SurfaceStyle::Fill);
    assert!(loaded.view.viewport.surface.chains.contains(&'A'));
    assert_eq!(
        loaded.view.viewport.chain_colors.get(&'A'),
        Some(&eframe::egui::Color32::from_rgb(100, 149, 237))
    );
    assert_eq!(loaded.view.viewport.ions.show_within, Some(3.5));
    assert!(!loaded.view.viewport.show_cell);

    let db = rusqlite::Connection::open(&session.project_db).unwrap();
    let chain_override_count: i64 = db
        .query_row(
            "select count(*) from render_overrides where target_type = 'chain'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let view_override_count: i64 = db
        .query_row(
            "select count(*) from render_overrides where target_type = 'view'",
            [],
            |row| row.get(0),
        )
        .unwrap();

    assert!(chain_override_count >= 2);
    assert!(view_override_count >= 2);
}

#[test]
fn entry_view_settings_roundtrip_without_leaking_to_other_entries() {
    let root = PathBuf::from("target/test-project-entry-view-settings");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join(".silicolab")).unwrap();
    let session = ProjectSession::from_root(root, "EntryView".to_string());
    initialize_project_databases(&session).unwrap();

    let mut entries = EntryStore::new_empty();
    let first = entries.add_entry(Structure::empty(), None, PathBuf::from("first.xyz"));
    let second = entries.add_entry(Structure::empty(), None, PathBuf::from("second.xyz"));

    let mut first_viewport = ViewportVisualState {
        surface: ViewportSurfaceState {
            style: SurfaceStyle::Mesh,
            ..Default::default()
        },
        ..ViewportVisualState::default()
    };
    first_viewport.surface.chains.insert('A');
    // Per-atom style override (entry-scoped).
    first_viewport.atom_styles.insert(0, AtomStyle::Sphere);

    let mut view = ProjectViewSettings::default();
    view.entry_viewports.insert(first, first_viewport);

    save_project_snapshot(
        &session,
        &ProjectSnapshot {
            name: "EntryView".to_string(),
            entries,
            tasks: TaskManager::default(),
            view,
            history: History::default(),
        },
        true,
    )
    .unwrap();
    let loaded = load_project_snapshot(&session).unwrap();

    let first_view = loaded.view.entry_viewports.get(&first).unwrap();
    assert_eq!(first_view.atom_styles.get(&0), Some(&AtomStyle::Sphere));
    assert_eq!(first_view.surface.style, SurfaceStyle::Mesh);
    assert!(first_view.surface.chains.contains(&'A'));
    assert!(!loaded.view.entry_viewports.contains_key(&second));
}

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

#[test]
fn entries_without_open_tabs_are_loaded_lazily() {
    let root = PathBuf::from("target/test-project-lazy-load");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join(".silicolab")).unwrap();
    let session = ProjectSession::from_root(root, "Lazy".to_string());
    initialize_project_databases(&session).unwrap();

    let mut entries = EntryStore::new_empty();
    let first = entries.add_entry(carbon("kept-open"), None, PathBuf::from("a.xyz"));
    let second = entries.add_entry(carbon("closed-tab"), None, PathBuf::from("b.xyz"));
    // Close the second entry's tab so it has no open tab on reload.
    let closed_index = entries
        .tabs
        .iter()
        .position(|tab| tab.entry_id == second)
        .unwrap();
    entries.close_tab(closed_index);

    save_project_snapshot(
        &session,
        &ProjectSnapshot {
            name: "Lazy".to_string(),
            entries,
            tasks: TaskManager::default(),
            view: ProjectViewSettings::default(),
            history: History::default(),
        },
        true,
    )
    .unwrap();

    let loaded = load_project_snapshot(&session).unwrap();
    let open_entry = loaded.entries.entry(first).unwrap();
    let lazy_entry = loaded.entries.entry(second).unwrap();
    assert!(open_entry.loaded, "tabbed entry should load eagerly");
    assert!(!lazy_entry.loaded, "untabbed entry should stay lazy");
    assert!(lazy_entry.structure.atoms.is_empty(), "lazy placeholder");

    // The real geometry is still retrievable on demand.
    let compound_id = lazy_entry.compound_id.unwrap();
    let structure = load_structure_for_compound(&session.compounds_db, compound_id).unwrap();
    assert_eq!(structure.title, "closed-tab");
    assert_eq!(structure.atoms.len(), 1);
}

#[test]
fn unchanged_compounds_are_not_rewritten() {
    let root = PathBuf::from("target/test-project-incremental");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join(".silicolab")).unwrap();
    let session = ProjectSession::from_root(root, "Inc".to_string());
    initialize_project_databases(&session).unwrap();

    let mut entries = EntryStore::new_empty();
    entries.add_entry(carbon("mol"), None, PathBuf::from("a.xyz"));
    let snapshot = ProjectSnapshot {
        name: "Inc".to_string(),
        entries,
        tasks: TaskManager::default(),
        view: ProjectViewSettings::default(),
        history: History::default(),
    };
    save_project_snapshot(&session, &snapshot, true).unwrap();

    // Corrupt the stored blob directly, then save again without bumping the
    // entry revision: the incremental path must skip it (blob left as-is).
    let db = Connection::open(&session.compounds_db).unwrap();
    db.execute(
        "update compounds set payload = ?1",
        params![vec![0u8, 1, 2]],
    )
    .unwrap();
    drop(db);
    save_project_snapshot(&session, &snapshot, true).unwrap();
    let db = Connection::open(&session.compounds_db).unwrap();
    let payload: Vec<u8> = db
        .query_row("select payload from compounds", [], |row| row.get(0))
        .unwrap();
    assert_eq!(payload, vec![0u8, 1, 2], "unchanged compound was rewritten");
}

#[test]
fn undo_history_survives_save_and_load() {
    use crate::frontend::AtomSelection;

    let root = PathBuf::from("target/test-project-undo-history");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join(".silicolab")).unwrap();
    let session = ProjectSession::from_root(root, "Undo".to_string());
    initialize_project_databases(&session).unwrap();

    let mut entries = EntryStore::new_empty();
    let entry_id = entries.add_entry(carbon("current"), None, PathBuf::from("a.xyz"));

    let mut history = History::default();
    history.set_active_entry(Some(entry_id));
    history.push_undo(EditSnapshot {
        structure: carbon("before-edit"),
        source_path: None,
        save_path: PathBuf::from("a.xyz"),
        selection: AtomSelection::from_parts([0], Some(0)),
    });

    save_project_snapshot(
        &session,
        &ProjectSnapshot {
            name: "Undo".to_string(),
            entries,
            tasks: TaskManager::default(),
            view: ProjectViewSettings::default(),
            history,
        },
        true,
    )
    .unwrap();

    let loaded = load_project_snapshot(&session).unwrap();
    let mut restored = loaded.history;
    restored.set_active_entry(Some(entry_id));
    assert!(restored.can_undo(), "undo stack should survive reload");
    let snapshot = restored.take_undo().expect("undo snapshot restored");
    assert_eq!(snapshot.structure.title, "before-edit");
    assert_eq!(snapshot.selection.ordered_indices(), vec![0]);
}
