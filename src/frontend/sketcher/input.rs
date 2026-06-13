//! Pointer-gesture handling for the canvas.
//!
//! Translates egui pointer events into sketch mutations according to the active
//! tool. Navigation (pan/zoom) is always available; primary drags/clicks drive
//! the tool; in Move mode a secondary drag rotates. Undo snapshots are taken at
//! the start of each mutating gesture or click.

use eframe::egui::{Key, PointerButton, Pos2, Rect, Response, Ui, Vec2};
use nalgebra::Point2;

use super::{Gesture, SketchTool, SketcherState, placement};
use crate::domain::{BondType, sketch::BOND_LENGTH};
use crate::frontend::navigation;

/// Picking radius in screen pixels.
const PICK_PX: f32 = 14.0;

/// Navigation tuning for the OS this binary targets (see [`navigation`]).
const NAV_PROFILE: navigation::InputProfile = navigation::platform_profile();

/// Everything the gesture logic needs about this frame's pointer.
pub(super) struct Frame {
    pub rect: Rect,
    pub pointer: Option<Pos2>,
    pub pointer_model: Option<Point2<f32>>,
    pub hovered_atom: Option<usize>,
    pub hovered_bond: Option<usize>,
    pub shift: bool,
}

pub(super) fn handle(state: &mut SketcherState, response: &Response, ui: &Ui, frame: &Frame) {
    navigate(state, response, ui, frame);

    // Delete key removes the selection while the canvas is hovered. If it
    // actually deletes, the sketch is reindexed and this frame's picked indices
    // (in `frame`) are stale, so skip the gesture/click handlers below.
    if response.hovered()
        && ui.input(|input| input.key_pressed(Key::Delete) || input.key_pressed(Key::Backspace))
        && state.delete_selection()
    {
        return;
    }

    if response.drag_started() {
        begin_gesture(state, ui, frame);
    }
    if response.dragged() && state.gesture.is_some() {
        update_gesture(state, frame);
    }
    if response.drag_stopped() && state.gesture.is_some() {
        finalize_gesture(state, frame);
        state.gesture = None;
    }

    if response.clicked() {
        primary_click(state, frame);
    }
    if response.secondary_clicked() {
        secondary_click(state, frame);
    }
}

/// Pan with two-finger trackpad scroll or middle / secondary drags; zoom (toward
/// the pointer) on mouse wheel, trackpad pinch, or Ctrl/Cmd + scroll.
fn navigate(state: &mut SketcherState, response: &Response, ui: &Ui, frame: &Frame) {
    // Pan: middle drag in any tool, or secondary drag outside Move (where it
    // rotates instead).
    let pan_secondary =
        response.dragged_by(PointerButton::Secondary) && state.tool != SketchTool::Move;
    if response.dragged_by(PointerButton::Middle) || pan_secondary {
        state.pan += ui.input(|input| input.pointer.delta());
    }

    if !response.hovered() {
        return;
    }

    // Trackpad two-finger scroll pans; wheel / pinch / Ctrl+scroll zoom. The
    // device split is handled by `route_events`, shared with the 3D viewport.
    let nav = ui.input(|input| navigation::route_events(&input.events, &NAV_PROFILE));
    if nav.pan != Vec2::ZERO {
        state.pan += nav.pan;
    }
    if (nav.zoom - 1.0).abs() > f32::EPSILON
        && let Some(pointer) = frame.pointer
    {
        // Keep the model point under the cursor fixed while zooming.
        let before = state.to_model(frame.rect, pointer);
        state.zoom = (state.zoom * nav.zoom).clamp(6.0, 200.0);
        let after = state.to_screen(frame.rect, before);
        state.pan += pointer - after;
    }
}

fn pick_radius(state: &SketcherState) -> f32 {
    PICK_PX / state.zoom
}

fn begin_gesture(state: &mut SketcherState, ui: &Ui, frame: &Frame) {
    let Some(start) = frame.pointer_model else {
        return;
    };
    let primary = ui.input(|input| input.pointer.primary_down());
    let secondary = ui.input(|input| input.pointer.secondary_down());

    if primary {
        begin_primary(state, frame, start);
    } else if secondary && state.tool == SketchTool::Move {
        begin_rotate(state, start);
    }
}

