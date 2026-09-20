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

#[derive(Debug, Clone)]
pub(crate) struct ImageExportPrompt {
    pub path: String,
    pub width: String,
    pub height: String,
    pub background: crate::frontend::viewport::ImageExportBackground,
}

impl ImageExportPrompt {
    pub fn new(size: [u32; 2]) -> Self {
        Self {
            path: "image.png".into(),
            width: size[0].to_string(),
            height: size[1].to_string(),
            background: Default::default(),
        }
    }

    pub fn validate(&self) -> anyhow::Result<(std::path::PathBuf, [u32; 2])> {
        anyhow::ensure!(!self.path.trim().is_empty(), "Choose a PNG output path");
        let path = std::path::PathBuf::from(&self.path);
        anyhow::ensure!(
            path.extension()
                .and_then(|s| s.to_str())
                .is_some_and(|s| s.eq_ignore_ascii_case("png")),
            "Output path must end in .png"
        );
        anyhow::ensure!(!path.is_dir(), "Output path is a directory");
        let dimension = |text: &str| -> anyhow::Result<u32> {
            let value = text.trim().parse::<u32>().ok().filter(|v| *v > 0);
            value.ok_or_else(|| anyhow::anyhow!("Width and height must be positive integers"))
        };
        if let crate::frontend::viewport::ImageExportBackground::Custom(color) = self.background {
            anyhow::ensure!(color.a() == 255, "Custom background must be opaque RGB");
        }
        Ok((path, [dimension(&self.width)?, dimension(&self.height)?]))
    }

    pub fn apply_chosen_path(&mut self, path: Option<std::path::PathBuf>) {
        if let Some(path) = path {
            self.path = path.to_string_lossy().into_owned();
        }
    }
}
