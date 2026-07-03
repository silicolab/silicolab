use std::path::PathBuf;

use super::*;
use crate::backend::runs::{QmSeries, SERIES_FILE, load_qm_series_file};
use crate::frontend::actions::{ChartAxis, ChartTarget};
use crate::frontend::state::{ChartExportDraft, ChartState, StaticView};
use crate::plot::spec::{
    AxisSpec, ChartSpec, ExportFormat, ExportStyle, Mark, PresetChoice, Series,
};

/// Build one `ChartSpec` per unit-compatible dataset from a run's saved
/// series, ordered so the primary dataset (what a thumbnail shows) comes
/// first. Frequencies are persisted but not charted until the spectra PR.
pub(crate) fn datasets_from_series(series: &QmSeries) -> Vec<ChartSpec> {
    fn indexed(values: &[f64]) -> Vec<[f64; 2]> {
        values
            .iter()
            .enumerate()
            .map(|(index, &value)| [(index + 1) as f64, value])
            .collect()
    }
    let mut datasets = Vec::new();
    if !series.opt_trace.is_empty() {
        datasets.push(ChartSpec {
            title: "Optimization energy".to_string(),
            x: AxisSpec::new("Step", ""),
            y: AxisSpec::new("Energy", "Eh"),
            series: vec![Series {
                name: "Energy".to_string(),
                points: indexed(&series.opt_trace),
                mark: Mark::Line,
            }],
        });
    }
    if !series.scf_trace.is_empty() {
        // For moving jobs hartree keeps only the final geometry's SCF, so the
        // title says which convergence this is.
        let title = if series.opt_trace.is_empty() {
            "SCF convergence"
        } else {
            "Final-step SCF convergence"
        };
        datasets.push(ChartSpec {
            title: title.to_string(),
            x: AxisSpec::new("Iteration", ""),
            y: AxisSpec::new("Energy", "Eh"),
            series: vec![Series {
                name: "SCF energy".to_string(),
                points: indexed(&series.scf_trace),
                mark: Mark::Line,
            }],
        });
    }
    datasets
}

/// Resolve an entry's `series.json` (sibling of its saved QM output report,
/// which is stored project-root-relative). `None` when the entry is not a QM
/// run or never saved a report.
pub(crate) fn entry_series_path(state: &AppState, entry_id: u64) -> Option<(String, PathBuf)> {
    let entry = state.entries.entry(entry_id)?;
    let name = entry.name.clone();
    let relative = entry.origin.qm_output()?.parent()?.join(SERIES_FILE);
    let absolute = match state.workspace.project() {
        Some(project) => project.root.join(&relative),
        None => relative,
    };
    Some((name, absolute))
}

fn task_series_path(state: &AppState, task_run_id: u64) -> Option<(String, PathBuf)> {
    let task = state.tasks.task_run(task_run_id)?;
    let run_dir = task.run_dir.as_ref()?;
    Some((task.title.clone(), run_dir.join(SERIES_FILE)))
}

pub(crate) fn open_chart(state: &mut AppState, target: ChartTarget, ctx: &egui::Context) {
    let resolved = match target {
        ChartTarget::Entry(entry_id) => entry_series_path(state, entry_id),
        ChartTarget::TaskRun(task_run_id) => task_series_path(state, task_run_id),
    };
    let Some((source_name, series_path)) = resolved else {
        state.set_message("This item has no chart data".to_string());
        return;
    };
    let mut chart = ChartState::new(source_name);
    match load_qm_series_file(&series_path) {
        Ok(series) => {
            chart.datasets = datasets_from_series(&series);
            if chart.datasets.is_empty() {
                chart.error = Some("No plottable data in this run".to_string());
            }
        }
        Err(_) => chart.error = Some("Data file missing".to_string()),
    }
    chart.export_draft = ChartExportDraft::from_prefs(&state.config.chart_export);
    state.ui.chart = Some(chart);
    state.ui.layout.dock.reveal_static(StaticView::Plot);
    let now = ctx.input(|input| input.time);
    state.mark_layout_dirty(now);
}

