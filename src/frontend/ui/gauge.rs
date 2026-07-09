use std::collections::VecDeque;

use eframe::egui::{
    self, Align, Color32, CornerRadius, Layout, Pos2, Rect, RichText, Sense, Stroke, Vec2,
};

/// The semantic *role* color for a utilization level, before pastel softening.
/// Below 80 % it follows the theme accent (so the normal state changes with the
/// color scheme); 80–90 % warns amber; 90 %+ alarms red. Amber/red are palette
/// roles too, so they also flip per theme.
fn gauge_role_color(pal: &crate::frontend::theme::Palette, pct: f32) -> Color32 {
    if pct < 80.0 {
        pal.accent
    } else if pct < 90.0 {
        pal.status_amber
    } else {
        pal.status_red
    }
}

/// Soften a color toward white for a pastel tone — lower saturation, higher
/// value — while staying legible on the near-white light surfaces.
fn pastel(c: Color32) -> Color32 {
    let mix = |x: u8| (x as f32 + (255.0 - x as f32) * 0.30).round() as u8;
    Color32::from_rgb(mix(c.r()), mix(c.g()), mix(c.b()))
}

/// Pastel-softened fill color for a utilization level. The normal state follows
/// the theme accent; 80 %+ warns amber and 90 %+ alarms red. Shared by the bars,
/// the sparklines, and the status-bar chips.
pub(crate) fn gauge_color(pal: &crate::frontend::theme::Palette, pct: f32) -> Color32 {
    pastel(gauge_role_color(pal, pct))
}

/// Draw a rounded track and a colored fill for `fraction` (0..=1) across `width`
/// at the current cursor. `None` leaves the track empty (e.g. an unavailable
/// GPU). The fill is at least `height` wide so a non-zero value always shows as a
/// rounded pill rather than vanishing.
fn draw_bar(ui: &mut egui::Ui, width: f32, height: f32, fraction: Option<f32>) {
    let pal = crate::frontend::theme::palette(ui);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
    let painter = ui.painter_at(rect);
    let radius = CornerRadius::same((height / 2.0) as u8);
    painter.rect_filled(rect, radius, pal.text_tertiary.gamma_multiply(0.3));
    if let Some(fraction) = fraction {
        let fraction = fraction.clamp(0.0, 1.0);
        if fraction > 0.0 {
            let fill_w = (rect.width() * fraction).max(height);
            let fill = Rect::from_min_size(rect.min, Vec2::new(fill_w, height));
            painter.rect_filled(fill, radius, gauge_color(&pal, fraction * 100.0));
        }
    }
}

/// A compact one-line utilization row (the always-visible footer style): a
/// fixed-width `label`, a flexible thin bar, then the right-aligned `value`.
pub(crate) fn utilization_row_inline(
    ui: &mut egui::Ui,
    label: &str,
    tooltip: Option<&str>,
    value: &str,
    fraction: Option<f32>,
) {
    let pal = crate::frontend::theme::palette(ui);
    let resp = ui
        .horizontal(|ui| {
            ui.add_sized(
                Vec2::new(30.0, 12.0),
                egui::Label::new(RichText::new(label).small().color(pal.text_muted)),
            );
            let value_w = 38.0;
            let bar_w = (ui.available_width() - value_w - 6.0).max(24.0);
            draw_bar(ui, bar_w, 5.0, fraction);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(RichText::new(value).small().color(pal.text_muted));
            });
        })
        .response;
    if let Some(tooltip) = tooltip {
        resp.on_hover_text(tooltip);
    }
}

