use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use eframe::egui::Color32;
use rusqlite::{Connection, OptionalExtension, params};

use crate::{
    backend::{
        entries::EntryRecordMetadata,
        entries::{EntryGroup, EntryOrigin, EntryStore, WorkspaceTab},
        history::{EditSnapshot, EntryHistory, History},
        project::{PROJECT_FORMAT_VERSION, ProjectSession},
        structure_codec::{
            PAYLOAD_FORMAT, decode_snapshot, decode_structure, encode_snapshot, encode_structure,
        },
        tasks::{TaskManager, TaskRun, TaskStatus, task_controller_by_id},
    },
    domain::{AtomCategory, Structure},
    frontend::{AtomStyle, LightPreset, SurfaceStyle, ViewportVisualState},
};

#[derive(Debug, Clone)]
pub struct ProjectSnapshot {
    pub name: String,
    pub entries: EntryStore,
    pub tasks: TaskManager,
    pub view: ProjectViewSettings,
    pub history: History,
}

/// Borrowed view of the data a save reads. Saving only needs read access, so the
/// hot autosave path builds one of these straight from the live `AppState`
/// instead of deep-cloning the whole workspace (every loaded entry's geometry +
/// undo history) into an owned [`ProjectSnapshot`] on each action.
pub struct ProjectSnapshotRef<'a> {
    pub name: &'a str,
    pub entries: &'a EntryStore,
    pub tasks: &'a TaskManager,
    pub view: &'a ProjectViewSettings,
    pub history: &'a History,
}

impl ProjectSnapshot {
    pub fn borrowed(&self) -> ProjectSnapshotRef<'_> {
        ProjectSnapshotRef {
            name: self.name.as_str(),
            entries: &self.entries,
            tasks: &self.tasks,
            view: &self.view,
            history: &self.history,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ProjectViewSettings {
    pub viewport: ViewportVisualState,
    pub entry_viewports: BTreeMap<u64, ViewportVisualState>,
}

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

fn load_compound_revisions(db: &Connection) -> Result<HashMap<i64, i64>> {
    let mut statement = db.prepare("select id, revision from compounds")?;
    let rows = statement.query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)))?;
    let mut map = HashMap::new();
    for row in rows {
        let (id, revision) = row?;
        map.insert(id, revision);
    }
    Ok(map)
}

