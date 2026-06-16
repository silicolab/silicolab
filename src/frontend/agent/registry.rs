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
        id: "gemini",
        label: "Google Gemini",
        kind: ProviderKind::OpenAiCompat,
        // Google's OpenAI-compatible surface: reuses the shared adapter (base-URL
        // swap), no native Gemini transport needed. `/models` lives under this
        // base too, so live fetch works the same as the other compat providers.
        base_url: "https://generativelanguage.googleapis.com/v1beta/openai",
        key_env: "GEMINI_API_KEY",
        reasoning_replay: false,
        models: &[
            ModelSpec {
                id: "gemini-2.5-pro",
                label: "Gemini 2.5 Pro",
                supports_effort: false,
            },
            ModelSpec {
                id: "gemini-2.5-flash",
                label: "Gemini 2.5 Flash",
                supports_effort: false,
            },
            ModelSpec {
                id: "gemini-2.5-flash-lite",
                label: "Gemini 2.5 Flash-Lite",
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
        id: "custom_openai",
        label: "Custom OpenAI-compatible",
        kind: ProviderKind::OpenAiCompat,
        base_url: "https://llm.ducksoft.site/v1",
        key_env: "SILICOLAB_CUSTOM_OPENAI_API_KEY",
        reasoning_replay: false,
        models: &[ModelSpec {
            id: "gpt-5.1",
            label: "gpt-5.1 (edit to your model)",
            supports_effort: false,
        }],
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

/// Where a resolved API key came from — surfaced in the settings UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeySource {
    /// No key required (a keyless local server).
    None,
    /// The provider's environment variable.
    Env,
    /// The app-managed key store file (`~/.silicolab/keys.json`).
    File,
    /// No key found anywhere.
    Missing,
}

/// Read the API key for `provider`: its environment variable first (explicit and
/// robust — the headless/CI path), then the app-managed key store. A keyless
/// provider (empty `key_env`) reports an empty key, which counts as "present".
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
    crate::backend::secrets::stored_key(provider.id).filter(|key| !key.trim().is_empty())
}

/// Where the active key for `provider` resolves from (for display).
pub fn key_source(provider: &ProviderSpec) -> KeySource {
    let env_present = !provider.key_env.is_empty()
        && std::env::var(provider.key_env)
            .ok()
            .is_some_and(|key| !key.trim().is_empty());
    let file_present =
        crate::backend::secrets::stored_key(provider.id).is_some_and(|key| !key.trim().is_empty());
    classify_key_source(provider.key_env.is_empty(), env_present, file_present)
}

/// Pure precedence rule behind [`key_source`]: keyless ⇒ None; else the env var
/// wins over the file store; else Missing. Split out so the precedence is
/// unit-testable without touching the environment or the real key file.
fn classify_key_source(keyless: bool, env_present: bool, file_present: bool) -> KeySource {
    if keyless {
        KeySource::None
    } else if env_present {
        KeySource::Env
    } else if file_present {
        KeySource::File
    } else {
        KeySource::Missing
    }
}

/// Every provider that currently has a usable key, paired with where it resolves
/// from (env var or the file store). Backs the "Stored keys" overview in
/// settings; keyless and key-less providers are omitted.
pub fn stored_keys() -> Vec<(&'static ProviderSpec, KeySource)> {
    PROVIDERS
        .iter()
        .filter_map(|spec| match key_source(spec) {
            source @ (KeySource::Env | KeySource::File) => Some((spec, source)),
            _ => None,
        })
        .collect()
}

/// The model ids to offer for `spec`: its built-in models first (id + label),
/// then any live-fetched ids not already listed (the id used as its own label).
/// Keeps the curated order/labels while surfacing fresh ids from the provider's
/// `/models` endpoint; de-duplicates on id.
pub fn merged_model_ids(spec: &ProviderSpec, fetched: &[String]) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = spec
        .models
        .iter()
        .map(|model| (model.id.to_string(), model.label.to_string()))
        .collect();
    for id in fetched {
        let id = id.trim();
        if !id.is_empty() && !out.iter().any(|(existing, _)| existing == id) {
            out.push((id.to_string(), id.to_string()));
        }
    }
    out
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

    #[test]
    fn custom_openai_provider_is_compatibility_first() {
        let custom = provider_spec("custom_openai").expect("custom provider exists");
        assert_eq!(custom.kind, ProviderKind::OpenAiCompat);
        assert_eq!(custom.key_env, "SILICOLAB_CUSTOM_OPENAI_API_KEY");
        assert_eq!(custom.base_url, "https://llm.ducksoft.site/v1");
        assert!(!custom.caps_for("gpt-5.1").supports_effort);
        assert!(!custom.caps_for("some-free-typed-model").supports_effort);
    }

    #[test]
    fn gemini_provider_is_openai_compatible() {
        let gemini = provider_spec("gemini").expect("gemini provider exists");
        assert_eq!(gemini.kind, ProviderKind::OpenAiCompat);
        assert_eq!(gemini.key_env, "GEMINI_API_KEY");
        assert!(
            gemini
                .base_url
                .contains("generativelanguage.googleapis.com")
        );
        assert!(!gemini.models.is_empty());
    }

    #[test]
    fn key_source_precedence_env_over_file() {
        // Keyless provider needs nothing.
        assert_eq!(classify_key_source(true, false, false), KeySource::None);
        // The env var wins even when a file key is also present.
        assert_eq!(classify_key_source(false, true, true), KeySource::Env);
        // File store is used when only it has a key.
        assert_eq!(classify_key_source(false, false, true), KeySource::File);
        // Nothing anywhere.
        assert_eq!(classify_key_source(false, false, false), KeySource::Missing);
    }

    #[test]
    fn merged_model_ids_keeps_static_first_and_dedups() {
        let openai = provider_spec("openai").unwrap();
        let fetched = vec![
            "gpt-5.1".to_string(), // already a built-in id
            "gpt-6-future".to_string(),
            "  ".to_string(), // blank ids are ignored
        ];
        let merged = merged_model_ids(openai, &fetched);

        // Built-in ids come first, in their declared order.
        assert_eq!(merged[0].0, "gpt-5.1");
        assert_eq!(merged[0].1, "GPT-5.1");
        // The already-present id is not duplicated.
        assert_eq!(merged.iter().filter(|(id, _)| id == "gpt-5.1").count(), 1);
        // A genuinely new id is appended, with the id as its own label.
        assert!(
            merged
                .iter()
                .any(|(id, label)| id == "gpt-6-future" && label == "gpt-6-future")
        );
        // Blank ids are dropped.
        assert!(!merged.iter().any(|(id, _)| id.trim().is_empty()));
    }
}
