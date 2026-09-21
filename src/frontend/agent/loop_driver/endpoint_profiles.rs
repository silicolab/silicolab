//! Named profiles of the custom OpenAI-compatible provider. The registry has a
//! single custom row whose live base URL, model and key every turn reads; a
//! profile is a saved copy of those, and activating one swaps it into the live
//! slot so provider construction stays unaware of profiles.

use super::*;

use crate::backend::config::{CUSTOM_ENDPOINT_PROVIDER, endpoint_key_id};
use crate::backend::secrets;
use crate::frontend::agent::registry;
use crate::frontend::state::{AppState, SystemSubsystem};

/// Keep the active profile's key in step with the live custom-provider key.
pub(super) fn mirror_key_to_active_endpoint(
    state: &AppState,
    provider_id: &str,
    key: &str,
) -> Result<(), String> {
    if provider_id != CUSTOM_ENDPOINT_PROVIDER {
        return Ok(());
    }
    match state.config.assistant.active_endpoint() {
        Some(profile) => secrets::set_stored_key(&endpoint_key_id(&profile.id), key),
        None => Ok(()),
    }
}

/// Adopt a custom endpoint configured before profiles existed as a first
/// profile, so creating a second one cannot overwrite it.
pub fn ensure_endpoint_profiles(state: &mut AppState) {
    let assistant = &mut state.config.assistant;
    if !assistant.custom_endpoints.is_empty() {
        return;
    }
    let key = secrets::stored_key(CUSTOM_ENDPOINT_PROVIDER);
    if key.is_none() && !assistant.base_urls.contains_key(CUSTOM_ENDPOINT_PROVIDER) {
        return;
    }
    let id = assistant.add_endpoint("Default", default_custom_model());
    assistant.active_custom_endpoint = Some(id.clone());
    assistant.capture_active_endpoint();
    if let Some(key) = key
        && let Err(error) = secrets::set_stored_key(&endpoint_key_id(&id), &key)
    {
        report(state, format!("Could not copy the stored API key: {error}"));
    }
    persist(state);
}

pub fn new_endpoint_profile(state: &mut AppState, name: &str) {
    let name = name.trim();
    if name.is_empty() {
        return;
    }
    ensure_endpoint_profiles(state);
    let id = state
        .config
        .assistant
        .add_endpoint(name, default_custom_model());
    select_endpoint_profile(state, &id);
}

pub fn select_endpoint_profile(state: &mut AppState, id: &str) {
    state.config.assistant.capture_active_endpoint();
    let Some(model) = state.config.assistant.apply_endpoint(id) else {
        return;
    };
    let key = secrets::stored_key(&endpoint_key_id(id)).unwrap_or_default();
    if let Err(error) = secrets::set_stored_key(CUSTOM_ENDPOINT_PROVIDER, &key) {
        report(state, format!("Could not switch the API key: {error}"));
    }
    state
        .ui
        .agent
        .fetched_models
        .remove(CUSTOM_ENDPOINT_PROVIDER);
    // Persists and refreshes the key status.
    switch_provider_model(state, CUSTOM_ENDPOINT_PROVIDER, &model);
    if let Some(profile) = state.config.assistant.active_endpoint() {
        state.status_success(format!("Using endpoint profile {}.", profile.name));
    }
}

pub fn rename_endpoint_profile(state: &mut AppState, id: &str, name: &str) {
    let name = name.trim();
    if name.is_empty() {
        return;
    }
    let profiles = &mut state.config.assistant.custom_endpoints;
    if let Some(profile) = profiles.iter_mut().find(|profile| profile.id == id) {
        profile.name = name.to_string();
        persist(state);
    }
}

pub fn delete_endpoint_profile(state: &mut AppState, id: &str) {
    let assistant = &mut state.config.assistant;
    let before = assistant.custom_endpoints.len();
    assistant
        .custom_endpoints
        .retain(|profile| profile.id != id);
    if assistant.custom_endpoints.len() == before {
        return;
    }
    if let Err(error) = secrets::clear_stored_key(&endpoint_key_id(id)) {
        report(state, format!("Could not remove the API key: {error}"));
    }
    let assistant = &mut state.config.assistant;
    if assistant.active_custom_endpoint.as_deref() != Some(id) {
        persist(state);
        return;
    }
    assistant.active_custom_endpoint = None;
    if let Some(next) = assistant.custom_endpoints.first().map(|p| p.id.clone()) {
        select_endpoint_profile(state, &next);
        return;
    }
    // The last profile is gone: empty the live slot too, or the next startup
    // would adopt it as a fresh "Default" profile.
    assistant.base_urls.remove(CUSTOM_ENDPOINT_PROVIDER);
    if let Err(error) = secrets::clear_stored_key(CUSTOM_ENDPOINT_PROVIDER) {
        report(state, format!("Could not remove the API key: {error}"));
    }
    persist(state);
    refresh_key_status(state);
}

fn default_custom_model() -> &'static str {
    registry::provider_spec(CUSTOM_ENDPOINT_PROVIDER)
        .and_then(|spec| spec.models.first())
        .map(|model| model.id)
        .unwrap_or_default()
}

fn report(state: &mut AppState, message: String) {
    state.report_system_error(SystemSubsystem::Settings, message);
}
