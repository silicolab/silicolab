use super::*;

/// Maps a controller's fine-grained `theme` into the broader, user-facing
/// category shown as a collapsible group in the task list.
pub(crate) fn task_category(theme: &str) -> &'static str {
    match theme {
        "Reticular Design" | "2D Materials" => "Structure Builder",
        "Geometry" => "Optimization",
        "Electronic Structure" => "Quantum Mechanics",
        "Molecular Dynamics" => "Molecular Dynamics",
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
];

pub(crate) fn render_tasks_view(
    state: &mut AppState,
    ui: &mut egui::Ui,
    actions: &mut Vec<AppAction>,
) {
    let search = state.tasks.task_list.search_query.to_lowercase();
    let pal = crate::frontend::theme::palette(ui);
    ScrollArea::vertical()
        // Wheel/trackpad plus content drag (touch-friendly), but the scroll bar
        // stays a non-interactive position indicator: excluding SCROLL_BAR
        // stops the bar from catching a drag that starts on the adjacent panel
        // resize divider — the bug where dragging the divider scrolled instead
        // of resizing.
        .scroll_source(
            egui::scroll_area::ScrollSource::MOUSE_WHEEL | egui::scroll_area::ScrollSource::DRAG,
        )
        .show(ui, |ui| {
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

                let header = ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), 0.0),
                    Layout::left_to_right(Align::Center),
                    |ui| {
                        ui.label(RichText::new(marker).size(11.0).color(pal.text_muted));
                        ui.label(RichText::new(*category).strong());
                        ui.response()
                    },
                );
                let header_interact = ui.interact(
                    header.response.rect,
                    Id::new(format!("task_category_{category}")),
                    Sense::click(),
                );
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
                        let card_radius = crate::frontend::theme::radius::CARD;
                        let response = Frame::default()
                            .fill(pal.item_fill)
                            .stroke(Stroke::NONE)
                            .corner_radius(egui::CornerRadius::same(card_radius))
                            .inner_margin(Margin::symmetric(10, 7))
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.label(RichText::new(controller.short_title));
                            })
                            .response
                            .interact(Sense::click())
                            .on_hover_text(controller.description);
                        if response.hovered() {
                            ui.painter().rect_filled(
                                response.rect,
                                f32::from(card_radius),
                                pal.blue_overlay(18),
                            );
                            ui.painter().rect_stroke(
                                response.rect,
                                f32::from(card_radius),
                                Stroke::new(1.0, pal.blue_overlay(72)),
                                egui::StrokeKind::Inside,
                            );
                        }
                        if response.clicked() {
                            actions.push(AppAction::CreateTask(controller.id));
                        }
                        ui.add_space(4.0);
                    }
                }
                ui.add_space(8.0);
            }
        });
}

/// Owned snapshot of one engine capability, decoupled from the registry
/// borrow so we can freely mutate the per-engine drafts while rendering.
pub(crate) struct EngineRowView {
    id: EngineId,
    name: &'static str,
    description: &'static str,
    built_in: bool,
    available: bool,
    version: Option<String>,
    launch: Option<EngineLaunch>,
}

