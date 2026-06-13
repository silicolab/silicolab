use eframe::egui::{self, Id, RichText, Sense};

use crate::frontend::{
    LightPreset, SurfaceStyle,
    actions::{AppAction, HydrogenDisplay, VisibilityCommand},
    state::{AppState, AtomStyle},
};

use super::{cartoon_section_controls, docked_sidebar_scroll_area, overlay_state_for_scope};

/// The Style primary view: per-structure appearance for the *selected* atoms.
///
/// Scope rule (mirrored by the dispatcher): every action here applies to the
/// current selection; when nothing is selected the structure is treated as if
/// all atoms were temporarily selected, so a style applies to everything.
/// Filtering is driven by the shared sidebar search button.
pub(super) fn render_style_panel(
    state: &mut AppState,
    ui: &mut egui::Ui,
    actions: &mut Vec<AppAction>,
) {
    let search = state.ui.style.search_query.to_lowercase();
    let pal = crate::frontend::theme::palette(ui);
    let selection_len = state.ui.selection.len();

    docked_sidebar_scroll_area()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            scope_banner(ui, &pal, selection_len);

            style_section(
                ui,
                "Quick",
                &search,
                &[
                    "visibility",
                    "show",
                    "hide",
                    "hydrogen",
                    "atom",
                    "bond",
                    "label",
                    "scene",
                    "background",
                    "unit cell",
                    "light",
                ],
                true,
                |ui| {
                    visibility_section(ui, actions, &pal);
                    representation_section(ui, actions);
                    labels_section(state, ui);
                    scene_quick_section(state, ui);
                },
            );

            style_section(
                ui,
                "Advanced",
                &search,
                &[
                    "cartoon",
                    "ribbon",
                    "helix",
                    "sheet",
                    "coil",
                    "surface",
                    "transparency",
                    "silhouette",
                ],
                false,
                |ui| {
                    cartoon_section(state, ui, actions, &pal);
                    surface_section(state, ui, actions);
                    scene_advanced_section(state, ui);
                },
            );
        });
}

/// The banner explaining what the controls below act on.
fn scope_banner(ui: &mut egui::Ui, pal: &crate::frontend::theme::Palette, selection_len: usize) {
    if selection_len == 0 {
        ui.label(
            RichText::new("No atoms selected - styles apply to all atoms").color(pal.text_muted),
        );
    } else {
        ui.label(
            RichText::new(format!("Acting on {selection_len} selected atom(s)"))
                .color(pal.text_muted),
        );
    }
    ui.add_space(2.0);
}

/// Show/hide, show-only, and per-element hydrogen visibility — all independent
/// of the drawing style (visibility is its own per-atom attribute).
fn visibility_section(
    ui: &mut egui::Ui,
    actions: &mut Vec<AppAction>,
    pal: &crate::frontend::theme::Palette,
) {
    style_group_label(ui, "Visibility", pal);
    ui.horizontal_wrapped(|ui| {
        if style_pill_button(ui, "Show").clicked() {
            actions.push(AppAction::SetSelectionVisibility(VisibilityCommand::Show));
        }
        if style_pill_button(ui, "Hide").clicked() {
            actions.push(AppAction::SetSelectionVisibility(VisibilityCommand::Hide));
        }
        if style_pill_button(ui, "Show only")
            .on_hover_text("Show the scope atoms and hide everything else")
            .clicked()
        {
            actions.push(AppAction::SetSelectionVisibility(
                VisibilityCommand::ShowOnly,
            ));
        }
    });

    ui.add_space(5.0);
    style_group_label(ui, "Hydrogen atoms", pal);
    ui.horizontal_wrapped(|ui| {
        if style_pill_button(ui, "Show all H").clicked() {
            actions.push(AppAction::SetHydrogenDisplay(HydrogenDisplay::All));
        }
        if style_pill_button(ui, "Hide all H").clicked() {
            actions.push(AppAction::SetHydrogenDisplay(HydrogenDisplay::None));
        }
    });
}

