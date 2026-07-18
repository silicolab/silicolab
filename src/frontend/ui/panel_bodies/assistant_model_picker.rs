use super::*;

use eframe::egui::{self, RichText};

use crate::frontend::{actions::AppAction, state::AppState};

pub(crate) fn render_assistant_model_picker(
    state: &AppState,
    ui: &mut egui::Ui,
    actions: &mut Vec<AppAction>,
    provider: &crate::frontend::agent::registry::ProviderSpec,
    current_model: &str,
    width: f32,
) -> bool {
    use crate::frontend::agent::registry;

    let can_switch = state.ui.agent.can_manage_conversations();
    let selected_label = provider
        .models
        .iter()
        .find(|model| model.id == current_model)
        .map(|model| model.label)
        .unwrap_or(current_model);
    let selected_label = if selected_label.trim().is_empty() {
        if matches!(provider.kind, registry::ProviderKind::ExternalAgent(_)) {
            "CLI default".to_string()
        } else {
            "Choose model…".to_string()
        }
    } else {
        compact_model_label(selected_label, if width < 76.0 { 8 } else { 14 })
    };
    let mut open_settings = false;
    let response = egui::ComboBox::from_id_salt("assistant.conversation_model")
        .selected_text(assistant_text(selected_label))
        .width(width.max(28.0))
        .truncate()
        .show_ui(ui, |ui| {
            crate::frontend::theme::stabilize_selectable_rows(ui);
            ui.set_min_width(220.0);
            for spec in registry::PROVIDERS {
                ui.label(RichText::new(spec.label).small().strong());
                let available = registry::api_key_for(spec).is_some();
                let fetched = state
                    .ui
                    .agent
                    .fetched_models
                    .get(spec.id)
                    .map(Vec::as_slice)
                    .unwrap_or_default();
                let models = registry::merged_model_ids(spec, fetched);
                if matches!(spec.kind, registry::ProviderKind::ExternalAgent(_)) {
                    let selected = spec.id == provider.id && current_model.trim().is_empty();
                    if ui
                        .add_enabled(
                            can_switch,
                            egui::Button::selectable(selected, "CLI default"),
                        )
                        .clicked()
                        && !selected
                    {
                        actions.push(AppAction::SwitchAssistantConversationModel {
                            provider: spec.id.to_string(),
                            model: String::new(),
                        });
                        ui.close();
                    }
                }
                if models.is_empty() {
                    ui.label(
                        RichText::new("Choose in Assistant settings")
                            .small()
                            .color(crate::frontend::theme::palette(ui).text_tertiary),
                    );
                }
                for (model, label) in models {
                    let selected = spec.id == provider.id && model == current_model;
                    let response = ui.add_enabled(
                        can_switch && available,
                        egui::Button::selectable(selected, assistant_text(&label)),
                    );
                    let response = if available {
                        response
                    } else {
                        response.on_hover_text("Add an API key for this provider first")
                    };
                    if response.clicked() && !selected {
                        actions.push(AppAction::SwitchAssistantConversationModel {
                            provider: spec.id.to_string(),
                            model,
                        });
                        ui.close();
                    }
                }
                if !available {
                    ui.label(
                        RichText::new("API key required")
                            .small()
                            .color(crate::frontend::theme::palette(ui).text_tertiary),
                    );
                }
                ui.add_space(3.0);
            }
            ui.separator();
            if ui.button("Manage providers and API keys…").clicked() {
                open_settings = true;
                ui.close();
            }
        });
    let model_label = if current_model.trim().is_empty()
        && matches!(provider.kind, registry::ProviderKind::ExternalAgent(_))
    {
        "CLI default"
    } else {
        current_model
    };
    let mut hover = format!("{} · {}", provider.label, model_label);
    if let Some(usage) = &state.ui.agent.last_usage {
        let session = &state.ui.agent.session_usage;
        hover.push_str(&format!(
            "\nLast: {} in / {} out\nSession: {} in / {} out",
            compact_tokens(usage.input_total()),
            compact_tokens(usage.output),
            compact_tokens(session.input_total()),
            compact_tokens(session.output)
        ));
    }
    response.response.on_hover_text(hover);
    open_settings
}

fn compact_tokens(value: u32) -> String {
    if value < 1_000 {
        return value.to_string();
    }
    if value < 1_000_000 {
        return format!("{:.1}k", value as f32 / 1_000.0);
    }
    format!("{:.1}m", value as f32 / 1_000_000.0)
}

fn compact_model_label(label: &str, max_chars: usize) -> String {
    let without_qualifier = label.split(" (").next().unwrap_or(label);
    let short = ["Claude ", "Anthropic ", "OpenAI ", "Google "]
        .iter()
        .find_map(|prefix| without_qualifier.strip_prefix(prefix))
        .unwrap_or(without_qualifier);
    if short.chars().count() <= max_chars {
        return short.to_string();
    }
    format!(
        "{}…",
        short
            .chars()
            .take(max_chars.saturating_sub(1))
            .collect::<String>()
    )
}
