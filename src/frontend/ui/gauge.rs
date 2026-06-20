use eframe::egui::{self, Pos2, Sense, Stroke, Vec2};

/// Points along a clockwise arc from 12 o'clock, sweeping `fraction` (0..=1) of a
/// full turn. Returns `segments + 1` points (empty for `fraction == 0`).
///
/// Used by the status bar (Task 3.3) to draw CPU and GPU utilization gauges.
pub(crate) fn arc_points(center: Pos2, radius: f32, fraction: f32, segments: usize) -> Vec<Pos2> {
    let fraction = fraction.clamp(0.0, 1.0);
    if fraction <= f32::EPSILON {
        return Vec::new();
    }
    if segments == 0 {
        return Vec::new();
    }
    let sweep = fraction * std::f32::consts::TAU;
    let start = -std::f32::consts::FRAC_PI_2; // 12 o'clock (top)
    (0..=segments)
        .map(|i| {
            let a = start + sweep * (i as f32 / segments as f32);
            Pos2::new(center.x + radius * a.cos(), center.y + radius * a.sin())
        })
        .collect()
}

/// A compact (~18 px) ring gauge with a centered value label.
///
/// Draws a full ring track in a muted color and overlays a progress arc colored
/// by utilization level (green < 60 %, amber < 85 %, red ≥ 85 %). When `value`
/// is `None` (e.g. GPU sampler not available), renders "N/A" without panicking.
///
/// Called by the status bar (Task 3.3) twice: once for CPU, once for GPU.
pub(crate) fn utilization_gauge(
    ui: &mut egui::Ui,
    label: &str,
    value: Option<f32>,
) -> egui::Response {
    let pal = crate::frontend::theme::palette(ui);
    let d = 18.0_f32;
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(d), Sense::hover());
    let painter = ui.painter_at(rect);
    let center = rect.center();
    let radius = d / 2.0 - 2.0;

    // Ring track: muted background circle.
    painter.circle_stroke(
        center,
        radius,
        Stroke::new(2.0, pal.text_tertiary.gamma_multiply(0.35)),
    );

    if let Some(pct) = value {
        let frac = (pct / 100.0).clamp(0.0, 1.0);
        let color = if pct < 60.0 {
            pal.status_green
        } else if pct < 85.0 {
            pal.status_amber
        } else {
            pal.status_red
        };
        let pts = arc_points(center, radius, frac, 40);
        if pts.len() > 1 {
            painter.add(egui::Shape::line(pts, Stroke::new(2.5, color)));
        }
    }

    // Centered label: "<name>\n<pct>%" or "<name>\nN/A".
    let text = match value {
        Some(pct) => format!("{}\n{:.0}%", label, pct),
        None => format!("{}\nN/A", label),
    };
    painter.text(
        center,
        egui::Align2::CENTER_CENTER,
        text,
        egui::FontId::proportional(5.5),
        pal.text_primary,
    );

    resp
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::Pos2;

    #[test]
    fn arc_starts_at_top_and_scales_with_fraction() {
        let c = Pos2::new(10.0, 10.0);
        let empty = arc_points(c, 5.0, 0.0, 32);
        assert_eq!(empty.len(), 0, "zero fraction draws nothing");
        let empty_segs = arc_points(c, 5.0, 0.5, 0);
        assert!(empty_segs.is_empty(), "zero segments draws nothing");
        let pts = arc_points(c, 5.0, 0.5, 32);
        assert_eq!(pts.len(), 33, "segments+1 points");
        // First point sits at the top of the circle (12 o'clock).
        assert!(
            (pts[0].x - c.x).abs() < 1e-3 && (pts[0].y - (c.y - 5.0)).abs() < 1e-3,
            "first point should be at 12 o'clock: expected ({}, {}), got ({}, {})",
            c.x,
            c.y - 5.0,
            pts[0].x,
            pts[0].y
        );
    }

    #[test]
    fn arc_clamps_fraction_above_one() {
        let c = Pos2::new(0.0, 0.0);
        let pts = arc_points(c, 10.0, 1.5, 16);
        // Clamped to 1.0 — full circle: 17 points, last ≈ first.
        assert_eq!(pts.len(), 17);
        assert!((pts[0].x - pts[16].x).abs() < 1e-3);
        assert!((pts[0].y - pts[16].y).abs() < 1e-3);
    }

    #[test]
    fn arc_full_circle_closes() {
        let c = Pos2::new(0.0, 0.0);
        let pts = arc_points(c, 5.0, 1.0, 64);
        assert_eq!(pts.len(), 65);
        // Start and end are the same point (12 o'clock).
        assert!((pts[0].x - pts[64].x).abs() < 1e-3);
        assert!((pts[0].y - pts[64].y).abs() < 1e-3);
    }
}
