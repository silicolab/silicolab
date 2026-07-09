//! The drawing surface: computes this frame's pointer/hover, runs the tool input,
//! then paints bonds, atoms, labels, charges, valence warnings, the hover halo,
//! the selection, and any in-progress preview (ring template, rubber-band bond,
//! chain count, rotation angle, marquee).

use eframe::egui::{
    Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, Vec2, vec2,
};
use nalgebra::Point2;

use super::{SketchTool, SketcherState, input, placement};
use crate::domain::{BondType, chemistry::element_style};

const PICK_PX: f32 = 14.0;
const BOND_WIDTH: f32 = 2.0;
const LABEL_SIZE: f32 = 15.0;

pub(super) fn show_canvas(state: &mut SketcherState, ui: &mut Ui) {
    let size = ui.available_size();
    let (rect, response) = ui.allocate_exact_size(size, Sense::click_and_drag());

    if state.take_needs_fit() {
        state.fit(rect);
    }

    // --- This frame's pointer / hover ---
    let pointer = ui.input(|input| input.pointer.interact_pos());
    let pointer_model = pointer.map(|screen| state.to_model(rect, screen));
    let radius = PICK_PX / state.zoom;
    let hovered_atom = if response.hovered() {
        pointer_model.and_then(|p| state.sketch.nearest_atom(p, radius))
    } else {
        None
    };
    let hovered_bond = if response.hovered() && hovered_atom.is_none() {
        pointer_model.and_then(|p| state.sketch.nearest_bond(p, radius))
    } else {
        None
    };
    let shift = ui.input(|input| input.modifiers.shift);

    let frame = input::Frame {
        rect,
        pointer,
        pointer_model,
        hovered_atom,
        hovered_bond,
        shift,
    };
    input::handle(state, &response, ui, &frame);
    set_cursor(state, ui, &response);

    // --- Paint ---
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 4.0, ui.visuals().extreme_bg_color);
    painter.rect_stroke(
        rect,
        4.0,
        Stroke::new(1.0_f32, ui.visuals().widgets.noninteractive.bg_stroke.color),
        StrokeKind::Inside,
    );

    draw_hover(state, &painter, rect, hovered_atom, hovered_bond);
    draw_bonds(state, &painter, rect);
    draw_atoms(state, &painter, rect);
    draw_overlays(state, &painter, rect, &frame);
}

fn set_cursor(state: &SketcherState, ui: &Ui, response: &eframe::egui::Response) {
    use eframe::egui::CursorIcon;
    if !response.hovered() {
        return;
    }
    let icon = match state.tool {
        SketchTool::Move => CursorIcon::Move,
        SketchTool::Erase => CursorIcon::NotAllowed,
        SketchTool::Select => CursorIcon::Crosshair,
        _ => CursorIcon::Default,
    };
    ui.ctx().set_cursor_icon(icon);
}

fn color32(color: nalgebra::Point3<f32>) -> Color32 {
    Color32::from_rgb(
        (color.x * 255.0) as u8,
        (color.y * 255.0) as u8,
        (color.z * 255.0) as u8,
    )
}

fn bond_base_color(painter_dark: bool) -> Color32 {
    if painter_dark {
        Color32::from_rgb(205, 209, 214)
    } else {
        Color32::from_rgb(70, 74, 80)
    }
}

fn draw_hover(
    state: &SketcherState,
    painter: &eframe::egui::Painter,
    rect: Rect,
    hovered_atom: Option<usize>,
    hovered_bond: Option<usize>,
) {
    let halo = Color32::from_rgba_unmultiplied(120, 170, 255, 70);
    if let Some(atom) = hovered_atom
        && atom < state.sketch.atoms.len()
    {
        let center = state.to_screen(rect, state.sketch.atoms[atom].pos);
        painter.circle_filled(center, 12.0, halo);
    } else if let Some(bond) = hovered_bond
        && let Some(bond) = state.sketch.bonds.get(bond)
    {
        let a = state.to_screen(rect, state.sketch.atoms[bond.a].pos);
        let b = state.to_screen(rect, state.sketch.atoms[bond.b].pos);
        painter.line_segment([a, b], Stroke::new(9.0_f32, halo));
    }
}

fn draw_bonds(state: &SketcherState, painter: &eframe::egui::Painter, rect: Rect) {
    let dark = painter_is_dark(painter);
    let base = bond_base_color(dark);
    let selection = Color32::from_rgb(90, 140, 245);
    let centroid = state.to_screen(rect, state.sketch.centroid());

    for (index, bond) in state.sketch.bonds.iter().enumerate() {
        let (Some(atom_a), Some(atom_b)) = (
            state.sketch.atoms.get(bond.a),
            state.sketch.atoms.get(bond.b),
        ) else {
            continue;
        };
        let a = state.to_screen(rect, atom_a.pos);
        let b = state.to_screen(rect, atom_b.pos);
        let selected = state.selected_bonds.contains(&index);
        let color = if selected { selection } else { base };
        let width = if selected {
            BOND_WIDTH + 1.0
        } else {
            BOND_WIDTH
        };
        draw_one_bond(painter, a, b, bond.order, color, width, centroid);
    }
}

