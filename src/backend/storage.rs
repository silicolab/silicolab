use std::collections::BTreeMap;

use crate::{
    backend::{entries::EntryStore, history::History, tasks::TaskManager},
    frontend::ViewportVisualState,
};

mod entries;
mod history;
mod project;
mod render_overrides;
mod schema;
mod structure_blob;
mod tasks;
mod view_load;
mod view_save;

pub(crate) use entries::*;
pub(crate) use history::*;
pub use project::*;
pub(crate) use render_overrides::*;
pub(crate) use schema::*;
pub(crate) use structure_blob::*;
pub(crate) use tasks::*;
pub(crate) use view_load::*;
pub(crate) use view_save::*;

#[cfg(test)]
mod tests;

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
