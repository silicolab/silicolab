//! Tool, element, bond, and fragment palettes plus the edit/view controls.
//!
//! These are pure widgets: they read and mutate the transient [`SketcherState`]
//! directly (tool selection, active element, undo/redo, clean-up, …). Nothing
//! here emits an [`crate::frontend::actions::AppAction`]; only Build/Cancel in
//! the footer do.

use eframe::egui::{Button, Grid, Key, Label, RichText, Ui, vec2};

use super::{COMMON_ELEMENTS, SketchTool, SketcherState};
use crate::domain::{BondType, sketch::RingTemplate};

/// Periodic-table layout: 18 columns per row, `""` for empty cells, with the
/// lanthanide/actinide series on the last two rows.
const PERIODIC_TABLE: &[[&str; 18]] = &[
    [
        "H", "", "", "", "", "", "", "", "", "", "", "", "", "", "", "", "", "He",
    ],
    [
        "Li", "Be", "", "", "", "", "", "", "", "", "", "", "B", "C", "N", "O", "F", "Ne",
    ],
    [
        "Na", "Mg", "", "", "", "", "", "", "", "", "", "", "Al", "Si", "P", "S", "Cl", "Ar",
    ],
    [
        "K", "Ca", "Sc", "Ti", "V", "Cr", "Mn", "Fe", "Co", "Ni", "Cu", "Zn", "Ga", "Ge", "As",
        "Se", "Br", "Kr",
    ],
    [
        "Rb", "Sr", "Y", "Zr", "Nb", "Mo", "Tc", "Ru", "Rh", "Pd", "Ag", "Cd", "In", "Sn", "Sb",
        "Te", "I", "Xe",
    ],
    [
        "Cs", "Ba", "La", "Hf", "Ta", "W", "Re", "Os", "Ir", "Pt", "Au", "Hg", "Tl", "Pb", "Bi",
        "Po", "At", "Rn",
    ],
    [
        "Fr", "Ra", "Ac", "Rf", "Db", "Sg", "Bh", "Hs", "Mt", "Ds", "Rg", "Cn", "Nh", "Fl", "Mc",
        "Lv", "Ts", "Og",
    ],
    [
        "", "", "Ce", "Pr", "Nd", "Pm", "Sm", "Eu", "Gd", "Tb", "Dy", "Ho", "Er", "Tm", "Yb", "Lu",
        "", "",
    ],
    [
        "", "", "Th", "Pa", "U", "Np", "Pu", "Am", "Cm", "Bk", "Cf", "Es", "Fm", "Md", "No", "Lr",
        "", "",
    ],
];

pub(super) fn tools_row(state: &mut SketcherState, ui: &mut Ui) {
    let empty = state.sketch.is_empty();
    ui.horizontal_wrapped(|ui| {
        tool_button(state, ui, SketchTool::Draw, "Draw", true);
        tool_button(state, ui, SketchTool::Bond, "Bond", true);
        tool_button(state, ui, SketchTool::Chain, "Chain", true);
        tool_button(state, ui, SketchTool::Ring, "Ring", true);
        tool_button(state, ui, SketchTool::Charge, "Charge ±", true);
        ui.separator();
        tool_button(state, ui, SketchTool::Select, "Select", !empty);
        tool_button(state, ui, SketchTool::Move, "Move/Rotate", !empty);
        tool_button(state, ui, SketchTool::Erase, "Erase", !empty);
    });
}

fn tool_button(
    state: &mut SketcherState,
    ui: &mut Ui,
    tool: SketchTool,
    label: &str,
    enabled: bool,
) {
    let selected = state.tool == tool;
    let response = ui.add_enabled(enabled, Button::new(label).selected(selected));
    if response.clicked() {
        state.tool = tool;
    }
}

pub(super) fn element_row(state: &mut SketcherState, ui: &mut Ui) {
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new("Element").weak());
        for symbol in COMMON_ELEMENTS {
            element_button(state, ui, symbol);
        }
        // Recently-used exotic elements.
        let recents = state.recent_elements.clone();
        for symbol in recents {
            element_button(state, ui, &symbol);
        }
        if ui.button("Periodic table…").clicked() {
            state.show_periodic_table = !state.show_periodic_table;
        }
        // Type-to-set entry.
        let response = ui.add(
            eframe::egui::TextEdit::singleline(&mut state.element_query)
                .hint_text("type symbol")
                .desired_width(70.0),
        );
        if response.lost_focus() && ui.input(|input| input.key_pressed(Key::Enter)) {
            let query = state.element_query.trim().to_string();
            if !query.is_empty() {
                state.set_element(&query);
            }
            state.element_query.clear();
        }
    });
}

fn element_button(state: &mut SketcherState, ui: &mut Ui, symbol: &str) {
    let selected = state.tool == SketchTool::Draw && state.active_element == symbol;
    let response = ui.add_sized([26.0, 22.0], Button::new(symbol).selected(selected));
    if response.clicked() {
        state.set_element(symbol);
    }
}