fn draw_one_bond(
    painter: &eframe::egui::Painter,
    a: Pos2,
    b: Pos2,
    order: BondType,
    color: Color32,
    width: f32,
    centroid: Pos2,
) {
    let stroke = Stroke::new(width, color);
    let perp = perpendicular(a, b, 3.0);
    match order {
        BondType::Single => {
            painter.line_segment([a, b], stroke);
        }
        BondType::Double => {
            painter.line_segment([a + perp, b + perp], Stroke::new(width * 0.85, color));
            painter.line_segment([a - perp, b - perp], Stroke::new(width * 0.85, color));
        }
        BondType::Triple => {
            let wide = perpendicular(a, b, 4.5);
            painter.line_segment([a, b], Stroke::new(width * 0.8, color));
            painter.line_segment([a + wide, b + wide], Stroke::new(width * 0.7, color));
            painter.line_segment([a - wide, b - wide], Stroke::new(width * 0.7, color));
        }
        BondType::Aromatic => {
            painter.line_segment([a, b], stroke);
            // Inner dashed line offset toward the molecule centre.
            let toward = inward(a, b, centroid, 3.5);
            draw_dashed(
                painter,
                a + toward,
                b + toward,
                Stroke::new(width * 0.8, color),
            );
        }
    }
}

fn draw_atoms(state: &SketcherState, painter: &eframe::egui::Painter, rect: Rect) {
    let dark = painter_is_dark(painter);
    let default_text = if dark {
        Color32::from_gray(225)
    } else {
        Color32::from_gray(35)
    };
    let selection = Color32::from_rgb(90, 140, 245);

    for index in 0..state.sketch.atoms.len() {
        let atom = &state.sketch.atoms[index];
        let center = state.to_screen(rect, atom.pos);
        let degree = state.sketch.neighbors(index).len();
        let is_carbon = atom.element == "C";
        let label_color = if state.heteroatom_color && !is_carbon {
            color32(element_style(&atom.element).color)
        } else {
            default_text
        };

        // Selection ring.
        if state.selected_atoms.contains(&index) {
            painter.circle_stroke(center, 11.0, Stroke::new(2.0_f32, selection));
        }

        // Carbons are drawn as bare vertices unless isolated.
        let show_label = !is_carbon || degree == 0;
        if show_label {
            let label = atom_label(state, index);
            // Readability disc behind the label.
            painter.circle_filled(center, 9.0, painter_bg(painter));
            painter.text(
                center,
                Align2::CENTER_CENTER,
                &label,
                FontId::proportional(LABEL_SIZE),
                label_color,
            );
        } else if degree == 1 {
            // A small node so terminal carbons read clearly.
            painter.circle_filled(center, 1.5, label_color);
        }

        if atom.charge != 0 {
            painter.text(
                center + vec2(9.0, -9.0),
                Align2::CENTER_CENTER,
                charge_label(atom.charge),
                FontId::proportional(12.0),
                Color32::from_rgb(220, 90, 90),
            );
        }

        if state.sketch.atom_overvalent(index) {
            let warn = Color32::from_rgb(225, 70, 70);
            painter.line_segment(
                [center + vec2(-8.0, 11.0), center + vec2(8.0, 11.0)],
                Stroke::new(2.0_f32, warn),
            );
        }
    }
}

fn atom_label(state: &SketcherState, index: usize) -> String {
    let element = &state.sketch.atoms[index].element;
    let hydrogens = state.sketch.implicit_hydrogens(index);
    match hydrogens {
        0 => element.clone(),
        1 => format!("{element}H"),
        n => format!("{element}H{n}"),
    }
}

fn charge_label(charge: i32) -> String {
    match charge {
        1 => "+".to_string(),
        -1 => "−".to_string(),
        n if n > 0 => format!("{n}+"),
        n => format!("{}−", n.abs()),
    }
}