/// A utilization chart (the detail-popover style): `label` on the left with a
/// short `headline` (e.g. the utilization `%`) on the right, an optional `detail`
/// line that wraps full-width beneath, then a time-series sparkline. `history` is
/// oldest-first (newest on the right); `current` colors the line by threshold.
///
/// `headline` stays short so it always fits to the right of the label; the verbose
/// `detail` (VRAM/temperature/power) goes on its own wrapping line so a long string
/// can't overflow left and overlap the label in a narrow popover.
pub(crate) fn utilization_chart(
    ui: &mut egui::Ui,
    label: &str,
    tooltip: Option<&str>,
    headline: &str,
    detail: Option<&str>,
    history: &VecDeque<Option<f32>>,
    current: Option<f32>,
) {
    let pal = crate::frontend::theme::palette(ui);
    ui.horizontal(|ui| {
        let label = ui.label(RichText::new(label).color(pal.text_strong));
        if let Some(tooltip) = tooltip {
            label.on_hover_text(tooltip);
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.label(RichText::new(headline).color(pal.text_muted));
        });
    });
    if let Some(detail) = detail {
        ui.add_space(2.0);
        ui.add(
            egui::Label::new(RichText::new(detail).small().color(pal.text_muted))
                .wrap_mode(egui::TextWrapMode::Wrap),
        );
    }
    ui.add_space(6.0);
    let color = current.map_or(pal.text_tertiary, |pct| gauge_color(&pal, pct));
    sparkline(ui, history, 36.0, color);
}

/// Draw a filled-area line chart of `history` (values 0..=100, oldest first) into
/// a `height`-tall full-width strip. A faint baseline always shows; the curve
/// (line + low-alpha fill) appears once there are at least two readings.
fn sparkline(ui: &mut egui::Ui, history: &VecDeque<Option<f32>>, height: f32, color: Color32) {
    let pal = crate::frontend::theme::palette(ui);
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
    let painter = ui.painter_at(rect);
    painter.hline(
        rect.left()..=rect.right(),
        rect.bottom() - 0.5,
        Stroke::new(1.0_f32, pal.text_tertiary.gamma_multiply(0.25)),
    );

    if history.len() < 2 {
        return;
    }
    let dx = rect.width() / (history.len() - 1) as f32;
    let plot_h = (height - 3.0).max(1.0);
    let pts: Vec<Pos2> = history
        .iter()
        .enumerate()
        .filter_map(|(i, value)| {
            value.map(|v| {
                let y = rect.bottom() - (v.clamp(0.0, 100.0) / 100.0) * plot_h;
                Pos2::new(rect.left() + dx * i as f32, y)
            })
        })
        .collect();
    if pts.len() < 2 {
        return;
    }

    // Low-alpha area fill via a triangle strip down to the baseline (robust for
    // the concave curve; egui's convex fill would mis-tessellate it).
    let fill = color.gamma_multiply(0.16);
    let mut mesh = egui::Mesh::default();
    for window in pts.windows(2) {
        let base = mesh.vertices.len() as u32;
        mesh.colored_vertex(window[0], fill);
        mesh.colored_vertex(window[1], fill);
        mesh.colored_vertex(Pos2::new(window[1].x, rect.bottom()), fill);
        mesh.colored_vertex(Pos2::new(window[0].x, rect.bottom()), fill);
        mesh.add_triangle(base, base + 1, base + 2);
        mesh.add_triangle(base, base + 2, base + 3);
    }
    painter.add(egui::Shape::mesh(mesh));
    painter.add(egui::Shape::line(pts, Stroke::new(1.6_f32, color)));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gauge_role_color_accent_then_amber_then_red() {
        let pal = crate::frontend::theme::Palette::warm_light();
        // Normal state follows the theme accent.
        assert_eq!(gauge_role_color(&pal, 0.0), pal.accent);
        assert_eq!(gauge_role_color(&pal, 79.9), pal.accent);
        // 80 and up warns amber, until 90.
        assert_eq!(gauge_role_color(&pal, 80.0), pal.status_amber);
        assert_eq!(gauge_role_color(&pal, 89.9), pal.status_amber);
        // 90 and up alarms red.
        assert_eq!(gauge_role_color(&pal, 90.0), pal.status_red);
        assert_eq!(gauge_role_color(&pal, 100.0), pal.status_red);
    }
}