fn save_history(db: &Connection, history: &History) -> Result<()> {
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

fn load_history(db: &Connection) -> Result<History> {
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

fn create_project_schema(db: &Connection) -> Result<()> {
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
fn create_compounds_schema(db: &Connection) -> Result<()> {
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

fn project_meta(db: &Connection, key: &str) -> Result<Option<String>> {
    db.query_row(
        "select value from project_meta where key = ?1",
        params![key],
        |row| row.get(0),
    )
    .optional()
    .map_err(Into::into)
}

fn load_entries(project_db: &Connection, compounds_db: &Connection) -> Result<EntryStore> {
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

fn load_tasks(db: &Connection) -> Result<TaskManager> {
    let mut manager = TaskManager::default();
    let mut statement = db.prepare(
        "select
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
         from task_runs
         order by id",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)? as u64,
            row.get::<_, Option<String>>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, Option<String>>(4)?,
            row.get::<_, Option<i64>>(5)?,
            row.get::<_, Option<i64>>(6)?,
            row.get::<_, Option<String>>(7)?,
            row.get::<_, i64>(8)? as u64,
            row.get::<_, Option<i64>>(9)?,
        ))
    })?;
    for row in rows {
        let (
            id,
            run_uuid,
            controller_id,
            status,
            run_dir,
            source_entry_id,
            result_entry_id,
            engine_label,
            created_at_ms,
            finished_at_ms,
        ) = row?;
        let Some(controller) = task_controller_by_id(&controller_id).copied() else {
            continue;
        };
        let mut run = TaskRun::from_controller(id, controller);
        // Preserve the persisted UUID; rows written before this column existed
        // keep the freshly generated one.
        if let Some(run_uuid) = run_uuid {
            run.run_uuid = run_uuid;
        }
        run.status = parse_task_status(&status);
        run.run_dir = run_dir.map(PathBuf::from);
        run.source_entry_id = source_entry_id.map(|value| value as u64);
        run.result_entry_id = result_entry_id.map(|value| value as u64);
        run.engine_label = engine_label;
        run.created_at_ms = created_at_ms;
        run.finished_at_ms = finished_at_ms.map(|value| value as u64);
        manager.tasks.push(run);
        manager.next_task_run_id = manager.next_task_run_id.max(id + 1);
    }
    Ok(manager)
}

fn save_project_view_settings(db: &Connection, view: &ProjectViewSettings) -> Result<()> {
    db.execute("delete from render_overrides", [])?;
    let default_viewport = ViewportVisualState::default();

    save_viewport_settings(
        db,
        RenderScope::project(),
        &view.viewport,
        &default_viewport,
    )?;
    for (entry_id, viewport) in &view.entry_viewports {
        save_viewport_settings(
            db,
            RenderScope::entry(*entry_id),
            viewport,
            &default_viewport,
        )?;
    }

    Ok(())
}

#[derive(Debug, Clone, Copy)]
struct RenderScope {
    scope_type: &'static str,
    scope_id: ScopeId,
}

#[derive(Debug, Clone, Copy)]
enum ScopeId {
    Project,
    Entry(u64),
}

impl RenderScope {
    fn project() -> Self {
        Self {
            scope_type: "project",
            scope_id: ScopeId::Project,
        }
    }

    fn entry(entry_id: u64) -> Self {
        Self {
            scope_type: "entry",
            scope_id: ScopeId::Entry(entry_id),
        }
    }
}

fn save_viewport_settings(
    db: &Connection,
    scope: RenderScope,
    viewport: &ViewportVisualState,
    default_viewport: &ViewportVisualState,
) -> Result<()> {
    // Project-level category styles live only at project scope; per-atom style
    // overrides live only at entry scope (atom indices belong to a compound).
    if matches!(scope.scope_id, ScopeId::Project) {
        for (category, style) in &viewport.category_styles {
            set_render_override_text(
                db,
                RenderTarget::atom_category(scope, category.token()),
                "style",
                style.token(),
            )?;
        }
    } else {
        for (atom_index, style) in &viewport.atom_styles {
            set_render_override_text(
                db,
                RenderTarget::atom(scope, *atom_index),
                "style",
                style.token(),
            )?;
        }
        // Per-atom visibility override (independent of style; see
        // [`ViewportVisualState::atom_hidden`]).
        for atom_index in &viewport.atom_hidden {
            set_render_override_bool(db, RenderTarget::atom(scope, *atom_index), "hidden", true)?;
        }
    }
    if viewport.background_color != default_viewport.background_color {
        set_render_override_json(
            db,
            RenderTarget::view(scope),
            "background_color",
            color_json(viewport.background_color),
        )?;
    }
    if viewport.show_cell != default_viewport.show_cell {
        set_render_override_bool(
            db,
            RenderTarget::view(scope),
            "show_cell",
            viewport.show_cell,
        )?;
    }
    if viewport.lighting.preset != default_viewport.lighting.preset {
        set_render_override_text(
            db,
            RenderTarget::view(scope),
            "light_preset",
            light_preset_token(viewport.lighting.preset),
        )?;
    }
    if viewport.lighting.silhouettes != default_viewport.lighting.silhouettes {
        set_render_override_bool(
            db,
            RenderTarget::view(scope),
            "silhouettes",
            viewport.lighting.silhouettes,
        )?;
    }
    if viewport.lighting.silhouette_width != default_viewport.lighting.silhouette_width {
        set_render_override_real(
            db,
            RenderTarget::view(scope),
            "silhouette_width",
            viewport.lighting.silhouette_width,
        )?;
    }
    save_cartoon_setting(
        db,
        scope,
        "cartoon_helix",
        viewport.cartoon.helix,
        default_viewport.cartoon.helix,
    )?;
    save_cartoon_setting(
        db,
        scope,
        "cartoon_sheet",
        viewport.cartoon.sheet,
        default_viewport.cartoon.sheet,
    )?;
    save_cartoon_setting(
        db,
        scope,
        "cartoon_coil",
        viewport.cartoon.coil,
        default_viewport.cartoon.coil,
    )?;
    if viewport.cartoon.smoothing != default_viewport.cartoon.smoothing {
        set_render_override_integer(
            db,
            RenderTarget::view(scope),
            "cartoon_smoothing",
            viewport.cartoon.smoothing as i64,
        )?;
    }
    if viewport.cartoon.profile_segments != default_viewport.cartoon.profile_segments {
        set_render_override_integer(
            db,
            RenderTarget::view(scope),
            "cartoon_profile_segments",
            viewport.cartoon.profile_segments as i64,
        )?;
    }
    for (chain, color) in &viewport.chain_colors {
        set_render_override_json(
            db,
            RenderTarget::chain(scope, *chain),
            "color",
            color_json(*color),
        )?;
    }
    for chain in &viewport.surface.chains {
        set_render_override_bool(
            db,
            RenderTarget::chain(scope, *chain),
            "surface_visible",
            true,
        )?;
    }
    if viewport.surface.style != default_viewport.surface.style {
        set_render_override_text(
            db,
            RenderTarget::view(scope),
            "surface_style",
            surface_style_token(viewport.surface.style),
        )?;
    }
    if viewport.surface.transparency != default_viewport.surface.transparency {
        set_render_override_real(
            db,
            RenderTarget::view(scope),
            "surface_transparency",
            viewport.surface.transparency,
        )?;
    }
    if viewport.ions.show_within != default_viewport.ions.show_within {
        match viewport.ions.show_within {
            Some(distance) => set_render_override_real(
                db,
                RenderTarget::atom_category(scope, "ion"),
                "show_within",
                distance,
            )?,
            None => set_render_override_json(
                db,
                RenderTarget::atom_category(scope, "ion"),
                "show_within",
                serde_json::Value::Null,
            )?,
        }
    }
    if viewport.ions.color != default_viewport.ions.color
        && let Some(color) = viewport.ions.color
    {
        set_render_override_json(
            db,
            RenderTarget::atom_category(scope, "ion"),
            "color",
            color_json(color),
        )?;
    }
    if viewport.hetero_atom_colors != default_viewport.hetero_atom_colors {
        set_render_override_bool(
            db,
            RenderTarget::atom_category(scope, "hetero"),
            "auto_color",
            viewport.hetero_atom_colors,
        )?;
    }

    Ok(())
}

fn load_project_view_settings(db: &Connection) -> Result<ProjectViewSettings> {
    let mut view = ProjectViewSettings::default();
    let mut statement = db.prepare(
        "select scope_type, scope_id, target_type, target_id, property, value_type, value_text, value_real, value_integer, value_json from render_overrides order by priority, id",
    )?;
    let rows = statement.query_map([], |row| {
        Ok(RenderOverrideRow {
            scope_type: row.get(0)?,
            scope_id: row.get(1)?,
            target_type: row.get(2)?,
            target_id: row.get(3)?,
            property: row.get(4)?,
            value_type: row.get(5)?,
            value_text: row.get(6)?,
            value_real: row.get(7)?,
            value_integer: row.get(8)?,
            value_json: row.get(9)?,
        })
    })?;
    for row in rows {
        let row = row?;
        apply_render_override_row(&mut view, row)?;
    }
    Ok(view)
}

#[derive(Debug, Clone)]
struct RenderTarget<'a> {
    scope_type: &'a str,
    scope_id: String,
    target_type: &'a str,
    target_id: String,
    priority: i64,
}

impl<'a> RenderTarget<'a> {
    fn view(scope: RenderScope) -> Self {
        Self {
            scope_type: scope.scope_type,
            scope_id: scope.scope_id.to_string(),
            target_type: "view",
            target_id: "default".to_string(),
            priority: 0,
        }
    }

    fn chain(scope: RenderScope, chain: char) -> Self {
        Self {
            scope_type: scope.scope_type,
            scope_id: scope.scope_id.to_string(),
            target_type: "chain",
            target_id: char_to_string(chain),
            priority: 20,
        }
    }

    fn atom_category(scope: RenderScope, category: &'a str) -> Self {
        Self {
            scope_type: scope.scope_type,
            scope_id: scope.scope_id.to_string(),
            target_type: "atom_category",
            target_id: category.to_string(),
            priority: 10,
        }
    }

    fn atom(scope: RenderScope, atom_index: usize) -> Self {
        Self {
            scope_type: scope.scope_type,
            scope_id: scope.scope_id.to_string(),
            target_type: "atom",
            target_id: atom_index.to_string(),
            priority: 30,
        }
    }
}

impl std::fmt::Display for ScopeId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Project => formatter.write_str("project"),
            Self::Entry(entry_id) => write!(formatter, "{entry_id}"),
        }
    }
}