/// One-click atom/bond representation modes for the scope.
fn representation_section(ui: &mut egui::Ui, actions: &mut Vec<AppAction>) {
    let pal = crate::frontend::theme::palette(ui);
    style_group_label(ui, "Atom style", &pal);
    ui.horizontal_wrapped(|ui| {
        // Cartoon is an additive overlay (its own section); Hidden is driven
        // by the Visibility section. The rest are mutually-exclusive base
        // representations applied to the scope in one click.
        for style in AtomStyle::all()
            .iter()
            .filter(|style| !matches!(style, AtomStyle::Cartoon | AtomStyle::Hidden))
        {
            if style_pill_button(ui, style.label()).clicked() {
                actions.push(AppAction::SetSelectionStyle(*style));
            }
        }
        if style_pill_button(ui, "Reset").clicked() {
            actions.push(AppAction::ResetSelectionStyle);
        }
    });
}

/// Labels / tags. Today only the auto-generated atom-type + serial labels exist,
/// toggled globally; custom tags are reserved.
fn labels_section(state: &mut AppState, ui: &mut egui::Ui) {
    let pal = crate::frontend::theme::palette(ui);
    style_group_label(ui, "Labels", &pal);
    ui.checkbox(
        &mut state.ui.viewport.show_atom_labels,
        "Show atom-type & serial labels",
    );
}

/// Cartoon ribbon: add/remove the overlay for the scope, plus the ribbon
/// geometry. Color / transparency / associated-atom visibility are reserved.
fn cartoon_section(
    state: &mut AppState,
    ui: &mut egui::Ui,
    actions: &mut Vec<AppAction>,
    pal: &crate::frontend::theme::Palette,
) {
    let (cartoon_on, _surface_on) = overlay_state_for_scope(state);
    style_group_label(ui, "Cartoon overlay", pal);
    let mut on = cartoon_on;
    if ui.checkbox(&mut on, "Cartoon ribbon for scope").changed() {
        actions.push(AppAction::SetCartoonOverlay(on));
    }

    ui.add_space(5.0);
    style_group_label(ui, "Ribbon geometry", pal);
    cartoon_section_controls(ui, "Helix", &mut state.ui.viewport.cartoon.helix);
    cartoon_section_controls(ui, "Sheet", &mut state.ui.viewport.cartoon.sheet);
    cartoon_section_controls(ui, "Coil", &mut state.ui.viewport.cartoon.coil);
    ui.add(egui::Slider::new(&mut state.ui.viewport.cartoon.smoothing, 1..=32).text("Smoothing"));
    ui.add(
        egui::Slider::new(&mut state.ui.viewport.cartoon.profile_segments, 6..=48).text("Profile"),
    );
}

/// Surface: first enable generates the molecular surface (the dispatcher seeds
/// its appearance); afterwards the checkbox just shows/hides it. Style and
/// transparency tune the generated surface.
fn surface_section(state: &mut AppState, ui: &mut egui::Ui, actions: &mut Vec<AppAction>) {
    let pal = crate::frontend::theme::palette(ui);
    let (_cartoon_on, surface_on) = overlay_state_for_scope(state);
    let surface_exists = !state.ui.viewport.surface_overlay.is_empty();
    style_group_label(ui, "Surface", &pal);
    if !surface_exists {
        if style_pill_button(ui, "Generate surface")
            .on_hover_text("Create a molecular surface for the scope")
            .clicked()
        {
            actions.push(AppAction::SetSurfaceOverlay(true));
        }
    } else {
        let mut on = surface_on;
        if ui.checkbox(&mut on, "Show surface for scope").changed() {
            actions.push(AppAction::SetSurfaceOverlay(on));
        }
    }

    ui.add_space(5.0);
    egui::ComboBox::from_label("Surface style")
        .selected_text(state.ui.viewport.surface.style.label())
        .show_ui(ui, |ui| {
            for style in SurfaceStyle::all() {
                ui.selectable_value(&mut state.ui.viewport.surface.style, *style, style.label());
            }
        });
    ui.add(
        egui::Slider::new(&mut state.ui.viewport.surface.transparency, 0.0..=1.0)
            .text("Transparency"),
    );
}

