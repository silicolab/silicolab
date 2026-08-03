use super::*;

mod engine_row;
mod engines;
mod remote;

pub(crate) use engines::*;
pub(crate) use remote::*;

/// Maps a controller's fine-grained `theme` into the broader, user-facing
/// category shown as a collapsible group in the task list.
pub(crate) fn task_category(theme: &str) -> &'static str {
    match theme {
        "Reticular Design" | "2D Materials" => "Structure Builder",
        "Geometry" => "Optimization",
        "Electronic Structure" => "Quantum Mechanics",
        "Molecular Dynamics" => "Molecular Dynamics",
        "Molecular Docking" => "Molecular Docking",
        // "Structure Editing" and "Crystal Editing" both fold into editing.
        _ => "Structure Editing",
    }
}

/// Display order of the task categories.
pub(crate) const TASK_CATEGORIES: &[&str] = &[
    "Structure Builder",
    "Structure Editing",
    "Optimization",
    "Quantum Mechanics",
    "Molecular Dynamics",
    "Molecular Docking",
];

pub(crate) fn render_tasks_view(
    state: &mut AppState,
    ui: &mut egui::Ui,
    actions: &mut Vec<AppAction>,
) {
    let search = state.tasks.task_list.search_query.to_lowercase();
    let pal = crate::frontend::theme::palette(ui);
    docked_sidebar_scroll_area().show(ui, |ui| {
        for category in TASK_CATEGORIES {
            let controllers = task_controllers()
                .iter()
                .copied()
                .filter(|controller| task_category(controller.theme) == *category)
                .filter(|controller| {
                    search.is_empty()
                        || controller.title.to_lowercase().contains(&search)
                        || controller.short_title.to_lowercase().contains(&search)
                        || controller.theme.to_lowercase().contains(&search)
                        || controller.method.to_lowercase().contains(&search)
                        || controller.application.to_lowercase().contains(&search)
                })
                .collect::<Vec<_>>();
            if controllers.is_empty() {
                continue;
            }

            // A search keeps every matching group expanded so results stay visible.
            let collapsed =
                search.is_empty() && state.tasks.task_list.collapsed_themes.contains(*category);
            let marker = if collapsed {
                egui_phosphor::regular::CARET_RIGHT
            } else {
                egui_phosphor::regular::CARET_DOWN
            };

            let header_interact = task_category_header(ui, category, marker, &pal);
            if header_interact.clicked()
                && !state
                    .tasks
                    .task_list
                    .collapsed_themes
                    .insert((*category).to_string())
            {
                state.tasks.task_list.collapsed_themes.remove(*category);
            }

            if !collapsed {
                ui.add_space(2.0);
                for controller in controllers {
                    let response = task_row(ui, controller.short_title, controller.description);
                    if response.clicked() {
                        actions.push(AppAction::CreateTask(controller.id));
                    }
                    ui.add_space(2.0);
                }
            }
            ui.add_space(8.0);
        }
    });
}

fn task_category_header(
    ui: &mut egui::Ui,
    category: &str,
    marker: &str,
    pal: &crate::frontend::theme::Palette,
) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 26.0), Sense::click());
    let radius = f32::from(crate::frontend::theme::radius::CONTROL);
    let fill = if response.is_pointer_button_down_on() {
        Some(pal.neutral_overlay(32))
    } else if response.hovered() {
        Some(pal.neutral_overlay(18))
    } else {
        None
    };
    if let Some(fill) = fill {
        ui.painter().rect_filled(rect, radius, fill);
    }

    let y = rect.center().y;
    ui.painter().text(
        egui::pos2(rect.left() + 8.0, y),
        egui::Align2::LEFT_CENTER,
        marker,
        egui::FontId::proportional(11.0),
        pal.text_tertiary,
    );
    ui.painter().text(
        egui::pos2(rect.left() + 25.0, y),
        egui::Align2::LEFT_CENTER,
        category.to_uppercase(),
        egui::FontId::proportional(11.0),
        pal.text_muted,
    );

    response
}

