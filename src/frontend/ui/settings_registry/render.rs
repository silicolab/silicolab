//! Rendering the settings registry: turning [`SettingDescriptor`]s (assembled by
//! [`super::catalog::registry`]) into widgets that emit `AppAction`s, plus the
//! category-visibility query that drives the modal's left rail.

use super::*;

use eframe::egui::{self, RichText};

use crate::frontend::{actions::AppAction, state::AppState};

/// The categories that currently have at least one setting matching `search`
/// (already lower-cased). With an empty search, every category that owns a
/// setting. Drives the modal's left rail — categories with nothing to show
/// under the active filter are hidden.
pub fn visible_categories(search: &str) -> Vec<SettingCategory> {
    let registry = registry();
    CATEGORY_ORDER
        .into_iter()
        .filter(|category| {
            registry
                .iter()
                .any(|descriptor| descriptor.category == *category && descriptor.matches(search))
        })
        .collect()
}

/// Render one category's groups and settings (the selected-category view, shown
/// while the search box is empty). Groups appear in first-appearance order as
/// open collapsing sections; any nested [`Subgroup`] renders as its own
/// collapser below the group's direct settings.
pub fn render_category(
    state: &mut AppState,
    ui: &mut egui::Ui,
    actions: &mut Vec<AppAction>,
    category: SettingCategory,
) {
    let registry = registry();
    let pal = crate::frontend::theme::palette(ui);

    let mut groups: Vec<&'static str> = Vec::new();
    for descriptor in &registry {
        if descriptor.category == category && !groups.contains(&descriptor.group) {
            groups.push(descriptor.group);
        }
    }

    for group in groups {
        let matching: Vec<&SettingDescriptor> = registry
            .iter()
            .filter(|descriptor| descriptor.category == category && descriptor.group == group)
            .collect();

        egui::CollapsingHeader::new(RichText::new(group).strong())
            .default_open(true)
            .show(ui, |ui| {
                // Settings directly under the group header first.
                for descriptor in matching.iter().filter(|d| d.subgroup.is_none()) {
                    render_descriptor(descriptor, state, ui, actions, &pal);
                }
                // Then each subgroup, in first-appearance order.
                let mut seen: Vec<&'static str> = Vec::new();
                for descriptor in &matching {
                    let Some(subgroup) = descriptor.subgroup else {
                        continue;
                    };
                    if seen.contains(&subgroup.title) {
                        continue;
                    }
                    seen.push(subgroup.title);
                    let members = matching
                        .iter()
                        .filter(|d| d.subgroup.map(|s| s.title) == Some(subgroup.title));
                    egui::CollapsingHeader::new(RichText::new(subgroup.title))
                        .id_salt((group, subgroup.title))
                        .default_open(subgroup.default_open)
                        .show(ui, |ui| {
                            for descriptor in members {
                                render_descriptor(descriptor, state, ui, actions, &pal);
                            }
                        });
                }
            });
    }
}

/// Render a flat list of every setting matching `search` (non-empty,
/// lower-cased) across all categories, with category labels as section
/// separators (a flat cross-category search — the selected category is ignored).
/// Subgroups are flattened here: search is already a flat view.
pub fn render_search_results(
    state: &mut AppState,
    ui: &mut egui::Ui,
    actions: &mut Vec<AppAction>,
    search: &str,
) {
    let registry = registry();
    let pal = crate::frontend::theme::palette(ui);

    let mut first = true;
    for category in CATEGORY_ORDER {
        let matching: Vec<&SettingDescriptor> = registry
            .iter()
            .filter(|descriptor| descriptor.category == category && descriptor.matches(search))
            .collect();
        if matching.is_empty() {
            continue;
        }

        if !first {
            ui.add_space(8.0);
        }
        first = false;
        ui.label(
            RichText::new(category.label())
                .strong()
                .color(pal.text_tertiary),
        );
        for descriptor in matching {
            render_descriptor(descriptor, state, ui, actions, &pal);
        }
    }

    if first {
        ui.add_space(4.0);
        ui.label(
            RichText::new("No settings match your search.")
                .italics()
                .color(pal.text_tertiary),
        );
    }
}

/// The inline "reset to default" affordance, right-aligned on the control's row.
/// Rendered only when the setting declares a default and currently differs from
/// it. Reads `AppState` to detect non-default; the reset itself flows through an
/// `AppAction`, preserving the single-mutator invariant.
fn reset_affordance(
    descriptor: &SettingDescriptor,
    state: &AppState,
    ui: &mut egui::Ui,
    actions: &mut Vec<AppAction>,
) {
    let (Some(is_default), Some(reset)) = (descriptor.is_default, descriptor.reset) else {
        return;
    };
    if is_default(state) {
        return;
    }
    if ui
        .add(egui::Button::new(
            RichText::new(egui_phosphor::regular::ARROW_COUNTER_CLOCKWISE).small(),
        ))
        .on_hover_text("Reset to default")
        .clicked()
    {
        actions.push(reset());
    }
}

