//! `Control::Custom` renderers: editors too complex to express declaratively
//! (path pickers, the assistant editor, the Advanced meta-settings). Each still
//! keeps the Elm flow — it reads `&mut AppState` only to display, and emits
//! `AppAction`s for any change.

use super::*;

use eframe::egui::{self, RichText};

use crate::frontend::{actions::AppAction, state::AppState};

/// Path settings are not a scalar control: show the current default project
/// folder and a button that emits the picker action (the dialog itself runs in
/// the dispatcher, the only place allowed to touch state).
pub(crate) fn render_default_project_dir(
    state: &mut AppState,
    ui: &mut egui::Ui,
    actions: &mut Vec<AppAction>,
) {
    let pal = crate::frontend::theme::palette(ui);
    ui.horizontal(|ui| {
        ui.label("Default project folder");
        if ui.button("Choose…").clicked() {
            actions.push(AppAction::PickDefaultProjectDir);
        }
    });
    ui.label(caption_text(
        state.config.default_project_dir.display().to_string(),
        pal.text_muted,
    ));
}

/// The global default compute target every task panel seeds from (each panel can
/// override it per run). Reuses the shared compute-target picker against a copy of
/// the config value, emitting the change so the dispatcher stays the sole mutator.
pub(crate) fn render_default_compute_target(
    state: &mut AppState,
    ui: &mut egui::Ui,
    actions: &mut Vec<AppAction>,
) {
    use crate::frontend::ui::{compute_target_picker, remote_host_options};

    let pal = crate::frontend::theme::palette(ui);
    let hosts = remote_host_options(state);
    let mut target = state.config.default_compute_target.clone();
    compute_target_picker(ui, &mut target, &hosts, actions, "default_compute_target");
    if target != state.config.default_compute_target {
        actions.push(AppAction::SetDefaultComputeTarget(target));
    }
    ui.label(caption_text(
        "New QM, docking, and MD panels start from this target; each can override it per run.",
        pal.text_muted,
    ));
}

