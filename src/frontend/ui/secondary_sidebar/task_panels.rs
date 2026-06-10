use super::*;

pub(crate) fn render_framework_task_panel(
    state: &mut AppState,
    ui: &mut egui::Ui,
    actions: &mut Vec<AppAction>,
) {
    if let Some(panel) = &mut state.ui.reticular_builder {
        panel.ui(ui);
        ui.separator();
        ui.horizontal(|ui| {
            if ui
                .button(format!("{}  Preview", egui_phosphor::regular::EYE))
                .clicked()
            {
                actions.push(AppAction::PreviewFramework);
            }
            if ui
                .button(format!("{}  Build", egui_phosphor::regular::HAMMER))
                .clicked()
            {
                actions.push(AppAction::BuildFramework);
            }
            if ui
                .button(format!("{}  Cancel", egui_phosphor::regular::X))
                .clicked()
            {
                actions.push(AppAction::CancelFramework);
            }
        });
    } else {
        ui.label("Task panel is not active.");
    }
}

pub(crate) fn render_nanosheet_task_panel(
    state: &mut AppState,
    ui: &mut egui::Ui,
    actions: &mut Vec<AppAction>,
) {
    if let Some(panel) = &mut state.ui.nanosheet_builder {
        panel.ui(ui);
        ui.separator();
        ui.horizontal(|ui| {
            if ui
                .button(format!("{}  Preview", egui_phosphor::regular::EYE))
                .clicked()
            {
                actions.push(AppAction::PreviewNanosheet);
            }
            if ui
                .button(format!("{}  Build", egui_phosphor::regular::HAMMER))
                .clicked()
            {
                actions.push(AppAction::BuildNanosheet);
            }
            if ui
                .button(format!("{}  Cancel", egui_phosphor::regular::X))
                .clicked()
            {
                actions.push(AppAction::CancelNanosheet);
            }
        });
    } else {
        ui.label("Task panel is not active.");
    }
}

pub(crate) fn render_building_block_task_panel(
    state: &mut AppState,
    ui: &mut egui::Ui,
    actions: &mut Vec<AppAction>,
) {
    let Some(structure) = state.entries.active_entry().map(|entry| &entry.structure) else {
        ui.label("Open an entry to edit a building block.");
        return;
    };
    if let Some(editor) = &mut state.ui.block_editor {
        editor.ui(ui, structure);
        ui.separator();
        ui.horizontal(|ui| {
            if ui
                .button(format!("{}  Save", egui_phosphor::regular::FLOPPY_DISK))
                .clicked()
            {
                actions.push(AppAction::SaveBuildingBlock);
            }
            if ui
                .button(format!("{}  Cancel", egui_phosphor::regular::X))
                .clicked()
            {
                actions.push(AppAction::CancelBuildingBlock);
            }
        });
    } else {
        ui.label("Task panel is not active.");
    }
}

