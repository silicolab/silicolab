use super::*;
use crate::frontend::state::SelfUpdateStatus;

/// Title-bar buttons (settings gear, sidebar toggle, window controls) sit
/// `TITLE_BAR_H_MARGIN` from the window's corner, so their hover rect derives
/// concentrically from the window radius (floored at `radius::MIN`). This keeps the gear's
/// curve visually nested inside the native (or self-drawn) window corner.
pub(crate) const CORE_BUTTON_CORNER_RADIUS: u8 = crate::frontend::theme::radius::concentric(
    crate::frontend::theme::radius::WINDOW,
    TITLE_BAR_H_MARGIN,
);

pub(crate) const SIDEBAR_HEADER_BUTTON_SIZE: [f32; 2] = [32.0, 28.0];

pub(crate) const SIDEBAR_HEADER_ICON_SIZE: f32 = 17.0;

pub(crate) fn render_title_bar(
    state: &mut AppState,
    ui: &mut egui::Ui,
    actions: &mut Vec<AppAction>,
) {
    let ctx = ui.ctx().clone();
    let maximized = ctx.input(|input| input.viewport().maximized.unwrap_or(false));
    let show_inline_menus = !cfg!(target_os = "macos");
    let has_active_entry = state.has_active_entry();
    let pal = crate::frontend::theme::palette(ui);
    let title_color = pal.text_primary;
    let muted_text = pal.text_muted;
    let centered_title = state.workspace_label();
    let title_bar_rect = ui.max_rect();
    // Edges of the leading and trailing (non-draggable) clusters, measured
    // after the controls render so the drag strips can never overlap a menu
    // button, the settings gear, or a window control.
    let mut left_cluster_right = title_bar_rect.left();
    let mut right_cluster_left = title_bar_rect.right();

    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        // Omitted on macOS, where the native traffic-light buttons sit here.
        #[cfg(not(target_os = "macos"))]
        ui.label(
            RichText::new("SilicoLab")
                .strong()
                .size(14.0)
                .color(title_color),
        );

        // Sidebar toggle, Claude Desktop convention: while the sidebar is
        // visible its own header strip carries the hide button, so the title
        // bar stays clean; when hidden, the title bar spans the full width and
        // carries the only way to bring the sidebar back — right of the macOS
        // traffic lights, which the spacer clears.
        if !state.ui.layout.show_primary_sidebar {
            #[cfg(target_os = "macos")]
            ui.add_space(MACOS_TRAFFIC_LIGHTS_WIDTH - 8.0);
            if with_core_button_style(ui, false, |ui| {
                ui.add_sized(
                    [28.0, 24.0],
                    Button::new(
                        RichText::new(egui_phosphor::regular::SIDEBAR_SIMPLE)
                            .color(core_button_text_color(&pal, false)),
                    ),
                )
            })
            .on_hover_text("Show sidebar")
            .clicked()
            {
                actions.push(AppAction::TogglePrimarySidebar);
            }
        }

        if show_inline_menus {
            with_core_button_style(ui, false, |ui| {
                ui.menu_button(RichText::new("File").color(title_color), |ui| {
                    if ui
                        .button(crate::frontend::shortcuts::menu_text(
                            "file.new_project",
                            "Create a new project...",
                        ))
                        .clicked()
                    {
                        actions.push(AppAction::CreateProject);
                        ui.close();
                    }
                    if ui
                        .button(crate::frontend::shortcuts::menu_text(
                            "file.open_project",
                            "Open Project...",
                        ))
                        .clicked()
                    {
                        actions.push(AppAction::OpenProject);
                        ui.close();
                    }
                    if ui
                        .button(crate::frontend::shortcuts::menu_text(
                            "file.save_project",
                            "Save Project",
                        ))
                        .clicked()
                    {
                        actions.push(AppAction::SaveProject);
                        ui.close();
                    }
                    if ui
                        .add_enabled(state.workspace.is_project(), Button::new("Close Project"))
                        .clicked()
                    {
                        actions.push(AppAction::CloseProject);
                        ui.close();
                    }
                    if !state.recent_projects.is_empty() {
                        ui.separator();
                        ui.menu_button("Recent Projects", |ui| {
                            for project in state.recent_projects.clone() {
                                if ui
                                    .button(format!("{}\n{}", project.name, project.path.display()))
                                    .clicked()
                                {
                                    actions.push(AppAction::OpenRecentProject(project.path));
                                    ui.close();
                                }
                            }
                        });
                    }
                    ui.separator();
                    if ui.button("New Empty Entry").clicked() {
                        actions.push(AppAction::NewEmptyEntry);
                        ui.close();
                    }
                    if ui.button("Sketch Molecule...").clicked() {
                        actions.push(AppAction::SketchMolecule);
                        ui.close();
                    }
                    if ui
                        .button(crate::frontend::shortcuts::menu_text(
                            "file.open_file",
                            "Open File...",
                        ))
                        .clicked()
                    {
                        actions.push(AppAction::OpenFile);
                        ui.close();
                    }
                    if ui.button("Fetch from PDB ID...").clicked() {
                        actions.push(AppAction::OpenPdbFetchDialog);
                        ui.close();
                    }
                    ui.separator();
                    if ui
                        .button(crate::frontend::shortcuts::menu_text(
                            "file.export",
                            "Export...",
                        ))
                        .clicked()
                    {
                        actions.push(AppAction::OpenExportDialog { entry_id: None });
                        ui.close();
                    }
                    ui.separator();
                    if ui
                        .button(crate::frontend::shortcuts::menu_text(
                            "app.settings",
                            "Settings...",
                        ))
                        .clicked()
                    {
                        state.ui.layout.settings_open = true;
                        ui.close();
                    }
                });
            });

            with_core_button_style(ui, false, |ui| {
                ui.menu_button(RichText::new("Edit").color(title_color), |ui| {
                    if ui
                        .add_enabled(
                            state.can_undo(),
                            Button::new(crate::frontend::shortcuts::menu_text("edit.undo", "Undo")),
                        )
                        .clicked()
                    {
                        actions.push(AppAction::Undo);
                        ui.close();
                    }
                    if ui
                        .add_enabled(
                            state.can_redo(),
                            Button::new(format!(
                                "{} / {}",
                                crate::frontend::shortcuts::menu_text("edit.redo", "Redo"),
                                crate::frontend::shortcuts::label_for("edit.redo_alt")
                                    .unwrap_or_else(|| "Ctrl+Shift+Z".to_string())
                            )),
                        )
                        .clicked()
                    {
                        actions.push(AppAction::Redo);
                        ui.close();
                    }
                    ui.separator();
                    if ui
                        .add_enabled(has_active_entry, Button::new("Edit Structure..."))
                        .clicked()
                    {
                        actions.push(AppAction::EditStructure);
                        ui.close();
                    }
                });
            });

            with_core_button_style(ui, false, |ui| {
                ui.menu_button(RichText::new("Selection").color(title_color), |ui| {
                    if ui.button("Select All").clicked() {
                        actions.push(AppAction::SelectAll);
                        ui.close();
                    }
                    if ui.button("Invert Selection").clicked() {
                        actions.push(AppAction::InvertSelection);
                        ui.close();
                    }
                    if ui.button("Clear Selection").clicked() {
                        actions.push(AppAction::ClearSelection);
                        ui.close();
                    }
                    ui.separator();
                    ui.label(
                        RichText::new("Select by type")
                            .small()
                            .color(pal.text_tertiary),
                    );
                    for category in crate::domain::AtomCategory::selectable() {
                        if ui.button(category.label()).clicked() {
                            actions.push(AppAction::SelectCategory(*category));
                            ui.close();
                        }
                    }
                });
            });

            with_core_button_style(ui, false, |ui| {
                ui.menu_button(RichText::new("View").color(title_color), |ui| {
                    let mut primary_visible = state.ui.layout.show_primary_sidebar;
                    if ui
                        .checkbox(
                            &mut primary_visible,
                            crate::frontend::shortcuts::menu_text(
                                "view.primary_sidebar",
                                "Primary Side Bar",
                            ),
                        )
                        .changed()
                    {
                        actions.push(AppAction::TogglePrimarySidebar);
                    }
                    // The dock areas' visibility is derived (a checkbox reflects
                    // whether the area is shown); the toggle routes through an
                    // action so revealing an empty area restores a default view.
                    let mut right_visible = state.ui.layout.dock.is_visible(DockArea::Right);
                    if ui
                        .checkbox(
                            &mut right_visible,
                            crate::frontend::shortcuts::menu_text(
                                "view.secondary_sidebar",
                                "Secondary Side Bar",
                            ),
                        )
                        .changed()
                    {
                        actions.push(AppAction::ToggleDockArea(DockArea::Right));
                    }
                    let mut bottom_visible = state.ui.layout.dock.is_visible(DockArea::Bottom);
                    if ui
                        .checkbox(
                            &mut bottom_visible,
                            crate::frontend::shortcuts::menu_text("view.panel", "Panel"),
                        )
                        .changed()
                    {
                        actions.push(AppAction::ToggleDockArea(DockArea::Bottom));
                    }
                    let mut show_atom_labels = state.ui.viewport.show_atom_labels;
                    if ui
                        .checkbox(
                            &mut show_atom_labels,
                            crate::frontend::shortcuts::menu_text(
                                "style.atom_labels",
                                "Show Atom Labels",
                            ),
                        )
                        .changed()
                    {
                        actions.push(AppAction::ToggleAtomLabels);
                    }
                    ui.separator();
                    if ui
                        .button(crate::frontend::shortcuts::menu_text(
                            "view.reset_layout",
                            "Reset Workbench Layout",
                        ))
                        .clicked()
                    {
                        actions.push(AppAction::ResetWorkbenchLayout);
                        ui.close();
                    }
                    ui.separator();
                    ui.menu_button("Appearance", |ui| {
                        let current = state.config.theme;
                        for mode in crate::backend::config::ThemeMode::all() {
                            if ui.radio(current == mode, mode.label()).clicked() {
                                actions.push(AppAction::SetThemeMode(mode));
                                ui.close();
                            }
                        }
                        ui.separator();
                        let scheme = state.config.color_scheme;
                        for option in crate::backend::config::ColorScheme::all() {
                            if ui.radio(scheme == option, option.label()).clicked() {
                                actions.push(AppAction::SetColorScheme(option));
                                ui.close();
                            }
                        }
                    });
                });
                ui.menu_button(RichText::new("Help").color(title_color), |ui| {
                    if ui.button("About SilicoLab").clicked() {
                        state.ui.layout.about_open = true;
                        ui.close();
                    }
                });
            });

            // The former "Style" menu lived here; its controls now belong to the
            // Style primary-view panel (sidebar), which scopes them to the
            // selection and offers the full visibility / coloring / cartoon /
            // surface modules. Removed to avoid duplicating that surface.
        }

        left_cluster_right = ui.cursor().min.x;

        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if !cfg!(target_os = "macos") {
                render_window_controls(ui, maximized);
            }
            // Settings gear (common editor convention): right_to_left adds it
            // AFTER the window controls, so it lands to their LEFT — clear of
            // minimize/maximize/close. On macOS (no window controls here) it is
            // the right-most item. Sized to match the sidebar-toggle button.
            let settings_hint = crate::frontend::shortcuts::label_for("app.settings")
                .map(|shortcut| format!("Settings ({shortcut})"))
                .unwrap_or_else(|| "Settings".to_string());
            let settings_hint = match &state.ui.available_update {
                Some(update) => {
                    format!("{settings_hint} — update available: {}", update.version)
                }
                None => settings_hint.to_string(),
            };
            let settings_response = with_core_button_style(ui, false, |ui| {
                ui.add_sized(
                    [28.0, 24.0],
                    Button::new(
                        RichText::new(egui_phosphor::regular::GEAR)
                            .color(core_button_text_color(&pal, false)),
                    ),
                )
            })
            .on_hover_text(settings_hint);
            if settings_response.clicked() {
                state.ui.layout.settings_open = true;
            }
            // Update-available badge: a red dot on the gear's top-right corner,
            // so the notice can't be missed. Painted over the button (not a
            // widget), so it never affects layout or hit-testing.
            if state.ui.available_update.is_some() {
                let center = settings_response.rect.right_top() + egui::vec2(-5.0, 5.0);
                ui.painter().circle_filled(center, 3.0, pal.status_red);
            }
            // The update affordance sits directly left of the gear (this layout
            // is right-to-left), so the dot and its action read as one cluster.
            // Its form follows the self-update lifecycle: a one-click "Update"
            // button while idle, a spinner while downloading, a "Restart" button
            // once installed, and the plain releases link when an in-place
            // update isn't possible (portable / read-only install) or failed.
            let update_info = state
                .ui
                .available_update
                .as_ref()
                .map(|update| (update.version.clone(), update.url.clone()));
            match state.ui.self_update.clone() {
                SelfUpdateStatus::Installed { version } => {
                    let response = ui
                        .add(Button::new(
                            RichText::new(format!(
                                "{}  Restart to update",
                                egui_phosphor::regular::ARROW_CLOCKWISE
                            ))
                            .color(pal.status_blue),
                        ))
                        .on_hover_text(format!("Restart into SilicoLab {version}"));
                    if response.clicked()
                        && let Err(error) = crate::io::self_update::restart_into_new_binary()
                    {
                        state.set_message(format!("Restart failed: {error}"));
                    }
                }
                SelfUpdateStatus::Downloading => {
                    ui.add(egui::Spinner::new().size(14.0));
                    ui.label(RichText::new("Updating…").color(muted_text));
                }
                status @ (SelfUpdateStatus::Idle | SelfUpdateStatus::Failed { .. }) => {
                    if let Some((version, url)) = update_info {
                        let failed = matches!(status, SelfUpdateStatus::Failed { .. });
                        let label = RichText::new(format!(
                            "{}  Update: {version}",
                            egui_phosphor::regular::ARROW_CIRCLE_UP
                        ))
                        .color(pal.status_blue);
                        // Offer one-click only when the install is writable and
                        // the last attempt didn't fail; otherwise fall back to
                        // the release page so the user can update manually.
                        if !failed && crate::io::self_update::is_self_update_supported() {
                            let response = ui
                                .add(Button::new(label))
                                .on_hover_text("Download and install this update");
                            if response.clicked() {
                                state.ui.self_update = SelfUpdateStatus::Downloading;
                                state.jobs.self_update =
                                    Some(crate::frontend::jobs::spawn_self_update());
                                state.set_message(format!("Downloading SilicoLab {version}…"));
                            }
                        } else {
                            let hover = match &status {
                                SelfUpdateStatus::Failed { error } => {
                                    format!("Update failed ({error}) — open the release page")
                                }
                                _ => "Open the release page".to_string(),
                            };
                            ui.hyperlink_to(label, &url).on_hover_text(hover);
                        }
                    }
                }
            }

            // In a right-to-left layout the cursor's max.x is where the next
            // widget would land — i.e. the left edge of everything rendered.
            right_cluster_left = ui.cursor().max.x;
        });
    });

    // Width of the non-draggable leading cluster, measured from the title bar's
    // own left edge after the menus rendered. Measuring (rather than a fixed
    // constant) guarantees the drag strip never overlaps the last menu button,
    // which previously made the "Style" menu hard to click.
    let left_reserved_width = left_cluster_right - title_bar_rect.left();
    // The right cluster (window controls plus the settings gear), measured the
    // same way so a drag never lands on the gear or a window button.
    let right_reserved_width = title_bar_rect.right() - right_cluster_left;
    let center_width = (title_bar_rect.width() - left_reserved_width - right_reserved_width - 16.0)
        .clamp(96.0, 320.0);
    let center_drag_rect = Rect::from_center_size(
        title_bar_rect.center(),
        egui::vec2(center_width, title_bar_rect.height() - 6.0),
    );
    let drag_strip_top = title_bar_rect.top() + 2.0;
    let drag_strip_bottom = title_bar_rect.bottom() - 2.0;
    let left_drag_edge = title_bar_rect.left() + left_reserved_width;
    let left_drag_rect = Rect::from_min_max(
        egui::pos2(left_drag_edge, drag_strip_top),
        egui::pos2(
            center_drag_rect.left().max(left_drag_edge),
            drag_strip_bottom,
        ),
    );
    let right_drag_rect = Rect::from_min_max(
        egui::pos2(
            center_drag_rect
                .right()
                .min(title_bar_rect.right() - right_reserved_width),
            drag_strip_top,
        ),
        egui::pos2(
            title_bar_rect.right() - right_reserved_width,
            drag_strip_bottom,
        ),
    );

    let center_drag_response = ui.interact(
        center_drag_rect,
        Id::new("title_bar_drag_area_center"),
        Sense::click_and_drag(),
    );
    let left_drag_response = ui.interact(
        left_drag_rect,
        Id::new("title_bar_drag_area_left"),
        Sense::click_and_drag(),
    );
    let right_drag_response = ui.interact(
        right_drag_rect,
        Id::new("title_bar_drag_area_right"),
        Sense::click_and_drag(),
    );
    ui.painter().text(
        center_drag_rect.center(),
        egui::Align2::CENTER_CENTER,
        centered_title,
        egui::FontId::proportional(14.0),
        muted_text,
    );
    if center_drag_response.drag_started()
        || left_drag_response.drag_started()
        || right_drag_response.drag_started()
    {
        ctx.send_viewport_cmd(ViewportCommand::StartDrag);
    }
    if (center_drag_response.double_clicked()
        || left_drag_response.double_clicked()
        || right_drag_response.double_clicked())
        && !cfg!(target_os = "macos")
    {
        ctx.send_viewport_cmd(ViewportCommand::Maximized(!maximized));
    }

    fn render_window_controls(ui: &mut Ui, maximized: bool) {
        let ctx = ui.ctx().clone();
        let pal = crate::frontend::theme::palette(ui);
        for (icon, command, hover_fill) in [
            (
                egui_phosphor::regular::X,
                ViewportCommand::Close,
                pal.status_red,
            ),
            (
                if maximized {
                    egui_phosphor::regular::CORNERS_IN
                } else {
                    egui_phosphor::regular::CORNERS_OUT
                },
                ViewportCommand::Maximized(!maximized),
                pal.item_fill_hover,
            ),
            (
                egui_phosphor::regular::MINUS,
                ViewportCommand::Minimized(true),
                pal.item_fill_hover,
            ),
        ] {
            let response = window_control_button(ui, icon, hover_fill);
            if response.clicked() {
                ctx.send_viewport_cmd(command);
            }
        }
    }
}

/// Height of the full-height sidebar's header strip, matched to the title bar's
/// 32px so the chrome band reads as one continuous row across the
/// sidebar/toolbar boundary (macOS 27 edge-to-edge sidebar).
pub(crate) const SIDEBAR_HEADER_HEIGHT: f32 = 32.0;

/// Width reserved at the sidebar's top-left for the native macOS traffic
/// lights, which overlay the full-height sidebar. The title bar's own spacer
/// (used when the sidebar is hidden) is this minus its 8px inner margin.
#[cfg(target_os = "macos")]
pub(crate) const MACOS_TRAFFIC_LIGHTS_WIDTH: f32 = 72.0;