fn begin_primary(state: &mut SketcherState, frame: &Frame, start: Point2<f32>) {
    match state.tool {
        SketchTool::Draw => {
            let anchor = frame
                .hovered_atom
                .and_then(|atom| state.sketch.atoms.get(atom))
                .map(|atom| atom.pos)
                .unwrap_or(start);
            state.gesture = Some(Gesture::DrawBond {
                from: frame.hovered_atom,
                anchor,
                order: BondType::Single,
            });
        }
        SketchTool::Bond => {
            let anchor = frame
                .hovered_atom
                .and_then(|atom| state.sketch.atoms.get(atom))
                .map(|atom| atom.pos)
                .unwrap_or(start);
            state.gesture = Some(Gesture::DrawBond {
                from: frame.hovered_atom,
                anchor,
                order: state.active_bond,
            });
        }
        SketchTool::Chain => {
            state.push_undo();
            let last = frame
                .hovered_atom
                .unwrap_or_else(|| state.sketch.add_atom("C", start));
            state.gesture = Some(Gesture::Chain { last, count: 0 });
        }
        SketchTool::Erase => {
            state.push_undo();
            state.gesture = Some(Gesture::Erase);
            erase_at(state, frame);
        }
        SketchTool::Select => {
            state.gesture = Some(Gesture::Marquee {
                start,
                additive: frame.shift,
            });
        }
        SketchTool::Move => {
            let atoms: Vec<usize> = state.selected_atoms.iter().copied().collect();
            let whole = atoms.is_empty();
            state.push_undo();
            state.gesture = Some(Gesture::Translate {
                last: start,
                atoms,
                whole,
            });
        }
        SketchTool::Ring | SketchTool::Charge => {
            // Placed / applied on click, not drag.
        }
    }
}

fn begin_rotate(state: &mut SketcherState, start: Point2<f32>) {
    let atoms: Vec<usize> = state.selected_atoms.iter().copied().collect();
    let whole = atoms.is_empty();
    let center = if whole {
        state.sketch.centroid()
    } else {
        selection_centroid(state, &atoms)
    };
    let last = (start - center).y.atan2((start - center).x);
    state.push_undo();
    state.gesture = Some(Gesture::Rotate {
        center,
        last,
        accum: 0.0,
        atoms,
        whole,
    });
}

fn update_gesture(state: &mut SketcherState, frame: &Frame) {
    let Some(pointer) = frame.pointer_model else {
        return;
    };
    // Clone out the gesture to satisfy the borrow checker, then write back.
    let Some(gesture) = state.gesture.take() else {
        return;
    };
    let gesture = match gesture {
        Gesture::Chain {
            mut last,
            mut count,
        } => {
            // Append carbons toward the pointer, one bond length apart.
            let mut guard = 0;
            while guard < 64 {
                let Some(last_pos) = state.sketch.atoms.get(last).map(|atom| atom.pos) else {
                    break;
                };
                let delta = pointer - last_pos;
                if delta.norm() < BOND_LENGTH {
                    break;
                }
                let step = delta.normalize() * BOND_LENGTH;
                let next = state.sketch.add_atom("C", last_pos + step);
                state.sketch.add_bond(last, next, BondType::Single);
                last = next;
                count += 1;
                guard += 1;
            }
            Gesture::Chain { last, count }
        }
        Gesture::Translate { last, atoms, whole } => {
            let delta = pointer - last;
            if whole {
                state.sketch.translate(delta);
            } else {
                state.sketch.translate_atoms(&atoms, delta);
            }
            Gesture::Translate {
                last: pointer,
                atoms,
                whole,
            }
        }
        Gesture::Rotate {
            center,
            last,
            accum,
            atoms,
            whole,
        } => {
            let current = (pointer - center).y.atan2((pointer - center).x);
            let mut delta = current - last;
            // Normalise into (−π, π].
            while delta > std::f32::consts::PI {
                delta -= std::f32::consts::TAU;
            }
            while delta < -std::f32::consts::PI {
                delta += std::f32::consts::TAU;
            }
            if whole {
                state.sketch.rotate(center, delta);
            } else {
                state.sketch.rotate_atoms(&atoms, center, delta);
            }
            Gesture::Rotate {
                center,
                last: current,
                accum: accum + delta,
                atoms,
                whole,
            }
        }
        Gesture::Erase => {
            erase_at(state, frame);
            Gesture::Erase
        }
        other => other,
    };
    state.gesture = Some(gesture);
}

