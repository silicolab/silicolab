use super::*;

use crate::frontend::ui::{execution_section, remote_host_options};

pub(crate) fn render_md_run_task_panel(
    state: &mut AppState,
    ui: &mut egui::Ui,
    actions: &mut Vec<AppAction>,
) {
    // Computed before the prompt borrow so the picker can list configured hosts.
    let hosts = remote_host_options(state);
    let pal = crate::frontend::theme::palette(ui);
    if let Some(prompt) = &mut state.ui.pending_md_run {
        use crate::frontend::state::MdEngineChoice;

        run_name_field(ui, &mut prompt.run_name);
        ui.separator();

        ui.horizontal(|ui| {
            ui.label("MD engine:");
            egui::ComboBox::from_id_salt("md_run_engine")
                .selected_text(prompt.engine.label())
                .show_ui(ui, |ui| {
                    crate::frontend::theme::stabilize_selectable_rows(ui);
                    for engine in MdEngineChoice::all() {
                        ui.selectable_value(&mut prompt.engine, *engine, engine.label());
                    }
                });
        });

        ui.separator();

        // Run the recommendation once (pure; reads only the effective context).
        let recommendation = prompt
            .effective()
            .map(|eff| crate::workflows::molecular_dynamics::run::recommend(&eff));

        // --- Inherited system + system-type overrides ----------------------
        if let Some(context) = prompt.context.clone() {
            ui.label(RichText::new("Inherited system").strong());
            ui.label(
                RichText::new(format!(
                    "Force field: {} ({}){}",
                    context.force_field_token,
                    context.force_field_family.label(),
                    context
                        .water_token
                        .as_deref()
                        .map(|water| format!(" · water {water}"))
                        .unwrap_or_else(|| " · dry (no solvent recorded)".to_string()),
                ))
                .small()
                .color(egui::Color32::GRAY),
            );

            // Override toggles edit the separate per-run overrides via actions and
            // NEVER write back into the persisted detection context; each shows
            // whether the value is auto-detected or user-set.
            if let Some(eff) = prompt.effective() {
                use crate::frontend::state::MdSystemAxis;
                use crate::workflows::molecular_dynamics::ValueSource;
                let axes = [
                    (MdSystemAxis::Membrane, "Membrane", eff.membrane()),
                    (MdSystemAxis::Ligand, "Ligand", eff.ligand()),
                    (MdSystemAxis::Nucleic, "Nucleic acid", eff.nucleic()),
                ];
                for (axis, label, (value, source)) in axes {
                    ui.horizontal(|ui| {
                        let mut checked = value;
                        if ui.checkbox(&mut checked, label).changed() {
                            actions.push(AppAction::SetMdRunOverride(axis, Some(checked)));
                        }
                        match source {
                            ValueSource::Detected => {
                                ui.label(
                                    RichText::new("auto-detected")
                                        .small()
                                        .color(egui::Color32::GRAY),
                                );
                            }
                            ValueSource::Overridden => {
                                ui.label(
                                    RichText::new("you set")
                                        .small()
                                        .color(egui::Color32::LIGHT_BLUE),
                                );
                                if ui.small_button("auto").clicked() {
                                    actions.push(AppAction::SetMdRunOverride(axis, None));
                                }
                            }
                        }
                    });
                }
            }

            if let Some(rec) = &recommendation {
                for note in &rec.notes {
                    ui.label(
                        RichText::new(format!("• {} → {}", note.reason, note.intent))
                            .small()
                            .color(egui::Color32::GRAY),
                    );
                }
                for warning in &rec.warnings {
                    ui.label(
                        RichText::new(format!("⚠ {warning}"))
                            .small()
                            .color(pal.status_amber),
                    );
                }
            }
        } else {
            ui.label(
                RichText::new("No build context found; using generic defaults.")
                    .small()
                    .color(egui::Color32::GRAY),
            );
        }

        ui.separator();

        // --- Preset --------------------------------------------------------
        {
            use crate::workflows::molecular_dynamics::PresetId;
            let recommended = recommendation.as_ref().map(|rec| rec.preset);
            ui.horizontal(|ui| {
                ui.label("Preset:");
                egui::ComboBox::from_id_salt("md_run_preset")
                    .selected_text(prompt.preset.title())
                    .show_ui(ui, |ui| {
                        crate::frontend::theme::stabilize_selectable_rows(ui);
                        for preset in PresetId::all() {
                            let applies =
                                prompt.effective().is_none_or(|eff| preset.applies_to(&eff));
                            let star = if recommended == Some(*preset) {
                                " ★"
                            } else {
                                ""
                            };
                            let na = if applies { "" } else { " (n/a)" };
                            if ui
                                .selectable_label(
                                    prompt.preset == *preset,
                                    format!("{}{star}{na}", preset.title()),
                                )
                                .clicked()
                            {
                                actions.push(AppAction::SetMdRunPreset(*preset));
                            }
                        }
                    });
            });
            ui.label(
                RichText::new(prompt.preset.description())
                    .small()
                    .color(egui::Color32::GRAY),
            );
        }

        ui.separator();

        // --- Basic parameters ----------------------------------------------
        {
            use crate::workflows::molecular_dynamics::ProductionLength;
            egui::Grid::new("md_run_basic_params")
                .num_columns(3)
                .show(ui, |ui| {
                    ui.label("Temperature (K):");
                    let mut temperature = prompt.params.temperature_k;
                    if ui
                        .add(
                            egui::DragValue::new(&mut temperature)
                                .range(1.0..=2_000.0_f32)
                                .speed(1.0),
                        )
                        .changed()
                    {
                        actions.push(AppAction::SetMdRunTemperature(temperature));
                    }
                    if ui.small_button("310 K").clicked() {
                        actions.push(AppAction::SetMdRunTemperature(310.0));
                    }
                    ui.end_row();

                    ui.label("Production:");
                    egui::ComboBox::from_id_salt("md_run_production")
                        .selected_text(prompt.params.production.label())
                        .show_ui(ui, |ui| {
                            crate::frontend::theme::stabilize_selectable_rows(ui);
                            for length in ProductionLength::all() {
                                if ui
                                    .selectable_label(
                                        prompt.params.production == *length,
                                        length.label(),
                                    )
                                    .clicked()
                                {
                                    actions.push(AppAction::SetMdRunProduction(*length));
                                }
                            }
                        });
                    ui.end_row();

                    ui.label("Timestep (ps):");
                    let mut timestep = prompt.params.timestep_ps;
                    if ui
                        .add(
                            egui::DragValue::new(&mut timestep)
                                .range(0.0005..=0.005_f32)
                                .speed(0.0005)
                                .fixed_decimals(4),
                        )
                        .changed()
                    {
                        actions.push(AppAction::SetMdRunTimestep(timestep));
                    }
                    ui.end_row();
                });

            let mut save = prompt.save_trajectory;
            if ui
                .checkbox(&mut save, "Save trajectory (play back each stage)")
                .changed()
            {
                actions.push(AppAction::SetMdRunSaveTrajectory(save));
            }
        }

        ui.separator();

        // --- Stage sequence (add / remove / reorder / edit) ----------------
        {
            use crate::workflows::molecular_dynamics::StageKind;
            ui.label(RichText::new("Stages").strong());
            ui.label(
                RichText::new("Preset-filled defaults; click a stage to edit its parameters.")
                    .small()
                    .color(egui::Color32::GRAY),
            );
            ui.horizontal_wrapped(|ui| {
                let adds = [
                    ("+ EM", StageKind::Minimize),
                    ("+ NVT", StageKind::NvtEquilibrate),
                    ("+ NPT", StageKind::NptEquilibrate),
                    ("+ Production", StageKind::Produce),
                    ("+ Anneal", StageKind::Anneal),
                    ("+ Extend", StageKind::Extend),
                ];
                for (label, kind) in adds {
                    if ui.button(label).clicked() {
                        actions.push(AppAction::AddMdRunStage(kind));
                    }
                }
            });

            let total = prompt.stages.len();
            let family = prompt.force_field_family();
            let expanded = prompt.expanded_stage;
            for (index, stage) in prompt.stages.iter().enumerate() {
                ui.add_space(4.0);
                render_md_stage_card(
                    ui,
                    index,
                    total,
                    stage,
                    expanded == Some(index),
                    family,
                    actions,
                );
            }
            if prompt.stages.is_empty() {
                ui.add_space(4.0);
                ui.label("No stages yet. Add one above or pick a preset.");
            }
        }

        // --- Validation ----------------------------------------------------
        if let Some(eff) = prompt.effective() {
            use crate::workflows::molecular_dynamics::run::IssueSeverity;
            let issues = crate::workflows::molecular_dynamics::run::validate(&prompt.stages, &eff);
            if !issues.is_empty() {
                ui.separator();
                for issue in &issues {
                    let (color, prefix) = match issue.severity {
                        IssueSeverity::Error => (pal.status_red, "error"),
                        IssueSeverity::Warning => (pal.status_amber, "warning"),
                    };
                    let stage = issue
                        .stage
                        .as_deref()
                        .map(|name| format!("[{name}] "))
                        .unwrap_or_default();
                    ui.label(
                        RichText::new(format!("{prefix}: {stage}{}", issue.message))
                            .small()
                            .color(color),
                    );
                }
            }
        }

        // --- Assembled .mdp preview ----------------------------------------
        // A read-only render of exactly what the run will hand the engine, so the
        // user sees how their inline/detail edits resolve. Computed through the
        // same realization path the launch uses.
        if !prompt.stages.is_empty() {
            ui.separator();
            egui::CollapsingHeader::new("Assembled .mdp preview")
                .id_salt("md_run_mdp_preview")
                .show(ui, |ui| {
                    let specs = crate::engines::gromacs::stage_specs_from_md_stages(
                        &prompt.stages,
                        prompt.force_field_family(),
                        None,
                    );
                    for spec in &specs {
                        ui.label(
                            RichText::new(format!("; {}.mdp", spec.stage_name))
                                .small()
                                .strong()
                                .monospace(),
                        );
                        let mdp = crate::engines::gromacs::input::render_mdp(&spec.settings);
                        let mut text = mdp.as_str();
                        ui.add(
                            egui::TextEdit::multiline(&mut text)
                                .font(egui::TextStyle::Monospace)
                                .desired_width(f32::INFINITY)
                                .interactive(false)
                                .code_editor(),
                        );
                        ui.add_space(4.0);
                    }
                });
        }

        ui.separator();
        ui.checkbox(&mut prompt.show_advanced, "Advanced");
        if prompt.show_advanced {
            ui.label("Topology override (.top/.itp):");
            ui.horizontal(|ui| {
                let label = prompt
                    .topology_override_path
                    .as_ref()
                    .map(|path| {
                        path.file_name()
                            .and_then(|name| name.to_str())
                            .unwrap_or("(unnamed)")
                            .to_string()
                    })
                    .unwrap_or_else(|| "Auto-detect / generated".to_string());
                ui.label(label);
                if ui
                    .button(format!("{}  Browse", egui_phosphor::regular::FOLDER))
                    .clicked()
                {
                    actions.push(AppAction::PickMdTopologyOverride);
                }
                if ui
                    .add_enabled(
                        prompt.topology_override_path.is_some(),
                        egui::Button::new(format!("{}  Clear", egui_phosphor::regular::X)),
                    )
                    .clicked()
                {
                    prompt.topology_override_path = None;
                }
            });
            ui.label(
                RichText::new(
                    "Without an override, SilicoLab reuses the captured MD topology or tries to generate one from the active structure.",
                )
                .small()
                .color(egui::Color32::GRAY),
            );
        }

        ui.separator();
        // The execution controls sit right above the Run button — target plus the
        // CPU/GPU envelope, which the GROMACS mdrun stages honour (local and relay).
        execution_section(
            ui,
            &mut prompt.prefs,
            crate::frontend::state::ExecutionCaps {
                cores: true,
                gpu: true,
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
                actions.push(AppAction::StartMdRun);
            }
            if ui
                .button(format!("{}  Cancel", egui_phosphor::regular::X))
                .clicked()
            {
                actions.push(AppAction::CancelMdRunPrompt);
            }
        });
    } else if state.jobs.engine_running() {
        ui.label("MD job is running.");
        if let Some(stage) = state
            .jobs
            .engine
            .as_ref()
            .and_then(|engine| engine.latest_stage.as_ref())
        {
            ui.label(RichText::new(stage).small());
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
        ui.label("MD configuration is unavailable.");
    }
}