struct RenderOverrideRow {
    scope_type: String,
    scope_id: String,
    target_type: String,
    target_id: String,
    property: String,
    value_type: String,
    value_text: Option<String>,
    value_real: Option<f64>,
    value_integer: Option<i64>,
    value_json: Option<String>,
}

struct RenderOverrideValue<'a> {
    value_type: &'a str,
    value_text: Option<&'a str>,
    value_real: Option<f64>,
    value_integer: Option<i64>,
    value_json: Option<&'a str>,
}

fn set_render_override_text(
    db: &Connection,
    target: RenderTarget<'_>,
    property: &str,
    value: &str,
) -> Result<()> {
    insert_render_override(
        db,
        target,
        property,
        RenderOverrideValue {
            value_type: "text",
            value_text: Some(value),
            value_real: None,
            value_integer: None,
            value_json: None,
        },
    )
}

fn set_render_override_real(
    db: &Connection,
    target: RenderTarget<'_>,
    property: &str,
    value: f32,
) -> Result<()> {
    insert_render_override(
        db,
        target,
        property,
        RenderOverrideValue {
            value_type: "real",
            value_text: None,
            value_real: Some(f64::from(value)),
            value_integer: None,
            value_json: None,
        },
    )
}

fn set_render_override_integer(
    db: &Connection,
    target: RenderTarget<'_>,
    property: &str,
    value: i64,
) -> Result<()> {
    insert_render_override(
        db,
        target,
        property,
        RenderOverrideValue {
            value_type: "integer",
            value_text: None,
            value_real: None,
            value_integer: Some(value),
            value_json: None,
        },
    )
}

