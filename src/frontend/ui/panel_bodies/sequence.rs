use eframe::egui::{
    self, Align, Color32, FontFamily, FontId, Pos2, Rect, RichText, ScrollArea, Sense, Stroke, Vec2,
};

use crate::{
    domain::{
        Structure,
        biopolymer::{
            ResiduePolymerKind, SecondaryStructureKind, residue_polymer_kind,
            residue_sequence_symbol,
        },
    },
    frontend::{
        AtomSelection,
        actions::{AppAction, ResidueSelectionMode},
        state::{AppState, SequenceDragState, SequenceViewerState},
    },
};

const CHAIN_RAIL_WIDTH: f32 = 58.0;
const CELL_WIDTH: f32 = 16.0;
const NUMBER_HEIGHT: f32 = 15.0;
const SEQUENCE_HEIGHT: f32 = 22.0;
const SECONDARY_HEIGHT: f32 = 5.0;
const CHAIN_ROW_HEIGHT: f32 = NUMBER_HEIGHT + SEQUENCE_HEIGHT + SECONDARY_HEIGHT + 10.0;
const DRAG_MIN_DISTANCE: f32 = 4.0;

#[derive(Debug, Clone)]
struct SequenceResidue {
    residue_index: usize,
    symbol: char,
    kind: ResiduePolymerKind,
    secondary: Option<SecondaryStructureKind>,
    chain_id: char,
    sequence_number: i32,
    insertion_code: char,
    residue_name: String,
    atom_count: usize,
    first_atom: Option<usize>,
    any_selected: bool,
    all_selected: bool,
    primary_selected: bool,
}

#[derive(Debug, Clone)]
struct SequenceChain {
    id: char,
    residues: Vec<SequenceResidue>,
}

