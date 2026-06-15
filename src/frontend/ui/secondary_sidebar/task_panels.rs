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
            state
                .ui
                .layout
                .dock
                .reveal_static(crate::frontend::state::StaticView::Output);
            let now = ui.input(|input| input.time);
            state.mark_layout_dirty(now);
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
    // A periodic run needs a real unit cell; the placeholder 1×1×1 cell some
    // tools write for a molecule does not count. Read this before borrowing the
    // prompt mutably below.
    let has_real_cell = state
        .structure()
        .cell
        .as_ref()
        .is_some_and(|cell| !cell.is_placeholder());

    if let Some(prompt) = &mut state.ui.pending_qm {
        // Offer molecular vs. periodic only when a cell is present; without one,
        // force the form back to molecular so a stale periodic selection (left
        // over from a previous entry) can't run against a non-periodic system.
        if has_real_cell {
            ui.label("Calculation target:");
            ui.horizontal(|ui| {
                ui.selectable_value(&mut prompt.periodic, false, "Molecular");
                ui.selectable_value(&mut prompt.periodic, true, "Periodic (crystal)");
            });
            ui.separator();
        } else {
            prompt.periodic = false;
        }

        if prompt.periodic {
            render_periodic_qm_form(ui, &mut prompt.periodic_form);
        } else {
            render_molecular_qm_form(ui, prompt);
        }

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
            state
                .ui
                .layout
                .dock
                .reveal_static(crate::frontend::state::StaticView::Output);
            let now = ui.input(|input| input.time);
            state.mark_layout_dirty(now);
        }
    } else {
        ui.label("QM configuration is unavailable.");
    }
}