fn set_render_override_bool(
    db: &Connection,
    target: RenderTarget<'_>,
    property: &str,
    value: bool,
) -> Result<()> {
    set_render_override_integer(db, target, property, bool_to_i64(value))
}

fn set_render_override_json(
    db: &Connection,
    target: RenderTarget<'_>,
    property: &str,
    value: serde_json::Value,
) -> Result<()> {
    insert_render_override(
        db,
        target,
        property,
        RenderOverrideValue {
            value_type: "json",
            value_text: None,
            value_real: None,
            value_integer: None,
            value_json: Some(&value.to_string()),
        },
    )
}

fn insert_render_override(
    db: &Connection,
    target: RenderTarget<'_>,
    property: &str,
    value: RenderOverrideValue<'_>,
) -> Result<()> {
    db.execute(
        "insert into render_overrides (scope_type, scope_id, target_type, target_id, property, value_type, value_text, value_real, value_integer, value_json, priority) values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            target.scope_type,
            target.scope_id,
            target.target_type,
            target.target_id,
            property,
            value.value_type,
            value.value_text,
            value.value_real,
            value.value_integer,
            value.value_json,
            target.priority,
        ],
    )?;
    Ok(())
}

fn apply_render_override_row(view: &mut ProjectViewSettings, row: RenderOverrideRow) -> Result<()> {
    let viewport = match row.scope_type.as_str() {
        "project" if row.scope_id == "project" => &mut view.viewport,
        "entry" => {
            let Ok(entry_id) = row.scope_id.parse::<u64>() else {
                return Ok(());
            };
            view.entry_viewports.entry(entry_id).or_default()
        }
        _ => return Ok(()),
    };
    match row.target_type.as_str() {
        "view" => apply_view_override(viewport, &row)?,
        "chain" => apply_chain_override(viewport, &row)?,
        "atom_category" => apply_atom_category_override(viewport, &row)?,
        "atom" => apply_atom_override(viewport, &row),
        _ => {}
    }
    Ok(())
}

fn apply_atom_override(viewport: &mut ViewportVisualState, row: &RenderOverrideRow) {
    let Ok(index) = row.target_id.parse::<usize>() else {
        return;
    };
    match row.property.as_str() {
        "style" => {
            if let Some(style) = row.value_text.as_deref().and_then(AtomStyle::from_token) {
                viewport.atom_styles.insert(index, style);
            }
        }
        "hidden" if row.value_integer.unwrap_or_default() != 0 => {
            viewport.atom_hidden.insert(index);
        }
        _ => {}
    }
}

