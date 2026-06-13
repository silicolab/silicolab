//! The sketcher window shell: palettes on top, the canvas in the middle, and a
//! footer with import/export, the title field, and the Build/Cancel actions.
//!
//! This is pure rendering in the architecture's sense: the live canvas mutates
//! [`SketcherState`] in place each frame, and only Build/Cancel cross the
//! `AppAction → dispatch` boundary ([`AppAction::CommitSketch`] /
//! [`AppAction::CancelSketch`]).

use eframe::egui::{self, Context, RichText, TextEdit, Vec2};

use super::{canvas, palette};
use crate::frontend::{actions::AppAction, state::AppState};

/// Footer height reserved below the canvas.
const FOOTER_HEIGHT: f32 = 96.0;

pub fn render_sketcher_window(state: &mut AppState, actions: &mut Vec<AppAction>, ctx: &Context) {
    if state.ui.sketcher.is_none() {
        return;
    }

    let mut commit = false;
    let mut cancel = false;

    egui::Window::new("Sketch Molecule")
        .default_size([920.0, 640.0])
        .min_width(660.0)
        .resizable(true)
        .collapsible(false)
        .show(ctx, |ui| {
            let Some(sketcher) = &mut state.ui.sketcher else {
                return;
            };
            palette::tools_row(sketcher, ui);
            palette::element_row(sketcher, ui);
            palette::bond_row(sketcher, ui);
            palette::edit_row(sketcher, ui);
            ui.separator();

            let available = ui.available_size();
            let canvas_height = (available.y - FOOTER_HEIGHT).max(220.0);
            ui.allocate_ui(Vec2::new(available.x, canvas_height), |ui| {
                canvas::show_canvas(sketcher, ui);
            });

            ui.separator();
            footer(sketcher, ui, &mut commit, &mut cancel);
        });

    if let Some(sketcher) = &mut state.ui.sketcher {
        palette::periodic_table_window(sketcher, ctx);
    }

    if commit {
        actions.push(AppAction::CommitSketch);
    } else if cancel {
        actions.push(AppAction::CancelSketch);
    }
}

fn footer(
    sketcher: &mut super::SketcherState,
    ui: &mut egui::Ui,
    commit: &mut bool,
    cancel: &mut bool,
) {
    // SMILES import / export.
    ui.horizontal(|ui| {
        ui.label(RichText::new("SMILES").weak());
        ui.add(
            TextEdit::singleline(&mut sketcher.smiles_input)
                .hint_text("paste or type a SMILES string")
                .desired_width(320.0),
        );
        if ui.button("Import").clicked() {
            let text = sketcher.smiles_input.trim().to_string();
            if !text.is_empty() {
                sketcher.import_smiles(&text);
            }
        }
        if ui
            .add_enabled(
                !sketcher.sketch.is_empty(),
                egui::Button::new("Copy as SMILES"),
            )
            .clicked()
        {
            let smiles = sketcher.export_smiles();
            ui.ctx().copy_text(smiles.clone());
            sketcher.smiles_input = smiles;
            sketcher.status = "Copied SMILES to clipboard".to_string();
        }
    });

    // Status + title + build/cancel.
    ui.horizontal(|ui| {
        let atoms = sketcher.sketch.atoms.len();
        let bonds = sketcher.sketch.bonds.len();
        let status = if sketcher.status.is_empty() {
            format!("{atoms} atoms · {bonds} bonds")
        } else {
            sketcher.status.clone()
        };
        ui.label(RichText::new(status).weak());

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .button(format!("{}  Cancel", egui_phosphor::regular::X))
                .clicked()
            {
                *cancel = true;
            }
            let can_build = !sketcher.sketch.is_empty();
            if ui
                .add_enabled(
                    can_build,
                    egui::Button::new(format!(
                        "{}  Build (Save as New)",
                        egui_phosphor::regular::CUBE
                    )),
                )
                .on_hover_text("Add hydrogens, generate 3D coordinates, and add a new entry")
                .clicked()
            {
                *commit = true;
            }
            ui.add(
                TextEdit::singleline(&mut sketcher.title)
                    .hint_text("title")
                    .desired_width(160.0),
            );
            ui.label("Title:");
        });
    });
}