/// Global scene properties that aren't selection-scoped (background, cell, and
/// lighting). Retained from the former Display panel — handy to keep alongside
/// the per-selection style controls.
fn scene_quick_section(state: &mut AppState, ui: &mut egui::Ui) {
    let pal = crate::frontend::theme::palette(ui);
    style_group_label(ui, "Scene", &pal);
    ui.horizontal(|ui| {
        ui.label("Background");
        ui.color_edit_button_srgba(&mut state.ui.viewport.background_color);
    });
    ui.checkbox(&mut state.ui.viewport.show_cell, "Show unit cell");
    egui::ComboBox::from_label("Light")
        .selected_text(state.ui.viewport.lighting.preset.label())
        .show_ui(ui, |ui| {
            for preset in LightPreset::all() {
                ui.selectable_value(
                    &mut state.ui.viewport.lighting.preset,
                    *preset,
                    preset.label(),
                );
            }
        });
}

fn scene_advanced_section(state: &mut AppState, ui: &mut egui::Ui) {
    let pal = crate::frontend::theme::palette(ui);
    style_group_label(ui, "Silhouettes", &pal);
    ui.checkbox(&mut state.ui.viewport.lighting.silhouettes, "Silhouettes");
    ui.add(
        egui::Slider::new(&mut state.ui.viewport.lighting.silhouette_width, 0.0..=6.0)
            .text("Silhouette width"),
    );
}

fn style_section(
    ui: &mut egui::Ui,
    title: &str,
    search: &str,
    keywords: &[&str],
    default_open: bool,
    add_contents: impl FnOnce(&mut egui::Ui),
) {
    if !search.is_empty()
        && !title.to_lowercase().contains(search)
        && !keywords.iter().any(|keyword| keyword.contains(search))
    {
        return;
    }

    let id = ui.make_persistent_id(("style_section", title));
    let force_open = !search.is_empty();
    let mut open = ui.data_mut(|data| data.get_temp::<bool>(id).unwrap_or(default_open));
    if force_open {
        open = true;
    }

    let response = style_section_header(ui, title, open);
    if response.clicked() && !force_open {
        open = !open;
    }
    if !force_open {
        ui.data_mut(|data| data.insert_temp(id, open));
    }

    if open {
        ui.indent(Id::new(("style_section_body", title)), |ui| {
            ui.add_space(2.0);
            add_contents(ui);
        });
    }
    ui.add_space(8.0);
}

fn style_section_header(ui: &mut egui::Ui, title: &str, open: bool) -> egui::Response {
    let pal = crate::frontend::theme::palette(ui);
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

    let marker = if open {
        egui_phosphor::regular::CARET_DOWN
    } else {
        egui_phosphor::regular::CARET_RIGHT
    };
    let y = rect.center().y;
    ui.painter().text(
        egui::pos2(rect.left() + 8.0, y),
        egui::Align2::LEFT_CENTER,
        marker,
        egui::FontId::proportional(12.0),
        pal.text_tertiary,
    );
    ui.painter().text(
        egui::pos2(rect.left() + 25.0, y),
        egui::Align2::LEFT_CENTER,
        title.to_uppercase(),
        egui::FontId::proportional(13.0),
        pal.text_muted,
    );

    response
}

fn style_group_label(ui: &mut egui::Ui, label: &str, pal: &crate::frontend::theme::Palette) {
    ui.add_space(2.0);
    ui.label(RichText::new(label).size(12.0).color(pal.text_muted));
}

fn style_pill_button(ui: &mut egui::Ui, label: &str) -> egui::Response {
    ui.add(
        egui::Button::new(RichText::new(label).size(12.5))
            .corner_radius(egui::CornerRadius::same(
                crate::frontend::theme::radius::CONTROL,
            ))
            .min_size(egui::vec2(0.0, 24.0)),
    )
}
