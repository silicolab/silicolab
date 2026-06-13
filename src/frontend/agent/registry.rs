//! Data-driven provider/model table. Adding a provider (GLM, DeepSeek,
//! OpenRouter, a local model) is a row here plus reuse of the OpenAI-compatible
//! adapter — not new loop code.

use crate::backend::config::AssistantConfig;
use crate::io::llm::anthropic::{AnthropicProvider, caps_for_model};
use crate::io::llm::openai_compat::OpenAiCompatProvider;
use crate::io::llm::provider::{LlmProvider, ProviderCaps};

/// How a provider speaks: a native protocol (Anthropic) or the shared
/// OpenAI-compatible `chat/completions` shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderKind {
    Native,
    OpenAiCompat,
}

/// One selectable model within a provider.
#[derive(Debug, Clone, Copy)]
pub struct ModelSpec {
    pub id: &'static str,
    pub label: &'static str,
    /// Whether to send a `reasoning_effort` knob for this model (OpenAI-compatible
    /// reasoning models). Ignored for native Anthropic, whose caps come from the
    /// model id directly.
    pub supports_effort: bool,
}

/// A provider row: id, transport kind, default base URL, the environment
/// variable its API key is read from, and its models.
#[derive(Debug, Clone, Copy)]
pub struct ProviderSpec {
    pub id: &'static str,
    pub label: &'static str,
    pub kind: ProviderKind,
    /// Default endpoint. An OpenAI-compatible provider may be overridden per
    /// install via `AssistantConfig::base_url` (e.g. a self-hosted gateway).
    pub base_url: &'static str,
    /// Environment variable holding the API key (never stored in config). Empty
    /// for a keyless local server.
    pub key_env: &'static str,
    /// Whether the prior assistant's reasoning must be replayed on the wire
    /// (DeepSeek thinking mode 400s otherwise). Harmless when the stored blob
    /// carries no reasoning.
    pub reasoning_replay: bool,
    pub models: &'static [ModelSpec],
}

impl ProviderSpec {
    /// Resolved capabilities for one of this provider's models. Native Anthropic
    /// derives them from the model id; OpenAI-compatible reads the model row's
    /// `supports_effort` (defaulting off for an unknown / free-typed id).
    pub fn caps_for(&self, model: &str) -> ProviderCaps {
        match self.kind {
            ProviderKind::Native => caps_for_model(model),
            ProviderKind::OpenAiCompat => {
                let supports_effort = self
                    .models
                    .iter()
                    .find(|spec| spec.id == model)
                    .is_some_and(|spec| spec.supports_effort);
                ProviderCaps {
                    supports_effort,
                    supports_thinking: supports_effort,
                    // These providers cache automatically (or not); the adapter
                    // does not place vendor cache breakpoints.
                    supports_prompt_cache: false,
                    supports_streaming: false,
                }
            }
        }
    }
}