fn finalize_gesture(state: &mut SketcherState, frame: &Frame) {
    let Some(gesture) = state.gesture.clone() else {
        return;
    };
    match gesture {
        Gesture::DrawBond {
            from,
            anchor,
            order,
        } => finalize_draw_bond(state, frame, from, anchor, order),
        Gesture::Marquee { start, additive } => finalize_marquee(state, frame, start, additive),
        // Chain / Translate / Rotate / Erase have already applied their effects.
        _ => {}
    }
}

fn finalize_draw_bond(
    state: &mut SketcherState,
    frame: &Frame,
    from: Option<usize>,
    anchor: Point2<f32>,
    order: BondType,
) {
    let Some(end) = frame.pointer_model else {
        return;
    };
    let element = if state.tool == SketchTool::Draw {
        state.active_element.clone()
    } else {
        "C".to_string()
    };
    let target = frame.hovered_atom.filter(|atom| Some(*atom) != from);

    match from {
        Some(a) => {
            let Some(from_pos) = state.sketch.atoms.get(a).map(|atom| atom.pos) else {
                return;
            };
            if let Some(b) = target {
                state.push_undo();
                if state.sketch.bond_between(a, b).is_none() {
                    state.sketch.add_bond(a, b, order);
                } else if let Some(index) = state.sketch.bond_between(a, b) {
                    state.sketch.set_bond_order(index, order);
                }
            } else if (end - from_pos).norm() > 0.3 * BOND_LENGTH {
                state.push_undo();
                let position = grow_or_pointer(state, a, end);
                let new = state.sketch.add_atom(&element, position);
                state.sketch.add_bond(a, new, order);
            }
        }
        None => {
            state.push_undo();
            match target {
                Some(b) => {
                    let start = state.sketch.add_atom(&element, anchor);
                    state.sketch.add_bond(start, b, order);
                }
                None => {
                    let start = state.sketch.add_atom(&element, anchor);
                    let finish = state.sketch.add_atom(&element, end);
                    state.sketch.add_bond(start, finish, order);
                }
            }
        }
    }
}

/// When the drag is short, snap the new atom to a tidy grown position; otherwise
/// honour where the pointer was released.
fn grow_or_pointer(state: &SketcherState, from: usize, end: Point2<f32>) -> Point2<f32> {
    let Some(from_pos) = state.sketch.atoms.get(from).map(|atom| atom.pos) else {
        return end;
    };
    let pointer_distance = (end - from_pos).norm();
    if pointer_distance < 0.6 * BOND_LENGTH {
        state.sketch.grow_position(from)
    } else {
        end
    }
}

fn finalize_marquee(state: &mut SketcherState, frame: &Frame, start: Point2<f32>, additive: bool) {
    let Some(end) = frame.pointer_model else {
        return;
    };
    let (min_x, max_x) = (start.x.min(end.x), start.x.max(end.x));
    let (min_y, max_y) = (start.y.min(end.y), start.y.max(end.y));
    // A zero-area marquee is a click on empty space → clear unless additive.
    if (max_x - min_x) < 0.05 && (max_y - min_y) < 0.05 {
        if !additive {
            state.clear_selection();
        }
        return;
    }
    if !additive {
        state.clear_selection();
    }
    for (index, atom) in state.sketch.atoms.iter().enumerate() {
        if atom.pos.x >= min_x && atom.pos.x <= max_x && atom.pos.y >= min_y && atom.pos.y <= max_y
        {
            state.selected_atoms.insert(index);
        }
    }
    // Bonds whose endpoints are both selected come along.
    let selected = state.selected_atoms.clone();
    for (index, bond) in state.sketch.bonds.iter().enumerate() {
        if selected.contains(&bond.a) && selected.contains(&bond.b) {
            state.selected_bonds.insert(index);
        }
    }
}

fn erase_at(state: &mut SketcherState, frame: &Frame) {
    let Some(pointer) = frame.pointer_model else {
        return;
    };
    let radius = pick_radius(state);
    if let Some(atom) = state.sketch.nearest_atom(pointer, radius) {
        state.sketch.remove_atom(atom);
        state.clamp_selection();
    } else if let Some(bond) = state.sketch.nearest_bond(pointer, radius) {
        state.sketch.remove_bond(bond);
        state.clamp_selection();
    }
}

