use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::{domain::Structure, frontend::AtomSelection};

/// Upper bound on how many undo steps are kept (and persisted) per entry. Each
/// step holds a full structure snapshot, so this caps both memory and the size
/// of the persisted history.
pub const MAX_UNDO_DEPTH: usize = 64;

#[derive(Debug, Clone)]
pub struct EditSnapshot {
    pub structure: Structure,
    pub source_path: Option<PathBuf>,
    pub save_path: PathBuf,
    pub selection: AtomSelection,
}

#[derive(Debug, Clone, Default)]
pub struct EntryHistory {
    pub undo_stack: Vec<EditSnapshot>,
    pub redo_stack: Vec<EditSnapshot>,
}

/// Per-entry undo/redo history.
///
/// Each entry keeps its own undo and redo stacks, so switching between entries
/// preserves their histories instead of discarding them. All the routed methods
/// (`push_undo`, `take_undo`, …) operate on whichever entry is currently
/// active, which keeps call sites unchanged from the previous single-stack API.
#[derive(Debug, Clone, Default)]
pub struct History {
    entries: BTreeMap<u64, EntryHistory>,
    active: Option<u64>,
}

impl History {
    /// Make `entry_id` the entry that routed undo/redo operations target.
    pub fn set_active_entry(&mut self, entry_id: Option<u64>) {
        self.active = entry_id;
    }

    /// Forget a single entry's history (e.g. when the entry is deleted).
    pub fn forget_entry(&mut self, entry_id: u64) {
        self.entries.remove(&entry_id);
        if self.active == Some(entry_id) {
            self.active = None;
        }
    }

    /// Drop every entry's history.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.active = None;
    }

    pub fn push_undo(&mut self, snapshot: EditSnapshot) {
        let Some(entry_id) = self.active else {
            return;
        };
        let history = self.entries.entry(entry_id).or_default();
        history.undo_stack.push(snapshot);
        history.redo_stack.clear();
        if history.undo_stack.len() > MAX_UNDO_DEPTH {
            let overflow = history.undo_stack.len() - MAX_UNDO_DEPTH;
            history.undo_stack.drain(0..overflow);
        }
    }

    pub fn push_redo(&mut self, snapshot: EditSnapshot) {
        let Some(entry_id) = self.active else {
            return;
        };
        self.entries
            .entry(entry_id)
            .or_default()
            .redo_stack
            .push(snapshot);
    }

    pub fn take_undo(&mut self) -> Option<EditSnapshot> {
        let entry_id = self.active?;
        self.entries.get_mut(&entry_id)?.undo_stack.pop()
    }

    pub fn take_redo(&mut self) -> Option<EditSnapshot> {
        let entry_id = self.active?;
        self.entries.get_mut(&entry_id)?.redo_stack.pop()
    }

    pub fn can_undo(&self) -> bool {
        self.active
            .and_then(|entry_id| self.entries.get(&entry_id))
            .is_some_and(|history| !history.undo_stack.is_empty())
    }

    pub fn can_redo(&self) -> bool {
        self.active
            .and_then(|entry_id| self.entries.get(&entry_id))
            .is_some_and(|history| !history.redo_stack.is_empty())
    }

    /// Replace the stored history for a single entry (used when loading a
    /// project's persisted undo/redo stacks).
    pub fn set_entry_history(&mut self, entry_id: u64, history: EntryHistory) {
        if history.undo_stack.is_empty() && history.redo_stack.is_empty() {
            self.entries.remove(&entry_id);
        } else {
            self.entries.insert(entry_id, history);
        }
    }

    /// Iterate over every entry's history for persistence.
    pub fn iter_entries(&self) -> impl Iterator<Item = (u64, &EntryHistory)> {
        self.entries.iter().map(|(id, history)| (*id, history))
    }
}