fn apply_view_override(viewport: &mut ViewportVisualState, row: &RenderOverrideRow) -> Result<()> {
    match row.property.as_str() {
        "background_color" => {
            if let Some(color) = row.json_value()?.as_ref().and_then(parse_color_json) {
                viewport.background_color = color;
            }
        }
        "show_cell" => set_bool_from_integer(row.value_integer, &mut viewport.show_cell),
        "light_preset" => {
            if let Some(token) = row.value_text.as_deref() {
                viewport.lighting.preset = parse_light_preset(token);
            }
        }
        "silhouettes" => {
            set_bool_from_integer(row.value_integer, &mut viewport.lighting.silhouettes)
        }
        "silhouette_width" => {
            set_f32_from_real(row.value_real, &mut viewport.lighting.silhouette_width)
        }
        "cartoon_helix" => {
            if let Some(value) = row.json_value()? {
                apply_cartoon_section(&value, &mut viewport.cartoon.helix);
            }
        }
        "cartoon_sheet" => {
            if let Some(value) = row.json_value()? {
                apply_cartoon_section(&value, &mut viewport.cartoon.sheet);
            }
        }
        "cartoon_coil" => {
            if let Some(value) = row.json_value()? {
                apply_cartoon_section(&value, &mut viewport.cartoon.coil);
            }
        }
        "cartoon_smoothing" => {
            if let Some(value) = row.value_integer {
                viewport.cartoon.smoothing = value.max(1) as usize;
            }
        }
        "cartoon_profile_segments" => {
            if let Some(value) = row.value_integer {
                viewport.cartoon.profile_segments = value.max(1) as usize;
            }
        }
        "surface_style" => {
            if let Some(token) = row.value_text.as_deref() {
                viewport.surface.style = parse_surface_style(token);
            }
        }
        "surface_transparency" => {
            set_f32_from_real(row.value_real, &mut viewport.surface.transparency)
        }
        _ => {}
    }
    Ok(())
}

fn apply_chain_override(viewport: &mut ViewportVisualState, row: &RenderOverrideRow) -> Result<()> {
    let chain = string_to_char(&row.target_id);
    match row.property.as_str() {
        "color" => {
            if let Some(color) = row.json_value()?.as_ref().and_then(parse_color_json) {
                viewport.chain_colors.insert(chain, color);
            }
        }
        "surface_visible" => {
            if row.value_integer.unwrap_or_default() != 0 {
                viewport.surface.chains.insert(chain);
            } else {
                viewport.surface.chains.remove(&chain);
            }
        }
        _ => {}
    }
    Ok(())
}

fn apply_atom_category_override(
    viewport: &mut ViewportVisualState,
    row: &RenderOverrideRow,
) -> Result<()> {
    // Project-level category style override (e.g. solvent → wireframe).
    if row.property == "style"
        && let (Some(category), Some(style)) = (
            AtomCategory::from_token(&row.target_id),
            row.value_text.as_deref().and_then(AtomStyle::from_token),
        )
    {
        viewport.category_styles.insert(category, style);
        return Ok(());
    }
    match (row.target_id.as_str(), row.property.as_str()) {
        ("ion", "show_within") => {
            viewport.ions.show_within = match row.value_type.as_str() {
                "real" => row.value_real.map(|value| value as f32),
                "json" => None,
                _ => viewport.ions.show_within,
            };
        }
        ("ion", "color") => {
            viewport.ions.color = row.json_value()?.as_ref().and_then(parse_color_json);
        }
        ("hetero", "auto_color") => {
            set_bool_from_integer(row.value_integer, &mut viewport.hetero_atom_colors);
        }
        _ => {}
    }
    Ok(())
}

impl RenderOverrideRow {
    fn json_value(&self) -> Result<Option<serde_json::Value>> {
        let Some(source) = self.value_json.as_deref() else {
            return Ok(None);
        };
        serde_json::from_str(source)
            .with_context(|| format!("failed to parse render override {}", self.property))
            .map(Some)
    }
}

fn save_cartoon_setting(
    db: &Connection,
    scope: RenderScope,
    key: &str,
    section: crate::frontend::CartoonSectionStyle,
    default_section: crate::frontend::CartoonSectionStyle,
) -> Result<()> {
    if section.width != default_section.width || section.thickness != default_section.thickness {
        set_render_override_json(
            db,
            RenderTarget::view(scope),
            key,
            serde_json::json!({
                "width": section.width,
                "thickness": section.thickness,
            }),
        )?;
    }
    Ok(())
}

