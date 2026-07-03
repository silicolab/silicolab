use eframe::egui::{self, ComboBox, RichText};

use crate::frontend::{
    actions::{AppAction, ChartAxis},
    state::AppState,
    ui::plot_view,
};

pub(crate) fn render_plot_panel(
    state: &mut AppState,
    ui: &mut egui::Ui,
    actions: &mut Vec<AppAction>,
) {
    let pal = crate::frontend::theme::palette(ui);
    ui.add_space(8.0);
    // Clone the small display state up front so the chart render below can
    // borrow `ui` freely and the bounds write-back can re-borrow `state`.
    let Some((source_name, titles, active, error, spec)) = state.ui.chart.as_ref().map(|chart| {
        (
            chart.source_name.clone(),
            chart
                .datasets
                .iter()
                .map(|dataset| dataset.title.clone())
                .collect::<Vec<_>>(),
            chart.active,
            chart.error.clone(),
            chart.active_dataset().cloned(),
        )
    }) else {
        ui.label(
            RichText::new(
                "No chart loaded. Open one from a QM entry's chart button or a completed task panel.",
            )
            .small()
            .color(pal.text_tertiary),
        );
        return;
    };

    if let Some(error) = error {
        ui.label(RichText::new(&source_name).small().color(pal.text_muted));
        ui.add_space(4.0);
        ui.label(RichText::new(error).small().color(pal.status_amber));
        return;
    }
    let Some(spec) = spec else {
        return;
    };

    let controls_width = 190.0;
    let chart_height = (ui.available_height() - 8.0).max(120.0);
    ui.horizontal_top(|ui| {
        let chart_width = (ui.available_width() - controls_width - 12.0).max(120.0);
        ui.vertical(|ui| {
            ui.set_width(chart_width);
            let bounds = plot_view::render_chart(ui, &spec, "plot-panel", chart_height, true);
            if let Some(chart) = state.ui.chart.as_mut() {
                chart.view_bounds = Some(bounds);
            }
        });
        ui.vertical(|ui| {
            ui.set_width(controls_width);
            ui.label(RichText::new(&source_name).small().color(pal.text_muted));
            ui.add_space(6.0);
            if titles.len() > 1 {
                ComboBox::from_id_salt("plot-dataset")
                    .width(controls_width - 8.0)
                    .selected_text(titles[active].clone())
                    .show_ui(ui, |ui| {
                        for (index, title) in titles.iter().enumerate() {
                            if ui.selectable_label(index == active, title).clicked() {
                                actions.push(AppAction::SelectChartDataset(index));
                            }
                        }
                    });
                ui.add_space(6.0);
            }
            ui.label(
                RichText::new("X axis label")
                    .small()
                    .color(pal.text_tertiary),
            );
            let mut x_label = spec.x.label.clone();
            if ui.text_edit_singleline(&mut x_label).changed() {
                actions.push(AppAction::SetChartAxisLabel {
                    axis: ChartAxis::X,
                    label: x_label,
                });
            }
            ui.label(
                RichText::new("Y axis label")
                    .small()
                    .color(pal.text_tertiary),
            );
            let mut y_label = spec.y.label.clone();
            if ui.text_edit_singleline(&mut y_label).changed() {
                actions.push(AppAction::SetChartAxisLabel {
                    axis: ChartAxis::Y,
                    label: y_label,
                });
            }
            ui.add_space(10.0);
            if ui.button("Export…").clicked()
                && let Some(chart) = state.ui.chart.as_mut()
            {
                chart.export_open = true;
            }
        });
    });

    if state
        .ui
        .chart
        .as_ref()
        .is_some_and(|chart| chart.export_open)
    {
        render_export_dialog(state, ui, actions);
    }
}

/// Modal-ish window editing the export draft. Draft edits mutate local widget
/// state directly (like the pending-* prompts); the export itself is an
/// action.
fn render_export_dialog(state: &mut AppState, ui: &mut egui::Ui, actions: &mut Vec<AppAction>) {
    use crate::plot::spec::{ExportFormat, PresetChoice};

    let pal = crate::frontend::theme::palette(ui);
    let has_view = state
        .ui
        .chart
        .as_ref()
        .is_some_and(|chart| chart.view_bounds.is_some());
    let Some(chart) = state.ui.chart.as_mut() else {
        return;
    };
    let draft = &mut chart.export_draft;
    let mut open = true;
    let mut close = false;
    egui::Window::new("Export Chart")
        .collapsible(false)
        .resizable(false)
        .open(&mut open)
        .show(ui.ctx(), |ui| {
            ui.label(RichText::new("Format").small().color(pal.text_tertiary));
            ui.horizontal(|ui| {
                for format in ExportFormat::all() {
                    ui.radio_value(&mut draft.format, format, format.label());
                }
            });
            ui.add_space(6.0);
            ui.label(RichText::new("Size").small().color(pal.text_tertiary));
            ui.radio_value(
                &mut draft.preset,
                PresetChoice::SingleColumn,
                PresetChoice::SingleColumn.label(),
            );
            ui.radio_value(
                &mut draft.preset,
                PresetChoice::DoubleColumn,
                PresetChoice::DoubleColumn.label(),
            );
            let is_custom = matches!(draft.preset, PresetChoice::Custom { .. });
            if ui.radio(is_custom, "Custom").clicked() && !is_custom {
                draft.preset = PresetChoice::Custom {
                    width_in: 5.0,
                    height_in: 3.8,
                };
            }
            if let PresetChoice::Custom {
                width_in,
                height_in,
            } = &mut draft.preset
            {
                ui.horizontal(|ui| {
                    ui.add(
                        egui::DragValue::new(width_in)
                            .range(1.0..=20.0)
                            .speed(0.1)
                            .suffix(" in"),
                    );
                    ui.label(RichText::new("by").small().color(pal.text_tertiary));
                    ui.add(
                        egui::DragValue::new(height_in)
                            .range(1.0..=20.0)
                            .speed(0.1)
                            .suffix(" in"),
                    );
                });
            }
            if draft.format == ExportFormat::Png {
                ui.add_space(6.0);
                ui.label(RichText::new("Resolution").small().color(pal.text_tertiary));
                ui.add(
                    egui::DragValue::new(&mut draft.dpi)
                        .range(72..=1200)
                        .suffix(" dpi"),
                );
            }
            ui.add_space(6.0);
            ui.add_enabled(
                has_view,
                egui::Checkbox::new(&mut draft.current_view, "Limit to current view"),
            );
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if ui.button("Export").clicked() {
                    actions.push(AppAction::ExportChart);
                }
                if ui.button("Cancel").clicked() {
                    close = true;
                }
            });
        });
    if close || !open {
        chart.export_open = false;
    }
}