fn task_row(ui: &mut egui::Ui, title: &str, description: &str) -> egui::Response {
    let pal = crate::frontend::theme::palette(ui);
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 31.0), Sense::click());
    let response = response.on_hover_text(description);
    let radius = f32::from(crate::frontend::theme::radius::CONTROL);
    let fill = if response.is_pointer_button_down_on() {
        Some(pal.neutral_overlay(34))
    } else if response.hovered() {
        Some(pal.neutral_overlay(20))
    } else {
        None
    };
    if let Some(fill) = fill {
        ui.painter().rect_filled(rect, radius, fill);
    }

    ui.painter().text(
        egui::pos2(rect.left() + 22.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        title,
        egui::FontId::proportional(13.0),
        pal.text_primary,
    );

    response
}

pub(crate) fn cartoon_section_controls(
    ui: &mut egui::Ui,
    label: &str,
    section: &mut CartoonSectionStyle,
) {
    ui.horizontal(|ui| {
        ui.label(label);
        ui.add(
            egui::DragValue::new(&mut section.width)
                .range(0.05..=10.0)
                .speed(0.05),
        );
        ui.add(
            egui::DragValue::new(&mut section.thickness)
                .range(0.05..=10.0)
                .speed(0.05),
        );
    });
}

pub(crate) fn render_structure_editor_window(
    state: &mut AppState,
    actions: &mut Vec<AppAction>,
    ctx: &egui::Context,
) {
    let Some(editor) = &mut state.ui.editor else {
        return;
    };

    let mut apply = false;
    let mut cancel = false;
    let mut preview_update = None;
    egui::Window::new("Edit Structure")
        .default_size([760.0, 420.0])
        .max_height(520.0)
        .resizable(true)
        .collapsible(false)
        .show(ctx, |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.label(if editor.draft.cell.is_some() {
                        "Periodic structure: atom coordinates are fractional."
                    } else {
                        "Non-periodic structure: atom coordinates are Cartesian."
                    });
                    ui.separator();
                    if editor.ui(ui) {
                        preview_update = Some(editor.draft.clone());
                    }
                });
            ui.separator();
            ui.horizontal(|ui| {
                if ui
                    .button(format!("{}  Apply", egui_phosphor::regular::CHECK))
                    .clicked()
                {
                    apply = true;
                }
                if ui
                    .button(format!("{}  Cancel", egui_phosphor::regular::X))
                    .clicked()
                {
                    cancel = true;
                }
            });
        });

    if let Some(draft) = preview_update {
        *state.structure_mut() = draft;
        state.mark_structure_changed();
        actions.push(AppAction::PostStatusNeutral(
            "Editing preview updated".to_string(),
        ));
    }

    if apply {
        actions.push(AppAction::ApplyStructureEdits);
    } else if cancel {
        actions.push(AppAction::CancelStructureEdits);
    }
}

/// The shared plain-text viewer window: a read-only monospace view of any
/// tool's textual output (e.g. a QM report opened via an entry's "QM" badge).
pub(crate) fn render_text_viewer_window(state: &mut AppState, ctx: &egui::Context) {
    let Some(viewer) = &state.ui.text_viewer else {
        return;
    };

    let mut open = true;
    egui::Window::new(viewer.title.clone())
        .default_size([640.0, 420.0])
        .max_height(560.0)
        .resizable(true)
        .collapsible(false)
        .open(&mut open)
        .show(ctx, |ui| {
            egui::ScrollArea::both()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.add(
                        egui::Label::new(egui::RichText::new(&viewer.text).monospace())
                            .wrap_mode(egui::TextWrapMode::Extend),
                    );
                });
        });
    if !open {
        state.ui.text_viewer = None;
    }
}

