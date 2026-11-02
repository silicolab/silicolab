//! The drawing surface: computes this frame's pointer/hover, runs the tool input,
//! then paints bonds, atoms, labels, charges, valence warnings, repeat-unit
//! brackets, the hover halo, the selection, and any in-progress preview (ring
//! template, rubber-band bond, chain count, rotation angle, marquee).

use eframe::egui::{
    Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, Vec2, vec2,
};
use nalgebra::Point2;

use super::{SketchTool, SketcherState, input, placement};
use crate::domain::{BondType, chemistry::element_style};

const PICK_PX: f32 = 14.0;
const BOND_WIDTH: f32 = 2.0;
const LABEL_SIZE: f32 = 15.0;
const MIN_FIT_ZOOM: f32 = 1.0;

pub(super) fn show_canvas(state: &mut SketcherState, ui: &mut Ui) {
    let size = ui.available_size();
    let (rect, response) = ui.allocate_exact_size(size, Sense::click_and_drag());

    if state.take_needs_fit() {
        state.fit(rect, ui.painter());
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
    state.report_removed_repeat_units();
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
    draw_repeat_units(state, &painter, rect);
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

struct RepeatUnitDrawing {
    lines: Vec<[Pos2; 2]>,
    label: Option<(Pos2, std::sync::Arc<eframe::egui::Galley>)>,
}

impl RepeatUnitDrawing {
    fn bounds(&self) -> Rect {
        let mut bounds = Rect::NOTHING;
        for line in &self.lines {
            bounds.extend_with(line[0]);
            bounds.extend_with(line[1]);
        }
        bounds = bounds.expand(BOND_WIDTH * 0.375);
        if let Some((pos, galley)) = &self.label {
            bounds = bounds.union(galley.mesh_bounds.translate(pos.to_vec2()));
        }
        bounds
    }
}

fn repeat_unit_drawing(
    state: &SketcherState,
    painter: &eframe::egui::Painter,
    rect: Rect,
    unit: &crate::domain::sketch::RepeatUnit,
) -> RepeatUnitDrawing {
    let serif = state.zoom * 0.2;
    let mut lines = Vec::new();
    let mut subscript_at: Option<(Pos2, Vec2)> = None;
    for [first, second] in &unit.brackets {
        let start = state.to_screen(rect, *first);
        let end = state.to_screen(rect, *second);
        let along = (end - start).normalized();
        let inward = vec2(-along.y, along.x) * serif;
        lines.extend([[start + inward, start], [start, end], [end, end + inward]]);
        for corner in [start, end] {
            if subscript_at.is_none_or(|(at, _)| lower_right(corner) > lower_right(at)) {
                subscript_at = Some((corner, -inward.normalized()));
            }
        }
    }
    let label = subscript_at
        .filter(|_| !unit.label.is_empty())
        .map(|(at, outward)| {
            let galley = painter.layout_no_wrap(
                unit.label.clone(),
                FontId::proportional(LABEL_SIZE * 0.8),
                bond_base_color(painter_is_dark(painter)),
            );
            let center = at + outward * (galley.size().length() * 0.5 + 2.0);
            (center - galley.size() * 0.5, galley)
        });
    RepeatUnitDrawing { lines, label }
}

/// Subscripts sit at the bracket's lower right, the usual place in polymer
/// formulas.
fn lower_right(pos: Pos2) -> f32 {
    pos.x + pos.y
}

fn draw_repeat_units(state: &SketcherState, painter: &eframe::egui::Painter, rect: Rect) {
    let color = bond_base_color(painter_is_dark(painter));
    let stroke = Stroke::new(BOND_WIDTH * 0.75, color);
    for unit in &state.sketch.repeat_units {
        let drawing = repeat_unit_drawing(state, painter, rect, unit);
        for line in drawing.lines {
            painter.line_segment(line, stroke);
        }
        if let Some((pos, galley)) = drawing.label {
            painter.galley(pos, galley, color);
        }
    }
}

pub(super) fn fit_repeat_units(
    state: &mut SketcherState,
    rect: Rect,
    painter: &eframe::egui::Painter,
) {
    let target = rect.shrink2(rect.size() * 0.09);
    let bounds_at = |state: &SketcherState| {
        let mut bounds = Rect::NOTHING;
        for atom in &state.sketch.atoms {
            bounds.extend_with(state.to_screen(rect, atom.pos));
        }
        for unit in &state.sketch.repeat_units {
            bounds = bounds.union(repeat_unit_drawing(state, painter, rect, unit).bounds());
        }
        bounds
    };
    let atom_fit = (state.zoom, state.pan);
    let mut low = 0.0;
    let mut high = state.zoom;
    // Text stays in screen pixels while bracket geometry scales with zoom.
    for _ in 0..24 {
        state.zoom = (low + high) * 0.5;
        let bounds = bounds_at(state);
        if bounds.width() <= target.width() && bounds.height() <= target.height() {
            low = state.zoom;
        } else {
            high = state.zoom;
        }
    }
    if low < MIN_FIT_ZOOM {
        // A label about as wide as the canvas leaves no room for the atoms.
        state.zoom = atom_fit.0;
        state.pan = atom_fit.1;
        return;
    }
    state.zoom = low;
    state.pan += rect.center() - bounds_at(state).center();
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

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_fitted(state: &SketcherState, painter: &eframe::egui::Painter, rect: Rect) {
        for atom in &state.sketch.atoms {
            assert!(rect.contains(state.to_screen(rect, atom.pos)));
        }
        for unit in &state.sketch.repeat_units {
            let drawing = repeat_unit_drawing(state, painter, rect, unit);
            assert!(
                rect.contains_rect(drawing.bounds()),
                "{:?}",
                drawing.bounds()
            );
            for [first, second] in &unit.brackets {
                assert!(rect.contains(state.to_screen(rect, *first)));
                assert!(rect.contains(state.to_screen(rect, *second)));
            }
            if let Some((pos, galley)) = drawing.label {
                assert!(rect.contains_rect(Rect::from_min_size(pos, galley.size())));
            }
        }
    }

    #[test]
    fn fit_includes_polymer_brackets_and_subscripts() {
        let ctx = eframe::egui::Context::default();
        let _ = ctx.run_ui(Default::default(), |ui| {
            for text in [
                include_str!("../../../compute-core/tests/fixtures/molfile/polystyrene.mol"),
                include_str!("../../../compute-core/tests/fixtures/molfile/quaternized_ppo.mol"),
                include_str!(
                    "../../../compute-core/tests/fixtures/molfile/piperidinium_polymer.mol"
                ),
            ] {
                let read = crate::io::molfile::read_molfile(text).unwrap();
                let original = read.sketch.clone();
                let mut state = SketcherState::with_sketch(read.sketch, read.title);
                for size in [vec2(920.0, 420.0), vec2(660.0, 220.0)] {
                    let rect = Rect::from_min_size(Pos2::new(100.0, 70.0), size);
                    state.zoom = 80.0;
                    state.pan = vec2(400.0, -300.0);
                    state.fit(rect, ui.painter());
                    assert_fitted(&state, ui.painter(), rect);
                    let fitted_zoom = state.zoom;
                    let fitted_pan = state.pan;
                    state.fit(rect, ui.painter());
                    assert!((state.zoom - fitted_zoom).abs() < 1e-4);
                    assert!((state.pan - fitted_pan).length() < 1e-3);
                    assert_eq!(state.sketch, original);
                }
            }
        });
    }

    #[test]
    fn fit_measures_long_subscripts_and_large_brackets() {
        let ctx = eframe::egui::Context::default();
        let _ = ctx.run_ui(Default::default(), |ui| {
            let read = crate::io::molfile::read_molfile(include_str!(
                "../../../compute-core/tests/fixtures/molfile/polystyrene.mol"
            ))
            .unwrap();
            let mut state = SketcherState::with_sketch(read.sketch, read.title);
            let rect = Rect::from_min_size(Pos2::new(50.0, 100.0), vec2(660.0, 220.0));
            state.sketch.repeat_units[0].brackets = vec![
                [Point2::new(-60.0, -30.0), Point2::new(-60.0, 30.0)],
                [Point2::new(60.0, 30.0), Point2::new(60.0, -30.0)],
            ];
            for label in ["n", "degree of polymerization (n + x + y)"] {
                state.sketch.repeat_units[0].label = label.to_string();
                state.fit(rect, ui.painter());
                assert_fitted(&state, ui.painter(), rect);
                assert!(state.zoom < 10.0);
            }
        });
    }

    #[test]
    fn fit_keeps_the_atoms_visible_when_a_label_cannot_fit() {
        let ctx = eframe::egui::Context::default();
        let _ = ctx.run_ui(Default::default(), |ui| {
            let read = crate::io::molfile::read_molfile(include_str!(
                "../../../compute-core/tests/fixtures/molfile/polystyrene.mol"
            ))
            .unwrap();
            let mut state = SketcherState::with_sketch(read.sketch, read.title);
            let rect = Rect::from_min_size(Pos2::new(50.0, 100.0), vec2(660.0, 220.0));
            state.sketch.repeat_units[0].label = "n".repeat(400);
            state.fit(rect, ui.painter());
            assert!(state.zoom >= 10.0, "{}", state.zoom);
        });
    }

    #[test]
    fn fit_without_repeat_units_keeps_the_existing_scale_and_center() {
        let ctx = eframe::egui::Context::default();
        let _ = ctx.run_ui(Default::default(), |ui| {
            let mut state = SketcherState::new();
            let rect = Rect::from_min_size(Pos2::new(100.0, 70.0), vec2(660.0, 220.0));
            state.fit(rect, ui.painter());
            assert_eq!(state.zoom, super::super::DEFAULT_ZOOM);
            assert_eq!(state.pan, Vec2::ZERO);
            state.sketch.add_atom("C", Point2::new(-10.0, -5.0));
            state.sketch.add_atom("C", Point2::new(20.0, 5.0));
            state.fit(rect, ui.painter());
            let expected_zoom = (660.0_f32 * 0.82 / 30.0).min(220.0 * 0.82 / 10.0);
            assert!((state.zoom - expected_zoom).abs() < 1e-5);
            assert!((state.pan - vec2(-5.0 * expected_zoom, 0.0)).length() < 1e-5);
        });
    }
}