/// The in-app LLM assistant settings: enable, provider, model (with live
/// refresh), effort, per-provider key entry, and a "Stored keys" overview. The
/// provider list is data-driven from `frontend::agent::registry`, so adding a
/// provider is a config row there, not new UI. Every control emits an
/// `AppAction`; keys go to the app key store (or an env var), never settings.json.
pub(crate) fn render_assistant_settings(
    state: &mut AppState,
    ui: &mut egui::Ui,
    actions: &mut Vec<AppAction>,
) {
    use crate::frontend::agent::{ModelFetchStatus, registry};

    let pal = crate::frontend::theme::palette(ui);

    let mut enabled = state.config.assistant.enabled;
    if ui.checkbox(&mut enabled, "Enable the assistant").changed() {
        actions.push(AppAction::SetAssistantEnabled(enabled));
    }
    ui.label(
        RichText::new(
            "Drives SilicoLab with the same console commands a user types. Pay your provider \
             directly; the API key is read from its environment variable, else the app key \
             store — never settings.json.",
        )
        .size(CAPTION_SIZE)
        .color(pal.text_tertiary),
    );
    ui.add_space(6.0);

    let provider = registry::default_provider(&state.config.assistant);
    let current_model = state.config.assistant.default_selection.model.clone();

    ui.label(
        RichText::new(
            "These defaults are copied into new conversations. Switch the current conversation's model directly in Assistant.",
        )
        .size(CAPTION_SIZE)
        .color(pal.text_tertiary),
    );
    ui.add_space(4.0);

    // Provider picker (data-driven).
    ui.horizontal(|ui| {
        ui.label("Default provider");
        egui::ComboBox::from_id_salt("assistant.provider")
            .selected_text(provider.label)
            .show_ui(ui, |ui| {
                crate::frontend::theme::stabilize_selectable_rows(ui);
                for spec in registry::PROVIDERS {
                    if ui
                        .selectable_label(spec.id == provider.id, spec.label)
                        .clicked()
                        && spec.id != provider.id
                    {
                        // Prefer a curated model, then a previously discovered
                        // live model. Dynamic providers such as Local remain
                        // intentionally blank until discovery or manual entry.
                        let model = spec
                            .models
                            .first()
                            .map(|model| model.id.to_string())
                            .or_else(|| {
                                state
                                    .ui
                                    .agent
                                    .fetched_models
                                    .get(spec.id)
                                    .and_then(|models| models.first())
                                    .cloned()
                            })
                            .unwrap_or_default();
                        actions.push(AppAction::SwitchProviderModel {
                            provider: spec.id.to_string(),
                            model,
                        });
                    }
                }
            });
    });

    // Model picker — built-in models first, then any live-fetched ids for this
    // provider. The static list always shows; a live refresh only augments it.
    let fetched = state
        .ui
        .agent
        .fetched_models
        .get(provider.id)
        .cloned()
        .unwrap_or_default();
    let models = registry::merged_model_ids(provider, &fetched);
    let fetch_status = state.ui.agent.model_fetch.clone();
    let fetching = matches!(fetch_status, ModelFetchStatus::Fetching);
    let selected_model_label = models
        .iter()
        .find(|(id, _)| *id == current_model)
        .map(|(_, label)| label.clone())
        .unwrap_or_else(|| {
            if current_model.trim().is_empty() {
                "Choose model…".to_string()
            } else {
                current_model.clone()
            }
        });
    ui.horizontal(|ui| {
        ui.label("Default model");
        egui::ComboBox::from_id_salt("assistant.model")
            .selected_text(selected_model_label)
            .show_ui(ui, |ui| {
                crate::frontend::theme::stabilize_selectable_rows(ui);
                if models.is_empty() {
                    ui.label(
                        RichText::new("No models detected")
                            .small()
                            .color(pal.text_tertiary),
                    );
                }
                for (id, label) in &models {
                    if ui.selectable_label(*id == current_model, label).clicked()
                        && *id != current_model
                    {
                        actions.push(AppAction::SwitchProviderModel {
                            provider: provider.id.to_string(),
                            model: id.clone(),
                        });
                    }
                }
            });
        ui.add_enabled_ui(!fetching, |ui| {
            if ui.button("Refresh models").clicked() {
                actions.push(AppAction::RefreshModels);
            }
        });
        if fetching {
            ui.spinner();
        }
    });
    // Live-fetch status under the picker: an amber note on failure, a count once
    // a live list is cached, and a one-line explanation either way.
    match &fetch_status {
        ModelFetchStatus::Error(reason) => {
            ui.label(
                RichText::new(format!("{}  {reason}", egui_phosphor::regular::WARNING))
                    .size(CAPTION_SIZE)
                    .color(pal.status_amber),
            );
        }
        ModelFetchStatus::Idle if !fetched.is_empty() => {
            ui.label(
                RichText::new(format!("{} models listed live.", fetched.len()))
                    .size(CAPTION_SIZE)
                    .color(pal.text_tertiary),
            );
        }
        _ => {}
    }
    let model_help = if provider.models.is_empty() {
        "Refresh from the local server or enter its exact model id below."
    } else {
        "Live list from the provider; offline keeps the built-in list."
    };
    ui.label(
        RichText::new(model_help)
            .size(CAPTION_SIZE)
            .color(pal.text_tertiary),
    );

    // Free-text model id — model ids drift, and OpenRouter/local take arbitrary
    // ids, so let the user type one directly. Committed on Enter / focus loss.
    if let Some(model) = committed_text_field(
        ui,
        "assistant.model_text",
        "Model id",
        &current_model,
        if provider.id == "local" {
            "e.g. llama3.1"
        } else {
            "e.g. deepseek-reasoner"
        },
    ) && model != current_model
    {
        actions.push(AppAction::SwitchProviderModel {
            provider: provider.id.to_string(),
            model,
        });
    }

    // Base-URL override for OpenAI-compatible providers (self-hosted gateway,
    // a regional endpoint, or a local server on a non-default port).
    if provider.kind == registry::ProviderKind::OpenAiCompat {
        let current_base = registry::effective_base_url(&state.config.assistant, provider);
        if let Some(base) = committed_text_field(
            ui,
            "assistant.base_url",
            "Base URL",
            &current_base,
            provider.base_url,
        ) && base != current_base
        {
            actions.push(AppAction::SetAssistantBaseUrl(base));
        }
    }

    // Effort picker — only meaningful where the model accepts it. Caps fold in
    // the per-model override (see `registry::effective_caps`), so a custom
    // endpoint pointed at a reasoning model isn't greyed out.
    let caps = registry::effective_caps(
        &state.config.assistant,
        &state.config.assistant.default_selection,
        provider,
    );
    let current_effort = state.config.assistant.effort;
    ui.horizontal(|ui| {
        ui.label("Effort");
        let combo =
            egui::ComboBox::from_id_salt("assistant.effort").selected_text(current_effort.label());
        ui.add_enabled_ui(caps.supports_effort, |ui| {
            combo.show_ui(ui, |ui| {
                crate::frontend::theme::stabilize_selectable_rows(ui);
                for effort in crate::io::llm::types::Effort::all() {
                    if ui
                        .selectable_label(*effort == current_effort, effort.label())
                        .clicked()
                        && *effort != current_effort
                    {
                        actions.push(AppAction::SetAssistantEffort(*effort));
                    }
                }
            });
        });
    });
    let current_mode = state.config.assistant.approval_mode;
    ui.horizontal(|ui| {
        ui.label("Approvals");
        egui::ComboBox::from_id_salt("assistant.approval_mode")
            .selected_text(current_mode.short_label())
            .show_ui(ui, |ui| {
                crate::frontend::theme::stabilize_selectable_rows(ui);
                for mode in crate::backend::config::ApprovalMode::all() {
                    if ui
                        .selectable_label(mode == current_mode, mode.label())
                        .clicked()
                        && mode != current_mode
                    {
                        actions.push(AppAction::SetApprovalMode(mode));
                    }
                }
            });
    });
    ui.label(
        RichText::new("Destructive commands (delete, running a script) always ask.")
            .size(CAPTION_SIZE)
            .color(pal.text_tertiary),
    );

    // OpenAI-compatible endpoints take arbitrary model ids the built-in table
    // can't know, so let the user pin effort support for the active model. The
    // checkbox shows the effective capability; toggling it persists an override.
    // Native providers (Anthropic) derive caps reliably from the id, so they
    // only get the explanatory note.
    if provider.kind == registry::ProviderKind::OpenAiCompat {
        let mut supported = caps.supports_effort;
        if ui
            .checkbox(&mut supported, "This model supports reasoning effort")
            .changed()
        {
            actions.push(AppAction::SetAssistantEffortSupported(supported));
        }
    } else if !caps.supports_effort {
        ui.label(
            RichText::new("This model does not use a reasoning-effort setting.")
                .size(CAPTION_SIZE)
                .color(pal.text_tertiary),
        );
    }

    // API key: environment variable (preferred) or the app key store. Never config.
    ui.add_space(6.0);
    if provider.key_env.is_empty() {
        ui.label(
            RichText::new("This provider needs no API key.")
                .size(CAPTION_SIZE)
                .color(pal.text_tertiary),
        );
        return;
    }

    let (icon, text, color) = match registry::key_source(provider) {
        registry::KeySource::Env => (
            egui_phosphor::regular::CHECK_CIRCLE,
            format!("Using the key from {}", provider.key_env),
            pal.status_green,
        ),
        registry::KeySource::File => (
            egui_phosphor::regular::CHECK_CIRCLE,
            "Using the stored key".to_string(),
            pal.status_green,
        ),
        registry::KeySource::Missing => (
            egui_phosphor::regular::WARNING,
            format!("No key — set {} or store one below", provider.key_env),
            pal.status_amber,
        ),
        registry::KeySource::None => (
            egui_phosphor::regular::INFO,
            String::new(),
            pal.text_tertiary,
        ),
    };
    ui.label(
        RichText::new(format!("{icon}  {text}"))
            .size(CAPTION_SIZE)
            .color(color),
    );

    // Store a key in the app key store (an alternative to the env var, which
    // still wins when set). Held in egui temp memory, committed on a button click.
    let key_id = ui.id().with("assistant.api_key");
    let mut key = ui
        .data(|data| data.get_temp::<String>(key_id))
        .unwrap_or_default();
    ui.horizontal(|ui| {
        let response = ui.add(
            egui::TextEdit::singleline(&mut key)
                .desired_width(220.0)
                .password(true)
                .hint_text("Paste a key to store"),
        );
        if response.changed() {
            ui.data_mut(|data| data.insert_temp(key_id, key.clone()));
        }
        if ui.button("Save key").clicked() && !key.trim().is_empty() {
            actions.push(AppAction::SetAssistantApiKey(key.clone()));
            ui.data_mut(|data| data.remove_temp::<String>(key_id));
        }
        if ui.button("Clear").clicked() {
            actions.push(AppAction::ClearStoredKey(provider.id.to_string()));
            ui.data_mut(|data| data.remove_temp::<String>(key_id));
        }
    });
    ui.label(
        RichText::new(
            "Stored in an app-managed file in ~/.silicolab (not settings.json; obfuscated at \
             rest, which is not encryption). The environment variable takes precedence when set.",
        )
        .size(CAPTION_SIZE)
        .color(pal.text_tertiary),
    );

    render_stored_keys_overview(ui, actions, &pal);
}