/// The provider table. Anthropic is native; the rest share one OpenAI-compatible
/// adapter (base-URL swap). Model ids drift — they are editable as free text in
/// the Assistant settings, and `base_url` is overridable per install.
pub const PROVIDERS: &[ProviderSpec] = &[
    ProviderSpec {
        id: "anthropic",
        label: "Anthropic (Claude)",
        kind: ProviderKind::Native,
        base_url: "https://api.anthropic.com/v1",
        key_env: "ANTHROPIC_API_KEY",
        reasoning_replay: false,
        models: &[
            ModelSpec {
                id: "claude-sonnet-4-6",
                label: "Claude Sonnet 4.6 (recommended)",
                supports_effort: true,
            },
            ModelSpec {
                id: "claude-opus-4-8",
                label: "Claude Opus 4.8 (most capable)",
                supports_effort: true,
            },
            ModelSpec {
                id: "claude-haiku-4-5",
                label: "Claude Haiku 4.5 (fastest)",
                supports_effort: false,
            },
        ],
    },
    ProviderSpec {
        id: "openai",
        label: "OpenAI (GPT)",
        kind: ProviderKind::OpenAiCompat,
        base_url: "https://api.openai.com/v1",
        key_env: "OPENAI_API_KEY",
        reasoning_replay: false,
        models: &[
            ModelSpec {
                id: "gpt-5.1",
                label: "GPT-5.1",
                supports_effort: true,
            },
            ModelSpec {
                id: "gpt-5.1-mini",
                label: "GPT-5.1 mini",
                supports_effort: true,
            },
            ModelSpec {
                id: "gpt-4.1",
                label: "GPT-4.1",
                supports_effort: false,
            },
        ],
    },
    ProviderSpec {
        id: "deepseek",
        label: "DeepSeek",
        kind: ProviderKind::OpenAiCompat,
        base_url: "https://api.deepseek.com",
        key_env: "DEEPSEEK_API_KEY",
        reasoning_replay: true,
        models: &[
            ModelSpec {
                id: "deepseek-chat",
                label: "DeepSeek Chat",
                supports_effort: false,
            },
            ModelSpec {
                id: "deepseek-reasoner",
                label: "DeepSeek Reasoner (thinking)",
                supports_effort: false,
            },
        ],
    },
    ProviderSpec {
        id: "glm",
        label: "GLM (Z.ai)",
        kind: ProviderKind::OpenAiCompat,
        base_url: "https://api.z.ai/api/openai/v1",
        key_env: "ZAI_API_KEY",
        reasoning_replay: false,
        models: &[
            ModelSpec {
                id: "glm-4.6",
                label: "GLM-4.6",
                supports_effort: false,
            },
            ModelSpec {
                id: "glm-5",
                label: "GLM-5",
                supports_effort: false,
            },
        ],
    },
    ProviderSpec {
        id: "openrouter",
        label: "OpenRouter",
        kind: ProviderKind::OpenAiCompat,
        base_url: "https://openrouter.ai/api/v1",
        key_env: "OPENROUTER_API_KEY",
        reasoning_replay: false,
        models: &[
            ModelSpec {
                id: "anthropic/claude-sonnet-4.6",
                label: "Claude Sonnet 4.6 (via OpenRouter)",
                supports_effort: false,
            },
            ModelSpec {
                id: "deepseek/deepseek-chat",
                label: "DeepSeek Chat (via OpenRouter)",
                supports_effort: false,
            },
        ],
    },
    ProviderSpec {
        id: "local",
        label: "Local (Ollama / vLLM)",
        kind: ProviderKind::OpenAiCompat,
        base_url: "http://localhost:11434/v1",
        // Keyless: a local server ignores the bearer token.
        key_env: "",
        reasoning_replay: false,
        models: &[ModelSpec {
            id: "llama3.1",
            label: "llama3.1 (edit to your model)",
            supports_effort: false,
        }],
    },
];

/// Look up a provider by id.
pub fn provider_spec(id: &str) -> Option<&'static ProviderSpec> {
    PROVIDERS.iter().find(|provider| provider.id == id)
}

/// The provider a config points at, falling back to the first row if its id is
/// unknown (e.g. a hand-edited or future-version `settings.json`).
pub fn active_provider(config: &AssistantConfig) -> &'static ProviderSpec {
    provider_spec(&config.provider).unwrap_or(&PROVIDERS[0])
}

/// The effective base URL for a config: its override, else the provider default.
pub fn effective_base_url(config: &AssistantConfig, spec: &ProviderSpec) -> String {
    config
        .base_url
        .as_deref()
        .map(str::trim)
        .filter(|url| !url.is_empty())
        .unwrap_or(spec.base_url)
        .to_string()
}

/// The OS-keychain service name under which assistant keys are stored, keyed by
/// provider id.
const KEYCHAIN_SERVICE: &str = "silicolab-assistant";

/// Where a resolved API key came from — surfaced in the settings UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeySource {
    /// No key required (a keyless local server).
    None,
    /// The provider's environment variable.
    Env,
    /// The OS keychain.
    Keychain,
    /// No key found anywhere.
    Missing,
}

