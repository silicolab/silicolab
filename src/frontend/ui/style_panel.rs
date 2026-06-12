use eframe::egui::{self, RichText, ScrollArea};

use crate::frontend::{
    LightPreset, SurfaceStyle,
    actions::{AppAction, HydrogenDisplay, VisibilityCommand},
    state::{AppState, AtomStyle},
};

use super::{cartoon_section_controls, overlay_state_for_scope, settings_section};

/// The Style primary view: per-structure appearance for the *selected* atoms.
///
/// Scope rule (mirrored by the dispatcher): every action here applies to the
/// current selection; when nothing is selected the structure is treated as if
/// all atoms were temporarily selected, so a style applies to everything. The
/// panel keeps the Settings panel's collapsing idiom via [`settings_section`].
/// Filtering is driven by the shared sidebar search button.
pub(super) fn render_style_panel(
    state: &mut AppState,
    ui: &mut egui::Ui,
    actions: &mut Vec<AppAction>,
) {
    let search = state.ui.style.search_query.to_lowercase();
    let pal = crate::frontend::theme::palette(ui);
    let selection_len = state.ui.selection.len();

    ScrollArea::vertical()
        .auto_shrink([false, false])
        // Wheel/trackpad plus content drag (touch-friendly); the scroll bar
        // stays a non-interactive position indicator (SCROLL_BAR excluded).
        .scroll_source(
            egui::scroll_area::ScrollSource::MOUSE_WHEEL | egui::scroll_area::ScrollSource::DRAG,
        )
        .show(ui, |ui| {
            scope_banner(ui, &pal, selection_len);

            visibility_section(ui, actions, &search, &pal);
            representation_section(ui, actions, &search, &pal);
            coloring_section(state, ui, &search, &pal);
            labels_section(state, ui, &search, &pal);
            cartoon_section(state, ui, actions, &search, &pal);
            surface_section(state, ui, actions, &search, &pal);
            scene_section(state, ui, &search);
        });
}