/// Render a `SystemTime` as a coarse relative age ("just now", "5m ago").
/// Avoids a date-formatting dependency; granularity is fine for "how stale is
/// this detection".
pub(crate) fn humanize_since(time: std::time::SystemTime) -> String {
    let Ok(elapsed) = time.elapsed() else {
        return "moments ago".to_string();
    };
    let secs = elapsed.as_secs();
    if secs < 5 {
        "just now".to_string()
    } else if secs < 60 {
        format!("{secs}s ago")
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else if secs < 86_400 {
        format!("{}h ago", secs / 3600)
    } else {
        format!("{}d ago", secs / 86_400)
    }
}

pub(crate) fn render_engine_settings(
    state: &mut AppState,
    ui: &mut egui::Ui,
    actions: &mut Vec<AppAction>,
) {
    // No section header here: the settings registry already wraps this editor in
    // the "Engines" group's collapsing header. Just the right-aligned Re-detect
    // action. The right-to-left layout must live inside a `horizontal` row: a
    // bare `with_layout` in a vertical ui claims the entire remaining pane
    // height, leaving the rest of the editor below a huge blank band.
    ui.horizontal(|ui| {
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui
                .button("Re-detect")
                .on_hover_text("Run each engine's --version (can be slow for WSL)")
                .clicked()
            {
                actions.push(AppAction::DetectEngineVersions);
            }
        });
    });

    // Availability is resolved lazily and cheaply (no subprocess). Version
    // strings are NOT probed here — that spawns `--version` per engine and a
    // WSL launch cold-starts the VM, which made first open slow. Versions are
    // detected only on explicit "Re-detect" / "Apply & Detect".
    let Some(registry) = state.ui.settings.engine_registry.as_ref() else {
        actions.push(AppAction::RefreshEngineRegistry);
        return;
    };

    let versions_caption = match state.ui.settings.engine_versions_checked_at {
        Some(checked_at) => format!("Versions last checked {}", humanize_since(checked_at)),
        None => "Versions not checked yet — click Re-detect".to_string(),
    };
    let pal = crate::frontend::theme::palette(ui);
    ui.label(
        RichText::new(versions_caption)
            .small()
            .color(pal.text_tertiary),
    );

    let rows: Vec<EngineRowView> = registry
        .capabilities()
        .iter()
        .map(|cap| EngineRowView {
            id: cap.id,
            name: cap.name,
            description: cap.description,
            built_in: cap.built_in,
            available: cap.available(),
            version: cap.version.clone(),
            launch: cap.launch.clone(),
        })
        .collect();

    for row in rows {
        // The name carries no status color; a trailing tag communicates the
        // engine's *type* (built-in) or, for external engines, its detection
        // status. Built-ins are always ready, so they get a calm accent-tinted
        // "Built-in" pill instead of the green check that would otherwise mark
        // every row "OK" and rob the check of meaning. The check/cross is
        // reserved for external engines, where availability actually varies.
        ui.horizontal(|ui| {
            ui.label(RichText::new(row.name).strong());
            if row.built_in {
                status_pill(ui, "Built-in", pal.blue_overlay(40), pal.accent);
            } else if row.available {
                ui.label(
                    RichText::new(format!(
                        "{}  Detected",
                        egui_phosphor::regular::CHECK_CIRCLE
                    ))
                    .small()
                    .color(pal.status_green),
                );
            } else {
                ui.label(
                    RichText::new(format!("{}  Not found", egui_phosphor::regular::X_CIRCLE))
                        .small()
                        .color(pal.text_muted),
                );
            }
        });
        if let Some(version) = &row.version {
            ui.label(RichText::new(format!("version {version}")).small());
        }
        ui.label(
            RichText::new(row.description)
                .small()
                .color(pal.text_tertiary),
        );

        if row.built_in {
            ui.add_space(6.0);
            continue;
        }

        // Seed the editable draft once, preferring an explicit override, then
        // the auto-detected launch, then empty.
        let key = row.id.as_str().to_string();
        if !state.ui.settings.engine_drafts.contains_key(&key) {
            let seed = state
                .config
                .engine_overrides
                .get(&key)
                .map(EngineDraft::from_launch)
                .or_else(|| row.launch.as_ref().map(EngineDraft::from_launch))
                .unwrap_or_default();
            state.ui.settings.engine_drafts.insert(key.clone(), seed);
        }
        let draft = state
            .ui
            .settings
            .engine_drafts
            .get_mut(&key)
            .expect("draft seeded above");

        ui.horizontal(|ui| {
            ui.label("Command prefix:");
            ui.add(
                egui::TextEdit::singleline(&mut draft.command_prefix).desired_width(f32::INFINITY),
            );
        });
        ui.label(
            RichText::new("e.g. `wsl.exe -e` to run inside WSL; leave blank for a native install")
                .small()
                .color(pal.text_tertiary),
        );
        ui.horizontal(|ui| {
            ui.label("Program:");
            // Reserve the Browse button on the right and let the text field fill
            // the space between it and the label. A plain left-to-right layout
            // gives the singleline edit an infinite desired width, which eats the
            // whole row and pushes Browse off the (clipped) right edge.
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button("Browse").clicked() {
                    actions.push(AppAction::BrowseEngineProgram(row.id));
                }
                ui.add(egui::TextEdit::singleline(&mut draft.program).desired_width(f32::INFINITY));
            });
        });
        ui.horizontal(|ui| {
            if ui.button("Apply & Detect").clicked() {
                actions.push(AppAction::ApplyEngineOverride(row.id));
            }
            if ui.button("Clear").clicked() {
                actions.push(AppAction::ClearEngineOverride(row.id));
            }
        });
        ui.add_space(8.0);
    }
}