/// One Run MD stage as a uniform card: the same component for every stage kind,
/// showing a header (reorder / remove / expand), an always-visible inline Basic
/// row (temperature / pressure / length), and — when expanded — the detail view
/// of the finer, tier-classified parameters and raw passthrough. Inapplicable
/// fields are simply hidden (a minimize card and an NPT card differ in fields, not
/// in shape). All edits flow out as [`AppAction`]s; this renders, never mutates.
#[allow(clippy::too_many_arguments)]
pub(crate) fn render_md_stage_card(
    ui: &mut egui::Ui,
    index: usize,
    total: usize,
    stage: &crate::workflows::molecular_dynamics::MdStage,
    expanded: bool,
    family: crate::workflows::molecular_dynamics::ForceFieldFamily,
    actions: &mut Vec<AppAction>,
) {
    use crate::frontend::state::MdStageEdit;

    Frame::group(ui.style()).show(ui, |ui| {
        ui.set_width(ui.available_width());
        // Header: position + name + kind, then reorder / remove / expand.
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("{}. {}", index + 1, stage.name)).strong());
            ui.label(
                RichText::new(stage.kind.label())
                    .small()
                    .color(egui::Color32::GRAY),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let caret = if expanded {
                    egui_phosphor::regular::CARET_UP
                } else {
                    egui_phosphor::regular::CARET_DOWN
                };
                if ui
                    .button(format!("{caret}  Details"))
                    .on_hover_text("Edit this stage's parameters")
                    .clicked()
                {
                    actions.push(AppAction::ToggleMdRunStageExpanded(index));
                }
                // A stage carrying free-form raw passthrough holds user-entered
                // Details that a delete would discard; gate it behind a confirm.
                let remove_clicked = if stage.raw_passthrough.is_empty() {
                    ui.add(egui::Button::new(egui_phosphor::regular::TRASH))
                        .on_hover_text("Remove stage")
                        .clicked()
                } else {
                    // Salt by stage name as well as index: keyed on index alone, a
                    // reorder mid-confirm would carry the armed state onto whatever
                    // stage slides into this slot.
                    crate::frontend::ui::widgets::confirm_destructive(
                        ui,
                        ("del_md_stage", index, stage.name.as_str()),
                        "Remove stage and its details?",
                        "Remove",
                        |ui| {
                            ui.add(egui::Button::new(egui_phosphor::regular::TRASH))
                                .on_hover_text("Remove stage")
                        },
                    )
                };
                if remove_clicked {
                    actions.push(AppAction::RemoveMdRunStage(index));
                }
                if ui
                    .add_enabled(
                        index + 1 < total,
                        egui::Button::new(egui_phosphor::regular::ARROW_DOWN),
                    )
                    .on_hover_text("Move down")
                    .clicked()
                {
                    actions.push(AppAction::MoveMdRunStage { index, up: false });
                }
                if ui
                    .add_enabled(
                        index > 0,
                        egui::Button::new(egui_phosphor::regular::ARROW_UP),
                    )
                    .on_hover_text("Move up")
                    .clicked()
                {
                    actions.push(AppAction::MoveMdRunStage { index, up: true });
                }
            });
        });

        // Inline Basic row: temperature (dynamics), pressure (coupled), length.
        ui.horizontal_wrapped(|ui| {
            if stage.kind.is_dynamics() {
                ui.label("T (K)");
                let mut t = stage.temperature_k;
                if ui
                    .add(
                        egui::DragValue::new(&mut t)
                            .range(1.0..=2_000.0_f32)
                            .speed(1.0),
                    )
                    .changed()
                {
                    actions.push(AppAction::EditMdRunStage {
                        index,
                        edit: MdStageEdit::Temperature(t),
                    });
                }
            }
            if let Some(pressure) = stage.pressure {
                ui.separator();
                ui.label("P (bar)");
                let mut p = pressure.ref_bar;
                if ui
                    .add(
                        egui::DragValue::new(&mut p)
                            .range(0.0..=1_000.0_f32)
                            .speed(0.1)
                            .fixed_decimals(1),
                    )
                    .changed()
                {
                    actions.push(AppAction::EditMdRunStage {
                        index,
                        edit: MdStageEdit::PressureBar(p),
                    });
                }
            }
            ui.separator();
            ui.label("Length");
            if let Some(length) = length_editor(ui, index, stage) {
                actions.push(AppAction::EditMdRunStage {
                    index,
                    edit: MdStageEdit::Length(length),
                });
            }
        });

        // A one-line summary keeps collapsed cards informative and uniform.
        let mut summary = format!("{} steps", stage.steps());
        if stage.restraint.is_restrained() {
            summary.push_str(" · restrained");
        }
        if stage.pressure.is_some() {
            summary.push_str(" · NPT");
        }
        ui.label(RichText::new(summary).small().color(egui::Color32::GRAY));

        if expanded {
            ui.separator();
            stage_detail_view(ui, index, stage, family, actions);
        }
    });
}

