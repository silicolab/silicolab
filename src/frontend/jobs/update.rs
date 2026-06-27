use std::sync::mpsc::Receiver;
use std::time::Duration;

use serde_json::Value;

/// The once-per-launch background query of GitHub Releases. No cancel flag:
/// the single HTTP request either answers or times out on its own, and the
/// result is ignored if the handle was dropped.
pub struct RunningUpdateCheck {
    pub receiver: Receiver<anyhow::Result<Option<crate::io::update_check::AvailableUpdate>>>,
}

/// Spawn the update check on a worker thread and return the polling handle.
pub fn spawn_update_check() -> RunningUpdateCheck {
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = sender.send(crate::io::update_check::check_for_update());
    });
    RunningUpdateCheck { receiver }
}

/// An in-flight one-click self-update: the worker downloads the matching
/// release asset and replaces the running executable, then sends the installed
/// version (or the failure). Like [`RunningUpdateCheck`] there is no cancel —
/// the replace is a single blocking operation and the result is ignored if the
/// handle was dropped.
pub struct RunningSelfUpdate {
    pub receiver: Receiver<anyhow::Result<String>>,
}

/// Spawn the download-and-replace on a worker thread and return the polling
/// handle. The blocking work lives entirely in [`crate::io::self_update`].
pub fn spawn_self_update() -> RunningSelfUpdate {
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = sender.send(crate::io::self_update::perform_update());
    });
    RunningSelfUpdate { receiver }
}

/// Model-list fetch tuning. The list is tiny, so a tight cap and a short
/// timeout keep a slow or wrong endpoint from hanging the Refresh button.
const MODEL_FETCH_TIMEOUT: Duration = Duration::from_secs(30);
const MODEL_FETCH_MAX_BYTES: u64 = 4 * 1024 * 1024;

/// An in-flight live model-list fetch for one provider's `/models` endpoint.
/// Like [`RunningUpdateCheck`] there is no cancel flag: it is a single bounded
/// HTTP request that answers or times out on its own, and a late result lands on
/// a closed channel if the handle was dropped. `provider_id` tags which provider
/// the resulting ids belong to so a stale answer for a since-switched provider
/// can be ignored.
pub struct RunningModelFetch {
    pub provider_id: String,
    pub receiver: Receiver<Result<Vec<String>, String>>,
}

/// Spawn a one-off `/models` query on a worker thread and return the polling
/// handle. The blocking HTTP lives here (network takes a moment); the driver
/// drains it in `poll_model_fetch`. OpenAI-compatible providers (incl. Gemini)
/// read `GET {base_url}/models` with a Bearer token; native Anthropic reads
/// `GET https://api.anthropic.com/v1/models` with `x-api-key` +
/// `anthropic-version`. Both list ids under `data[].id`.
pub fn spawn_model_fetch(
    provider_id: String,
    kind: crate::frontend::agent::registry::ProviderKind,
    base_url: String,
    api_key: String,
) -> RunningModelFetch {
    let (sender, receiver) = std::sync::mpsc::channel();
    let handle_id = provider_id.clone();
    std::thread::spawn(move || {
        let _ = sender.send(fetch_model_ids(kind, &base_url, &api_key));
    });
    RunningModelFetch {
        provider_id: handle_id,
        receiver,
    }
}

/// Blocking `/models` GET shared by both transports. Returns the parsed ids, or
/// a short user-facing error string on a transport / HTTP / parse failure.
fn fetch_model_ids(
    kind: crate::frontend::agent::registry::ProviderKind,
    base_url: &str,
    api_key: &str,
) -> Result<Vec<String>, String> {
    use crate::frontend::agent::registry::ProviderKind;

    let config = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(MODEL_FETCH_TIMEOUT))
        .build();
    let agent = ureq::Agent::new_with_config(config);

    // The model-list fetch sends the same bearer key as the completion transport,
    // so it gates on the same rule: never put the key on the wire in cleartext.
    // (Native targets a fixed https endpoint, so it is always safe.)
    if matches!(kind, ProviderKind::OpenAiCompat)
        && !compute_core::io::llm::endpoint_is_safe(base_url)
    {
        return Err(format!(
            "refusing to send the API key to {base_url} over plaintext HTTP; \
             use an https:// base URL (http:// is allowed only for a localhost endpoint)"
        ));
    }

    let response = match kind {
        ProviderKind::OpenAiCompat => agent
            .get(format!("{}/models", base_url.trim_end_matches('/')))
            .header("authorization", &format!("Bearer {api_key}"))
            .call(),
        // The Anthropic models list lives at the fixed API root; its version
        // header matches the completions adapter (`anthropic.rs`).
        ProviderKind::Native => agent
            .get("https://api.anthropic.com/v1/models")
            .header("x-api-key", api_key)
            .header("anthropic-version", "2023-06-01")
            .call(),
    };

    let mut response = response.map_err(|error| error.to_string())?;
    let status = response.status().as_u16();
    let text = response
        .body_mut()
        .with_config()
        .limit(MODEL_FETCH_MAX_BYTES)
        .read_to_string()
        .map_err(|error| error.to_string())?;
    interpret_models_response(status, &text)
}

/// Turn a `/models` HTTP response into model ids, or a readable error. A
/// non-JSON body (HTML error page, empty, a relay's SPA index) almost always
/// means the Base URL is wrong, so it gets the same "points at the API root
/// (… /v1)" hint the completion path gives — regardless of status, since a wrong
/// URL can 404 to a page as readily as 200 to one. A valid JSON body with a
/// non-200 status is a real API error, surfaced as the status.
pub(crate) fn interpret_models_response(status: u16, body: &str) -> Result<Vec<String>, String> {
    let Ok(json) = serde_json::from_str::<Value>(body) else {
        return Err(crate::io::llm::openai_compat::non_json_response_message(
            body,
        ));
    };
    if status != 200 {
        let message = crate::io::llm::openai_compat::extract_error_message(body);
        if message.trim().is_empty() {
            return Err(format!("provider returned HTTP {status}"));
        }
        return Err(format!("provider returned HTTP {status}: {message}"));
    }
    Ok(parse_model_ids(&json))
}

/// Extract model ids from a `/models` response. Both the OpenAI-compatible and
/// Anthropic list endpoints return `{"data":[{"id":"…"}, …]}`; anything that
/// doesn't match yields an empty list (the caller keeps its static models).
pub fn parse_model_ids(json: &Value) -> Vec<String> {
    json.get("data")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.get("id").and_then(Value::as_str))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}