/// The banner explaining what the controls below act on.
fn scope_banner(ui: &mut egui::Ui, pal: &crate::frontend::theme::Palette, selection_len: usize) {
    if selection_len == 0 {
        ui.label(
            RichText::new("No atoms selected — styles apply to all atoms").color(pal.text_tertiary),
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
    search: &str,
    pal: &crate::frontend::theme::Palette,
) {
    settings_section(ui, "Visibility", search, |ui| {
        ui.horizontal_wrapped(|ui| {
            if ui.button("Show").clicked() {
                actions.push(AppAction::SetSelectionVisibility(VisibilityCommand::Show));
            }
            if ui.button("Hide").clicked() {
                actions.push(AppAction::SetSelectionVisibility(VisibilityCommand::Hide));
            }
            if ui
                .button("Show only")
                .on_hover_text("Show the scope atoms and hide everything else")
                .clicked()
            {
                actions.push(AppAction::SetSelectionVisibility(
                    VisibilityCommand::ShowOnly,
                ));
            }
        });

        ui.add_space(4.0);
        ui.label(
            RichText::new("Hydrogen atoms")
                .small()
                .color(pal.text_tertiary),
        );
        ui.horizontal_wrapped(|ui| {
            // Polar-only detection isn't implemented yet; reserve the control but
            // keep it disabled so the layout stays stable when it lands.
            ui.add_enabled(false, egui::Button::new("Polar only"))
                .on_disabled_hover_text("Polar-hydrogen detection is not yet implemented");
            if ui.button("Show all H").clicked() {
                actions.push(AppAction::SetHydrogenDisplay(HydrogenDisplay::All));
            }
            if ui.button("Hide all H").clicked() {
                actions.push(AppAction::SetHydrogenDisplay(HydrogenDisplay::None));
            }
        });
    });
}

/// One-click atom/bond representation modes for the scope.
fn representation_section(
    ui: &mut egui::Ui,
    actions: &mut Vec<AppAction>,
    search: &str,
    pal: &crate::frontend::theme::Palette,
) {
    settings_section(ui, "Atom & bond style", search, |ui| {
        ui.horizontal_wrapped(|ui| {
            // Cartoon is an additive overlay (its own section); Hidden is driven
            // by the Visibility section. The rest are mutually-exclusive base
            // representations applied to the scope in one click.
            for style in AtomStyle::all()
                .iter()
                .filter(|style| !matches!(style, AtomStyle::Cartoon | AtomStyle::Hidden))
            {
                if ui.button(style.label()).clicked() {
                    actions.push(AppAction::SetSelectionStyle(*style));
                }
            }
        });
        if ui.button("Reset to default").clicked() {
            actions.push(AppAction::ResetSelectionStyle);
        }
        ui.label(
            RichText::new("Reset clears per-atom style, overlays, and visibility for the scope.")
                .small()
                .color(pal.text_tertiary),
        );
    });
}

/// Coloring — reserved. The color-scheme engine and single-fill application are
/// not wired to the renderer yet, so the controls are placeholders.
fn coloring_section(
    state: &mut AppState,
    ui: &mut egui::Ui,
    search: &str,
    pal: &crate::frontend::theme::Palette,
) {
    settings_section(ui, "Coloring", search, |ui| {
        ui.add_enabled_ui(false, |ui| {
            egui::ComboBox::from_label("Color scheme")
                .selected_text("By element")
                .show_ui(ui, |_ui| {});
        });
        ui.horizontal(|ui| {
            ui.label("Single fill color");
            // Bound to scratch state so the picker works; it does not yet affect
            // rendering (reserved until the coloring engine lands).
            ui.color_edit_button_srgba(&mut state.ui.style.pending_fill_color);
        });
        ui.label(
            RichText::new("Coloring is not yet applied to the view (reserved).")
                .small()
                .color(pal.text_tertiary),
        );
    });
}

/// Labels / tags. Today only the auto-generated atom-type + serial labels exist,
/// toggled globally; custom tags are reserved.
fn labels_section(
    state: &mut AppState,
    ui: &mut egui::Ui,
    search: &str,
    pal: &crate::frontend::theme::Palette,
) {
    settings_section(ui, "Labels", search, |ui| {
        ui.checkbox(
            &mut state.ui.viewport.show_atom_labels,
            "Show atom-type & serial labels",
        );
        ui.add_enabled(false, egui::Button::new("Add custom tag…"))
            .on_disabled_hover_text("Custom tags are not yet implemented");
        ui.label(
            RichText::new("Only the automatic atom-type / serial labels exist today.")
                .small()
                .color(pal.text_tertiary),
        );
    });
}

/// Cartoon ribbon: add/remove the overlay for the scope, plus the ribbon
/// geometry. Color / transparency / associated-atom visibility are reserved.
fn cartoon_section(
    state: &mut AppState,
    ui: &mut egui::Ui,
    actions: &mut Vec<AppAction>,
    search: &str,
    pal: &crate::frontend::theme::Palette,
) {
    let (cartoon_on, _surface_on) = overlay_state_for_scope(state);
    settings_section(ui, "Cartoon", search, |ui| {
        let mut on = cartoon_on;
        if ui.checkbox(&mut on, "Cartoon ribbon for scope").changed() {
            actions.push(AppAction::SetCartoonOverlay(on));
        }

        ui.add_space(4.0);
        ui.label(
            RichText::new("Ribbon geometry")
                .small()
                .color(pal.text_tertiary),
        );
        cartoon_section_controls(ui, "Helix", &mut state.ui.viewport.cartoon.helix);
        cartoon_section_controls(ui, "Sheet", &mut state.ui.viewport.cartoon.sheet);
        cartoon_section_controls(ui, "Coil", &mut state.ui.viewport.cartoon.coil);
        ui.add(
            egui::Slider::new(&mut state.ui.viewport.cartoon.smoothing, 1..=32).text("Smoothing"),
        );
        ui.add(
            egui::Slider::new(&mut state.ui.viewport.cartoon.profile_segments, 6..=48)
                .text("Profile"),
        );

        ui.add_space(4.0);
        ui.label(RichText::new("Reserved").small().color(pal.text_tertiary));
        ui.add_enabled_ui(false, |ui| {
            ui.horizontal(|ui| {
                ui.label("Color");
                let mut reserved = egui::Color32::from_rgb(120, 150, 210);
                ui.color_edit_button_srgba(&mut reserved);
            });
            let mut transparency = 0.0_f32;
            ui.add(egui::Slider::new(&mut transparency, 0.0..=1.0).text("Transparency"));
            let mut show_atoms = true;
            ui.checkbox(&mut show_atoms, "Show associated atoms");
        });
    });
}

/// Surface: first enable generates the molecular surface (the dispatcher seeds
/// its appearance); afterwards the checkbox just shows/hides it. Style and
/// transparency tune the generated surface.
fn surface_section(
    state: &mut AppState,
    ui: &mut egui::Ui,
    actions: &mut Vec<AppAction>,
    search: &str,
    pal: &crate::frontend::theme::Palette,
) {
    let (_cartoon_on, surface_on) = overlay_state_for_scope(state);
    let surface_exists = !state.ui.viewport.surface_overlay.is_empty();
    settings_section(ui, "Surface", search, |ui| {
        if !surface_exists {
            if ui
                .button("Generate surface")
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

        ui.add_space(4.0);
        egui::ComboBox::from_label("Surface style")
            .selected_text(state.ui.viewport.surface.style.label())
            .show_ui(ui, |ui| {
                for style in SurfaceStyle::all() {
                    ui.selectable_value(
                        &mut state.ui.viewport.surface.style,
                        *style,
                        style.label(),
                    );
                }
            });
        ui.add(
            egui::Slider::new(&mut state.ui.viewport.surface.transparency, 0.0..=1.0)
                .text("Transparency"),
        );
        ui.label(
            RichText::new("Surface coloring is reserved.")
                .small()
                .color(pal.text_tertiary),
        );
    });
}

/// Global scene properties that aren't selection-scoped (background, cell, and
/// lighting). Retained from the former Display panel — handy to keep alongside
/// the per-selection style controls.
fn scene_section(state: &mut AppState, ui: &mut egui::Ui, search: &str) {
    settings_section(ui, "Scene", search, |ui| {
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
        ui.checkbox(&mut state.ui.viewport.lighting.silhouettes, "Silhouettes");
        ui.add(
            egui::Slider::new(&mut state.ui.viewport.lighting.silhouette_width, 0.0..=6.0)
                .text("Silhouette width"),
        );
    });
}
