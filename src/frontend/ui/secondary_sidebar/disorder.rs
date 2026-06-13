use super::*;

use crate::frontend::state::{DisorderAmount, DisorderRegionKind};

/// The Build Disordered System task panel: pick molecules + amounts, a region,
/// packing parameters, and an optional obstacle, then launch the packer. A pure
/// view over `pending_disorder` — every edit is emitted as an [`AppAction`] the
/// dispatcher applies, so the console and panel share one mutation path.
pub(crate) fn render_disorder_task_panel(
    state: &mut AppState,
    ui: &mut egui::Ui,
    actions: &mut Vec<AppAction>,
) {
    // While a packing run streams into the result entry, show its live status
    // instead of the form (the structure fills in the viewport).
    if state.jobs.disorder_running() {
        ui.label("Packing in progress…");
        if let Some(report) = state
            .jobs
            .disorder
            .as_ref()
            .and_then(|job| job.latest_report.as_ref())
        {
            ui.label(
                RichText::new(format!(
                    "{}/{} placed · {} steps · worst overlap {:.2} Å",
                    report.total_placed(),
                    report.total_requested(),
                    report.steps,
                    report.max_overlap,
                ))
                .small()
                .color(egui::Color32::GRAY),
            );
        }
        ui.label(
            RichText::new("Press Esc to stop and keep what is packed so far.")
                .small()
                .color(egui::Color32::GRAY),
        );
        return;
    }

    // Snapshot the workspace entries for the molecule / obstacle pickers before
    // the prompt is borrowed mutably (the md_run pre-borrow pattern).
    let entries: Vec<(u64, String)> = state
        .entries
        .records
        .iter()
        .map(|entry| {
            let atoms = entry.structure.atoms.len();
            let label = if atoms > 0 {
                format!("{} ({atoms} atoms)", entry.name)
            } else {
                entry.name.clone()
            };
            (entry.id, label)
        })
        .collect();

    let Some(prompt) = &state.ui.pending_disorder else {
        ui.label("Open this task to configure packing.");
        return;
    };

    // --- 1. Result name -----------------------------------------------------
    ui.horizontal(|ui| {
        ui.label("Result name:");
        let mut name = prompt.output_name.clone();
        if ui
            .add(
                egui::TextEdit::singleline(&mut name)
                    .hint_text("Disordered system")
                    .desired_width(200.0),
            )
            .changed()
        {
            actions.push(AppAction::SetDisorderName(name));
        }
    });
    ui.separator();

    // --- 2. Molecules -------------------------------------------------------
    ui.label(RichText::new("Molecules").strong());
    ui.horizontal(|ui| {
        ui.label("Specify amount by:");
        egui::ComboBox::from_id_salt("disorder_amount_mode")
            .selected_text(prompt.amount_mode.label())
            .show_ui(ui, |ui| {
                for mode in [
                    DisorderAmount::Count,
                    DisorderAmount::DensityGCm3,
                    DisorderAmount::ConcentrationMolar,
                ] {
                    if ui
                        .selectable_label(prompt.amount_mode == mode, mode.label())
                        .clicked()
                    {
                        actions.push(AppAction::SetDisorderAmountMode(mode));
                    }
                }
            });
    });

    if entries.is_empty() {
        ui.label(
            RichText::new("Open or sketch a molecule first, then add it here.")
                .small()
                .color(egui::Color32::GRAY),
        );
    } else if prompt.components.is_empty() {
        ui.label(
            RichText::new("No molecules yet — add one to pack.")
                .small()
                .color(egui::Color32::GRAY),
        );
    }

    for (index, component) in prompt.components.iter().enumerate() {
        ui.horizontal(|ui| {
            let current = entries
                .iter()
                .find(|(id, _)| *id == component.entry_id)
                .map(|(_, label)| label.clone())
                .unwrap_or_else(|| "Pick a molecule".to_string());
            egui::ComboBox::from_id_salt(("disorder_component", index))
                .selected_text(current)
                .show_ui(ui, |ui| {
                    for (id, label) in &entries {
                        if ui
                            .selectable_label(component.entry_id == *id, label)
                            .clicked()
                        {
                            actions.push(AppAction::SetDisorderComponentEntry {
                                index,
                                entry_id: *id,
                            });
                        }
                    }
                });

            match prompt.amount_mode {
                DisorderAmount::Count => {
                    let mut count = component.count as f64;
                    if ui
                        .add(
                            egui::DragValue::new(&mut count)
                                .speed(1.0)
                                .range(0.0..=1_000_000.0)
                                .suffix(" copies"),
                        )
                        .changed()
                    {
                        actions.push(AppAction::SetDisorderComponentCount {
                            index,
                            count: count.max(0.0) as u32,
                        });
                    }
                }
                DisorderAmount::DensityGCm3 => {
                    let mut value = component.amount_value;
                    if ui
                        .add(
                            egui::DragValue::new(&mut value)
                                .speed(0.01)
                                .range(0.0..=30.0)
                                .suffix(" g/cm³"),
                        )
                        .changed()
                    {
                        actions.push(AppAction::SetDisorderComponentAmount { index, value });
                    }
                }
                DisorderAmount::ConcentrationMolar => {
                    let mut value = component.amount_value;
                    if ui
                        .add(
                            egui::DragValue::new(&mut value)
                                .speed(0.01)
                                .range(0.0..=100.0)
                                .suffix(" mol/L"),
                        )
                        .changed()
                    {
                        actions.push(AppAction::SetDisorderComponentAmount { index, value });
                    }
                }
            }

            if ui
                .button(egui_phosphor::regular::X)
                .on_hover_text("Remove this molecule")
                .clicked()
            {
                actions.push(AppAction::RemoveDisorderComponent(index));
            }
        });
    }

    if ui
        .button(format!("{}  Add molecule", egui_phosphor::regular::PLUS))
        .clicked()
    {
        actions.push(AppAction::AddDisorderComponent(None));
    }
    ui.separator();

    // --- 3. Region ----------------------------------------------------------
    ui.label(RichText::new("Region").strong());
    ui.horizontal(|ui| {
        for (kind, label) in [
            (DisorderRegionKind::Box, "Box"),
            (DisorderRegionKind::Sphere, "Sphere"),
            (DisorderRegionKind::Cylinder, "Cylinder"),
        ] {
            if ui
                .selectable_label(prompt.region_kind == kind, label)
                .clicked()
            {
                actions.push(AppAction::SetDisorderRegionKind(kind));
            }
        }
    });

    match prompt.region_kind {
        DisorderRegionKind::Box => {
            ui.horizontal(|ui| {
                ui.label("Size (Å):");
                for (axis, value) in prompt.box_lengths.iter().enumerate() {
                    let mut length = *value;
                    if ui
                        .add(
                            egui::DragValue::new(&mut length)
                                .speed(0.5)
                                .range(1.0..=1000.0),
                        )
                        .changed()
                    {
                        actions.push(AppAction::SetDisorderBoxLength {
                            axis,
                            value: length,
                        });
                    }
                }
            });
        }
        DisorderRegionKind::Sphere => {
            ui.horizontal(|ui| {
                ui.label("Radius (Å):");
                let mut radius = prompt.sphere_radius;
                if ui
                    .add(
                        egui::DragValue::new(&mut radius)
                            .speed(0.5)
                            .range(1.0..=1000.0),
                    )
                    .changed()
                {
                    actions.push(AppAction::SetDisorderSphereRadius(radius));
                }
            });
        }
        DisorderRegionKind::Cylinder => {
            ui.horizontal(|ui| {
                ui.label("Radius (Å):");
                let mut radius = prompt.cyl_radius;
                let mut length = prompt.cyl_length;
                let radius_changed = ui
                    .add(
                        egui::DragValue::new(&mut radius)
                            .speed(0.5)
                            .range(1.0..=1000.0),
                    )
                    .changed();
                ui.label("Length (Å):");
                let length_changed = ui
                    .add(
                        egui::DragValue::new(&mut length)
                            .speed(0.5)
                            .range(1.0..=1000.0),
                    )
                    .changed();
                if radius_changed || length_changed {
                    actions.push(AppAction::SetDisorderCylinder { radius, length });
                }
            });
        }
    }

    // Packing outside only makes sense for a sphere/cylinder void; a box fills
    // its own bounds, so it offers periodicity + cell options instead.
    if prompt.region_kind == DisorderRegionKind::Box {
        let mut set_cell = prompt.set_cell_from_region;
        if ui
            .checkbox(
                &mut set_cell,
                "Use the region as the result's simulation cell",
            )
            .changed()
        {
            actions.push(AppAction::SetDisorderSetCell(set_cell));
        }
        let mut periodic = prompt.periodic;
        if ui
            .checkbox(
                &mut periodic,
                "Pack periodically (no clashes across box edges)",
            )
            .changed()
        {
            actions.push(AppAction::SetDisorderPeriodic(periodic));
        }
    } else {
        let mut outside = prompt.sense_outside;
        if ui
            .checkbox(&mut outside, "Pack outside the region (carve a void)")
            .changed()
        {
            actions.push(AppAction::SetDisorderSense(outside));
        }
    }
    ui.separator();

    // --- 4. Packing parameters ---------------------------------------------
    ui.label(RichText::new("Packing parameters").strong());
    egui::Grid::new("disorder_params")
        .num_columns(2)
        .spacing([12.0, 6.0])
        .show(ui, |ui| {
            ui.label("Spacing (Å):");
            let mut tolerance = prompt.tolerance_angstrom;
            if ui
                .add(
                    egui::DragValue::new(&mut tolerance)
                        .speed(0.05)
                        .range(0.5..=10.0),
                )
                .on_hover_text("Minimum distance between atoms of different molecules")
                .changed()
            {
                actions.push(AppAction::SetDisorderTolerance(tolerance));
            }
            ui.end_row();

            ui.label("Seed:");
            ui.horizontal(|ui| {
                let mut seed = prompt.seed as f64;
                if ui
                    .add(
                        egui::DragValue::new(&mut seed)
                            .speed(1.0)
                            .range(0.0..=u32::MAX as f64),
                    )
                    .changed()
                {
                    actions.push(AppAction::SetDisorderSeed(seed.max(0.0) as u64));
                }
                if ui
                    .button("Randomize")
                    .on_hover_text("Pick a fresh random seed")
                    .clicked()
                {
                    actions.push(AppAction::RandomizeDisorderSeed);
                }
            });
            ui.end_row();
        });

    let mut show_advanced = prompt.show_advanced;
    if ui.checkbox(&mut show_advanced, "Advanced").changed() {
        actions.push(AppAction::SetDisorderShowAdvanced(show_advanced));
    }
    if prompt.show_advanced {
        egui::Grid::new("disorder_advanced")
            .num_columns(2)
            .spacing([12.0, 6.0])
            .show(ui, |ui| {
                let mut restarts = prompt.max_restarts as f64;
                let mut steps = prompt.max_steps as f64;
                ui.label("Max restarts:");
                let restarts_changed = ui
                    .add(
                        egui::DragValue::new(&mut restarts)
                            .speed(1.0)
                            .range(0.0..=200.0),
                    )
                    .changed();
                ui.end_row();
                ui.label("Max steps:");
                let steps_changed = ui
                    .add(
                        egui::DragValue::new(&mut steps)
                            .speed(10.0)
                            .range(1.0..=100_000.0),
                    )
                    .changed();
                ui.end_row();
                if restarts_changed || steps_changed {
                    actions.push(AppAction::SetDisorderLimits {
                        max_restarts: restarts.max(0.0) as u32,
                        max_steps: steps.max(1.0) as u32,
                    });
                }
            });
    }
    ui.separator();

    // --- 5. Avoid an existing structure (optional) -------------------------
    ui.label(RichText::new("Avoid an existing structure (optional)").strong());
    ui.horizontal(|ui| {
        ui.label("Pack around:");
        let current = prompt
            .obstacle_entry_id
            .and_then(|id| entries.iter().find(|(entry, _)| *entry == id))
            .map(|(_, label)| label.clone())
            .unwrap_or_else(|| "None".to_string());
        egui::ComboBox::from_id_salt("disorder_obstacle")
            .selected_text(current)
            .show_ui(ui, |ui| {
                if ui
                    .selectable_label(prompt.obstacle_entry_id.is_none(), "None")
                    .clicked()
                {
                    actions.push(AppAction::SetDisorderObstacle(None));
                }
                for (id, label) in &entries {
                    if ui
                        .selectable_label(prompt.obstacle_entry_id == Some(*id), label)
                        .clicked()
                    {
                        actions.push(AppAction::SetDisorderObstacle(Some(*id)));
                    }
                }
            });
    });
    ui.separator();

    // --- 6. Build / Cancel --------------------------------------------------
    let can_build = !prompt.components.is_empty();
    ui.horizontal(|ui| {
        if ui
            .add_enabled(
                can_build,
                egui::Button::new(format!("{}  Build", egui_phosphor::regular::PLAY)),
            )
            .clicked()
        {
            actions.push(AppAction::StartDisorder);
        }
        if ui
            .button(format!("{}  Cancel", egui_phosphor::regular::X))
            .clicked()
        {
            actions.push(AppAction::CancelDisorderPrompt);
        }
    });
}
