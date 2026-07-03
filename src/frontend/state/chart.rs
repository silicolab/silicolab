use crate::plot::spec::{ChartSpec, ExportFormat, PresetChoice};

/// The Plot panel's loaded chart: every dataset from one run's `series.json`
/// plus the panel's view/export state. `None` in `UiState` means the panel
/// shows its empty state.
#[derive(Debug, Clone)]
pub struct ChartState {
    /// Entry or task name; seeds export file names.
    pub source_name: String,
    pub datasets: Vec<ChartSpec>,
    pub active: usize,
    /// Inline empty-state text (e.g. "Data file missing") when loading failed.
    pub error: Option<String>,
    /// Plot bounds of the last rendered frame, for "current view" exports.
    pub view_bounds: Option<[[f64; 2]; 2]>,
    pub export_open: bool,
    pub export_draft: ChartExportDraft,
}

impl ChartState {
    pub fn new(source_name: String) -> Self {
        Self {
            source_name,
            datasets: Vec::new(),
            active: 0,
            error: None,
            view_bounds: None,
            export_open: false,
            export_draft: ChartExportDraft::default(),
        }
    }

    pub fn active_dataset(&self) -> Option<&ChartSpec> {
        self.datasets.get(self.active)
    }

    pub fn active_dataset_mut(&mut self) -> Option<&mut ChartSpec> {
        self.datasets.get_mut(self.active)
    }
}

/// Draft choices in the export dialog. Local widget state (like the pending-*
/// prompts); the dispatcher persists the last-used values on a successful
/// export.
#[derive(Debug, Clone)]
pub struct ChartExportDraft {
    pub format: ExportFormat,
    pub preset: PresetChoice,
    pub dpi: u32,
    pub current_view: bool,
}

impl Default for ChartExportDraft {
    fn default() -> Self {
        Self {
            format: ExportFormat::Png,
            preset: PresetChoice::SingleColumn,
            dpi: 300,
            current_view: false,
        }
    }
}

impl ChartExportDraft {
    pub fn from_prefs(prefs: &crate::backend::config::ChartExportPrefs) -> Self {
        Self {
            format: prefs.format,
            preset: prefs.preset,
            dpi: prefs.dpi,
            current_view: false,
        }
    }
}
