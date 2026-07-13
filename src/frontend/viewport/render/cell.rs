use eframe::egui::{Align2, Color32, FontId, Vec2};
use nalgebra::Point3;

use crate::domain::UnitCell;

use super::super::camera::Projector;

pub(crate) fn draw_cell_labels(
    painter: &eframe::egui::Painter,
    viewport: &Projector,
    cell: &UnitCell,
) {
    let corners = cell.corners();
    draw_cell_label(
        painter,
        viewport,
        &(corners[0] + cell.vectors[0] * 0.5),
        format!("a={:.2}", cell.a),
    );
    draw_cell_label(
        painter,
        viewport,
        &(corners[0] + cell.vectors[1] * 0.5),
        format!("b={:.2}", cell.b),
    );
    draw_cell_label(
        painter,
        viewport,
        &(corners[0] + cell.vectors[2] * 0.5),
        format!("c={:.2}", cell.c),
    );
}

fn draw_cell_label(
    painter: &eframe::egui::Painter,
    viewport: &Projector,
    point: &Point3<f32>,
    label: String,
) {
    painter.text(
        viewport.project(*point).pos + Vec2::new(6.0, -6.0),
        Align2::LEFT_BOTTOM,
        label,
        FontId::proportional(12.0),
        Color32::from_rgb(35, 40, 46),
    );
}
