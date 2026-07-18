use super::*;

use eframe::egui;

use crate::backend::config::{ApprovalMode, ExternalAgentAccess};
use crate::frontend::agent::registry;
use crate::frontend::agent::session::{AssistantConversationId, ModelFetchStatus};
use crate::frontend::jobs::spawn_model_fetch;
use crate::frontend::state::{AppState, SystemSubsystem};
use crate::io::llm::types::Effort;

pub fn new_assistant_conversation(state: &mut AppState) {
    let selection = state.config.assistant.default_selection.clone();
    state.ui.agent.start_new_conversation(selection);
    refresh_key_status(state);
}

pub fn switch_assistant_conversation(
    state: &mut AppState,
    id: AssistantConversationId,
    ctx: &egui::Context,
) {
    state.ui.agent.switch_conversation(id);
    refresh_key_status(state);
    // A background job may have finished while this conversation was inactive; its
    // queued follow-up now dispatches against the freshly-active conversation.
    pump_queue(state, ctx);
}

pub fn rename_assistant_conversation(
    state: &mut AppState,
    id: AssistantConversationId,
    title: &str,
) {
    state.ui.agent.rename_conversation(id, title);
}

pub fn delete_assistant_conversation(state: &mut AppState, id: AssistantConversationId) {
    // Stop any background jobs the chat launched before it disappears, else their
    // workers leak and their results are orphaned on completion. Gated on the same
    // condition `delete_conversation` uses, so we only cancel when it will delete.
    if state.ui.agent.can_manage_conversations() {
        cancel_conversation_jobs(state, id);
    }
    state.ui.agent.delete_conversation(id);
    refresh_key_status(state);
}

pub fn switch_assistant_conversation_model(state: &mut AppState, provider: &str, model: &str) {
    let Some(spec) = registry::provider_spec(provider) else {
        return;
    };
    // Providers without a curated model list (currently Local) may be selected
    // before discovery/manual entry. The composer treats that blank selection
    // as incomplete and disables Send until a real model id is chosen.
    let incomplete_dynamic_provider = model.trim().is_empty() && spec.models.is_empty();
    if !state.ui.agent.can_manage_conversations()
        || (model.trim().is_empty() && !incomplete_dynamic_provider)
    {
        return;
    }
    let conversation = state.ui.agent.active_mut();
    if conversation.selection.provider == provider && conversation.selection.model == model {
        return;
    }
    strip_reasoning(&mut conversation.history);
    conversation.selection = crate::backend::config::AssistantModelSelection {
        provider: provider.to_string(),
        model: model.to_string(),
    };
    refresh_key_status(state);
}

/// Change the provider/model defaults copied into new conversations.
pub fn switch_provider_model(state: &mut AppState, provider: &str, model: &str) {
    state.config.assistant.default_selection = crate::backend::config::AssistantModelSelection {
        provider: provider.to_string(),
        model: model.to_string(),
    };
    if !state.ui.agent.has_activity() {
        switch_assistant_conversation_model(state, provider, model);
    }
    // The fetch status is global; clear it so a prior provider's spinner or
    // error note doesn't bleed onto the newly selected one. The fetched model
    // ids are keyed per provider, so they survive the switch.
    state.ui.agent.model_fetch = ModelFetchStatus::Idle;
    persist(state);
    refresh_key_status(state);
}

/// Enable or disable the assistant and persist.
pub fn set_assistant_enabled(state: &mut AppState, enabled: bool) {
    state.config.assistant.enabled = enabled;
    persist(state);
}

/// Set the reasoning effort and persist.
pub fn set_assistant_effort(state: &mut AppState, effort: Effort) {
    state.config.assistant.effort = effort;
    persist(state);
}

/// Set the command-approval mode and persist.
pub fn set_approval_mode(state: &mut AppState, mode: ApprovalMode) {
    state.config.assistant.approval_mode = mode;
    persist(state);
}

/// Pin whether the default OpenAI-compatible model accepts a reasoning-effort
/// knob, overriding the registry heuristic, and persist it per provider/model.
pub fn set_assistant_effort_supported(state: &mut AppState, supported: bool) {
    let selection = &state.config.assistant.default_selection;
    state
        .config
        .assistant
        .model_effort_overrides
        .entry(selection.provider.clone())
        .or_default()
        .insert(selection.model.clone(), supported);
    persist(state);
}

/// Set (or clear, when blank) the default provider's base-URL override.
pub fn set_assistant_base_url(state: &mut AppState, base_url: &str) {
    let trimmed = base_url.trim();
    let provider = state.config.assistant.default_selection.provider.clone();
    if trimmed.is_empty() {
        state.config.assistant.base_urls.remove(&provider);
    } else {
        state
            .config
            .assistant
            .base_urls
            .insert(provider, trimmed.to_string());
    }
    state.ui.agent.model_fetch = ModelFetchStatus::Idle;
    persist(state);
}