/// The "Stored keys" overview: every provider that currently has a usable key,
/// across env vars and the app key store — the answer to "where did my key go".
/// File-store rows get a Remove button; env rows are read-only (managed outside
/// the app). This list doubles as the save confirmation: a just-saved provider
/// appears here as "stored".
fn render_stored_keys_overview(
    ui: &mut egui::Ui,
    actions: &mut Vec<AppAction>,
    pal: &crate::frontend::theme::Palette,
) {
    use crate::frontend::agent::registry;

    ui.add_space(8.0);
    ui.separator();
    ui.label(RichText::new("Stored keys").strong());

    let stored = registry::stored_keys();
    if stored.is_empty() {
        ui.label(
            RichText::new("No keys configured yet.")
                .size(CAPTION_SIZE)
                .color(pal.text_tertiary),
        );
        return;
    }
    for (spec, source) in stored {
        ui.horizontal(|ui| {
            ui.label(RichText::new(egui_phosphor::regular::KEY).color(pal.text_muted));
            ui.label(spec.label);
            match source {
                registry::KeySource::Env => {
                    ui.label(
                        RichText::new(format!("from {} — managed outside the app", spec.key_env))
                            .size(CAPTION_SIZE)
                            .color(pal.text_tertiary),
                    );
                }
                registry::KeySource::File => {
                    ui.label(
                        RichText::new("stored")
                            .size(CAPTION_SIZE)
                            .color(pal.status_green),
                    );
                    if ui.button("Remove").clicked() {
                        actions.push(AppAction::ClearStoredKey(spec.id.to_string()));
                    }
                }
                _ => {}
            }
        });
    }
}