#[derive(Clone, Copy)]
struct ResidueCell {
    residue_index: usize,
    rect: Rect,
}
pub(crate) fn render_sequence_panel(
    state: &mut AppState,
    ui: &mut egui::Ui,
    actions: &mut Vec<AppAction>,
) {
    let pal = crate::frontend::theme::palette(ui);
    ui.set_width(ui.available_width());

    let selection = state.ui.selection.clone();
    let chains = sequence_chains(state.structure(), &selection);
    render_sequence_header(ui, &chains, selection.len());
    ui.separator();

    if chains.is_empty() {
        state.ui.sequence.last_clicked_residue = None;
        state.ui.sequence.last_scrolled_primary_atom = None;
        state.ui.sequence.drag = None;
        ui.add_space(8.0);
        ui.label(
            RichText::new("No protein or nucleic-acid sequence metadata for the active structure.")
                .small()
                .color(pal.text_tertiary),
        );
        return;
    }

    let primary_atom = selection.primary();
    let primary_residue = primary_residue(&chains);
    let primary_chain = primary_residue.map(|(_, chain_id)| chain_id);
    let chain_colors = state.ui.viewport.chain_colors.clone();
    let viewer = &mut state.ui.sequence;
    if viewer
        .chain_filter
        .is_some_and(|id| !chains.iter().any(|chain| chain.id == id))
    {
        viewer.chain_filter = None;
    }

    render_sequence_toolbar(ui, viewer, &chains, primary_chain);

    let scroll_target = primary_atom.and_then(|atom_index| {
        (viewer.last_scrolled_primary_atom != Some(atom_index))
            .then(|| primary_residue.map(|(residue_index, _)| residue_index))
            .flatten()
    });
    if primary_atom.is_none() {
        viewer.last_scrolled_primary_atom = None;
    }

    let chain_filter = viewer.chain_filter;
    let mut scrolled_to_primary = false;
    let mut cells = Vec::new();
    ScrollArea::both()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            for chain in chains
                .iter()
                .filter(|chain| chain_filter.is_none_or(|id| chain.id == id))
            {
                let chain_color = chain_colors.get(&chain.id).copied();
                scrolled_to_primary |= render_sequence_chain(
                    ui,
                    chain,
                    chain_color,
                    viewer,
                    actions,
                    scroll_target,
                    &mut cells,
                );
                ui.add_space(4.0);
            }
        });

    update_drag_selection(ui, viewer, &cells, actions);

    if scrolled_to_primary {
        viewer.last_scrolled_primary_atom = primary_atom;
    }
}
fn render_sequence_header(ui: &mut egui::Ui, chains: &[SequenceChain], selected_atoms: usize) {
    let pal = crate::frontend::theme::palette(ui);
    let selected_residues = chains
        .iter()
        .flat_map(|chain| &chain.residues)
        .filter(|residue| residue.any_selected)
        .count();

    ui.horizontal(|ui| {
        ui.label(RichText::new("Sequence").strong());
        ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
            if selected_atoms > 0 {
                ui.label(
                    RichText::new(format!(
                        "{selected_residues} residue(s) / {selected_atoms} atom(s) selected"
                    ))
                    .small()
                    .color(pal.text_tertiary),
                );
            }
        });
    });
}
fn render_sequence_toolbar(
    ui: &mut egui::Ui,
    viewer: &mut SequenceViewerState,
    chains: &[SequenceChain],
    primary_chain: Option<char>,
) {
    ui.horizontal(|ui| {
        if chains.len() > 1 {
            let before = viewer.chain_filter;
            egui::ComboBox::from_id_salt("sequence_chain_filter")
                .selected_text(
                    viewer
                        .chain_filter
                        .map(|id| format!("Chain {}", display_chain_id(id)))
                        .unwrap_or_else(|| "All chains".to_string()),
                )
                .width(130.0)
                .show_ui(ui, |ui| {
                    crate::frontend::theme::stabilize_selectable_rows(ui);
                    ui.selectable_value(&mut viewer.chain_filter, None, "All chains");
                    for chain in chains {
                        ui.selectable_value(
                            &mut viewer.chain_filter,
                            Some(chain.id),
                            format!("Chain {}", display_chain_id(chain.id)),
                        );
                    }
                });
            if viewer.chain_filter != before {
                viewer.last_scrolled_primary_atom = None;
            }
        }

        if let Some(chain_id) = primary_chain {
            if ui
                .button("Primary")
                .on_hover_text("Show primary residue")
                .clicked()
            {
                viewer.chain_filter = Some(chain_id);
                viewer.last_scrolled_primary_atom = None;
            }
        } else {
            ui.add_enabled(false, egui::Button::new("Primary"))
                .on_hover_text("Show primary residue");
        }
    });
    ui.add_space(4.0);
}
fn sequence_chains(structure: &Structure, selection: &AtomSelection) -> Vec<SequenceChain> {
    let Some(biopolymer) = structure
        .biopolymer
        .as_ref()
        .filter(|bio| bio.is_compatible_with_atom_count(structure.atoms.len()))
    else {
        return Vec::new();
    };

    let primary = selection.primary();
    biopolymer
        .chains
        .iter()
        .filter_map(|chain| {
            let residues = chain
                .residue_indices
                .iter()
                .filter_map(|&residue_index| {
                    let residue = biopolymer.residues.get(residue_index)?;
                    let kind = residue_polymer_kind(&residue.residue_name, residue)?;
                    let selected_count = residue
                        .atom_indices
                        .iter()
                        .filter(|&&atom_index| selection.contains(atom_index))
                        .count();
                    let atom_count = residue.atom_indices.len();
                    Some(SequenceResidue {
                        residue_index,
                        symbol: residue_sequence_symbol(&residue.residue_name, kind),
                        kind,
                        secondary: (kind == ResiduePolymerKind::Protein)
                            .then(|| secondary_structure_for_residue(biopolymer, residue))
                            .flatten(),
                        chain_id: residue.id.chain_id,
                        sequence_number: residue.id.sequence_number,
                        insertion_code: residue.id.insertion_code,
                        residue_name: residue.residue_name.clone(),
                        atom_count,
                        first_atom: residue.atom_indices.first().copied(),
                        any_selected: selected_count > 0,
                        all_selected: atom_count > 0 && selected_count == atom_count,
                        primary_selected: primary
                            .is_some_and(|atom_index| residue.atom_indices.contains(&atom_index)),
                    })
                })
                .collect::<Vec<_>>();

            (!residues.is_empty()).then_some(SequenceChain {
                id: chain.id,
                residues,
            })
        })
        .collect()
}
fn secondary_structure_for_residue(
    biopolymer: &crate::domain::biopolymer::Biopolymer,
    residue: &crate::domain::biopolymer::ResidueRecord,
) -> Option<SecondaryStructureKind> {
    let residue_key = residue.id.ordering_key();
    biopolymer
        .secondary_structures
        .iter()
        .find(|span| {
            if span.start.chain_id != residue.id.chain_id
                || span.end.chain_id != residue.id.chain_id
            {
                return false;
            }
            let start = span.start.ordering_key();
            let end = span.end.ordering_key();
            if start <= end {
                residue_key >= start && residue_key <= end
            } else {
                residue_key >= end && residue_key <= start
            }
        })
        .map(|span| span.kind)
}
fn render_sequence_chain(
    ui: &mut egui::Ui,
    chain: &SequenceChain,
    chain_color: Option<Color32>,
    viewer: &mut SequenceViewerState,
    actions: &mut Vec<AppAction>,
    scroll_target: Option<usize>,
    cells: &mut Vec<ResidueCell>,
) -> bool {
    let pal = crate::frontend::theme::palette(ui);
    let sequence_width = chain.residues.len() as f32 * CELL_WIDTH;
    let width = (CHAIN_RAIL_WIDTH + sequence_width + 8.0).max(ui.available_width());
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(width, CHAIN_ROW_HEIGHT), Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    let sequence_left = rect.left() + CHAIN_RAIL_WIDTH;
    let number_top = rect.top() + 2.0;
    let sequence_top = rect.top() + NUMBER_HEIGHT;
    let secondary_top = sequence_top + SEQUENCE_HEIGHT + 2.0;
    let sequence_rect = Rect::from_min_size(
        Pos2::new(sequence_left, sequence_top),
        Vec2::new(sequence_width, SEQUENCE_HEIGHT),
    );
    let hover_residue = response
        .hover_pos()
        .and_then(|pos| residue_at_pos(chain, rect, pos));

    draw_chain_rail(&painter, rect, chain, chain_color, pal);
    draw_number_axis(&painter, chain, sequence_left, number_top, pal);
    draw_secondary_track(&painter, chain, sequence_left, secondary_top, ui);
    draw_selection_runs(&painter, chain, sequence_left, sequence_top, pal);

    if let Some(residue_index) = hover_residue
        && let Some((index, residue)) = chain
            .residues
            .iter()
            .enumerate()
            .find(|(_, residue)| residue.residue_index == residue_index)
    {
        let cell = residue_rect(sequence_left, sequence_top, index);
        if !residue.any_selected {
            painter.rect_filled(
                cell.shrink(1.0),
                egui::CornerRadius::same(crate::frontend::theme::radius::CHIP),
                pal.neutral_overlay(18),
            );
        }
    }

    painter.rect_stroke(
        sequence_rect.expand2(Vec2::new(1.0, 1.0)),
        egui::CornerRadius::same(crate::frontend::theme::radius::CHIP),
        Stroke::new(1.0_f32, pal.neutral_overlay(24)),
        egui::StrokeKind::Inside,
    );

    let mut scrolled_to_primary = false;
    for (index, residue) in chain.residues.iter().enumerate() {
        let cell = residue_rect(sequence_left, sequence_top, index);
        cells.push(ResidueCell {
            residue_index: residue.residue_index,
            rect: cell,
        });

        if should_mark_gap(chain, index) {
            painter.line_segment(
                [
                    Pos2::new(cell.left(), sequence_top + 2.0),
                    Pos2::new(cell.left(), sequence_top + SEQUENCE_HEIGHT - 2.0),
                ],
                Stroke::new(1.0_f32, pal.status_amber),
            );
        }

        if residue.primary_selected {
            painter.rect_stroke(
                cell.shrink(1.0),
                egui::CornerRadius::same(crate::frontend::theme::radius::CHIP),
                Stroke::new(2.0_f32, pal.accent),
                egui::StrokeKind::Inside,
            );
        } else if residue.any_selected && !residue.all_selected {
            painter.line_segment(
                [
                    Pos2::new(cell.left() + 3.0, cell.bottom() - 3.0),
                    Pos2::new(cell.right() - 3.0, cell.bottom() - 3.0),
                ],
                Stroke::new(2.0_f32, pal.accent),
            );
        }

        let text_color = if residue.primary_selected {
            Color32::WHITE
        } else {
            pal.text_primary
        };
        painter.text(
            cell.center(),
            egui::Align2::CENTER_CENTER,
            residue.symbol,
            FontId::new(13.0, FontFamily::Monospace),
            text_color,
        );

        if scroll_target == Some(residue.residue_index) {
            ui.scroll_to_rect(cell, Some(Align::Center));
            scrolled_to_primary = true;
        }
    }

    let response = response.on_hover_text(hover_residue_tooltip(chain, hover_residue));
    handle_chain_interaction(ui, &response, rect, chain, viewer, actions);
    scrolled_to_primary
}
fn draw_chain_rail(
    painter: &egui::Painter,
    rect: Rect,
    chain: &SequenceChain,
    chain_color: Option<Color32>,
    pal: crate::frontend::theme::Palette,
) {
    let rail = Rect::from_min_max(
        rect.min,
        Pos2::new(rect.left() + CHAIN_RAIL_WIDTH - 8.0, rect.bottom()),
    );
    painter.rect_filled(
        rail.shrink2(Vec2::new(0.0, 2.0)),
        egui::CornerRadius::same(crate::frontend::theme::radius::CONTROL),
        pal.neutral_overlay(10),
    );
    let swatch = Rect::from_min_size(
        Pos2::new(rail.left() + 7.0, rail.top() + 9.0),
        Vec2::new(5.0, CHAIN_ROW_HEIGHT - 18.0),
    );
    painter.rect_filled(
        swatch,
        egui::CornerRadius::same(crate::frontend::theme::radius::MIN),
        chain_color.unwrap_or_else(|| pal.neutral_overlay(70)),
    );
    painter.text(
        Pos2::new(rail.left() + 24.0, rail.center().y - 5.0),
        egui::Align2::LEFT_CENTER,
        display_chain_id(chain.id),
        FontId::new(14.0, FontFamily::Monospace),
        pal.text_strong,
    );
    painter.text(
        Pos2::new(rail.left() + 24.0, rail.center().y + 9.0),
        egui::Align2::LEFT_CENTER,
        chain.residues.len().to_string(),
        FontId::proportional(10.0),
        pal.text_tertiary,
    );
}
fn draw_number_axis(
    painter: &egui::Painter,
    chain: &SequenceChain,
    sequence_left: f32,
    top: f32,
    pal: crate::frontend::theme::Palette,
) {
    for (index, residue) in chain.residues.iter().enumerate() {
        let x = sequence_left + index as f32 * CELL_WIDTH + CELL_WIDTH / 2.0;
        let show_label = index == 0 || residue.sequence_number.rem_euclid(10) == 0;
        let tick_height = if show_label { 5.0 } else { 2.5 };
        painter.line_segment(
            [
                Pos2::new(x, top + 10.0),
                Pos2::new(x, top + 10.0 + tick_height),
            ],
            Stroke::new(1.0_f32, pal.neutral_overlay(60)),
        );
        if show_label {
            painter.text(
                Pos2::new(x, top + 3.0),
                egui::Align2::CENTER_CENTER,
                residue_number_label(residue),
                FontId::new(10.0, FontFamily::Monospace),
                pal.text_tertiary,
            );
        }
    }
}
fn draw_secondary_track(
    painter: &egui::Painter,
    chain: &SequenceChain,
    sequence_left: f32,
    top: f32,
    ui: &egui::Ui,
) {
    for (index, residue) in chain.residues.iter().enumerate() {
        let Some(kind) = residue.secondary else {
            continue;
        };
        let x = sequence_left + index as f32 * CELL_WIDTH + 2.0;
        let rect = Rect::from_min_size(
            Pos2::new(x, top),
            Vec2::new(CELL_WIDTH - 4.0, SECONDARY_HEIGHT),
        );
        painter.rect_filled(
            rect,
            egui::CornerRadius::same(crate::frontend::theme::radius::MIN),
            secondary_structure_fill(ui, kind),
        );
    }
}
fn draw_selection_runs(
    painter: &egui::Painter,
    chain: &SequenceChain,
    sequence_left: f32,
    sequence_top: f32,
    pal: crate::frontend::theme::Palette,
) {
    for (index, residue) in chain.residues.iter().enumerate() {
        if residue.any_selected && !residue.all_selected {
            let rect = residue_rect(sequence_left, sequence_top, index);
            painter.rect_filled(
                rect.shrink(1.0),
                egui::CornerRadius::same(crate::frontend::theme::radius::CHIP),
                pal.blue_overlay(44),
            );
        }
    }

    let mut start = None;
    for index in 0..=chain.residues.len() {
        let selected = chain
            .residues
            .get(index)
            .is_some_and(|residue| residue.all_selected);
        match (start, selected) {
            (None, true) => start = Some(index),
            (Some(run_start), false) => {
                let left = sequence_left + run_start as f32 * CELL_WIDTH + 1.0;
                let right = sequence_left + index as f32 * CELL_WIDTH - 1.0;
                let rect = Rect::from_min_max(
                    Pos2::new(left, sequence_top + 1.0),
                    Pos2::new(right, sequence_top + SEQUENCE_HEIGHT - 1.0),
                );
                painter.rect_filled(
                    rect,
                    egui::CornerRadius::same(crate::frontend::theme::radius::CHIP),
                    pal.selection_fill,
                );
                start = None;
            }
            _ => {}
        }
    }
}
fn handle_chain_interaction(
    ui: &egui::Ui,
    response: &egui::Response,
    rect: Rect,
    chain: &SequenceChain,
    viewer: &mut SequenceViewerState,
    actions: &mut Vec<AppAction>,
) {
    if response.double_clicked()
        && response
            .interact_pointer_pos()
            .is_some_and(|pos| pos.x < rect.left() + CHAIN_RAIL_WIDTH - 8.0)
    {
        actions.push(AppAction::SelectResidues {
            residue_indices: chain
                .residues
                .iter()
                .map(|residue| residue.residue_index)
                .collect(),
            mode: ResidueSelectionMode::Replace,
        });
        viewer.last_clicked_residue = chain.residues.last().map(|residue| residue.residue_index);
        viewer.last_scrolled_primary_atom = None;
        return;
    }

    if response.drag_started_by(egui::PointerButton::Primary)
        && let Some(pos) = response.interact_pointer_pos()
    {
        let mode = drag_selection_mode(ui);
        viewer.drag = Some(SequenceDragState {
            origin: pos,
            current: pos,
            mode,
        });
    }

    if response.clicked_by(egui::PointerButton::Primary)
        && let Some(pos) = response.interact_pointer_pos()
        && let Some(residue_index) = residue_at_pos(chain, rect, pos)
    {
        let modifiers = ui.input(|input| input.modifiers);
        let toggle = modifiers.ctrl || modifiers.command;
        if let Some(anchor) = viewer.last_clicked_residue.filter(|anchor| {
            modifiers.shift
                && chain
                    .residues
                    .iter()
                    .any(|residue| residue.residue_index == *anchor)
        }) {
            actions.push(AppAction::SelectResidueRange {
                chain_id: chain.id,
                start: anchor,
                end: residue_index,
                toggle,
            });
        } else {
            actions.push(AppAction::SelectResidue {
                residue_index,
                toggle,
            });
        }
        viewer.last_clicked_residue = Some(residue_index);
        viewer.last_scrolled_primary_atom = None;
    }
}
fn update_drag_selection(
    ui: &mut egui::Ui,
    viewer: &mut SequenceViewerState,
    cells: &[ResidueCell],
    actions: &mut Vec<AppAction>,
) {
    if let Some(drag) = &mut viewer.drag
        && let Some(pos) = ui.ctx().pointer_interact_pos()
    {
        drag.current = pos;
    }

    let Some(drag) = viewer.drag else {
        return;
    };

    let rect = Rect::from_two_pos(drag.origin, drag.current);
    let drag_distance = drag.origin.distance(drag.current);
    let selected = residues_in_drag_rect(cells, rect);
    if drag_distance >= DRAG_MIN_DISTANCE {
        let pal = crate::frontend::theme::palette(ui);
        for cell in cells
            .iter()
            .filter(|cell| selected.contains(&cell.residue_index))
        {
            ui.painter().rect_filled(
                cell.rect.shrink(1.0),
                egui::CornerRadius::same(crate::frontend::theme::radius::CHIP),
                drag_preview_fill(drag.mode, pal),
            );
        }
        ui.painter().rect_stroke(
            rect,
            egui::CornerRadius::same(crate::frontend::theme::radius::CONTROL),
            Stroke::new(1.0_f32, pal.accent),
            egui::StrokeKind::Inside,
        );
        ui.ctx()
            .request_repaint_after(crate::frontend::viewport::STRUCTURE_INTERACTION_FRAME);
    }

    let released = ui.input(|input| input.pointer.button_released(egui::PointerButton::Primary));
    let primary_down = ui.input(|input| input.pointer.button_down(egui::PointerButton::Primary));
    if released {
        viewer.drag = None;
        if drag_distance >= DRAG_MIN_DISTANCE && !selected.is_empty() {
            viewer.last_clicked_residue = selected.last().copied();
            viewer.last_scrolled_primary_atom = None;
            actions.push(AppAction::SelectResidues {
                residue_indices: selected,
                mode: drag.mode,
            });
        }
    } else if !primary_down {
        viewer.drag = None;
    }
}
fn residue_at_pos(chain: &SequenceChain, chain_rect: Rect, pos: Pos2) -> Option<usize> {
    let sequence_left = chain_rect.left() + CHAIN_RAIL_WIDTH;
    let sequence_top = chain_rect.top() + NUMBER_HEIGHT;
    let sequence_bottom = sequence_top + SEQUENCE_HEIGHT + SECONDARY_HEIGHT + 3.0;
    if pos.x < sequence_left || pos.y < chain_rect.top() || pos.y > sequence_bottom {
        return None;
    }

    let index = ((pos.x - sequence_left) / CELL_WIDTH).floor() as usize;
    chain
        .residues
        .get(index)
        .map(|residue| residue.residue_index)
}
fn residues_in_drag_rect(cells: &[ResidueCell], rect: Rect) -> Vec<usize> {
    let mut seen = std::collections::BTreeSet::new();
    cells
        .iter()
        .filter(|cell| cell.rect.intersects(rect))
        .filter_map(|cell| {
            seen.insert(cell.residue_index)
                .then_some(cell.residue_index)
        })
        .collect()
}
fn drag_selection_mode(ui: &egui::Ui) -> ResidueSelectionMode {
    let modifiers = ui.input(|input| input.modifiers);
    if modifiers.alt {
        ResidueSelectionMode::Remove
    } else if modifiers.ctrl || modifiers.command {
        ResidueSelectionMode::Toggle
    } else if modifiers.shift {
        ResidueSelectionMode::Add
    } else {
        ResidueSelectionMode::Replace
    }
}
fn drag_preview_fill(mode: ResidueSelectionMode, pal: crate::frontend::theme::Palette) -> Color32 {
    match mode {
        ResidueSelectionMode::Replace | ResidueSelectionMode::Add => pal.blue_overlay(56),
        ResidueSelectionMode::Toggle => pal.status_amber.gamma_multiply(0.35),
        ResidueSelectionMode::Remove => pal.status_red.gamma_multiply(0.28),
    }
}
fn residue_rect(sequence_left: f32, sequence_top: f32, index: usize) -> Rect {
    Rect::from_min_size(
        Pos2::new(sequence_left + index as f32 * CELL_WIDTH, sequence_top),
        Vec2::new(CELL_WIDTH, SEQUENCE_HEIGHT),
    )
}
fn should_mark_gap(chain: &SequenceChain, index: usize) -> bool {
    let Some(previous) = index
        .checked_sub(1)
        .and_then(|index| chain.residues.get(index))
    else {
        return false;
    };
    let residue = &chain.residues[index];
    display_insertion_code(previous.insertion_code).is_empty()
        && display_insertion_code(residue.insertion_code).is_empty()
        && residue.sequence_number != previous.sequence_number + 1
}
fn secondary_structure_fill(ui: &egui::Ui, kind: SecondaryStructureKind) -> Color32 {
    match (ui.visuals().dark_mode, kind) {
        (false, SecondaryStructureKind::Helix) => {
            Color32::from_rgba_unmultiplied(230, 135, 68, 122)
        }
        (false, SecondaryStructureKind::Sheet) => {
            Color32::from_rgba_unmultiplied(70, 156, 118, 122)
        }
        (true, SecondaryStructureKind::Helix) => Color32::from_rgba_unmultiplied(202, 103, 50, 150),
        (true, SecondaryStructureKind::Sheet) => Color32::from_rgba_unmultiplied(54, 139, 105, 150),
    }
}
fn primary_residue(chains: &[SequenceChain]) -> Option<(usize, char)> {
    chains.iter().find_map(|chain| {
        chain
            .residues
            .iter()
            .find(|residue| residue.primary_selected)
            .map(|residue| (residue.residue_index, chain.id))
    })
}
fn hover_residue_tooltip(chain: &SequenceChain, hover_residue: Option<usize>) -> String {
    hover_residue
        .and_then(|residue_index| {
            chain
                .residues
                .iter()
                .find(|residue| residue.residue_index == residue_index)
        })
        .map(residue_tooltip)
        .unwrap_or_else(|| format!("Chain {}", display_chain_id(chain.id)))
}
fn residue_tooltip(residue: &SequenceResidue) -> String {
    let kind = match residue.kind {
        ResiduePolymerKind::Protein => "protein",
        ResiduePolymerKind::NucleicAcid => "nucleic acid",
    };
    let secondary = match residue.secondary {
        Some(SecondaryStructureKind::Helix) => " / helix",
        Some(SecondaryStructureKind::Sheet) => " / sheet",
        None => "",
    };
    let atoms = residue
        .first_atom
        .map(|index| format!("{} atom(s), first atom {}", residue.atom_count, index + 1))
        .unwrap_or_else(|| format!("{} atom(s)", residue.atom_count));
    format!(
        "Chain {} / Residue {} / {} / {atoms} / {kind}{secondary}",
        display_chain_id(residue.chain_id),
        residue_number_label(residue),
        residue.residue_name,
    )
}
fn residue_number_label(residue: &SequenceResidue) -> String {
    format!(
        "{}{}",
        residue.sequence_number,
        display_insertion_code(residue.insertion_code)
    )
}
fn display_chain_id(chain_id: char) -> String {
    if chain_id == ' ' {
        "(blank)".to_string()
    } else {
        chain_id.to_string()
    }
}
fn display_insertion_code(insertion_code: char) -> String {
    if insertion_code == ' ' || insertion_code == '\0' {
        String::new()
    } else {
        insertion_code.to_string()
    }
}