/// Inline length editor: a value plus a unit (steps / ps / ns). Returns the new
/// [`StageLength`] when the user changes either, else `None`.
pub(crate) fn length_editor(
    ui: &mut egui::Ui,
    index: usize,
    stage: &crate::workflows::molecular_dynamics::MdStage,
) -> Option<crate::workflows::molecular_dynamics::StageLength> {
    use crate::workflows::molecular_dynamics::StageLength;

    #[derive(PartialEq, Clone, Copy)]
    enum Unit {
        Steps,
        Ps,
        Ns,
    }
    // Decompose the current length into a display value and unit.
    let (mut amount, mut unit) = match stage.length {
        StageLength::Steps(n) => (n as f64, Unit::Steps),
        StageLength::Picoseconds(ps) if ps >= 1_000.0 => (ps / 1_000.0, Unit::Ns),
        StageLength::Picoseconds(ps) => (ps, Unit::Ps),
    };
    let compose = |amount: f64, unit: Unit| match unit {
        Unit::Steps => StageLength::Steps(amount.max(0.0) as u64),
        Unit::Ps => StageLength::Picoseconds(amount.max(0.0)),
        Unit::Ns => StageLength::Picoseconds((amount * 1_000.0).max(0.0)),
    };

    let mut changed = None;
    let value_changed = ui
        .add(
            egui::DragValue::new(&mut amount)
                .speed(1.0)
                .range(0.0..=1e12),
        )
        .changed();
    if value_changed {
        changed = Some(compose(amount, unit));
    }
    let before = unit;
    egui::ComboBox::from_id_salt(("md_stage_len_unit", index))
        .selected_text(match unit {
            Unit::Steps => "steps",
            Unit::Ps => "ps",
            Unit::Ns => "ns",
        })
        .show_ui(ui, |ui| {
            crate::frontend::theme::stabilize_selectable_rows(ui);
            ui.selectable_value(&mut unit, Unit::Steps, "steps");
            ui.selectable_value(&mut unit, Unit::Ps, "ps");
            ui.selectable_value(&mut unit, Unit::Ns, "ns");
        });
    if unit != before {
        // Switching unit re-expresses the same amount in the new unit.
        changed = Some(compose(amount, unit));
    }
    changed
}
