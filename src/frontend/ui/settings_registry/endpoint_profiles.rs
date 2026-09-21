//! Settings widgets for the custom OpenAI-compatible provider's named endpoint
//! profiles: the picker above the endpoint fields and the rows in the
//! "Stored keys" overview.

use eframe::egui::{self, RichText};

use crate::backend::config::endpoint_key_id;
use crate::frontend::{actions::AppAction, state::AppState, theme::Palette};

use super::CAPTION_SIZE;

#[derive(Clone)]
enum Draft {
    New(String),
    Rename(String),
    ConfirmDelete,
}

pub(super) fn render_profile_picker(
    state: &AppState,
    ui: &mut egui::Ui,
    actions: &mut Vec<AppAction>,
    pal: &Palette,
) {
    let assistant = &state.config.assistant;
    let active = assistant.active_endpoint();
    let draft_id = ui.id().with("assistant.endpoint_profile_draft");
    let mut draft = ui
        .data(|data| data.get_temp::<Option<Draft>>(draft_id))
        .flatten();

    ui.horizontal(|ui| {
        ui.label("Profile");
        egui::ComboBox::from_id_salt("assistant.endpoint_profile")
            .selected_text(active.map_or("Unsaved", |profile| profile.name.as_str()))
            .show_ui(ui, |ui| {
                crate::frontend::theme::stabilize_selectable_rows(ui);
                for profile in &assistant.custom_endpoints {
                    let selected = active.is_some_and(|active| active.id == profile.id);
                    if ui.selectable_label(selected, &profile.name).clicked() && !selected {
                        actions.push(AppAction::SelectEndpointProfile(profile.id.clone()));
                    }
                }
            });
        if ui.button("New…").clicked() {
            draft = Some(Draft::New(String::new()));
        }
        if let Some(profile) = active {
            if ui.button("Rename…").clicked() {
                draft = Some(Draft::Rename(profile.name.clone()));
            }
            if ui.button("Delete").clicked() {
                draft = Some(Draft::ConfirmDelete);
            }
        }
    });

    let mut done = false;
    match &mut draft {
        Some(Draft::New(name)) => {
            if let Some(name) = name_row(ui, name, "Create", &mut done) {
                actions.push(AppAction::NewEndpointProfile(name));
            }
        }
        Some(Draft::Rename(name)) => {
            if let Some(name) = name_row(ui, name, "Rename", &mut done)
                && let Some(profile) = active
            {
                actions.push(AppAction::RenameEndpointProfile {
                    id: profile.id.clone(),
                    name,
                });
            }
        }
        Some(Draft::ConfirmDelete) => {
            ui.horizontal(|ui| {
                let name = active.map_or("", |profile| profile.name.as_str());
                ui.label(
                    RichText::new(format!("Delete {name} and its stored key?"))
                        .color(pal.status_amber),
                );
                if ui.button("Delete").clicked() {
                    if let Some(profile) = active {
                        actions.push(AppAction::DeleteEndpointProfile(profile.id.clone()));
                    }
                    done = true;
                }
                done |= ui.button("Cancel").clicked();
            });
        }
        None => {}
    }
    ui.data_mut(|data| data.insert_temp(draft_id, draft.filter(|_| !done)));

    ui.label(
        RichText::new(
            "A profile remembers this endpoint's base URL, model and key. Edits below are saved \
             into the selected profile.",
        )
        .size(CAPTION_SIZE)
        .color(pal.text_tertiary),
    );
}

fn name_row(
    ui: &mut egui::Ui,
    name: &mut String,
    confirm: &str,
    done: &mut bool,
) -> Option<String> {
    let mut committed = None;
    ui.horizontal(|ui| {
        let response = ui.add(
            egui::TextEdit::singleline(name)
                .desired_width(180.0)
                .hint_text("Profile name"),
        );
        let enter = response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
        if (ui.button(confirm).clicked() || enter) && !name.trim().is_empty() {
            committed = Some(name.trim().to_string());
            *done = true;
        }
        *done |= ui.button("Cancel").clicked();
    });
    committed
}

/// One row per saved profile under the "Stored keys" overview, each with a
/// `Use` button — the way back to an endpoint configured earlier.
pub(super) fn render_profile_rows(
    state: &AppState,
    ui: &mut egui::Ui,
    actions: &mut Vec<AppAction>,
    pal: &Palette,
) {
    let assistant = &state.config.assistant;
    if assistant.custom_endpoints.is_empty() {
        return;
    }
    let stored = crate::backend::secrets::stored_provider_ids();
    let using_custom =
        assistant.default_selection.provider == crate::backend::config::CUSTOM_ENDPOINT_PROVIDER;
    ui.add_space(4.0);
    ui.label(RichText::new("Custom endpoint profiles").strong());
    for profile in &assistant.custom_endpoints {
        ui.horizontal(|ui| {
            ui.label(RichText::new(egui_phosphor::regular::PLUGS).color(pal.text_muted));
            ui.label(&profile.name);
            let url = if profile.base_url.is_empty() {
                "no base URL"
            } else {
                profile.base_url.as_str()
            };
            ui.label(
                RichText::new(format!("{url} · {}", profile.model))
                    .size(CAPTION_SIZE)
                    .color(pal.text_tertiary),
            );
            let (text, color) = if stored.contains(&endpoint_key_id(&profile.id)) {
                ("stored", pal.status_green)
            } else {
                ("no key", pal.status_amber)
            };
            ui.label(RichText::new(text).size(CAPTION_SIZE).color(color));
            let active = assistant.active_custom_endpoint.as_deref() == Some(&profile.id);
            if active && using_custom {
                ui.label(
                    RichText::new("in use")
                        .size(CAPTION_SIZE)
                        .color(pal.text_muted),
                );
            } else if ui.button("Use").clicked() {
                actions.push(AppAction::SelectEndpointProfile(profile.id.clone()));
            }
        });
    }
}