/// One labeled single-line text field row for a remote-host draft.
fn remote_host_field(ui: &mut egui::Ui, label: &str, value: &mut String) {
    ui.horizontal(|ui| {
        ui.label(label);
        ui.add(egui::TextEdit::singleline(value).desired_width(f32::INFINITY));
    });
}

/// The connection-status indicator for a remote host.
fn remote_status_indicator(
    ui: &mut egui::Ui,
    status: &crate::frontend::state::RemoteHostStatus,
    pal: &crate::frontend::theme::Palette,
) {
    use crate::frontend::state::RemoteHostStatus;
    match status {
        RemoteHostStatus::Unknown => {
            ui.label(RichText::new("Not checked").small().color(pal.text_muted));
        }
        RemoteHostStatus::Checking => {
            ui.label(RichText::new("Checking…").small().color(pal.text_tertiary));
        }
        RemoteHostStatus::Ready => {
            ui.label(
                RichText::new(format!(
                    "{}  Connected",
                    egui_phosphor::regular::CHECK_CIRCLE
                ))
                .small()
                .color(pal.status_green),
            );
        }
        RemoteHostStatus::NeedsSetup => {
            ui.label(
                RichText::new(format!(
                    "{}  Needs passwordless setup",
                    egui_phosphor::regular::WARNING
                ))
                .small()
                .color(pal.text_muted),
            );
        }
        RemoteHostStatus::Unreachable(reason) => {
            ui.label(
                RichText::new(format!("{}  Unreachable", egui_phosphor::regular::X_CIRCLE))
                    .small()
                    .color(pal.text_muted),
            )
            .on_hover_text(reason);
        }
    }
}