fn save_structure(
    db: &Connection,
    compound_id: i64,
    revision: i64,
    structure: &Structure,
) -> Result<()> {
    let blob = encode_structure(structure)?;
    let kind = if structure.biopolymer.is_some() {
        "biopolymer"
    } else if structure.cell.is_some() {
        "periodic"
    } else {
        "structure"
    };
    db.execute(
        "insert or replace into compounds (id, title, kind, atom_count, bond_count, revision, format, payload, uncompressed_len) values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            compound_id,
            structure.title,
            kind,
            structure.atoms.len() as i64,
            structure.bonds.len() as i64,
            revision,
            PAYLOAD_FORMAT,
            blob.bytes,
            blob.uncompressed_len as i64,
        ],
    )?;
    Ok(())
}

fn load_structure(db: &Connection, compound_id: i64) -> Result<Structure> {
    let (payload, uncompressed_len) = db.query_row(
        "select payload, uncompressed_len from compounds where id = ?1",
        params![compound_id],
        |row| Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, i64>(1)? as usize)),
    )?;
    decode_structure(&payload, uncompressed_len)
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

fn path_to_string(path: impl AsRef<std::path::Path>) -> String {
    path.as_ref().to_string_lossy().to_string()
}

fn light_preset_token(preset: LightPreset) -> &'static str {
    match preset {
        LightPreset::Soft => "soft",
        LightPreset::Gentle => "gentle",
        LightPreset::Studio => "studio",
    }
}

fn parse_light_preset(token: &str) -> LightPreset {
    match token {
        "gentle" => LightPreset::Gentle,
        "studio" => LightPreset::Studio,
        _ => LightPreset::Soft,
    }
}

fn surface_style_token(style: SurfaceStyle) -> &'static str {
    match style {
        SurfaceStyle::Fill => "fill",
        SurfaceStyle::Mesh => "mesh",
    }
}

fn parse_surface_style(token: &str) -> SurfaceStyle {
    match token {
        "mesh" => SurfaceStyle::Mesh,
        _ => SurfaceStyle::Fill,
    }
}

fn color_json(color: Color32) -> serde_json::Value {
    serde_json::json!([color.r(), color.g(), color.b(), color.a()])
}

fn parse_color_json(value: &serde_json::Value) -> Option<Color32> {
    let channels = value.as_array()?;
    Some(Color32::from_rgba_unmultiplied(
        channels.first()?.as_u64()? as u8,
        channels.get(1)?.as_u64()? as u8,
        channels.get(2)?.as_u64()? as u8,
        channels.get(3)?.as_u64()? as u8,
    ))
}

fn set_bool_from_integer(value: Option<i64>, target: &mut bool) {
    if let Some(value) = value {
        *target = value != 0;
    }
}

fn set_f32_from_real(value: Option<f64>, target: &mut f32) {
    if let Some(value) = value {
        *target = value as f32;
    }
}

fn apply_cartoon_section(
    value: &serde_json::Value,
    section: &mut crate::frontend::CartoonSectionStyle,
) {
    if let Some(width) = value.get("width").and_then(serde_json::Value::as_f64) {
        section.width = width as f32;
    }
    if let Some(thickness) = value.get("thickness").and_then(serde_json::Value::as_f64) {
        section.thickness = thickness as f32;
    }
}

fn char_to_string(value: char) -> String {
    value.to_string()
}

fn string_to_char(value: &str) -> char {
    value.chars().next().unwrap_or(' ')
}

fn bool_to_i64(value: bool) -> i64 {
    i64::from(value)
}

fn task_status_token(status: TaskStatus) -> &'static str {
    match status {
        TaskStatus::Ready => "ready",
        TaskStatus::WaitingInput => "waiting_input",
        TaskStatus::Running => "running",
        TaskStatus::Completed => "completed",
        TaskStatus::Failed => "failed",
    }
}

fn parse_task_status(token: &str) -> TaskStatus {
    match token {
        "waiting_input" => TaskStatus::WaitingInput,
        "running" => TaskStatus::Running,
        "completed" => TaskStatus::Completed,
        "failed" => TaskStatus::Failed,
        _ => TaskStatus::Ready,
    }
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

#[cfg(test)]
mod tests {
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
}