fn render_descriptor(
    descriptor: &SettingDescriptor,
    state: &mut AppState,
    ui: &mut egui::Ui,
    actions: &mut Vec<AppAction>,
    pal: &crate::frontend::theme::Palette,
) {
    if descriptor.indent {
        ui.indent(descriptor.id, |ui| {
            render_descriptor_body(descriptor, state, ui, actions, pal);
        });
    } else {
        render_descriptor_body(descriptor, state, ui, actions, pal);
    }
}

fn render_descriptor_body(
    descriptor: &SettingDescriptor,
    state: &mut AppState,
    ui: &mut egui::Ui,
    actions: &mut Vec<AppAction>,
    pal: &crate::frontend::theme::Palette,
) {
    match &descriptor.control {
        Control::Toggle { read, on_change } => {
            // Control first (so it sits flush left, like every other row), then
            // a right-to-left region filling the remainder pins the reset button
            // to the row's right edge without disturbing the control.
            let enabled = descriptor.enabled.is_none_or(|gate| gate(state));
            ui.horizontal(|ui| {
                let mut value = read(state);
                // A setting is a persistent, immediately-applied on/off, so it
                // gets a sliding toggle switch rather than a checkbox. Centralizing
                // the toggle render here means every `Control::Toggle` in the
                // registry picks up the switch from this one site.
                //
                // Honour `descriptor.enabled` like the Slider/Value branches:
                // `toggle_switch` returns a bare `Response`, so gate it through
                // an `add_enabled_ui` wrapper rather than `add_enabled`.
                let changed = ui
                    .add_enabled_ui(enabled, |ui| {
                        crate::frontend::ui::toggle_switch(ui, &mut value, descriptor.title, pal)
                            .changed()
                    })
                    .inner;
                if changed {
                    actions.push(on_change(value));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    reset_affordance(descriptor, state, ui, actions);
                });
            });
        }
        Control::Choice {
            read,
            options,
            on_change,
        } => {
            // Honour `descriptor.enabled` like the Toggle/Slider/Value branches,
            // greying the combo out (label stays legible) when its gate is unmet.
            let enabled = descriptor.enabled.is_none_or(|gate| gate(state));
            let current = read(state);
            ui.horizontal(|ui| {
                ui.label(descriptor.title);
                let selected = options.get(current).copied().unwrap_or_default();
                ui.add_enabled_ui(enabled, |ui| {
                    egui::ComboBox::from_id_salt(descriptor.id)
                        .selected_text(selected)
                        .show_ui(ui, |ui| {
                            crate::frontend::theme::stabilize_selectable_rows(ui);
                            for (index, option) in options.iter().enumerate() {
                                if ui.selectable_label(index == current, *option).clicked() {
                                    actions.push(on_change(index));
                                }
                            }
                        });
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    reset_affordance(descriptor, state, ui, actions);
                });
            });
        }
        Control::Slider {
            read,
            range,
            on_change,
            show_value,
        } => {
            let enabled = descriptor.enabled.is_none_or(|gate| gate(state));
            let mut value = read(state);
            ui.horizontal(|ui| {
                let slider = egui::Slider::new(&mut value, range.clone())
                    .text(descriptor.title)
                    .show_value(*show_value);
                let response = ui.add_enabled(enabled, slider);
                // Mirror the glass-intensity pattern: live preview while
                // dragging (commit = false), persist on a discrete change
                // or on release.
                if response.changed() {
                    actions.push(on_change(value, !response.dragged()));
                } else if response.drag_stopped() {
                    actions.push(on_change(value, true));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    reset_affordance(descriptor, state, ui, actions);
                });
            });
        }
        Control::Value {
            read,
            range,
            unit,
            speed,
            on_change,
        } => {
            let enabled = descriptor.enabled.is_none_or(|gate| gate(state));
            let mut value = read(state);
            ui.horizontal(|ui| {
                ui.label(descriptor.title);
                let drag = egui::DragValue::new(&mut value)
                    .range(range.clone())
                    .speed(*speed)
                    .suffix(*unit);
                if ui.add_enabled(enabled, drag).changed() {
                    actions.push(on_change(value));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    reset_affordance(descriptor, state, ui, actions);
                });
            });
        }
        Control::Custom(render) => render(state, ui, actions),
    }

    if !descriptor.description.is_empty() {
        ui.label(caption_text(descriptor.description, pal.text_muted));
    }
}