pub(crate) fn select_chart_dataset(state: &mut AppState, index: usize) {
    if let Some(chart) = state.ui.chart.as_mut()
        && index < chart.datasets.len()
    {
        chart.active = index;
        chart.view_bounds = None;
    }
}

/// Memoized: does this entry's QM run have saved series data? Checked once per
/// entry. `UiState` is built once at app start and persists, so the memo is
/// explicitly cleared on a project switch (see `reset_chart_caches`) — entry
/// ids restart per project and would otherwise collide.
pub(crate) fn entry_chart_available(state: &mut AppState, entry_id: u64) -> bool {
    if let Some(&known) = state.ui.chart_availability.get(&entry_id) {
        return known;
    }
    let available = entry_series_path(state, entry_id).is_some_and(|(_, path)| path.is_file());
    state.ui.chart_availability.insert(entry_id, available);
    available
}

/// Memoized primary dataset of a task run's saved series, for embedded
/// thumbnails. Missing/unreadable series memoize as `None` — one stat, not one
/// per frame.
pub(crate) fn task_chart_thumbnail(state: &mut AppState, task_run_id: u64) -> Option<ChartSpec> {
    if let Some(cached) = state.ui.task_chart_thumbnails.get(&task_run_id) {
        return cached.clone();
    }
    let loaded = task_series_path(state, task_run_id)
        .and_then(|(_, path)| load_qm_series_file(&path).ok())
        .and_then(|series| datasets_from_series(&series).into_iter().next());
    state
        .ui
        .task_chart_thumbnails
        .insert(task_run_id, loaded.clone());
    loaded
}

pub(crate) fn set_chart_axis_label(state: &mut AppState, axis: ChartAxis, label: String) {
    if let Some(spec) = state
        .ui
        .chart
        .as_mut()
        .and_then(ChartState::active_dataset_mut)
    {
        match axis {
            ChartAxis::X => spec.x.label = label,
            ChartAxis::Y => spec.y.label = label,
        }
    }
}

fn slug(text: &str) -> String {
    let mut out = String::new();
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    let trimmed = out.trim_matches('-');
    if trimmed.is_empty() {
        "chart".to_string()
    } else {
        trimmed.to_string()
    }
}

/// `<source>-<dataset>-<preset>[-<dpi>dpi].<ext>`; DPI only matters for PNG.
pub(crate) fn export_file_name(source: &str, dataset: &str, draft: &ChartExportDraft) -> String {
    let preset = match draft.preset {
        PresetChoice::SingleColumn => "single-column",
        PresetChoice::DoubleColumn => "double-column",
        PresetChoice::Custom { .. } => "custom",
    };
    let mut name = format!("{}-{}-{preset}", slug(source), slug(dataset));
    if draft.format == ExportFormat::Png {
        name.push_str(&format!("-{}dpi", draft.dpi));
    }
    format!("{name}.{}", draft.format.extension())
}