pub(crate) fn render_optimization_task_panel(
    state: &mut AppState,
    ui: &mut egui::Ui,
    actions: &mut Vec<AppAction>,
) {
    let has_selection = !state.ui.selection.is_empty();
    if let Some(prompt) = &mut state.ui.pending_optimization {
        ui.label("Atomic coordinates:");
        ui.radio_value(
            &mut prompt.coordinate_scope,
            CoordinateOptimizationScope::AllAtoms,
            "Optimize all atoms",
        );
        ui.add_enabled_ui(has_selection, |ui| {
            ui.radio_value(
                &mut prompt.coordinate_scope,
                CoordinateOptimizationScope::SelectedAtoms,
                format!("Optimize selected atoms ({})", state.ui.selection.len()),
            );
        });
        if !has_selection {
            ui.label("No atoms selected. Use the viewport or Selection panel to pick atoms.");
            prompt.coordinate_scope = CoordinateOptimizationScope::AllAtoms;
        }

        if prompt.allow_cell_optimization {
            ui.separator();
            ui.horizontal(|ui| {
                if ui
                    .button(format!(
                        "{}  Select all",
                        egui_phosphor::regular::CHECK_SQUARE
                    ))
                    .clicked()
                {
                    prompt.cell = crate::engines::forcefield::CellOptimizationOptions::all();
                }
                if ui
                    .button(format!("{}  Clear", egui_phosphor::regular::SQUARE))
                    .clicked()
                {
                    prompt.cell = crate::engines::forcefield::CellOptimizationOptions::default();
                }
            });
            egui::Grid::new("sidebar_cell_optimization_options")
                .num_columns(3)
                .show(ui, |ui| {
                    ui.checkbox(&mut prompt.cell.a, "a");
                    ui.checkbox(&mut prompt.cell.b, "b");
                    ui.checkbox(&mut prompt.cell.c, "c");
                    ui.end_row();
                    ui.checkbox(&mut prompt.cell.alpha, "alpha");
                    ui.checkbox(&mut prompt.cell.beta, "beta");
                    ui.checkbox(&mut prompt.cell.gamma, "gamma");
                });
        }

        ui.separator();
        ui.horizontal(|ui| {
            if ui
                .button(format!("{}  Start", egui_phosphor::regular::PLAY))
                .clicked()
            {
                actions.push(AppAction::StartOptimization);
            }
            if ui
                .button(format!("{}  Cancel", egui_phosphor::regular::X))
                .clicked()
            {
                actions.push(AppAction::CancelOptimizationPrompt);
            }
        });
    } else if state.jobs.optimization_running() {
        ui.label("Optimization is running.");
        if ui
            .button(format!("{}  Show Output", egui_phosphor::regular::TERMINAL))
            .clicked()
        {
            state.ui.layout.show_panel = true;
            state.ui.layout.active_panel_tab = PanelTab::Output;
        }
    } else {
        ui.label("Optimization configuration is unavailable.");
    }
}

pub(crate) fn render_qm_task_panel(
    state: &mut AppState,
    ui: &mut egui::Ui,
    actions: &mut Vec<AppAction>,
) {
    use crate::engines::qm::{QmKind, QmMethod};

    if let Some(prompt) = &mut state.ui.pending_qm {
        egui::Grid::new("sidebar_qm_options")
            .num_columns(2)
            .spacing([8.0, 6.0])
            .show(ui, |ui| {
                ui.label("Method:");
                egui::ComboBox::from_id_salt("qm_method")
                    .selected_text(prompt.method.label())
                    .show_ui(ui, |ui| {
                        for method in QmMethod::presets() {
                            let label = method.label();
                            ui.selectable_value(&mut prompt.method, method, label);
                        }
                    });
                ui.end_row();

                ui.label("Basis set:");
                egui::ComboBox::from_id_salt("qm_basis")
                    .selected_text(prompt.basis.clone())
                    .show_ui(ui, |ui| {
                        // chemx's bundled basis sets (H–Ar), smallest to largest.
                        for basis in [
                            "sto-3g",
                            "6-31g",
                            "6-311g(d,p)",
                            "cc-pvdz",
                            "cc-pvtz",
                            "def2-svp",
                            "def2-tzvp",
                        ] {
                            ui.selectable_value(&mut prompt.basis, basis.to_string(), basis);
                        }
                    });
                ui.end_row();

                ui.label("Charge:");
                ui.add(egui::DragValue::new(&mut prompt.charge).range(-10..=10));
                ui.end_row();

                ui.label("Spin (2S+1):");
                ui.add(egui::DragValue::new(&mut prompt.multiplicity).range(1..=11));
                ui.end_row();
            });

        ui.separator();
        ui.label("Calculation:");
        ui.radio_value(&mut prompt.kind, QmKind::SinglePoint, "Single-point energy");
        ui.radio_value(&mut prompt.kind, QmKind::Optimize, "Geometry optimization");
        ui.radio_value(
            &mut prompt.kind,
            QmKind::Frequencies,
            "Vibrational frequencies",
        );
        ui.checkbox(
            &mut prompt.compute_properties,
            "Compute dipole and atomic charges",
        );

        ui.separator();
        ui.horizontal(|ui| {
            if ui
                .button(format!("{}  Run", egui_phosphor::regular::PLAY))
                .clicked()
            {
                actions.push(AppAction::StartQmCalculation);
            }
            if ui
                .button(format!("{}  Cancel", egui_phosphor::regular::X))
                .clicked()
            {
                actions.push(AppAction::CancelQmPrompt);
            }
        });
    } else if state.jobs.qm_running() {
        ui.label("QM calculation is running. Press Esc to stop.");
        if ui
            .button(format!("{}  Show Output", egui_phosphor::regular::TERMINAL))
            .clicked()
        {
            state.ui.layout.show_panel = true;
            state.ui.layout.active_panel_tab = PanelTab::Output;
        }
    } else {
        ui.label("QM configuration is unavailable.");
    }
}