/// The molecular QM form: method, basis, charge/spin, the calculation kind, the
/// properties toggle, and the advanced chemx options.
fn render_molecular_qm_form(ui: &mut egui::Ui, prompt: &mut crate::frontend::state::QmPrompt) {
    use crate::engines::qm::{QmKind, QmMethod};

    let presets = QmMethod::presets();
    let method_is_composite = matches!(prompt.method, QmMethod::Composite(_));

    egui::Grid::new("sidebar_qm_options")
        .num_columns(2)
        .spacing([8.0, 6.0])
        .show(ui, |ui| {
            ui.label("Method:");
            let is_custom = !presets.iter().any(|m| m == &prompt.method);
            egui::ComboBox::from_id_salt("qm_method")
                .selected_text(if is_custom {
                    format!("Custom: {}", prompt.method.label())
                } else {
                    prompt.method.label()
                })
                .show_ui(ui, |ui| {
                    for method in &presets {
                        let label = method.label();
                        ui.selectable_value(&mut prompt.method, method.clone(), label);
                    }
                    if ui
                        .selectable_label(is_custom, "Custom functional…")
                        .clicked()
                    {
                        let name = if prompt.custom_functional.trim().is_empty() {
                            "pbe".to_string()
                        } else {
                            prompt.custom_functional.clone()
                        };
                        prompt.custom_functional = name.clone();
                        prompt.method = QmMethod::Dft(name);
                    }
                });
            ui.end_row();

            // Free-text functional name, shown when the method is a DFT
            // functional outside the preset list.
            if !presets.iter().any(|m| m == &prompt.method)
                && matches!(prompt.method, QmMethod::Dft(_))
            {
                ui.label("Functional:");
                if ui
                    .text_edit_singleline(&mut prompt.custom_functional)
                    .changed()
                {
                    prompt.method = QmMethod::Dft(prompt.custom_functional.clone());
                }
                ui.end_row();
            }

            ui.label("Basis set:");
            // A composite carries its own implied basis.
            if method_is_composite {
                ui.label("(implied by composite)");
            } else {
                egui::ComboBox::from_id_salt("qm_basis")
                    .selected_text(prompt.basis.clone())
                    .show_ui(ui, |ui| {
                        // chemx's bundled basis sets, smallest to largest.
                        for basis in [
                            "sto-3g",
                            "6-31g",
                            "6-311g(d,p)",
                            "cc-pvdz",
                            "cc-pvtz",
                            "cc-pvqz",
                            "def2-svp",
                            "def2-tzvp",
                            "def2-tzvpp",
                            "def2-qzvp",
                        ] {
                            ui.selectable_value(&mut prompt.basis, basis.to_string(), basis);
                        }
                    });
            }
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

    // The three calculation kinds above are mutually exclusive (radios). The
    // property toggle below is an independent on/off, but we draw it with the
    // same filled-circle indicator so the panel speaks one visual language
    // ("selected = dot"). It is set apart under its own "Options:" header so
    // the shared dot style is not misread as "mutually exclusive with the
    // calculation above". `ui.radio` is purely visual here — the click
    // handler flips the bool; it does not join the calculation-kind group.
    ui.separator();
    ui.label("Options:");
    if ui
        .radio(
            prompt.options.compute_properties,
            "Compute dipole, charges & bond orders",
        )
        .clicked()
    {
        prompt.options.compute_properties = !prompt.options.compute_properties;
    }

    render_qm_advanced(ui, prompt, method_is_composite);
}

/// The periodic (crystalline) QM form: the LDA functional, GTH basis, k-point
/// mesh, grid cutoff, SCF iteration cap, and the optional force/stress outputs.
/// Shown only when the active structure carries a real unit cell.
fn render_periodic_qm_form(ui: &mut egui::Ui, form: &mut crate::frontend::state::PeriodicQmForm) {
    use crate::engines::qm::{PeriodicFunctional, periodic};

    egui::Grid::new("sidebar_periodic_qm_options")
        .num_columns(2)
        .spacing([8.0, 6.0])
        .show(ui, |ui| {
            ui.label("Functional:");
            egui::ComboBox::from_id_salt("periodic_functional")
                .selected_text(form.functional.label())
                .show_ui(ui, |ui| {
                    for functional in PeriodicFunctional::all() {
                        ui.selectable_value(&mut form.functional, functional, functional.label());
                    }
                });
            ui.end_row();

            ui.label("Basis (GTH):");
            egui::ComboBox::from_id_salt("periodic_basis")
                .selected_text(form.basis.clone())
                .show_ui(ui, |ui| {
                    for basis in periodic::PERIODIC_BASES {
                        ui.selectable_value(&mut form.basis, basis.to_string(), *basis);
                    }
                });
            ui.end_row();

            ui.label("k-point mesh:");
            ui.horizontal(|ui| {
                for division in &mut form.kmesh {
                    ui.add(egui::DragValue::new(division).range(1..=12));
                }
            });
            ui.end_row();

            ui.label("Grid cutoff (Ry):");
            // Floor at 100 Ry: below that a GPW real-space grid is too coarse to
            // give a meaningful energy (chemx's own default is 280 Ry).
            ui.add(
                egui::DragValue::new(&mut form.e_cut_ry)
                    .range(100.0..=1200.0)
                    .speed(10.0),
            );
            ui.end_row();

            ui.label("Max SCF iters:");
            ui.add(egui::DragValue::new(&mut form.max_iter).range(10..=500));
            ui.end_row();
        });

    ui.separator();
    ui.label("Outputs:");
    ui.checkbox(&mut form.forces, "Forces on atoms");
    ui.checkbox(&mut form.stress, "Cell stress tensor");

    ui.add_space(4.0);
    ui.small(
        "Periodic GPW uses GTH pseudopotentials (closed-shell LDA); net charge \
         and spin are not modeled.",
    );
}

/// Common implicit-solvation solvent names offered in the panel dropdown. chemx
/// accepts more (especially for SMD); the console `qm` command takes any name.
const QM_SOLVENTS: &[&str] = &[
    "water",
    "acetonitrile",
    "methanol",
    "ethanol",
    "dmso",
    "acetone",
    "thf",
    "dmf",
    "toluene",
    "benzene",
    "chloroform",
];

/// The "Advanced (chemx 0.4)" collapsing section of the QM panel: dispersion,
/// solvation, SCF backend, relativity, smearing, FOD, and thermochemistry knobs.
/// Options that do not apply to the chosen method are hidden rather than shown
/// disabled.
fn render_qm_advanced(
    ui: &mut egui::Ui,
    prompt: &mut crate::frontend::state::QmPrompt,
    method_is_composite: bool,
) {
    use crate::engines::qm::{CpcmDielectric, QmDispersion, QmMethod, QmScfBackend, QmSolvation};

    let method_is_post_hf = matches!(
        prompt.method,
        QmMethod::Mp2 | QmMethod::Ccsd | QmMethod::CcsdT
    );
    let method_is_mp2 = matches!(prompt.method, QmMethod::Mp2);

    egui::CollapsingHeader::new("Advanced (chemx 0.4)")
        .default_open(false)
        .show(ui, |ui| {
            // Dispersion — composites carry their own; post-HF has none.
            if !method_is_composite && !method_is_post_hf {
                ui.horizontal(|ui| {
                    ui.label("Dispersion:");
                    egui::ComboBox::from_id_salt("qm_disp")
                        .selected_text(match prompt.options.dispersion {
                            None => "none",
                            Some(QmDispersion::D3Bj) => "D3(BJ)",
                            Some(QmDispersion::D4) => "D4",
                        })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut prompt.options.dispersion, None, "none");
                            ui.selectable_value(
                                &mut prompt.options.dispersion,
                                Some(QmDispersion::D3Bj),
                                "D3(BJ)",
                            );
                            ui.selectable_value(
                                &mut prompt.options.dispersion,
                                Some(QmDispersion::D4),
                                "D4",
                            );
                        });
                });
            }

            // Solvation: model + solvent name, reassembled each frame.
            let (mut model, mut name) = match &prompt.options.solvation {
                None => (0u8, "water".to_string()),
                Some(QmSolvation::Cpcm(CpcmDielectric::Named(n))) => (1, n.clone()),
                Some(QmSolvation::Cpcm(CpcmDielectric::Epsilon(_))) => (1, "water".to_string()),
                Some(QmSolvation::Smd(n)) => (2, n.clone()),
                Some(QmSolvation::Alpb(n)) => (3, n.clone()),
                Some(QmSolvation::Gbsa(n)) => (4, n.clone()),
            };
            ui.horizontal(|ui| {
                ui.label("Solvation:");
                egui::ComboBox::from_id_salt("qm_solv_model")
                    .selected_text(match model {
                        1 => "C-PCM",
                        2 => "SMD",
                        3 => "ALPB",
                        4 => "GBSA",
                        _ => "none",
                    })
                    .show_ui(ui, |ui| {
                        for (value, label) in [
                            (0, "none"),
                            (1, "C-PCM"),
                            (2, "SMD"),
                            (3, "ALPB"),
                            (4, "GBSA"),
                        ] {
                            ui.selectable_value(&mut model, value as u8, label);
                        }
                    });
                if model != 0 {
                    egui::ComboBox::from_id_salt("qm_solv_name")
                        .selected_text(name.clone())
                        .show_ui(ui, |ui| {
                            for s in QM_SOLVENTS {
                                ui.selectable_value(&mut name, s.to_string(), *s);
                            }
                        });
                }
            });
            prompt.options.solvation = match model {
                1 => Some(QmSolvation::Cpcm(CpcmDielectric::Named(name))),
                2 => Some(QmSolvation::Smd(name)),
                3 => Some(QmSolvation::Alpb(name)),
                4 => Some(QmSolvation::Gbsa(name)),
                _ => None,
            };

            // SCF backend.
            ui.horizontal(|ui| {
                ui.label("SCF backend:");
                egui::ComboBox::from_id_salt("qm_backend")
                    .selected_text(prompt.options.scf_backend.label())
                    .show_ui(ui, |ui| {
                        for backend in [
                            QmScfBackend::InCore,
                            QmScfBackend::Direct,
                            QmScfBackend::RiJk,
                            QmScfBackend::Cosx,
                        ] {
                            ui.selectable_value(
                                &mut prompt.options.scf_backend,
                                backend,
                                backend.label(),
                            );
                        }
                    });
            });

            // DFT grid level override.
            let mut grid_override = prompt.options.grid_level.is_some();
            ui.horizontal(|ui| {
                if ui
                    .checkbox(&mut grid_override, "Override DFT grid")
                    .changed()
                {
                    prompt.options.grid_level = grid_override.then_some(3);
                }
                if let Some(level) = &mut prompt.options.grid_level {
                    ui.add(egui::DragValue::new(level).range(0..=4));
                }
            });

            // Fermi smearing.
            let mut smear_on = prompt.options.smearing_temperature_k.is_some();
            ui.horizontal(|ui| {
                if ui.checkbox(&mut smear_on, "Fermi smearing (K)").changed() {
                    prompt.options.smearing_temperature_k = smear_on.then_some(1000.0);
                }
                if let Some(temp) = &mut prompt.options.smearing_temperature_k {
                    ui.add(egui::DragValue::new(temp).range(1.0..=50_000.0).speed(50.0));
                }
            });

            // Method-specific toggles.
            if method_is_mp2 {
                ui.checkbox(&mut prompt.options.ri_mp2, "RI-MP2 (density-fit MP2)");
            }
            if method_is_post_hf {
                ui.checkbox(
                    &mut prompt.options.all_electron,
                    "All-electron (no frozen core)",
                );
            }
            ui.checkbox(&mut prompt.options.x2c, "X2C scalar relativity");
            ui.checkbox(&mut prompt.options.fod, "FOD multireference diagnostic");
            ui.checkbox(
                &mut prompt.options.single_point_hessian,
                "Single-point Hessian (approx. frequencies)",
            );

            // Thermochemistry parameters (used by frequency runs).
            egui::Grid::new("qm_thermo")
                .num_columns(2)
                .spacing([8.0, 6.0])
                .show(ui, |ui| {
                    ui.label("Symmetry number σ:");
                    ui.add(egui::DragValue::new(&mut prompt.options.symmetry_number).range(1..=48));
                    ui.end_row();
                    ui.label("quasi-RRHO ω₀ (cm⁻¹):");
                    ui.add(
                        egui::DragValue::new(&mut prompt.options.qrrho_w0_cm1)
                            .range(1.0..=1000.0)
                            .speed(1.0),
                    );
                    ui.end_row();
                });
        });
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