pub(crate) fn render_pdb_fetch_window(
    state: &mut AppState,
    actions: &mut Vec<AppAction>,
    ctx: &egui::Context,
) {
    let Some(id) = &mut state.ui.pending_pdb_fetch else {
        return;
    };

    let mut fetch = false;
    let mut cancel = false;
    let mut open = true;
    egui::Window::new("Fetch from PDB ID")
        .collapsible(false)
        .resizable(false)
        .open(&mut open)
        .show(ctx, |ui| {
            ui.label("Enter a 4-character PDB id.");
            ui.label("The structure is downloaded from rcsb.org into the structures/ folder.");
            ui.add_space(6.0);
            let response = ui.add(egui::TextEdit::singleline(id).desired_width(120.0));
            // Focus the field when the dialog first appears without stealing
            // focus on later frames.
            if ui.memory(|memory| memory.focused().is_none()) {
                response.request_focus();
            }
            let submitted =
                response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
            let can_fetch = !id.trim().is_empty();
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        can_fetch,
                        Button::new(format!(
                            "{}  Fetch",
                            egui_phosphor::regular::DOWNLOAD_SIMPLE
                        )),
                    )
                    .clicked()
                {
                    fetch = true;
                }
                if ui
                    .button(format!("{}  Cancel", egui_phosphor::regular::X))
                    .clicked()
                {
                    cancel = true;
                }
            });
            if submitted && can_fetch {
                fetch = true;
            }
        });

    if fetch {
        actions.push(AppAction::FetchPdb);
    } else if cancel || !open {
        actions.push(AppAction::CancelPdbFetch);
    }
}