pub(crate) fn render_supercell_task_panel(
    state: &mut AppState,
    ui: &mut egui::Ui,
    actions: &mut Vec<AppAction>,
) {
    let cell_metrics = state
        .structure()
        .cell
        .as_ref()
        .map(|cell| (cell.a, cell.b, cell.c))
        .unwrap_or((0.0, 0.0, 0.0));
    let atom_count = state.structure().atoms.len();
    let bond_count = state.structure().bonds.len();
    if let Some(prompt) = &mut state.ui.pending_supercell {
        ui.label(format!(
            "Current cell: {:.2} x {:.2} x {:.2} A",
            cell_metrics.0, cell_metrics.1, cell_metrics.2,
        ));
        ui.label(format!("{atom_count} atoms, {bond_count} bonds"));
        ui.separator();
        ui.horizontal(|ui| {
            ui.label("Repeats:");
            supercell_repeat_value(ui, &mut prompt.repeats[0]);
            ui.label("x");
            supercell_repeat_value(ui, &mut prompt.repeats[1]);
            ui.label("x");
            supercell_repeat_value(ui, &mut prompt.repeats[2]);
        });

        let total_atoms =
            atom_count * (prompt.repeats[0] * prompt.repeats[1] * prompt.repeats[2]) as usize;
        ui.label(format!(
            "Result: {}x{}x{} supercell, {} atoms",
            prompt.repeats[0], prompt.repeats[1], prompt.repeats[2], total_atoms,
        ));

        ui.separator();
        ui.horizontal(|ui| {
            if ui
                .button(format!("{}  Expand", egui_phosphor::regular::ARROWS_OUT))
                .clicked()
            {
                actions.push(AppAction::ConfirmSupercell);
            }
            if ui
                .button(format!("{}  Cancel", egui_phosphor::regular::X))
                .clicked()
            {
                actions.push(AppAction::CancelSupercellPrompt);
            }
        });
    } else {
        ui.label("Supercell panel is unavailable.");
    }
}

pub(crate) fn render_protein_prep_task_panel(
    state: &mut AppState,
    ui: &mut egui::Ui,
    actions: &mut Vec<AppAction>,
) {
    let atom_count = state.structure().atoms.len();
    if let Some(prompt) = &mut state.ui.pending_protein_prep {
        ui.label(format!("{atom_count} atoms"));
        ui.label(
            "Prepare a biomolecule for simulation. The prepared structure is added as a new entry.",
        );
        ui.separator();

        ui.strong("Cleanup");
        ui.checkbox(&mut prompt.add_hydrogens, "Add missing hydrogens");

        ui.separator();
        ui.label(
            egui::RichText::new(
                "Coming soon: protonation states, terminus patching, and missing-atom repair.",
            )
            .small()
            .color(egui::Color32::GRAY),
        );

        ui.separator();
        ui.horizontal(|ui| {
            if ui
                .button(format!("{}  Prepare", egui_phosphor::regular::SPARKLE))
                .clicked()
            {
                actions.push(AppAction::ConfirmProteinPrep);
            }
            if ui
                .button(format!("{}  Cancel", egui_phosphor::regular::X))
                .clicked()
            {
                actions.push(AppAction::CancelProteinPrepPrompt);
            }
        });
    } else {
        ui.label("Protein preparation panel is unavailable.");
    }
}
