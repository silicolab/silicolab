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
        let trace = state
            .jobs
            .optimizer
            .as_ref()
            .map(|running| running.energy_trace.clone())
            .unwrap_or_default();
        if trace.len() > 1 {
            let spec = crate::plot::spec::ChartSpec {
                title: String::new(),
                x: crate::plot::spec::AxisSpec::new("Step", ""),
                y: crate::plot::spec::AxisSpec::new("Energy", ""),
                series: vec![crate::plot::spec::Series {
                    name: "Energy".to_string(),
                    points: trace,
                    mark: crate::plot::spec::Mark::Line,
                }],
            };
            crate::frontend::ui::plot_view::render_chart(
                ui,
                &spec,
                "opt-live-trace",
                110.0,
                false,
                false,
            );
        }
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

    // Read before the prompt is borrowed mutably.
    let structure_is_empty = state.structure().atoms.is_empty();
    let hosts = crate::frontend::ui::remote_host_options(state);
    // Open entries (id, name) for the transition-state product picker.
    let entry_options: Vec<(u64, String)> = state
        .entries
        .records
        .iter()
        .map(|record| (record.id, record.name.clone()))
        .collect();
    // Fingerprint of the live config, to hide a memory estimate once the form or
    // structure has drifted from what it was computed for.
    let current_memory_sig = state
        .ui
        .pending_qm
        .as_ref()
        .map(|prompt| prompt.memory_signature(state.structure()));

    if let Some(prompt) = &mut state.ui.pending_qm {
        let previous_engine = prompt.engine;
        ui.horizontal(|ui| {
            ui.label("Engine:");
            egui::ComboBox::from_id_salt("qm_engine")
                .selected_text(prompt.engine.label())
                .show_ui(ui, |ui| {
                    crate::frontend::theme::stabilize_selectable_rows(ui);
                    ui.selectable_value(
                        &mut prompt.engine,
                        crate::engines::qm::QmEngine::Hartree,
                        crate::engines::qm::QmEngine::Hartree.label(),
                    );
                    ui.selectable_value(
                        &mut prompt.engine,
                        crate::engines::qm::QmEngine::Orca,
                        crate::engines::qm::QmEngine::Orca.label(),
                    );
                });
        });
        if prompt.engine != previous_engine && prompt.engine == crate::engines::qm::QmEngine::Orca {
            prompt.prefs.cores_per_subtask = 1;
        }
        if prompt.engine == crate::engines::qm::QmEngine::Orca {
            prompt.periodic = false;
            prompt.options.compute_properties = false;
            if prompt.kind == crate::engines::qm::QmKind::TransitionState {
                prompt.kind = crate::engines::qm::QmKind::SinglePoint;
            }
            ui.small("ORCA requires a program path configured for the selected compute target.");
            ui.separator();
        }
        // Offer molecular vs. periodic only when a cell is present; without one,
        // force the form back to molecular so a stale periodic selection (left
        // over from a previous entry) can't run against a non-periodic system.
        if has_real_cell && prompt.engine == crate::engines::qm::QmEngine::Hartree {
            ui.label("Calculation target:");
            ui.horizontal(|ui| {
                crate::frontend::theme::stabilize_selectable_rows(ui);
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
            render_molecular_qm_form(ui, prompt, &entry_options);
        }

        ui.separator();
        // Where and how this job runs (compute target + cores), seeded from the
        // global defaults and overridable per run. The memory estimate is
        // QM-specific, so it sits just below.
        crate::frontend::ui::execution_section(
            ui,
            &mut prompt.prefs,
            crate::frontend::state::ExecutionCaps {
                cores: true,
                memory: true,
                walltime: true,
                ..Default::default()
            },
            &hosts,
            actions,
        );
        // The memory estimate models the molecular in-core ERI tensor; a periodic
        // GPW run has none, so the button is molecular-only.
        if !prompt.periodic
            && prompt.engine == crate::engines::qm::QmEngine::Hartree
            && ui
                .add_enabled(
                    !structure_is_empty,
                    egui::Button::new(format!(
                        "{}  Estimate memory",
                        egui_phosphor::regular::MEMORY
                    )),
                )
                .on_hover_text("Predict peak RAM for the current method, basis, and backend")
                .clicked()
        {
            actions.push(AppAction::EstimateQmMemory);
        }
        // Show the estimate only while it still matches the live config; a drifted
        // one stays in state (cheap to keep) but is hidden until re-estimated.
        if let Some(estimate) = &prompt.memory_report
            && current_memory_sig == Some(estimate.signature)
        {
            render_qm_memory_report(ui, &estimate.report, &estimate.location);
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

pub(crate) fn render_docking_task_panel(
    state: &mut AppState,
    ui: &mut egui::Ui,
    actions: &mut Vec<AppAction>,
) {
    let pal = crate::frontend::theme::palette(ui);
    // The available entries (id + name), collected before the prompt is borrowed
    // mutably so the receptor/ligand pickers can list them.
    let entries: Vec<(u64, String)> = state
        .entries
        .records
        .iter()
        .map(|record| (record.id, record.name.clone()))
        .collect();
    let hosts = crate::frontend::ui::remote_host_options(state);

    if let Some(prompt) = &mut state.ui.pending_docking {
        let label_for = |selected: Option<u64>| -> String {
            selected
                .and_then(|id| entries.iter().find(|(eid, _)| *eid == id))
                .map(|(_, name)| name.clone())
                .unwrap_or_else(|| "— choose —".to_string())
        };

        egui::Grid::new("docking_inputs")
            .num_columns(2)
            .spacing([8.0, 6.0])
            .show(ui, |ui| {
                ui.label("Receptor:");
                egui::ComboBox::from_id_salt("dock_receptor")
                    .selected_text(label_for(prompt.receptor_entry))
                    .show_ui(ui, |ui| {
                        crate::frontend::theme::stabilize_selectable_rows(ui);
                        for (id, name) in &entries {
                            ui.selectable_value(
                                &mut prompt.receptor_entry,
                                Some(*id),
                                name.as_str(),
                            );
                        }
                    });
                ui.end_row();

                ui.label("Ligand:");
                egui::ComboBox::from_id_salt("dock_ligand")
                    .selected_text(label_for(prompt.ligand_entry))
                    .show_ui(ui, |ui| {
                        crate::frontend::theme::stabilize_selectable_rows(ui);
                        for (id, name) in &entries {
                            ui.selectable_value(&mut prompt.ligand_entry, Some(*id), name.as_str());
                        }
                    });
                ui.end_row();
            });

        ui.separator();
        ui.label(RichText::new("Search box (Å)").strong());
        egui::Grid::new("docking_box")
            .num_columns(4)
            .spacing([6.0, 6.0])
            .show(ui, |ui| {
                ui.label("Center:");
                for axis in 0..3 {
                    ui.add(egui::DragValue::new(&mut prompt.box_center[axis]).speed(0.25));
                }
                ui.end_row();
                ui.label("Size:");
                for axis in 0..3 {
                    ui.add(
                        egui::DragValue::new(&mut prompt.box_size[axis])
                            .speed(0.25)
                            .range(1.0..=100.0),
                    );
                }
                ui.end_row();
            });

        ui.separator();
        egui::Grid::new("docking_params")
            .num_columns(2)
            .spacing([8.0, 6.0])
            .show(ui, |ui| {
                ui.label("Exhaustiveness:");
                ui.add(egui::DragValue::new(&mut prompt.exhaustiveness).range(1..=64));
                ui.end_row();
                ui.label("Binding modes:");
                ui.add(egui::DragValue::new(&mut prompt.num_modes).range(1..=20));
                ui.end_row();
                ui.label("Seed:");
                ui.add(egui::DragValue::new(&mut prompt.seed));
                ui.end_row();
            });
        ui.checkbox(&mut prompt.score_only, "Score input pose only (no search)");

        ui.separator();
        // Where this job runs. Docking is single-threaded today, so the resource
        // knobs render disabled for now.
        crate::frontend::ui::execution_section(
            ui,
            &mut prompt.prefs,
            crate::frontend::state::ExecutionCaps {
                memory: true,
                walltime: true,
                ..Default::default()
            },
            &hosts,
            actions,
        );
        ui.horizontal(|ui| {
            if ui
                .button(format!("{}  Run", egui_phosphor::regular::PLAY))
                .clicked()
            {
                actions.push(AppAction::StartDocking);
            }
            if ui
                .button(format!("{}  Cancel", egui_phosphor::regular::X))
                .clicked()
            {
                actions.push(AppAction::CancelDockingPrompt);
            }
        });

        ui.add_space(4.0);
        ui.label(
            RichText::new(
                "Receptor and ligand are prepared heuristically from the chosen entries \
                 (approximate atom typing + torsion tree). Import already-prepared .pdbqt files \
                 for production-quality results.",
            )
            .small()
            .color(pal.text_tertiary),
        );
    } else if state.jobs.docking_running() {
        ui.label("Docking is running. Press Esc to stop.");
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
        ui.label("Docking configuration is unavailable.");
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