/// The Remote Hosts editor (Settings → Engines → Remote Hosts). Lists configured
/// hosts with editable connection fields, a Detect/Test/Set-up-passwordless row,
/// and an "add host" form. Network actions run on worker threads (see the
/// dispatcher), so the panel stays responsive.
pub(crate) fn render_remote_hosts_settings(
    state: &mut AppState,
    ui: &mut egui::Ui,
    actions: &mut Vec<AppAction>,
) {
    use crate::engines::registry::EngineId;
    use crate::frontend::state::RemoteHostDraft;

    let pal = crate::frontend::theme::palette(ui);
    ui.label(
        RichText::new(
            "Run external engines (GROMACS) on a remote host over SSH. Login is key-based — no \
             passwords are stored. Pick a host per task with the Compute selector in the Run/Build \
             panels.",
        )
        .small()
        .color(pal.text_tertiary),
    );

    // Owned, sorted (id, label) list so the loop doesn't borrow config while the
    // per-host draft is edited mutably.
    let mut hosts: Vec<(String, String)> = state
        .config
        .remote_hosts
        .values()
        .map(|host| (host.id.clone(), host.label.clone()))
        .collect();
    hosts.sort_by_key(|host| host.1.to_lowercase());

    if hosts.is_empty() {
        ui.label(
            RichText::new("No remote hosts configured yet.")
                .small()
                .color(pal.text_muted),
        );
    }

    for (id, label) in &hosts {
        // Seed the editable draft from the stored host on first show.
        if !state.ui.settings.remote_host_drafts.contains_key(id)
            && let Some(host) = state.config.remote_hosts.get(id)
        {
            state
                .ui
                .settings
                .remote_host_drafts
                .insert(id.clone(), RemoteHostDraft::from_host(host));
        }
        // Snapshots taken before the mutable draft borrow below.
        let status = state
            .ui
            .settings
            .remote_status
            .get(id)
            .cloned()
            .unwrap_or_default();
        let bootstrap_cmd = match &state.ui.settings.remote_bootstrap {
            Some((bid, cmd)) if bid == id => Some(cmd.clone()),
            _ => None,
        };
        let version = state.config.remote_hosts.get(id).and_then(|host| {
            host.engine_versions
                .get(EngineId::GROMACS.as_str())
                .cloned()
        });

        ui.separator();
        ui.horizontal(|ui| {
            ui.label(RichText::new(label).strong());
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                remote_status_indicator(ui, &status, &pal);
            });
        });

        if let Some(draft) = state.ui.settings.remote_host_drafts.get_mut(id) {
            remote_host_field(ui, "Label:", &mut draft.label);
            remote_host_field(ui, "Host:", &mut draft.hostname);
            remote_host_field(ui, "User:", &mut draft.username);
            remote_host_field(ui, "Port:", &mut draft.port);
            remote_host_field(ui, "Work dir:", &mut draft.work_root);
            ui.label(
                RichText::new("Setup commands (one per line, e.g. module load gromacs):")
                    .small()
                    .color(pal.text_tertiary),
            );
            ui.add(
                egui::TextEdit::multiline(&mut draft.prelude)
                    .desired_rows(2)
                    .desired_width(f32::INFINITY),
            );
            remote_host_field(ui, "GROMACS path:", &mut draft.gmx_program);
        }
        if let Some(version) = &version {
            ui.label(
                RichText::new(format!("Detected GROMACS {version}"))
                    .small()
                    .color(pal.status_green),
            );
        }

        ui.horizontal(|ui| {
            if ui.button("Save").clicked() {
                actions.push(AppAction::SaveRemoteHost(id.clone()));
            }
            if ui.button("Detect GROMACS").clicked() {
                actions.push(AppAction::DetectRemoteGromacs(id.clone()));
            }
            if ui.button("Test connection").clicked() {
                actions.push(AppAction::CheckRemoteHost(id.clone()));
            }
            if ui.button("Set up passwordless login").clicked() {
                actions.push(AppAction::SetupRemoteHostKey(id.clone()));
            }
            if ui.button("Remove").clicked() {
                actions.push(AppAction::RemoveRemoteHost(id.clone()));
            }
        });

        if let Some(mut command) = bootstrap_cmd {
            ui.label(
                RichText::new(
                    "Run this once on the remote host (paste into a terminal, or type \
                     `! <command>` in this prompt), then Verify:",
                )
                .small()
                .color(pal.text_tertiary),
            );
            ui.add(
                egui::TextEdit::multiline(&mut command)
                    .desired_rows(2)
                    .desired_width(f32::INFINITY)
                    .font(egui::TextStyle::Monospace),
            );
            if ui.button("Verify").clicked() {
                actions.push(AppAction::CheckRemoteHost(id.clone()));
            }
        }
        ui.add_space(8.0);
    }

    // ---- Add a host ------------------------------------------------------
    ui.separator();
    ui.label(RichText::new("Add a remote host").strong());
    let draft = &mut state.ui.settings.new_remote_host;
    remote_host_field(ui, "Label:", &mut draft.label);
    remote_host_field(ui, "Host:", &mut draft.hostname);
    remote_host_field(ui, "User:", &mut draft.username);
    remote_host_field(ui, "Port:", &mut draft.port);
    remote_host_field(ui, "Work dir:", &mut draft.work_root);
    ui.label(
        RichText::new("Setup commands (one per line):")
            .small()
            .color(pal.text_tertiary),
    );
    ui.add(
        egui::TextEdit::multiline(&mut draft.prelude)
            .desired_rows(2)
            .desired_width(f32::INFINITY),
    );
    remote_host_field(
        ui,
        "GROMACS path (optional — use Detect after adding):",
        &mut draft.gmx_program,
    );
    if ui.button("Add host").clicked() {
        actions.push(AppAction::AddRemoteHost);
    }
}

/// A collapsing settings section that is filtered by the search query and
/// forced open whenever a search is active so matches stay visible.
pub(crate) fn settings_section(
    ui: &mut egui::Ui,
    title: &str,
    search: &str,
    add_contents: impl FnOnce(&mut egui::Ui),
) {
    if !search.is_empty() && !title.to_lowercase().contains(search) {
        return;
    }
    let mut header = egui::CollapsingHeader::new(RichText::new(title).strong()).default_open(true);
    if !search.is_empty() {
        header = header.open(Some(true));
    }
    header.show(ui, add_contents);
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
        state.set_message("Editing preview updated".to_string());
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