/// A labeled single-line text field that edits a buffer in egui temp memory and
/// returns the new value only when the user commits (Enter or focus loss), so a
/// setting persists once per edit rather than on every keystroke. The buffer
/// reseeds from `current` whenever it is not being edited.
fn committed_text_field(
    ui: &mut egui::Ui,
    id_salt: &str,
    label: &str,
    current: &str,
    hint: &str,
) -> Option<String> {
    let buffer_id = ui.id().with(id_salt);
    let mut buffer = ui
        .data(|data| data.get_temp::<String>(buffer_id))
        .unwrap_or_else(|| current.to_string());

    let mut committed = None;
    ui.horizontal(|ui| {
        ui.label(label);
        let response = ui.add(
            egui::TextEdit::singleline(&mut buffer)
                .desired_width(260.0)
                .hint_text(hint),
        );
        if response.changed() {
            ui.data_mut(|data| data.insert_temp(buffer_id, buffer.clone()));
        }
        let enter = response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
        if enter || (response.lost_focus() && buffer != current) {
            committed = Some(buffer.trim().to_string());
            // Clear the scratch buffer so the field reseeds from the committed
            // config value next frame.
            ui.data_mut(|data| data.remove_temp::<String>(buffer_id));
        }
    });
    committed
}

/// Show the settings.json path with a button that reveals it in the OS file
/// manager. The blocking shell-out runs in the dispatcher; this only emits the
/// action.
pub(crate) fn render_settings_location(
    _state: &mut AppState,
    ui: &mut egui::Ui,
    actions: &mut Vec<AppAction>,
) {
    let pal = crate::frontend::theme::palette(ui);
    let path = crate::backend::config::settings_path();
    ui.horizontal(|ui| {
        ui.label("Settings file");
        if ui
            .button(format!("{}  Reveal", egui_phosphor::regular::FOLDER_OPEN))
            .clicked()
        {
            actions.push(AppAction::RevealSettingsFile);
        }
    });
    ui.label(caption_text(path.display().to_string(), pal.text_muted));
}

/// Reset-everything, gated behind an explicit inline confirmation so a single
/// click can't wipe the user's settings. The confirm flag is parked in egui's
/// per-widget temp memory (transient UI, never persisted), keeping this renderer
/// free of any persisted-state mutation.
pub(crate) fn render_reset_all(
    _state: &mut AppState,
    ui: &mut egui::Ui,
    actions: &mut Vec<AppAction>,
) {
    let pal = crate::frontend::theme::palette(ui);
    let confirm_id = ui.id().with("settings_reset_all_confirm");
    let confirming = ui
        .data(|data| data.get_temp::<bool>(confirm_id))
        .unwrap_or(false);

    if confirming {
        ui.horizontal(|ui| {
            ui.label(RichText::new("Reset every setting to its default?").color(pal.status_red));
            if ui.button("Reset all").clicked() {
                actions.push(AppAction::ResetAllSettings);
                ui.data_mut(|data| data.insert_temp(confirm_id, false));
            }
            if ui.button("Cancel").clicked() {
                ui.data_mut(|data| data.insert_temp(confirm_id, false));
            }
        });
    } else if ui.button("Reset all settings to defaults…").clicked() {
        ui.data_mut(|data| data.insert_temp(confirm_id, true));
    }
}

/// Export / import the whole settings file via native dialogs (run in the
/// dispatcher, like the other pickers).
pub(crate) fn render_export_import(
    _state: &mut AppState,
    ui: &mut egui::Ui,
    actions: &mut Vec<AppAction>,
) {
    ui.horizontal(|ui| {
        if ui
            .button(format!(
                "{}  Export…",
                egui_phosphor::regular::UPLOAD_SIMPLE
            ))
            .clicked()
        {
            actions.push(AppAction::ExportSettings);
        }
        if ui
            .button(format!(
                "{}  Import…",
                egui_phosphor::regular::DOWNLOAD_SIMPLE
            ))
            .clicked()
        {
            actions.push(AppAction::ImportSettings);
        }
    });
}
