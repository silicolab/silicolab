use serde::{Deserialize, Serialize};

/// One renderable chart: a titled pair of axes plus one or more series in the
/// same units. Datasets with different units get separate `ChartSpec`s.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChartSpec {
    pub title: String,
    pub x: AxisSpec,
    pub y: AxisSpec,
    pub series: Vec<Series>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AxisSpec {
    pub label: String,
    /// Unit shown in parentheses after the label; empty for unitless axes.
    pub unit: String,
    /// Draw the axis with values decreasing left→right / bottom→top
    /// (spectroscopy convention; unused by the PR-1 QM datasets).
    pub inverted: bool,
    /// Explicit range override; `None` fits the data.
    pub range: Option<[f64; 2]>,
}

impl AxisSpec {
    pub fn new(label: &str, unit: &str) -> Self {
        Self {
            label: label.to_string(),
            unit: unit.to_string(),
            inverted: false,
            range: None,
        }
    }

    pub fn display_label(&self) -> String {
        if self.unit.is_empty() {
            self.label.clone()
        } else {
            format!("{} ({})", self.label, self.unit)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Series {
    pub name: String,
    pub points: Vec<[f64; 2]>,
    pub mark: Mark,
}

/// How a series is drawn: a polyline, or vertical sticks from the zero line
/// (spectral peaks).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Mark {
    Line,
    Sticks,
}

/// Physical figure geometry for export: page size in inches plus the print
/// type sizes in points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct JournalPreset {
    pub width_in: f64,
    pub height_in: f64,
    /// Title / axis-label size.
    pub base_pt: f64,
    /// Tick-label / legend size.
    pub tick_pt: f64,
}

/// The user-facing preset menu. Serialized into `settings.json` as the
/// remembered export choice, so variants keep stable names.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum PresetChoice {
    SingleColumn,
    DoubleColumn,
    Custom { width_in: f64, height_in: f64 },
}

impl PresetChoice {
    pub fn preset(self) -> JournalPreset {
        let (width_in, height_in) = match self {
            Self::SingleColumn => (3.3, 2.5),
            Self::DoubleColumn => (7.0, 4.2),
            Self::Custom {
                width_in,
                height_in,
            } => (width_in, height_in),
        };
        JournalPreset {
            width_in,
            height_in,
            base_pt: 8.0,
            tick_pt: 7.0,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::SingleColumn => "Single column (3.3 in)",
            Self::DoubleColumn => "Double column (7 in)",
            Self::Custom { .. } => "Custom",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExportFormat {
    Png,
    Svg,
    Pdf,
}

impl ExportFormat {
    pub fn all() -> [Self; 3] {
        [Self::Png, Self::Svg, Self::Pdf]
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Png => "PNG",
            Self::Svg => "SVG",
            Self::Pdf => "PDF",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Svg => "svg",
            Self::Pdf => "pdf",
        }
    }
}

/// Fixed print-light styling for exports: white background, near-black ink,
/// and the Okabe–Ito colorblind-safe series palette. Exports never follow the
/// app theme.
#[derive(Debug, Clone, PartialEq)]
pub struct ExportStyle {
    pub background: [u8; 3],
    pub ink: [u8; 3],
    pub grid: [u8; 3],
    pub series_colors: Vec<[u8; 3]>,
}

impl Default for ExportStyle {
    fn default() -> Self {
        Self {
            background: [255, 255, 255],
            ink: [26, 26, 26],
            grid: [210, 210, 210],
            series_colors: vec![
                [0, 114, 178],   // blue
                [213, 94, 0],    // vermillion
                [0, 158, 115],   // green
                [230, 159, 0],   // orange
                [86, 180, 233],  // sky blue
                [204, 121, 167], // reddish purple
                [240, 228, 66],  // yellow
                [0, 0, 0],       // black
            ],
        }
    }
}

impl ExportStyle {
    pub fn series_color(&self, index: usize) -> [u8; 3] {
        self.series_colors[index % self.series_colors.len()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_spec() -> ChartSpec {
        ChartSpec {
            title: "SCF convergence".to_string(),
            x: AxisSpec::new("Iteration", ""),
            y: AxisSpec::new("Energy", "Eh"),
            series: vec![Series {
                name: "SCF energy".to_string(),
                points: vec![[1.0, -74.90], [2.0, -74.95], [3.0, -74.96]],
                mark: Mark::Line,
            }],
        }
    }

    #[test]
    fn chart_spec_round_trips_through_json() {
        let spec = sample_spec();
        let json = serde_json::to_string(&spec).unwrap();
        let back: ChartSpec = serde_json::from_str(&json).unwrap();
        assert_eq!(back, spec);
    }

    #[test]
    fn axis_display_label_appends_unit_when_present() {
        assert_eq!(AxisSpec::new("Iteration", "").display_label(), "Iteration");
        assert_eq!(AxisSpec::new("Energy", "Eh").display_label(), "Energy (Eh)");
    }

    #[test]
    fn presets_have_journal_column_widths() {
        let single = PresetChoice::SingleColumn.preset();
        assert_eq!(single.width_in, 3.3);
        assert_eq!(single.height_in, 2.5);
        let double = PresetChoice::DoubleColumn.preset();
        assert_eq!(double.width_in, 7.0);
        let custom = PresetChoice::Custom {
            width_in: 5.0,
            height_in: 4.0,
        }
        .preset();
        assert_eq!(custom.width_in, 5.0);
        assert_eq!(custom.height_in, 4.0);
        // All presets share the print type sizes.
        assert_eq!(single.base_pt, 8.0);
        assert_eq!(single.tick_pt, 7.0);
    }

    #[test]
    fn export_format_extensions() {
        assert_eq!(ExportFormat::Png.extension(), "png");
        assert_eq!(ExportFormat::Svg.extension(), "svg");
        assert_eq!(ExportFormat::Pdf.extension(), "pdf");
    }

    #[test]
    fn export_style_series_colors_cycle() {
        let style = ExportStyle::default();
        let n = style.series_colors.len();
        assert!(n >= 6);
        assert_eq!(style.series_color(0), style.series_color(n));
        assert_eq!(style.background, [255, 255, 255]);
    }
}
