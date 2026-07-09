use eframe::egui::{self, Align, Color32, FontFamily, FontId, RichText, ScrollArea, Stroke, Vec2};

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
        actions::AppAction,
        state::{AppState, SequenceViewerState},
    },
};

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
    any_selected: bool,
    all_selected: bool,
    primary_selected: bool,
}

#[derive(Debug, Clone)]
struct SequenceChain {
    id: char,
    residues: Vec<SequenceResidue>,
}

pub(crate) fn render_sequence_panel(
    state: &mut AppState,
    ui: &mut egui::Ui,
    actions: &mut Vec<AppAction>,
) {
    let pal = crate::frontend::theme::palette(ui);
    ui.set_width(ui.available_width());

    ui.horizontal(|ui| {
        ui.label(RichText::new("Sequence").strong());
        ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
            let selected = state.ui.selection.len();
            if selected > 0 {
                ui.label(
                    RichText::new(format!("{selected} atom(s) selected"))
                        .small()
                        .color(pal.text_tertiary),
                );
            }
        });
    });
    ui.separator();

    let selection = state.ui.selection.clone();
    let chains = sequence_chains(state.structure(), &selection);
    if chains.is_empty() {
        state.ui.sequence.last_clicked_residue = None;
        state.ui.sequence.last_scrolled_primary_atom = None;
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
    ScrollArea::both()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            for chain in chains
                .iter()
                .filter(|chain| chain_filter.is_none_or(|id| chain.id == id))
            {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = Vec2::new(3.0, 4.0);
                    ui.label(
                        RichText::new(format!("Chain {}", display_chain_id(chain.id)))
                            .small()
                            .strong()
                            .color(pal.text_primary),
                    );
                    ui.add_space(4.0);
                    for residue in &chain.residues {
                        scrolled_to_primary |=
                            render_residue_cell(ui, residue, chain, viewer, actions, scroll_target);
                    }
                });
                ui.add_space(8.0);
            }
        });

    if scrolled_to_primary {
        viewer.last_scrolled_primary_atom = primary_atom;
    }
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
                        .map(display_chain_id)
                        .unwrap_or_else(|| "All chains".to_string()),
                )
                .width(120.0)
                .show_ui(ui, |ui| {
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

fn render_residue_cell(
    ui: &mut egui::Ui,
    residue: &SequenceResidue,
    chain: &SequenceChain,
    viewer: &mut SequenceViewerState,
    actions: &mut Vec<AppAction>,
    scroll_target: Option<usize>,
) -> bool {
    const CELL: Vec2 = Vec2::new(24.0, 24.0);
    let pal = crate::frontend::theme::palette(ui);
    let (rect, response) = ui.allocate_exact_size(CELL, egui::Sense::click());
    let hovered = response.hovered();

    let fill = if residue.primary_selected {
        pal.accent
    } else if residue.all_selected {
        pal.selection_fill
    } else if residue.any_selected {
        pal.selection_fill.gamma_multiply(0.55)
    } else if hovered {
        pal.neutral_overlay(18)
    } else if let Some(kind) = residue.secondary {
        secondary_structure_fill(ui, kind)
    } else {
        Color32::TRANSPARENT
    };
    if fill != Color32::TRANSPARENT {
        ui.painter().rect_filled(
            rect,
            egui::CornerRadius::same(crate::frontend::theme::radius::CHIP),
            fill,
        );
    }

    let stroke = if residue.any_selected || residue.primary_selected || hovered {
        Stroke::new(1.0_f32, pal.accent)
    } else {
        Stroke::new(1.0_f32, pal.neutral_overlay(24))
    };
    ui.painter().rect_stroke(
        rect,
        egui::CornerRadius::same(crate::frontend::theme::radius::CHIP),
        stroke,
        egui::StrokeKind::Inside,
    );

    let text_color = if residue.primary_selected {
        Color32::WHITE
    } else {
        pal.text_primary
    };
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        residue.symbol,
        FontId::new(13.0, FontFamily::Monospace),
        text_color,
    );

    let response = response.on_hover_text(residue_tooltip(residue));
    let mut scrolled = false;
    if scroll_target == Some(residue.residue_index) {
        response.scroll_to_me(Some(Align::Center));
        scrolled = true;
    }

    if response.clicked() {
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
                chain_id: residue.chain_id,
                start: anchor,
                end: residue.residue_index,
                toggle,
            });
        } else {
            actions.push(AppAction::SelectResidue {
                residue_index: residue.residue_index,
                toggle,
            });
        }
        viewer.last_clicked_residue = Some(residue.residue_index);
        viewer.last_scrolled_primary_atom = None;
    }

    scrolled
}

fn secondary_structure_fill(ui: &egui::Ui, kind: SecondaryStructureKind) -> Color32 {
    match (ui.visuals().dark_mode, kind) {
        (false, SecondaryStructureKind::Helix) => Color32::from_rgba_unmultiplied(230, 135, 68, 64),
        (false, SecondaryStructureKind::Sheet) => Color32::from_rgba_unmultiplied(70, 156, 118, 64),
        (true, SecondaryStructureKind::Helix) => Color32::from_rgba_unmultiplied(202, 103, 50, 88),
        (true, SecondaryStructureKind::Sheet) => Color32::from_rgba_unmultiplied(54, 139, 105, 88),
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
    format!(
        "Chain {} / Residue {}{} / {} / {} atom(s) / {kind}{secondary}",
        display_chain_id(residue.chain_id),
        residue.sequence_number,
        display_insertion_code(residue.insertion_code),
        residue.residue_name,
        residue.atom_count,
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
