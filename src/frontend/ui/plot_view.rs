use eframe::egui;
use egui_plot::{Legend, Line, Plot, PlotPoints, Points};

use crate::frontend::theme::{self, Palette};
use crate::plot::spec::{ChartSpec, Mark};

/// Theme-derived series color cycle; the accent leads so single-series charts
/// (the common QM case) match the app accent.
fn series_color(pal: &Palette, index: usize) -> egui::Color32 {
    let cycle = [
        pal.accent,
        pal.status_green,
        pal.status_amber,
        pal.status_red,
        pal.status_blue,
    ];
    cycle[index % cycle.len()]
}

/// Render `spec` with egui_plot and return the plotted bounds as
/// `[[x_min, y_min], [x_max, y_max]]` (feeds "current view" exports).
/// Non-interactive mode is for embedded thumbnails / live traces.
/// `reset` discards the pan/zoom memory egui keeps per plot id — pass it on
/// the first frame after the plotted data changes.
pub(crate) fn render_chart(
    ui: &mut egui::Ui,
    spec: &ChartSpec,
    id_salt: impl std::hash::Hash,
    height: f32,
    interactive: bool,
    reset: bool,
) -> [[f64; 2]; 2] {
    let pal = theme::palette(ui);
    let mut plot = Plot::new(id_salt)
        .height(height)
        .x_axis_label(spec.x.display_label())
        .y_axis_label(spec.y.display_label())
        .allow_zoom(interactive)
        .allow_drag(interactive)
        .allow_scroll(interactive)
        .allow_boxed_zoom(interactive)
        .label_formatter(|name, point| {
            if name.is_empty() {
                format!("{:.6}, {:.6}", point.x, point.y)
            } else {
                format!("{name}\n{:.6}, {:.6}", point.x, point.y)
            }
        });
    if spec.series.len() > 1 {
        plot = plot.legend(Legend::default());
    }
    if reset {
        plot = plot.reset();
    }
    let response = plot.show(ui, |plot_ui| {
        for (index, series) in spec.series.iter().enumerate() {
            let color = series_color(&pal, index);
            match series.mark {
                Mark::Line => plot_ui.line(
                    Line::new(series.name.clone(), PlotPoints::from(series.points.clone()))
                        .color(color)
                        .width(1.5_f32),
                ),
                Mark::Sticks => plot_ui.points(
                    Points::new(series.name.clone(), PlotPoints::from(series.points.clone()))
                        .stems(0.0_f32)
                        .radius(1.5_f32)
                        .color(color),
                ),
            }
        }
    });
    let bounds = response.transform.bounds();
    [bounds.min(), bounds.max()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn series_colors_cycle_through_the_palette_set() {
        let pal = theme::Palette::warm_dark();
        assert_eq!(series_color(&pal, 0), series_color(&pal, 5));
        assert_ne!(series_color(&pal, 0), series_color(&pal, 1));
    }
}