pub fn set_assistant_executable(state: &mut AppState, path: &str) {
    let provider = state.config.assistant.default_selection.provider.clone();
    let path = path.trim();
    if path.is_empty() {
        state
            .config
            .assistant
            .external_agent_executables
            .remove(&provider);
    } else {
        state
            .config
            .assistant
            .external_agent_executables
            .insert(provider, path.to_string());
    }
    persist(state);
}

/// Set the active conversation's external-agent sandbox posture and persist.
pub fn set_assistant_external_access(state: &mut AppState, access: ExternalAgentAccess) {
    state.ui.agent.external_access = access;
    persist(state);
}

/// Store the default provider's API key in the app key store (never in config).
pub fn set_assistant_api_key(state: &mut AppState, key: &str) {
    let provider = registry::default_provider(&state.config.assistant);
    match crate::backend::secrets::set_stored_key(provider.id, key.trim()) {
        Ok(()) => state.status_success(format!("Saved the API key for {}.", provider.label)),
        Err(error) => state.report_system_error(
            SystemSubsystem::Settings,
            format!("Could not save the API key: {error}"),
        ),
    }
    refresh_key_status(state);
}

/// Remove a provider's stored key from the app key store. Takes the provider id
/// rather than assuming the active one, so it backs both the active "Clear"
/// button and the per-row Remove in the keys overview.
pub fn clear_stored_key(state: &mut AppState, provider_id: &str) {
    let label = registry::provider_spec(provider_id)
        .map(|spec| spec.label)
        .unwrap_or(provider_id);
    match crate::backend::secrets::clear_stored_key(provider_id) {
        Ok(()) => state.status_success(format!("Removed the stored API key for {label}.")),
        Err(error) => state.report_system_error(
            SystemSubsystem::Settings,
            format!("Could not remove the API key: {error}"),
        ),
    }
    refresh_key_status(state);
}

/// Recompute whether a key is available for the active provider (reads env + the
/// key store) and cache it on the session, so the render path never hits the
/// key store. Called on provider/key changes and once at startup.
pub fn refresh_key_status(state: &mut AppState) {
    let provider = registry::provider_spec(&state.ui.agent.selection.provider);
    let available = provider.and_then(registry::api_key_for).is_some();
    state.ui.agent.key_available = Some(available);
}

/// Kick off a live `/models` fetch for the default provider. Resolves the key the
/// same way a turn does (env → key store); with no key it records an error
/// instead of spawning. The result is drained in [`poll_model_fetch`]. A fetch
/// already in flight is left to finish.
pub fn fetch_models(state: &mut AppState, ctx: &egui::Context) {
    if state.jobs.model_fetch.is_some() {
        return;
    }
    let spec = registry::default_provider(&state.config.assistant);
    if matches!(spec.kind, registry::ProviderKind::ExternalAgent(_)) {
        state.ui.agent.model_fetch = ModelFetchStatus::Error(
            "CLI agents use their own model selection; model enumeration is not requested.".into(),
        );
        ctx.request_repaint();
        return;
    }
    let Some(key) = registry::api_key_for(spec) else {
        state.ui.agent.model_fetch = ModelFetchStatus::Error(format!(
            "Add a key for {} first to list its models.",
            spec.label
        ));
        ctx.request_repaint();
        return;
    };
    let base_url = registry::effective_base_url(&state.config.assistant, spec);
    state.jobs.model_fetch = Some(spawn_model_fetch(
        spec.id.to_string(),
        spec.kind,
        base_url,
        key,
    ));
    state.ui.agent.model_fetch = ModelFetchStatus::Fetching;
    ctx.request_repaint_after(AGENT_POLL);
}

/// Drain the in-flight model fetch (called from `poll_jobs`). On success the ids
/// are cached under their provider id and the status returns to Idle; on failure
/// the status carries a short reason. The cached list is keyed by provider, so a
/// result arriving after the user switched providers is still stored correctly.
pub fn poll_model_fetch(state: &mut AppState, ctx: &egui::Context) {
    let Some(job) = state.jobs.model_fetch.take() else {
        return;
    };
    match job.receiver.try_recv() {
        Ok(Ok(ids)) => {
            let count = ids.len();
            state.ui.agent.fetched_models.insert(job.provider_id, ids);
            state.ui.agent.model_fetch = ModelFetchStatus::Idle;
            state.status_success(format!("Listed {count} models from the provider."));
            ctx.request_repaint();
        }
        Ok(Err(error)) => {
            state.ui.agent.model_fetch = ModelFetchStatus::Error(error);
            ctx.request_repaint();
        }
        Err(std::sync::mpsc::TryRecvError::Empty) => {
            state.jobs.model_fetch = Some(job);
            ctx.request_repaint_after(AGENT_POLL);
        }
        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
            state.ui.agent.model_fetch =
                ModelFetchStatus::Error("model fetch worker stopped".to_string());
            ctx.request_repaint();
        }
    }
}
