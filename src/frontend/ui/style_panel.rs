use eframe::egui::{self, Id, RichText, Sense};

use crate::frontend::{
    LightPreset, SurfaceStyle,
    actions::{AppAction, HydrogenDisplay, VisibilityCommand},
    state::{AppState, AtomStyle},
};

use super::{
    cartoon_section_controls, configure_core_button_visuals, core_button_text_color,
    docked_sidebar_scroll_area, overlay_state_for_scope, scope_base_style, toggle_switch,
};

/// The Style primary view. Every action applies to the current selection; with
/// nothing selected the whole structure is the scope (mirrored by the dispatcher).
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
                    "representation",
                    "style",
                    "atom",
                    "bond",
                    "cartoon",
                    "surface",
                    "overlay",
                    "visibility",
                    "show",
                    "hide",
                    "isolate",
                    "hydrogen",
                ],
                true,
                |ui| {
                    representation_section(state, ui, actions, &pal);
                    ui.add_space(6.0);
                    overlays_section(state, ui, actions, &pal);
                    ui.add_space(6.0);
                    visibility_section(ui, actions, &pal);
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
                    "scene",
                    "background",
                    "unit cell",
                    "cell",
                    "atom labels",
                    "light",
                    "silhouette",
                ],
                false,
                |ui| {
                    cartoon_section(state, ui, &pal);
                    ui.add_space(6.0);
                    surface_section(state, ui, &pal);
                    ui.add_space(6.0);
                    scene_section(state, ui, &pal);
                    ui.add_space(6.0);
                    scene_advanced_section(state, ui, &pal);
                },
            );
        });
}

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

/// Mutually-exclusive base representations as a wrapping row of icon buttons; the
/// one applied uniformly across the scope reads as selected.
fn representation_section(
    state: &mut AppState,
    ui: &mut egui::Ui,
    actions: &mut Vec<AppAction>,
    pal: &crate::frontend::theme::Palette,
) {
    let scope_base = scope_base_style(state);

    style_group_label(ui, "Representation", pal);
    ui.horizontal_wrapped(|ui| {
        // Cartoon/Surface are overlay switches below; Hidden is the Visibility section.
        for style in AtomStyle::all()
            .iter()
            .filter(|style| !matches!(style, AtomStyle::Cartoon | AtomStyle::Hidden))
        {
            let selected = scope_base == Some(*style);
            if icon_action_button(ui, atom_style_icon(*style), style.label(), selected, pal)
                .clicked()
            {
                actions.push(AppAction::SetSelectionStyle(*style));
            }
        }

        ui.add_space(8.0);
        if icon_action_button(
            ui,
            egui_phosphor::regular::ARROW_COUNTER_CLOCKWISE,
            "Reset to default style",
            false,
            pal,
        )
        .clicked()
        {
            actions.push(AppAction::ResetSelectionStyle);
        }
    });
}

/// Additive overlays as sliding switches. Enabling Surface builds it the first
/// time; geometry/style/transparency are tuned under Advanced.
fn overlays_section(
    state: &mut AppState,
    ui: &mut egui::Ui,
    actions: &mut Vec<AppAction>,
    pal: &crate::frontend::theme::Palette,
) {
    let (cartoon_on, surface_on) = overlay_state_for_scope(state);
    let surface_exists = !state.ui.viewport.surface_overlay.is_empty();

    style_group_label(ui, "Overlays", pal);
    ui.horizontal(|ui| {
        let mut on = cartoon_on;
        if toggle_switch(ui, &mut on, "Cartoon ribbon", pal)
            .on_hover_text("Backbone ribbon for protein / nucleic-acid chains in the scope")
            .changed()
        {
            actions.push(AppAction::SetCartoonOverlay(on));
        }
    });
    ui.horizontal(|ui| {
        let mut on = surface_on;
        let hint = if surface_exists {
            "Show the molecular surface for the scope"
        } else {
            "Generate a molecular surface for the scope (first enable builds it)"
        };
        if toggle_switch(ui, &mut on, "Surface", pal)
            .on_hover_text(hint)
            .changed()
        {
            actions.push(AppAction::SetSurfaceOverlay(on));
        }
    });
}

fn visibility_section(
    ui: &mut egui::Ui,
    actions: &mut Vec<AppAction>,
    pal: &crate::frontend::theme::Palette,
) {
    use egui_phosphor::regular;

    style_group_label(ui, "Visibility", pal);
    ui.horizontal_wrapped(|ui| {
        if icon_action_button(ui, regular::EYE, "Show", false, pal).clicked() {
            actions.push(AppAction::SetSelectionVisibility(VisibilityCommand::Show));
        }
        if icon_action_button(ui, regular::EYE_SLASH, "Hide", false, pal).clicked() {
            actions.push(AppAction::SetSelectionVisibility(VisibilityCommand::Hide));
        }
        if icon_action_button(
            ui,
            regular::TARGET,
            "Isolate (show only the scope, hide the rest)",
            false,
            pal,
        )
        .clicked()
        {
            actions.push(AppAction::SetSelectionVisibility(
                VisibilityCommand::ShowOnly,
            ));
        }
    });

    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new("Hydrogens").size(12.0).color(pal.text_muted));
        ui.add_space(2.0);
        if icon_action_button(ui, regular::EYE, "Show all hydrogens", false, pal).clicked() {
            actions.push(AppAction::SetHydrogenDisplay(HydrogenDisplay::All));
        }
        if icon_action_button(ui, regular::EYE_SLASH, "Hide all hydrogens", false, pal).clicked() {
            actions.push(AppAction::SetHydrogenDisplay(HydrogenDisplay::None));
        }
    });
}

