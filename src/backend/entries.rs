use std::path::{Path, PathBuf};

use crate::domain::Structure;

#[derive(Debug, Clone)]
pub struct EntryGroup {
    pub id: String,
    pub name: String,
}

/// Where an entry's structure came from. Drives provenance labelling in the UI
/// (e.g. an "MD" badge) and feature availability (trajectory playback). New
/// provenance kinds can be added as variants without disturbing existing ones.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum EntryOrigin {
    /// Imported or created by the user (the default for normal entries).
    #[default]
    User,
    /// Produced by a molecular-dynamics engine run. `trajectory`, when present,
    /// is the run's trajectory file *relative to the project root*; it stays in
    /// the task run directory (never copied into the project database) and is
    /// read on demand for playback.
    MdRun { trajectory: Option<PathBuf> },
    /// Produced by a quantum-mechanics calculation. `output`, when present, is
    /// the run's saved output report (e.g. `runs/qm-optimize-1/output.txt`)
    /// *relative to the project root*; clicking the entry's "QM" badge opens it.
    QmRun { output: Option<PathBuf> },
    /// Produced by a molecular docking run. `poses`, when present, is the run's
    /// saved multi-pose `.pdbqt` artifact (e.g. `runs/dock-ligand-1/poses.pdbqt`)
    /// *relative to the project root*; clicking the entry's "Dock" badge opens it.
    DockRun { poses: Option<PathBuf> },
}

impl EntryOrigin {
    /// Short stable token persisted in the `entries.origin_kind` column.
    pub fn kind_token(&self) -> &'static str {
        match self {
            Self::User => "user",
            Self::MdRun { .. } => "md",
            Self::QmRun { .. } => "qm",
            Self::DockRun { .. } => "dock",
        }
    }

    /// Project-relative trajectory path, when this origin carries one.
    pub fn trajectory(&self) -> Option<&Path> {
        match self {
            Self::MdRun { trajectory } => trajectory.as_deref(),
            _ => None,
        }
    }

    /// Project-relative QM output-report path, when this origin carries one.
    pub fn qm_output(&self) -> Option<&Path> {
        match self {
            Self::QmRun { output } => output.as_deref(),
            _ => None,
        }
    }

    /// Project-relative docking poses `.pdbqt` path, when this origin carries one.
    pub fn dock_poses(&self) -> Option<&Path> {
        match self {
            Self::DockRun { poses } => poses.as_deref(),
            _ => None,
        }
    }

    /// The path persisted alongside the kind in the `entries.origin_trajectory`
    /// column: the MD trajectory, the QM output report, or the docking poses file,
    /// depending on the kind.
    pub fn stored_path(&self) -> Option<&Path> {
        match self {
            Self::MdRun { trajectory } => trajectory.as_deref(),
            Self::QmRun { output } => output.as_deref(),
            Self::DockRun { poses } => poses.as_deref(),
            Self::User => None,
        }
    }

    /// Whether this entry is the output of an MD run (used for the badge and to
    /// gate trajectory playback).
    pub fn is_md_run(&self) -> bool {
        matches!(self, Self::MdRun { .. })
    }

    /// Whether this entry is the output of a QM calculation (used for the badge
    /// and to open the saved output report).
    pub fn is_qm_run(&self) -> bool {
        matches!(self, Self::QmRun { .. })
    }

    /// Whether this entry is a docking pose (used for the badge and to open the
    /// saved poses file).
    pub fn is_dock_run(&self) -> bool {
        matches!(self, Self::DockRun { .. })
    }

    /// Rebuild an origin from its persisted `(origin_kind, origin_trajectory)`
    /// columns (the path column holds the trajectory for MD runs, the output
    /// report for QM runs, and the poses file for docking runs).
    pub fn from_storage(kind: Option<&str>, path: Option<PathBuf>) -> Self {
        match kind {
            Some("md") => Self::MdRun { trajectory: path },
            Some("qm") => Self::QmRun { output: path },
            Some("dock") => Self::DockRun { poses: path },
            _ => Self::User,
        }
    }
}

#[derive(Debug, Clone)]
pub struct EntryRecord {
    pub id: u64,
    pub name: String,
    pub group_id: String,
    pub structure: Structure,
    pub revision: u64,
    pub source_path: Option<PathBuf>,
    /// Where this entry was last exported, and so where a no-dialog re-export
    /// writes. Only an absolute path is a target the user chose; the relative
    /// default (`edited.xyz`) means "never exported, ask first".
    pub save_path: PathBuf,
    pub compound_id: Option<i64>,
    /// Whether `structure` holds the real geometry. When a project is opened,
    /// only entries with an open tab are materialized; the rest carry a
    /// placeholder structure with `loaded = false` until they are activated.
    pub loaded: bool,
    /// Provenance of this entry (user import vs. MD-run output, ...).
    pub origin: EntryOrigin,
}