pub(crate) fn export_chart(state: &mut AppState) {
    let Some(chart) = state.ui.chart.as_ref() else {
        return;
    };
    let Some(dataset) = chart.active_dataset() else {
        state.set_message("No dataset to export".to_string());
        return;
    };
    let mut spec = dataset.clone();
    let draft = chart.export_draft.clone();
    let source_name = chart.source_name.clone();
    if draft.current_view
        && let Some(bounds) = chart.view_bounds
    {
        spec.x.range = Some([bounds[0][0], bounds[1][0]]);
        spec.y.range = Some([bounds[0][1], bounds[1][1]]);
    }
    let Some(path) = rfd::FileDialog::new()
        .set_file_name(export_file_name(&source_name, &spec.title, &draft))
        .add_filter(draft.format.label(), &[draft.format.extension()])
        .save_file()
    else {
        return;
    };
    let result = crate::plot::export::export_bytes(
        &spec,
        &draft.preset.preset(),
        &ExportStyle::default(),
        draft.format,
        draft.dpi,
    )
    .and_then(|bytes| std::fs::write(&path, bytes).map_err(Into::into));
    match result {
        Ok(()) => {
            state.config.chart_export = crate::backend::config::ChartExportPrefs {
                format: draft.format,
                preset: draft.preset,
                dpi: draft.dpi,
            };
            // The prefs save is a best-effort follow-up; fold any failure into
            // the export confirmation so the "Chart exported" message isn't
            // silently overwritten by the warning.
            let exported = format!("Chart exported to {}", path.display());
            if let Err(error) = save_config(&state.config) {
                state.set_message(format!(
                    "{exported}. Export preferences were not saved: {error}"
                ));
            } else {
                state.set_message(exported);
            }
            if let Some(chart) = state.ui.chart.as_mut() {
                chart.export_open = false;
            }
        }
        Err(error) => state.set_message(format!("Chart export failed: {error}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::tasks::task_controller_by_id;

    fn scratch_with_qm_task(run_dir: Option<&std::path::Path>) -> (AppState, u64) {
        let mut state = AppState::scratch(Default::default(), Vec::new());
        let controller = *task_controller_by_id("qm-energy").expect("qm-energy controller");
        let task_id = state.tasks.create_task_run(controller);
        if let Some(dir) = run_dir {
            state.tasks.set_run_dir(task_id, dir.to_path_buf());
        }
        (state, task_id)
    }

    #[test]
    fn datasets_follow_the_qm_kind_mapping() {
        let optimize = QmSeries {
            version: 1,
            scf_trace: vec![-74.95, -74.96],
            opt_trace: vec![-74.90, -74.96],
            frequencies: Vec::new(),
        };
        let datasets = datasets_from_series(&optimize);
        assert_eq!(datasets.len(), 2);
        assert_eq!(datasets[0].title, "Optimization energy");
        assert_eq!(datasets[0].x.label, "Step");
        assert_eq!(
            datasets[0].series[0].points,
            vec![[1.0, -74.90], [2.0, -74.96]]
        );
        assert_eq!(datasets[1].title, "Final-step SCF convergence");

        let single_point = QmSeries {
            version: 1,
            scf_trace: vec![-74.1, -74.96],
            opt_trace: Vec::new(),
            frequencies: Vec::new(),
        };
        let datasets = datasets_from_series(&single_point);
        assert_eq!(datasets.len(), 1);
        assert_eq!(datasets[0].title, "SCF convergence");
        assert_eq!(datasets[0].x.label, "Iteration");
        assert_eq!(datasets[0].y.unit, "Eh");

        // Frequencies are surfaced and persisted but not charted until PR 3.
        let frequencies_only = QmSeries {
            version: 1,
            scf_trace: Vec::new(),
            opt_trace: Vec::new(),
            frequencies: vec![4401.2],
        };
        assert!(datasets_from_series(&frequencies_only).is_empty());
    }

    #[test]
    fn open_chart_loads_a_task_run_series() {
        let dir = std::env::temp_dir().join(format!("silicolab_chart_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let series = QmSeries {
            version: 1,
            scf_trace: vec![-74.1, -74.96],
            opt_trace: Vec::new(),
            frequencies: Vec::new(),
        };
        crate::backend::runs::save_qm_series_file(&dir, &series).unwrap();

        let (mut state, task_id) = scratch_with_qm_task(Some(&dir));
        open_chart(
            &mut state,
            ChartTarget::TaskRun(task_id),
            &egui::Context::default(),
        );

        let chart = state.ui.chart.as_ref().expect("chart loaded");
        assert!(chart.error.is_none());
        assert_eq!(chart.datasets.len(), 1);
        assert!(
            state
                .ui
                .layout
                .dock
                .area(crate::frontend::state::DockArea::Bottom)
                .tabs
                .contains(&crate::frontend::state::DockTab::Static(StaticView::Plot))
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn open_chart_with_missing_file_shows_the_missing_state() {
        let dir =
            std::env::temp_dir().join(format!("silicolab_chart_missing_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (mut state, task_id) = scratch_with_qm_task(Some(&dir));
        open_chart(
            &mut state,
            ChartTarget::TaskRun(task_id),
            &egui::Context::default(),
        );
        let chart = state.ui.chart.as_ref().expect("chart state set");
        assert_eq!(chart.error.as_deref(), Some("Data file missing"));
        assert!(chart.datasets.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn dataset_selection_and_axis_labels_update_state() {
        let mut state = AppState::scratch(Default::default(), Vec::new());
        let mut chart = ChartState::new("h2".to_string());
        chart.datasets = vec![
            ChartSpec {
                title: "a".into(),
                x: AxisSpec::new("Step", ""),
                y: AxisSpec::new("Energy", "Eh"),
                series: vec![Series {
                    name: "e".into(),
                    points: vec![[1.0, 2.0]],
                    mark: Mark::Line,
                }],
            },
            ChartSpec {
                title: "b".into(),
                x: AxisSpec::new("Iteration", ""),
                y: AxisSpec::new("Energy", "Eh"),
                series: Vec::new(),
            },
        ];
        state.ui.chart = Some(chart);

        select_chart_dataset(&mut state, 1);
        assert_eq!(state.ui.chart.as_ref().unwrap().active, 1);
        select_chart_dataset(&mut state, 99);
        assert_eq!(
            state.ui.chart.as_ref().unwrap().active,
            1,
            "out of range is ignored"
        );

        set_chart_axis_label(&mut state, ChartAxis::Y, "Total energy".to_string());
        assert_eq!(
            state.ui.chart.as_ref().unwrap().datasets[1].y.label,
            "Total energy"
        );
    }

    #[test]
    fn entry_chart_availability_is_memoized() {
        let mut state = AppState::scratch(Default::default(), Vec::new());
        // Unknown entry: resolved once to false and cached.
        assert!(!entry_chart_available(&mut state, 42));
        assert_eq!(state.ui.chart_availability.get(&42), Some(&false));
        // The cache short-circuits: no re-stat, the stored value wins.
        state.ui.chart_availability.insert(42, true);
        assert!(entry_chart_available(&mut state, 42));
    }

    #[test]
    fn task_thumbnails_load_once_and_cache_the_result() {
        let dir = std::env::temp_dir().join(format!("silicolab_thumb_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let series = QmSeries {
            version: 1,
            scf_trace: vec![-74.1, -74.96],
            opt_trace: Vec::new(),
            frequencies: Vec::new(),
        };
        crate::backend::runs::save_qm_series_file(&dir, &series).unwrap();

        let (mut state, task_id) = scratch_with_qm_task(Some(&dir));
        let spec = task_chart_thumbnail(&mut state, task_id).expect("thumbnail loads");
        assert_eq!(spec.title, "SCF convergence");

        // Deleting the file does not evict the memo — no per-frame stats.
        std::fs::remove_file(dir.join(SERIES_FILE)).unwrap();
        assert!(task_chart_thumbnail(&mut state, task_id).is_some());

        // Eviction (what run-completion does) forces a reload.
        state.ui.task_chart_thumbnails.remove(&task_id);
        assert!(task_chart_thumbnail(&mut state, task_id).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn export_file_names_are_slugged_and_carry_dpi_for_png() {
        use crate::frontend::state::ChartExportDraft;
        use crate::plot::spec::{ExportFormat, PresetChoice};

        let png = ChartExportDraft {
            format: ExportFormat::Png,
            preset: PresetChoice::SingleColumn,
            dpi: 300,
            current_view: false,
        };
        assert_eq!(
            export_file_name("H2O (opt)", "SCF convergence", &png),
            "h2o-opt-scf-convergence-single-column-300dpi.png"
        );
        let pdf = ChartExportDraft {
            format: ExportFormat::Pdf,
            preset: PresetChoice::Custom {
                width_in: 5.0,
                height_in: 4.0,
            },
            dpi: 300,
            current_view: false,
        };
        assert_eq!(
            export_file_name("h2", "Optimization energy", &pdf),
            "h2-optimization-energy-custom.pdf"
        );
    }
}