fn draw_overlays(
    state: &SketcherState,
    painter: &eframe::egui::Painter,
    rect: Rect,
    frame: &input::Frame,
) {
    let preview = Color32::from_rgb(110, 170, 120);

    // Ring template preview follows the cursor.
    if state.tool == SketchTool::Ring
        && let Some(pointer) = frame.pointer_model
        && frame.pointer.is_some()
    {
        let radius = (PICK_PX / state.zoom) * 2.5;
        let placement = placement::place_ring(&state.sketch, state.active_ring, pointer, radius);
        draw_ring_preview(state, painter, rect, &placement, preview);
    }

    // In-progress gesture overlays.
    match state.gesture_preview() {
        Some(GesturePreview::Rubber { anchor }) => {
            if let Some(pointer) = frame.pointer_model {
                let a = state.to_screen(rect, anchor);
                let b = state.to_screen(rect, pointer);
                draw_dashed(painter, a, b, Stroke::new(BOND_WIDTH, preview));
                painter.circle_stroke(b, 6.0, Stroke::new(1.5_f32, preview));
            }
        }
        Some(GesturePreview::Chain { count }) => {
            if let Some(pointer) = frame.pointer {
                painter.text(
                    pointer + vec2(14.0, -14.0),
                    Align2::LEFT_CENTER,
                    format!("{count}"),
                    FontId::proportional(13.0),
                    Color32::from_rgb(110, 170, 120),
                );
            }
        }
        Some(GesturePreview::Rotate { center, degrees }) => {
            let c = state.to_screen(rect, center);
            painter.circle_stroke(
                c,
                4.0,
                Stroke::new(1.5_f32, Color32::from_rgb(110, 170, 120)),
            );
            painter.text(
                c + vec2(12.0, -12.0),
                Align2::LEFT_CENTER,
                format!("{degrees:.0}°"),
                FontId::proportional(13.0),
                Color32::from_rgb(110, 170, 120),
            );
        }
        Some(GesturePreview::Marquee { start }) => {
            if let Some(pointer) = frame.pointer {
                let a = state.to_screen(rect, start);
                let marquee = Rect::from_two_pos(a, pointer);
                painter.rect_filled(
                    marquee,
                    0.0,
                    Color32::from_rgba_unmultiplied(90, 140, 245, 30),
                );
                painter.rect_stroke(
                    marquee,
                    0.0,
                    Stroke::new(1.0_f32, Color32::from_rgb(90, 140, 245)),
                    StrokeKind::Inside,
                );
            }
        }
        None => {}
    }
}

fn draw_ring_preview(
    state: &SketcherState,
    painter: &eframe::egui::Painter,
    rect: Rect,
    placement: &placement::RingPlacement,
    color: Color32,
) {
    for &(a, b, order) in &placement.bonds {
        let pa = state.to_screen(rect, placement.positions[a]);
        let pb = state.to_screen(rect, placement.positions[b]);
        let stroke = Stroke::new(BOND_WIDTH, color);
        if order == BondType::Aromatic || order == BondType::Double {
            let perp = perpendicular(pa, pb, 3.0);
            painter.line_segment([pa + perp, pb + perp], stroke);
            painter.line_segment([pa - perp, pb - perp], Stroke::new(BOND_WIDTH * 0.8, color));
        } else {
            painter.line_segment([pa, pb], stroke);
        }
    }
    for (local, vertex) in placement.vertices.iter().enumerate() {
        let pos = state.to_screen(rect, placement.positions[local]);
        if vertex.is_some() {
            painter.circle_stroke(pos, 6.0, Stroke::new(1.5_f32, color));
        } else {
            painter.circle_filled(pos, 2.0, color);
        }
    }
}

// --- small geometry / theme helpers ---

fn perpendicular(a: Pos2, b: Pos2, offset: f32) -> Vec2 {
    let dir = (b - a).normalized();
    vec2(-dir.y, dir.x) * offset
}

/// Perpendicular offset pointing from the bond toward `target` (used for the
/// aromatic inner line).
fn inward(a: Pos2, b: Pos2, target: Pos2, offset: f32) -> Vec2 {
    let perp = perpendicular(a, b, offset);
    let mid = a + (b - a) * 0.5;
    if (mid + perp - target).length() <= (mid - perp - target).length() {
        perp
    } else {
        -perp
    }
}

fn draw_dashed(painter: &eframe::egui::Painter, a: Pos2, b: Pos2, stroke: Stroke) {
    let total = (b - a).length();
    if total < 1.0 {
        painter.line_segment([a, b], stroke);
        return;
    }
    let dir = (b - a) / total;
    let dash = 6.0;
    let gap = 4.0;
    let mut cursor = 0.0;
    while cursor < total {
        let start = a + dir * cursor;
        let end = a + dir * (cursor + dash).min(total);
        painter.line_segment([start, end], stroke);
        cursor += dash + gap;
    }
}

fn painter_is_dark(painter: &eframe::egui::Painter) -> bool {
    painter.ctx().global_style().visuals.dark_mode
}

fn painter_bg(painter: &eframe::egui::Painter) -> Color32 {
    painter.ctx().global_style().visuals.extreme_bg_color
}

// --- gesture preview extraction (kept here so input stays free of egui draw) ---

enum GesturePreview {
    Rubber { anchor: Point2<f32> },
    Chain { count: usize },
    Rotate { center: Point2<f32>, degrees: f32 },
    Marquee { start: Point2<f32> },
}

impl SketcherState {
    fn gesture_preview(&self) -> Option<GesturePreview> {
        match self.gesture.as_ref()? {
            super::Gesture::DrawBond { anchor, .. } => {
                Some(GesturePreview::Rubber { anchor: *anchor })
            }
            super::Gesture::Chain { count, .. } => Some(GesturePreview::Chain { count: *count }),
            super::Gesture::Rotate { center, accum, .. } => Some(GesturePreview::Rotate {
                center: *center,
                degrees: accum.to_degrees(),
            }),
            super::Gesture::Marquee { start, .. } => {
                Some(GesturePreview::Marquee { start: *start })
            }
            super::Gesture::Translate { .. } | super::Gesture::Erase => None,
        }
    }
}
