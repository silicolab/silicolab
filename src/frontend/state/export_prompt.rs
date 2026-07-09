use crate::io::structure_format::{MultiStructureFile, StructureFormat};

/// Which structures an export writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportScope {
    /// The entries selected in the sidebar when the dialog opened.
    Selected,
    /// The entry in the active tab.
    Active,
    /// Every entry in the project.
    All,
}

/// How an export of several structures lands on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportLayout {
    SingleFile,
    FilePerStructure,
}

/// Draft state of the open Export dialog.
#[derive(Debug, Clone)]
pub struct ExportPrompt {
    pub scope: ExportScope,
    pub format: StructureFormat,
    pub layout: ExportLayout,
    /// Entries selected in the sidebar when the dialog opened. Captured rather
    /// than read live so a click behind the dialog cannot change what it writes.
    pub selected_entry_ids: Vec<u64>,
}

impl ExportPrompt {
    pub fn new(scope: ExportScope, format: StructureFormat, selected_entry_ids: Vec<u64>) -> Self {
        Self {
            scope,
            format,
            layout: ExportLayout::SingleFile,
            selected_entry_ids,
        }
    }

    /// Whether `count` structures may share one file in the chosen format.
    pub fn can_combine(&self, count: usize) -> bool {
        count <= 1 || self.format.multi_structure_file() == MultiStructureFile::Concatenated
    }

    /// The layout actually used for `count` structures: a format that cannot
    /// combine falls back to one file each, whatever the draft says.
    pub fn effective_layout(&self, count: usize) -> ExportLayout {
        if self.can_combine(count) {
            self.layout
        } else {
            ExportLayout::FilePerStructure
        }
    }
}