impl EntryRecord {
    pub fn new(
        id: u64,
        structure: Structure,
        source_path: Option<PathBuf>,
        save_path: PathBuf,
        group_id: String,
    ) -> Self {
        Self {
            id,
            name: normalize_entry_name(&structure),
            group_id,
            structure,
            revision: 0,
            source_path,
            save_path,
            compound_id: None,
            loaded: true,
            origin: EntryOrigin::User,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct WorkspaceTab {
    pub entry_id: u64,
}

#[derive(Debug, Clone)]
pub struct EntryRecordMetadata {
    pub id: u64,
    pub name: String,
    pub structure: Structure,
    pub source_path: Option<PathBuf>,
    pub save_path: PathBuf,
    pub group_id: String,
    pub compound_id: Option<i64>,
    pub revision: u64,
    pub loaded: bool,
    pub origin: EntryOrigin,
}

#[derive(Debug, Clone)]
pub struct EntryStore {
    pub groups: Vec<EntryGroup>,
    pub records: Vec<EntryRecord>,
    pub tabs: Vec<WorkspaceTab>,
    pub active_tab: usize,
    pub(crate) next_entry_id: u64,
    pub(crate) next_group_id: u64,
}

impl EntryStore {
    pub fn new_empty() -> Self {
        Self {
            groups: Vec::new(),
            records: Vec::new(),
            tabs: Vec::new(),
            active_tab: 0,
            next_entry_id: 1,
            next_group_id: 1,
        }
    }

    pub fn with_initial(
        initial: Structure,
        source_path: Option<PathBuf>,
        save_path: PathBuf,
    ) -> Self {
        let mut store = Self::new_empty();
        let group_id = String::new();
        let entry_id = store.insert_entry(initial, source_path, save_path, group_id);
        store.tabs.push(WorkspaceTab { entry_id });
        store
    }

    pub fn active_entry_id(&self) -> Option<u64> {
        self.tabs.get(self.active_tab).map(|tab| tab.entry_id)
    }

    pub fn active_entry(&self) -> Option<&EntryRecord> {
        self.active_entry_id()
            .and_then(|entry_id| self.entry(entry_id))
    }

    pub fn active_entry_mut(&mut self) -> Option<&mut EntryRecord> {
        let entry_id = self.active_entry_id()?;
        self.entry_mut(entry_id)
    }

    pub fn bump_active_revision(&mut self) {
        if let Some(entry) = self.active_entry_mut() {
            entry.revision = entry.revision.wrapping_add(1);
        }
    }

    pub fn entry(&self, entry_id: u64) -> Option<&EntryRecord> {
        self.records.iter().find(|entry| entry.id == entry_id)
    }

    pub fn entry_mut(&mut self, entry_id: u64) -> Option<&mut EntryRecord> {
        self.records.iter_mut().find(|entry| entry.id == entry_id)
    }

    pub fn group(&self, group_id: &str) -> Option<&EntryGroup> {
        self.groups.iter().find(|group| group.id == group_id)
    }

    pub fn add_entry(
        &mut self,
        structure: Structure,
        source_path: Option<PathBuf>,
        save_path: PathBuf,
    ) -> u64 {
        let group_id = String::new();
        let entry_id = self.insert_entry(structure, source_path, save_path, group_id);
        self.ensure_tab_for_entry(entry_id);
        self.activate_entry(entry_id);
        entry_id
    }

    /// Insert an entry into `group_id` (empty for ungrouped) with an optional
    /// explicit `name` (falling back to the title-derived name). When `activate`
    /// is set, the entry gets an open tab and becomes active; otherwise it is
    /// added to the entry list without opening a tab. Used to import a
    /// multi-model PDB as a group where only the first model opens.
    pub fn add_entry_to_group(
        &mut self,
        structure: Structure,
        source_path: Option<PathBuf>,
        save_path: PathBuf,
        group_id: String,
        name: Option<String>,
        activate: bool,
    ) -> u64 {
        let entry_id = self.insert_entry(structure, source_path, save_path, group_id);
        if let Some(name) = name {
            self.rename_entry(entry_id, name);
        }
        if activate {
            self.ensure_tab_for_entry(entry_id);
            self.activate_entry(entry_id);
        }
        entry_id
    }

    fn insert_entry(
        &mut self,
        structure: Structure,
        source_path: Option<PathBuf>,
        save_path: PathBuf,
        group_id: String,
    ) -> u64 {
        let id = self.next_entry_id;
        self.next_entry_id += 1;
        self.records.push(EntryRecord::new(
            id,
            structure,
            source_path,
            save_path,
            group_id,
        ));
        id
    }

    pub fn insert_entry_with_metadata(&mut self, metadata: EntryRecordMetadata) -> u64 {
        let id = metadata.id;
        self.next_entry_id = self.next_entry_id.max(id + 1);
        self.records.push(EntryRecord {
            id: metadata.id,
            name: metadata.name,
            group_id: metadata.group_id,
            structure: metadata.structure,
            revision: metadata.revision,
            source_path: metadata.source_path,
            save_path: metadata.save_path,
            compound_id: metadata.compound_id,
            loaded: metadata.loaded,
            origin: metadata.origin,
        });
        id
    }

    /// Record the provenance of an existing entry (e.g. mark it as the output of
    /// an MD run once the run's trajectory path is known).
    pub fn set_entry_origin(&mut self, entry_id: u64, origin: EntryOrigin) {
        if let Some(entry) = self.entry_mut(entry_id) {
            entry.origin = origin;
        }
    }

    pub fn recompute_next_ids(&mut self) {
        self.next_entry_id = self
            .records
            .iter()
            .map(|entry| entry.id + 1)
            .max()
            .unwrap_or(1);
        self.next_group_id = self
            .groups
            .iter()
            .filter_map(|group| group.id.strip_prefix("group-"))
            .filter_map(|suffix| suffix.parse::<u64>().ok())
            .map(|id| id + 1)
            .max()
            .unwrap_or(1);
    }

    pub fn activate_entry(&mut self, entry_id: u64) {
        let tab_index = self.ensure_tab_for_entry(entry_id);
        self.active_tab = tab_index;
    }

    pub fn ensure_tab_for_entry(&mut self, entry_id: u64) -> usize {
        if let Some(index) = self.tabs.iter().position(|tab| tab.entry_id == entry_id) {
            index
        } else {
            self.tabs.push(WorkspaceTab { entry_id });
            self.tabs.len() - 1
        }
    }

    pub fn close_tab(&mut self, index: usize) -> Option<u64> {
        if index >= self.tabs.len() {
            return self.active_entry_id();
        }

        self.tabs.remove(index);
        if self.tabs.is_empty() {
            self.active_tab = 0;
        } else if self.active_tab >= self.tabs.len() {
            self.active_tab = self.tabs.len() - 1;
        } else if index < self.active_tab {
            self.active_tab -= 1;
        }
        self.active_entry_id()
    }

    pub fn delete_entry(&mut self, entry_id: u64) -> bool {
        let Some(entry_index) = self.records.iter().position(|entry| entry.id == entry_id) else {
            return false;
        };
        let active_entry_id = self.active_entry_id();
        self.records.remove(entry_index);
        self.tabs.retain(|tab| tab.entry_id != entry_id);

        if self.tabs.is_empty() {
            self.active_tab = 0;
        } else if active_entry_id == Some(entry_id) {
            self.active_tab = self.active_tab.min(self.tabs.len() - 1);
        } else if let Some(active_entry_id) = active_entry_id {
            self.active_tab = self
                .tabs
                .iter()
                .position(|tab| tab.entry_id == active_entry_id)
                .unwrap_or_else(|| self.active_tab.min(self.tabs.len() - 1));
        } else {
            self.active_tab = self.active_tab.min(self.tabs.len() - 1);
        }

        true
    }

    pub fn rename_entry(&mut self, entry_id: u64, new_name: String) {
        if let Some(entry) = self.entry_mut(entry_id) {
            let trimmed = new_name.trim();
            if !trimmed.is_empty() {
                entry.name = trimmed.to_string();
            }
        }
    }

    pub fn create_group(&mut self, name: &str) -> Option<String> {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return None;
        }
        let id = format!("group-{}", self.next_group_id);
        self.next_group_id += 1;
        self.groups.push(EntryGroup {
            id: id.clone(),
            name: trimmed.to_string(),
        });
        Some(id)
    }

    pub fn rename_group(&mut self, group_id: &str, new_name: &str) {
        let trimmed = new_name.trim();
        if trimmed.is_empty() {
            return;
        }
        if let Some(group) = self.groups.iter_mut().find(|group| group.id == group_id) {
            group.name = trimmed.to_string();
        }
    }

    pub fn delete_group(&mut self, group_id: &str) -> bool {
        let Some(index) = self.groups.iter().position(|group| group.id == group_id) else {
            return false;
        };
        // Entries in the deleted group become ungrouped, not reassigned.
        for entry in &mut self.records {
            if entry.group_id == group_id {
                entry.group_id.clear();
            }
        }
        self.groups.remove(index);
        true
    }

    pub fn move_entry_to_group(&mut self, entry_id: u64, group_id: &str) -> bool {
        if !group_id.is_empty() && self.group(group_id).is_none() {
            return false;
        }
        if let Some(entry) = self.entry_mut(entry_id) {
            entry.group_id = group_id.to_string();
            return true;
        }
        false
    }
}

pub fn normalize_entry_name(structure: &Structure) -> String {
    let trimmed = structure.title.trim();
    if trimmed.is_empty() {
        "Untitled".to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use crate::domain::Structure;

    use super::{EntryOrigin, EntryStore};

    #[test]
    fn origin_round_trips_through_storage_columns() {
        for origin in [
            EntryOrigin::User,
            EntryOrigin::MdRun {
                trajectory: Some(PathBuf::from("runs/run-md-1/prod.xtc")),
            },
            EntryOrigin::QmRun {
                output: Some(PathBuf::from("runs/qm-optimize-1/output.txt")),
            },
            EntryOrigin::QmRun { output: None },
        ] {
            let restored = EntryOrigin::from_storage(
                Some(origin.kind_token()),
                origin.stored_path().map(Path::to_path_buf),
            );
            assert_eq!(restored, origin);
        }
    }

    #[test]
    fn store_can_start_empty() {
        let store = EntryStore::new_empty();

        assert_eq!(store.active_entry_id(), None);
        assert_eq!(store.records.len(), 0);
        assert_eq!(store.tabs.len(), 0);
        assert_eq!(store.groups.len(), 0);
    }

    #[test]
    fn store_initializes_first_entry_and_tab_when_requested() {
        let store = EntryStore::with_initial(
            Structure::empty(),
            None,
            PathBuf::from(r"C:\tmp\untitled.xyz"),
        );

        assert_eq!(store.active_entry_id(), Some(1));
        assert_eq!(store.records.len(), 1);
        assert_eq!(store.tabs.len(), 1);
        assert_eq!(store.groups.len(), 0);
        assert_eq!(store.records[0].group_id, "");
    }

    #[test]
    fn rename_entry_keeps_existing_name_when_blank() {
        let mut store = EntryStore::with_initial(
            Structure::empty(),
            None,
            PathBuf::from(r"C:\tmp\untitled.xyz"),
        );
        let id = store.active_entry_id().expect("seeded entry");
        let original = store.entry(id).expect("entry exists").name.clone();

        store.rename_entry(id, "   ".to_string());
        assert_eq!(store.entry(id).unwrap().name, original);

        store.rename_entry(id, "  Ethanol  ".to_string());
        assert_eq!(store.entry(id).unwrap().name, "Ethanol");
    }

    #[test]
    fn deleting_group_moves_entries_to_fallback() {
        let mut store = EntryStore::with_initial(
            Structure::empty(),
            None,
            PathBuf::from(r"C:\tmp\untitled.xyz"),
        );
        let custom = store.create_group("Favorites").unwrap();
        let entry_id = store.add_entry(Structure::empty(), None, PathBuf::from("edited.xyz"));
        assert!(store.move_entry_to_group(entry_id, &custom));

        assert!(store.delete_group(&custom));
        assert_ne!(store.entry(entry_id).unwrap().group_id, custom);
    }

    #[test]
    fn entries_can_be_moved_back_to_ungrouped() {
        let mut store = EntryStore::with_initial(
            Structure::empty(),
            None,
            PathBuf::from(r"C:\tmp\untitled.xyz"),
        );
        let custom = store.create_group("Favorites").unwrap();
        let entry_id = store.add_entry(Structure::empty(), None, PathBuf::from("edited.xyz"));

        assert!(store.move_entry_to_group(entry_id, &custom));
        assert!(store.move_entry_to_group(entry_id, ""));
        assert_eq!(store.entry(entry_id).unwrap().group_id, "");
    }

    #[test]
    fn closing_last_tab_leaves_zero_open_tabs() {
        let mut store = EntryStore::with_initial(
            Structure::empty(),
            None,
            PathBuf::from(r"C:\tmp\untitled.xyz"),
        );

        let active_entry = store.close_tab(0);

        assert_eq!(active_entry, None);
        assert_eq!(store.active_entry_id(), None);
        assert_eq!(store.tabs.len(), 0);
        assert_eq!(store.records.len(), 1);
    }

    #[test]
    fn deleting_entry_removes_tabs_and_selects_neighbor() {
        let mut store = EntryStore::with_initial(
            Structure::empty(),
            None,
            PathBuf::from(r"C:\tmp\untitled.xyz"),
        );
        let second = store.add_entry(Structure::empty(), None, PathBuf::from("second.xyz"));
        let third = store.add_entry(Structure::empty(), None, PathBuf::from("third.xyz"));
        store.activate_entry(second);

        assert!(store.delete_entry(second));

        assert!(store.entry(second).is_none());
        assert!(store.tabs.iter().all(|tab| tab.entry_id != second));
        assert_eq!(store.active_entry_id(), Some(third));
    }
}