fn primary_click(state: &mut SketcherState, frame: &Frame) {
    let Some(pointer) = frame.pointer_model else {
        return;
    };
    match state.tool {
        SketchTool::Draw => {
            if let Some(atom) = frame.hovered_atom {
                if state.active_element == "C" {
                    state.push_undo();
                    let position = state.sketch.grow_position(atom);
                    let new = state.sketch.add_atom("C", position);
                    state.sketch.add_bond(atom, new, BondType::Single);
                } else {
                    state.push_undo();
                    state.sketch.atoms[atom].element = state.active_element.clone();
                }
            } else if let Some(bond) = frame.hovered_bond {
                state.push_undo();
                state.sketch.cycle_bond_order(bond);
            } else {
                state.push_undo();
                let element = state.active_element.clone();
                state.sketch.add_atom(element, pointer);
            }
        }
        SketchTool::Bond => {
            if let Some(bond) = frame.hovered_bond {
                state.push_undo();
                state.sketch.set_bond_order(bond, state.active_bond);
            } else if let Some(atom) = frame.hovered_atom {
                state.push_undo();
                let position = state.sketch.grow_position(atom);
                let new = state.sketch.add_atom("C", position);
                state.sketch.add_bond(atom, new, state.active_bond);
            }
        }
        SketchTool::Chain => {
            if let Some(atom) = frame.hovered_atom {
                state.push_undo();
                let position = state.sketch.grow_position(atom);
                let new = state.sketch.add_atom("C", position);
                state.sketch.add_bond(atom, new, BondType::Single);
            } else {
                state.push_undo();
                state.sketch.add_atom("C", pointer);
            }
        }
        SketchTool::Ring => {
            let placement = placement::place_ring(
                &state.sketch,
                state.active_ring,
                pointer,
                pick_radius(state) * 2.5,
            );
            state.commit_ring(&placement);
        }
        SketchTool::Erase => {
            if let Some(atom) = frame.hovered_atom {
                state.push_undo();
                state.sketch.remove_atom(atom);
                state.clamp_selection();
            } else if let Some(bond) = frame.hovered_bond {
                state.push_undo();
                step_bond_down(state, bond);
            }
        }
        SketchTool::Select => select_click(state, frame),
        SketchTool::Charge => {
            if let Some(atom) = frame.hovered_atom {
                state.push_undo();
                state.sketch.adjust_charge(atom, 1);
            }
        }
        SketchTool::Move => {}
    }
}

fn secondary_click(state: &mut SketcherState, frame: &Frame) {
    match state.tool {
        SketchTool::Charge => {
            if let Some(atom) = frame.hovered_atom {
                state.push_undo();
                state.sketch.adjust_charge(atom, -1);
            }
        }
        SketchTool::Erase => {
            if let Some(bond) = frame.hovered_bond {
                state.push_undo();
                state.sketch.remove_bond(bond);
                state.clamp_selection();
            }
        }
        _ => {}
    }
}

fn select_click(state: &mut SketcherState, frame: &Frame) {
    if let Some(atom) = frame.hovered_atom {
        if !frame.shift {
            state.clear_selection();
        }
        toggle(&mut state.selected_atoms, atom);
    } else if let Some(bond) = frame.hovered_bond {
        if !frame.shift {
            state.clear_selection();
        }
        toggle(&mut state.selected_bonds, bond);
    } else if !frame.shift {
        state.clear_selection();
    }
}

fn toggle(set: &mut std::collections::BTreeSet<usize>, value: usize) {
    if !set.insert(value) {
        set.remove(&value);
    }
}

/// Step a bond's order down, deleting it when it was already single.
fn step_bond_down(state: &mut SketcherState, bond: usize) {
    let Some(order) = state.sketch.bonds.get(bond).map(|b| b.order) else {
        return;
    };
    match order {
        BondType::Triple => state.sketch.set_bond_order(bond, BondType::Double),
        BondType::Double | BondType::Aromatic => {
            state.sketch.set_bond_order(bond, BondType::Single)
        }
        BondType::Single => {
            state.sketch.remove_bond(bond);
            state.clamp_selection();
        }
    }
}

fn selection_centroid(state: &SketcherState, atoms: &[usize]) -> Point2<f32> {
    if atoms.is_empty() {
        return state.sketch.centroid();
    }
    let sum = atoms
        .iter()
        .filter_map(|index| state.sketch.atoms.get(*index))
        .fold(nalgebra::Vector2::zeros(), |acc, atom| {
            acc + atom.pos.coords
        });
    Point2::from(sum / atoms.len() as f32)
}