/// Read the API key for `provider`: its environment variable first (explicit and
/// robust — the headless/CI path), then the OS keychain. A keyless provider
/// (empty `key_env`) reports an empty key, which counts as "present".
pub fn api_key_for(provider: &ProviderSpec) -> Option<String> {
    if provider.key_env.is_empty() {
        return Some(String::new());
    }
    if let Ok(key) = std::env::var(provider.key_env) {
        let key = key.trim().to_string();
        if !key.is_empty() {
            return Some(key);
        }
    }
    keychain_key(provider.id).filter(|key| !key.trim().is_empty())
}

/// Where the active key for `provider` resolves from (for display).
pub fn key_source(provider: &ProviderSpec) -> KeySource {
    if provider.key_env.is_empty() {
        return KeySource::None;
    }
    if std::env::var(provider.key_env)
        .ok()
        .is_some_and(|key| !key.trim().is_empty())
    {
        return KeySource::Env;
    }
    if keychain_key(provider.id).is_some_and(|key| !key.trim().is_empty()) {
        return KeySource::Keychain;
    }
    KeySource::Missing
}

/// Read a stored key from the OS keychain (`None` if absent or the backend is
/// unavailable — the env-var path remains the robust fallback).
pub fn keychain_key(provider_id: &str) -> Option<String> {
    keyring::Entry::new(KEYCHAIN_SERVICE, provider_id)
        .ok()?
        .get_password()
        .ok()
}

/// Store a key in the OS keychain for `provider_id`.
pub fn set_keychain_key(provider_id: &str, key: &str) -> Result<(), String> {
    keyring::Entry::new(KEYCHAIN_SERVICE, provider_id)
        .map_err(|error| error.to_string())?
        .set_password(key)
        .map_err(|error| error.to_string())
}

/// Remove a stored key from the OS keychain (a missing entry is not an error).
pub fn clear_keychain_key(provider_id: &str) -> Result<(), String> {
    let entry =
        keyring::Entry::new(KEYCHAIN_SERVICE, provider_id).map_err(|error| error.to_string())?;
    match entry.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

/// Build a provider trait object from the assistant config + its env key, or a
/// user-facing reason it can't be built (unknown provider or missing key).
pub fn build_provider(config: &AssistantConfig) -> Result<Box<dyn LlmProvider>, String> {
    let spec = provider_spec(&config.provider)
        .ok_or_else(|| format!("Unknown assistant provider `{}`.", config.provider))?;
    let key = api_key_for(spec).ok_or_else(|| {
        format!(
            "Set the {} environment variable to use {}.",
            spec.key_env, spec.label
        )
    })?;
    let caps = spec.caps_for(&config.model);
    match spec.kind {
        ProviderKind::Native => Ok(Box::new(AnthropicProvider::new(key, config.model.clone()))),
        ProviderKind::OpenAiCompat => Ok(Box::new(OpenAiCompatProvider::new(
            key,
            effective_base_url(config, spec),
            config.model.clone(),
            caps,
            spec.reasoning_replay,
            spec.id,
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_provider_is_keyless() {
        let local = provider_spec("local").unwrap();
        assert_eq!(api_key_for(local), Some(String::new()));
    }

    #[test]
    fn base_url_override_wins() {
        let spec = provider_spec("deepseek").unwrap();
        let mut config = AssistantConfig {
            provider: "deepseek".into(),
            base_url: Some("https://proxy.internal/v1".into()),
            ..AssistantConfig::default()
        };
        assert_eq!(
            effective_base_url(&config, spec),
            "https://proxy.internal/v1"
        );
        config.base_url = Some("   ".into());
        assert_eq!(effective_base_url(&config, spec), spec.base_url);
    }

    #[test]
    fn caps_track_model_effort_for_openai_compat() {
        let openai = provider_spec("openai").unwrap();
        assert!(openai.caps_for("gpt-5.1").supports_effort);
        assert!(!openai.caps_for("gpt-4.1").supports_effort);
        // Unknown / free-typed model defaults to no effort.
        assert!(!openai.caps_for("some-new-model").supports_effort);
    }
}