pub(crate) fn render_online_structure_search_window(
    state: &mut AppState,
    actions: &mut Vec<AppAction>,
    ctx: &egui::Context,
) {
    let Some(panel) = state.ui.online_structure_search.as_mut() else {
        return;
    };
    let mut open = true;
    let mut search = false;
    let mut import = None;
    let mut build_molecule = false;
    egui::Window::new("Find Online Structure")
        .default_size([760.0, 560.0])
        .min_size([620.0, 420.0])
        .open(&mut open)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                for kind in [
                    crate::io::online_structures::QueryKind::Auto,
                    crate::io::online_structures::QueryKind::Name,
                    crate::io::online_structures::QueryKind::Formula,
                    crate::io::online_structures::QueryKind::Smiles,
                ] {
                    ui.selectable_value(&mut panel.kind, kind, kind.label());
                }
            });
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let response = ui.add(
                    egui::TextEdit::singleline(&mut panel.query)
                        .hint_text("compound name, formula, or SMILES")
                        .desired_width(f32::INFINITY),
                );
                let busy = panel.phase != crate::frontend::state::OnlineStructurePhase::Idle;
                if ui
                    .add_enabled(
                        !busy && !panel.query.trim().is_empty(),
                        egui::Button::new(format!(
                            "{}  Search",
                            egui_phosphor::regular::MAGNIFYING_GLASS
                        )),
                    )
                    .clicked()
                {
                    search = true;
                }
                if response.lost_focus()
                    && ui.input(|input| input.key_pressed(egui::Key::Enter))
                    && !busy
                    && !panel.query.trim().is_empty()
                {
                    search = true;
                }
            });
            ui.horizontal(|ui| {
                ui.checkbox(&mut panel.include_disorder, "Show disorder records");
                ui.label("Limit");
                ui.add(egui::DragValue::new(&mut panel.limit).range(1..=20));
            });
            ui.separator();

            match panel.phase {
                crate::frontend::state::OnlineStructurePhase::Searching => {
                    ui.spinner();
                    ui.label("Searching PubChem and COD...");
                }
                crate::frontend::state::OnlineStructurePhase::Importing => {
                    ui.spinner();
                    ui.label("Downloading and validating the selected CIF...");
                }
                crate::frontend::state::OnlineStructurePhase::Idle => {}
            }
            if let Some(error) = &panel.error {
                ui.colored_label(ui.visuals().error_fg_color, error);
            }

            if let Some(result) = &panel.result {
                for warning in &result.warnings {
                    ui.label(egui::RichText::new(warning).weak());
                }
                ui.label(egui::RichText::new("Experimental crystals (COD)").strong());
                ui.add_space(2.0);
                egui::ScrollArea::both()
                    .id_salt("online_crystal_results")
                    .max_height(255.0)
                    .show(ui, |ui| {
                        egui::Grid::new("online_crystal_grid")
                            .num_columns(7)
                            .striped(true)
                            .spacing([12.0, 5.0])
                            .show(ui, |ui| {
                                ui.strong("COD ID");
                                ui.strong("Name / formula");
                                ui.strong("T");
                                ui.strong("Space group");
                                ui.strong("R");
                                ui.strong("Quality");
                                ui.strong("DOI");
                                ui.end_row();
                                for (index, candidate) in result.crystals.iter().enumerate() {
                                    if ui
                                        .selectable_label(
                                            panel.selected == Some(index),
                                            &candidate.cod_id,
                                        )
                                        .clicked()
                                    {
                                        panel.selected = Some(index);
                                    }
                                    ui.vertical(|ui| {
                                        ui.label(&candidate.name);
                                        ui.label(egui::RichText::new(&candidate.formula).weak());
                                    });
                                    ui.label(
                                        candidate
                                            .temperature_k
                                            .map(|value| format!("{value:.0} K"))
                                            .unwrap_or_else(|| "-".to_string()),
                                    );
                                    ui.label(candidate.space_group.as_deref().unwrap_or("-"));
                                    ui.label(
                                        candidate
                                            .r_factor
                                            .map(|value| format!("{value:.4}"))
                                            .unwrap_or_else(|| "-".to_string()),
                                    );
                                    ui.label(if candidate.warnings.is_empty() {
                                        "Compatible"
                                    } else {
                                        "Review"
                                    })
                                    .on_hover_text(candidate.warnings.join("\n"));
                                    if let Some(doi) = &candidate.doi {
                                        ui.hyperlink_to("Open", format!("https://doi.org/{doi}"));
                                    } else {
                                        ui.label("-");
                                    }
                                    ui.end_row();
                                }
                            });
                    });

                if let Some(candidate) = panel.selected.and_then(|index| result.crystals.get(index))
                {
                    ui.add_space(6.0);
                    ui.horizontal_wrapped(|ui| {
                        ui.label(format!("COD {}", candidate.cod_id));
                        if let Some(revision) = &candidate.revision {
                            ui.label(format!("revision {revision}"));
                        }
                        if let Some(doi) = &candidate.doi {
                            ui.hyperlink_to(format!("DOI {doi}"), format!("https://doi.org/{doi}"));
                        }
                    });
                    for warning in &candidate.warnings {
                        ui.label(egui::RichText::new(warning).weak());
                    }
                }
                ui.horizontal(|ui| {
                    let idle = panel.phase == crate::frontend::state::OnlineStructurePhase::Idle;
                    let importable = panel
                        .selected
                        .and_then(|index| result.crystals.get(index))
                        .is_some_and(|candidate| candidate.is_importable());
                    if ui
                        .add_enabled(
                            idle && importable,
                            egui::Button::new(format!(
                                "{}  Import selected CIF",
                                egui_phosphor::regular::DOWNLOAD_SIMPLE
                            )),
                        )
                        .clicked()
                    {
                        import = panel.selected;
                    }
                });

                ui.separator();
                ui.label(egui::RichText::new("Non-periodic molecular fallback").strong());
                if let Some(compound) = &result.resolved {
                    ui.label(format!(
                        "{} (PubChem CID {})  {}",
                        compound.title, compound.cid, compound.formula
                    ));
                    ui.label(egui::RichText::new(&compound.smiles).monospace());
                    ui.label(
                        egui::RichText::new(
                            "Generated from SMILES; this is not an experimental crystal.",
                        )
                        .weak(),
                    );
                    if ui
                        .add_enabled(
                            panel.phase == crate::frontend::state::OnlineStructurePhase::Idle,
                            egui::Button::new("Build molecule"),
                        )
                        .clicked()
                    {
                        build_molecule = true;
                    }
                } else {
                    ui.label(egui::RichText::new("No resolved PubChem molecule.").weak());
                }
            }
        });

    if search {
        actions.push(AppAction::StartOnlineStructureSearch);
    }
    if let Some(index) = import {
        actions.push(AppAction::ImportOnlineCrystal(index));
    }
    if build_molecule {
        actions.push(AppAction::BuildResolvedOnlineMolecule);
    }
    if !open {
        actions.push(AppAction::CloseOnlineStructureSearch);
    }
}
