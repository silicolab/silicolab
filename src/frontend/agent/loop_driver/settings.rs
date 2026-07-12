use super::*;

use eframe::egui;

use crate::backend::config::ApprovalMode;
use crate::frontend::agent::registry;
use crate::frontend::agent::session::{AssistantConversationId, ModelFetchStatus};
use crate::frontend::jobs::spawn_model_fetch;
use crate::frontend::state::{AppState, SystemSubsystem};
use crate::io::llm::types::Effort;

pub fn new_assistant_conversation(state: &mut AppState) {
    state.ui.agent.start_new_conversation();
}

pub fn switch_assistant_conversation(
    state: &mut AppState,
    id: AssistantConversationId,
    ctx: &egui::Context,
) {
    state.ui.agent.switch_conversation(id);
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
}

/// Switch the active provider + model and persist. Strips prior-provider
/// reasoning blobs from the replayed history (ignored-but-billed, or
/// shape-incompatible, on a different provider/model) and clears a stale base-URL
/// override when the provider changes.
pub fn switch_provider_model(state: &mut AppState, provider: &str, model: &str) {
    if state.config.assistant.provider != provider {
        // The base-URL override is provider-specific; drop it on a provider change.
        state.config.assistant.base_url = None;
    }
    // The effort override is per-model; drop it so the new model falls back to
    // the registry heuristic until the user pins it again.
    state.config.assistant.effort_override = None;
    state.config.assistant.provider = provider.to_string();
    state.config.assistant.model = model.to_string();
    for conversation in &mut state.ui.agent.conversations {
        strip_reasoning(&mut conversation.history);
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

/// Pin whether the active OpenAI-compatible model accepts a reasoning-effort
/// knob, overriding the registry heuristic, and persist. Cleared automatically
/// when the model or provider changes.
pub fn set_assistant_effort_supported(state: &mut AppState, supported: bool) {
    state.config.assistant.effort_override = Some(supported);
    persist(state);
}

/// Set (or clear, when blank) the base-URL override for an OpenAI-compatible
/// provider and persist.
pub fn set_assistant_base_url(state: &mut AppState, base_url: &str) {
    let trimmed = base_url.trim();
    state.config.assistant.base_url = if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    };
    state.ui.agent.model_fetch = ModelFetchStatus::Idle;
    persist(state);
}

/// Store the active provider's API key in the app key store (never in config).
pub fn set_assistant_api_key(state: &mut AppState, key: &str) {
    let provider = registry::active_provider(&state.config.assistant);
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
    let available =
        registry::api_key_for(registry::active_provider(&state.config.assistant)).is_some();
    state.ui.agent.key_available = Some(available);
}

/// Kick off a live `/models` fetch for the active provider. Resolves the key the
/// same way a turn does (env → key store); with no key it records an error
/// instead of spawning. The result is drained in [`poll_model_fetch`]. A fetch
/// already in flight is left to finish.
pub fn fetch_models(state: &mut AppState, ctx: &egui::Context) {
    if state.jobs.model_fetch.is_some() {
        return;
    }
    let spec = registry::active_provider(&state.config.assistant);
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
