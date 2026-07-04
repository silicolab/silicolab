//! Persisted chart export preferences, re-exported from `backend::config`.

use serde::{Deserialize, Serialize};

/// Last-used chart export choices; updated after each successful export so
/// the dialog reopens as the user left it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChartExportPrefs {
    pub format: crate::plot::spec::ExportFormat,
    pub preset: crate::plot::spec::PresetChoice,
    pub dpi: u32,
}

impl Default for ChartExportPrefs {
    fn default() -> Self {
        Self {
            format: crate::plot::spec::ExportFormat::Png,
            preset: crate::plot::spec::PresetChoice::SingleColumn,
            dpi: 300,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ChartExportPrefs;
    use crate::backend::config::AppConfig;

    #[test]
    fn chart_export_prefs_round_trip_and_default_when_missing() {
        let mut config = AppConfig::default();
        config.chart_export.dpi = 600;
        let json = serde_json::to_string(&config).unwrap();
        let back: AppConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back.chart_export.dpi, 600);

        // Older settings.json has no chart_export key at all.
        let mut value: serde_json::Value = serde_json::from_str(&json).unwrap();
        value.as_object_mut().unwrap().remove("chart_export");
        let legacy: AppConfig = serde_json::from_str(&value.to_string()).unwrap();
        assert_eq!(legacy.chart_export, ChartExportPrefs::default());
    }
}