pub(super) fn bond_row(state: &mut SketcherState, ui: &mut Ui) {
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new("Bond").weak());
        bond_button(state, ui, BondType::Single, "Single");
        bond_button(state, ui, BondType::Double, "Double");
        bond_button(state, ui, BondType::Triple, "Triple");
        bond_button(state, ui, BondType::Aromatic, "Aromatic");
        ui.separator();
        ui.label(RichText::new("Rings").weak());
        for template in RingTemplate::all() {
            ring_button(state, ui, *template);
        }
    });
}

fn bond_button(state: &mut SketcherState, ui: &mut Ui, order: BondType, label: &str) {
    let selected = state.tool == SketchTool::Bond && state.active_bond == order;
    if ui.add(Button::new(label).selected(selected)).clicked() {
        state.tool = SketchTool::Bond;
        state.active_bond = order;
    }
}

fn ring_button(state: &mut SketcherState, ui: &mut Ui, template: RingTemplate) {
    let selected = state.tool == SketchTool::Ring && state.active_ring == template;
    let response = ui
        .add(Button::new(template.short_label()).selected(selected))
        .on_hover_text(template.label());
    if response.clicked() {
        state.tool = SketchTool::Ring;
        state.active_ring = template;
    }
}

pub(super) fn edit_row(state: &mut SketcherState, ui: &mut Ui) {
    ui.horizontal_wrapped(|ui| {
        if ui
            .add_enabled(state.can_undo(), Button::new("Undo"))
            .clicked()
        {
            state.undo();
        }
        if ui
            .add_enabled(state.can_redo(), Button::new("Redo"))
            .clicked()
        {
            state.redo();
        }
        ui.separator();
        if ui
            .button("Clean up")
            .on_hover_text("Tidy the 2D layout")
            .clicked()
        {
            state.clean_up();
        }
        if ui.button("Fit").clicked() {
            state.request_fit();
        }
        if ui.button("Flip H").clicked() {
            state.flip_horizontal();
        }
        if ui.button("Flip V").clicked() {
            state.flip_vertical();
        }
        ui.separator();
        ui.label(RichText::new("Charge").weak());
        if ui
            .button(egui_phosphor::regular::PLUS)
            .on_hover_text("Increase formal charge (+1)")
            .clicked()
        {
            state.step_charge(1);
        }
        if ui
            .button(egui_phosphor::regular::MINUS)
            .on_hover_text("Decrease formal charge (−1)")
            .clicked()
        {
            state.step_charge(-1);
        }
        ui.separator();
        let has_selection = state.has_selection();
        if ui.button("Select all").clicked() {
            state.select_all();
        }
        if ui
            .add_enabled(has_selection, Button::new("Invert"))
            .clicked()
        {
            state.invert_selection();
        }
        if ui
            .add_enabled(has_selection, Button::new("Clear"))
            .clicked()
        {
            state.clear_selection();
        }
        if ui
            .add_enabled(has_selection, Button::new("Delete"))
            .clicked()
        {
            state.delete_selection();
        }
        ui.separator();
        ui.checkbox(&mut state.heteroatom_color, "Heteroatom color");
    });
}

/// The periodic-table popup, shown while [`SketcherState::show_periodic_table`].
pub(super) fn periodic_table_window(state: &mut SketcherState, ctx: &eframe::egui::Context) {
    if !state.show_periodic_table {
        return;
    }
    let mut open = true;
    let mut chosen: Option<String> = None;
    eframe::egui::Window::new("Periodic table")
        .open(&mut open)
        .resizable(false)
        .collapsible(false)
        .show(ctx, |ui| {
            // Uniform square cells laid out on a true grid so every period and
            // group lines up in straight columns (an 18-wide periodic chart).
            const CELL: f32 = 30.0;
            ui.spacing_mut().button_padding = vec2(2.0, 2.0);
            Grid::new("periodic_table_grid")
                .spacing(vec2(2.0, 2.0))
                .min_col_width(CELL)
                .max_col_width(CELL)
                .show(ui, |ui| {
                    for row in PERIODIC_TABLE {
                        for symbol in row {
                            if symbol.is_empty() {
                                // An empty placeholder keeps the column aligned.
                                ui.add_sized([CELL, CELL], Label::new(""));
                                continue;
                            }
                            let selected = state.active_element == *symbol;
                            if ui
                                .add_sized([CELL, CELL], Button::new(*symbol).selected(selected))
                                .clicked()
                            {
                                chosen = Some((*symbol).to_string());
                            }
                        }
                        ui.end_row();
                    }
                });
        });
    if let Some(symbol) = chosen {
        state.set_element(&symbol);
        state.show_periodic_table = false;
    }
    if !open {
        state.show_periodic_table = false;
    }
}