fn scene_section(state: &mut AppState, ui: &mut egui::Ui, pal: &crate::frontend::theme::Palette) {
    style_group_label(ui, "Scene", pal);
    ui.horizontal(|ui| {
        ui.label("Background");
        ui.color_edit_button_srgba(&mut state.ui.viewport.background_color);
    });
    ui.horizontal(|ui| {
        toggle_switch(ui, &mut state.ui.viewport.show_cell, "Unit cell", pal);
    });
    ui.horizontal(|ui| {
        toggle_switch(
            ui,
            &mut state.ui.viewport.show_atom_labels,
            "Atom labels",
            pal,
        );
    });
    egui::ComboBox::from_label("Light")
        .selected_text(state.ui.viewport.lighting.preset.label())
        .show_ui(ui, |ui| {
            crate::frontend::theme::stabilize_selectable_rows(ui);
            for preset in LightPreset::all() {
                ui.selectable_value(
                    &mut state.ui.viewport.lighting.preset,
                    *preset,
                    preset.label(),
                );
            }
        });
}

fn cartoon_section(state: &mut AppState, ui: &mut egui::Ui, pal: &crate::frontend::theme::Palette) {
    style_group_label(ui, "Cartoon ribbon", pal);
    cartoon_section_controls(ui, "Helix", &mut state.ui.viewport.cartoon.helix);
    cartoon_section_controls(ui, "Sheet", &mut state.ui.viewport.cartoon.sheet);
    cartoon_section_controls(ui, "Coil", &mut state.ui.viewport.cartoon.coil);
    ui.add(egui::Slider::new(&mut state.ui.viewport.cartoon.smoothing, 1..=32).text("Smoothing"));
    ui.add(
        egui::Slider::new(&mut state.ui.viewport.cartoon.profile_segments, 6..=48).text("Profile"),
    );
}

fn surface_section(state: &mut AppState, ui: &mut egui::Ui, pal: &crate::frontend::theme::Palette) {
    style_group_label(ui, "Surface", pal);
    egui::ComboBox::from_label("Surface style")
        .selected_text(state.ui.viewport.surface.style.label())
        .show_ui(ui, |ui| {
            crate::frontend::theme::stabilize_selectable_rows(ui);
            for style in SurfaceStyle::all() {
                ui.selectable_value(&mut state.ui.viewport.surface.style, *style, style.label());
            }
        });
    ui.add(
        egui::Slider::new(&mut state.ui.viewport.surface.transparency, 0.0..=1.0)
            .text("Transparency"),
    );
}

fn scene_advanced_section(
    state: &mut AppState,
    ui: &mut egui::Ui,
    pal: &crate::frontend::theme::Palette,
) {
    style_group_label(ui, "Silhouettes", pal);
    ui.horizontal(|ui| {
        toggle_switch(
            ui,
            &mut state.ui.viewport.lighting.silhouettes,
            "Silhouettes",
            pal,
        );
    });
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

/// The `Cartoon | Hidden` arm is never reached (those are filtered out of the
/// row); it only keeps the match exhaustive.
fn atom_style_icon(style: AtomStyle) -> &'static str {
    use egui_phosphor::regular;
    match style {
        AtomStyle::BallAndStick => regular::POLYGON,
        AtomStyle::Stick => regular::CYLINDER,
        AtomStyle::Wireframe => regular::HEXAGON,
        AtomStyle::Sphere => regular::SPHERE,
        AtomStyle::Point => regular::DOTS_NINE,
        AtomStyle::Cartoon | AtomStyle::Hidden => regular::CIRCLE,
    }
}

/// Configures the core-button visuals on the current `ui` (then restores) rather
/// than via `with_core_button_style`'s child scope: a scoped child is placed
/// without a wrap check, so in a `horizontal_wrapped` row it would overflow and
/// clip a narrow panel instead of wrapping.
fn icon_action_button(
    ui: &mut egui::Ui,
    icon: &str,
    tooltip: &str,
    selected: bool,
    pal: &crate::frontend::theme::Palette,
) -> egui::Response {
    let prev_widgets = ui.style().visuals.widgets.clone();
    configure_core_button_visuals(ui, selected);
    let response = ui
        .add(
            egui::Button::new(
                RichText::new(icon)
                    .size(17.0)
                    .color(core_button_text_color(pal, selected)),
            )
            .min_size(egui::vec2(36.0, 28.0)),
        )
        .on_hover_text(tooltip);
    ui.style_mut().visuals.widgets = prev_widgets;
    response
}
