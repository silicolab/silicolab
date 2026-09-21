//! Persisted settings for the in-app LLM assistant: new-conversation defaults,
//! provider options, and the command-approval policy.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssistantModelSelection {
    pub provider: String,
    pub model: String,
}

/// Provider id of the single bring-your-own OpenAI-compatible registry row.
/// Its live base URL, model and key are what [`EndpointProfile`]s swap.
pub const CUSTOM_ENDPOINT_PROVIDER: &str = "custom_openai";

/// A named, saved configuration of the custom OpenAI-compatible provider. The
/// matching API key lives in the key store under [`endpoint_key_id`], never here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EndpointProfile {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub supports_effort: Option<bool>,
}

/// Key-store id holding the API key of the endpoint profile `profile_id`.
pub fn endpoint_key_id(profile_id: &str) -> String {
    format!("{CUSTOM_ENDPOINT_PROVIDER}:{profile_id}")
}

/// Sandbox posture handed to an external agent CLI. `Controlled` maps to the
/// CLI's read-only/plan mode; `Unrestricted` opts into its approval- and
/// sandbox-bypass flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ExternalAgentAccess {
    #[default]
    Controlled,
    Unrestricted,
}

impl ExternalAgentAccess {
    pub fn all() -> [ExternalAgentAccess; 2] {
        [
            ExternalAgentAccess::Controlled,
            ExternalAgentAccess::Unrestricted,
        ]
    }

    /// Full description for a menu row.
    pub fn label(self) -> &'static str {
        match self {
            ExternalAgentAccess::Controlled => "Controlled — read-only / plan sandbox",
            ExternalAgentAccess::Unrestricted => "Unrestricted — bypass CLI approvals & sandbox",
        }
    }

    /// Compact label for the collapsed picker.
    pub fn short_label(self) -> &'static str {
        match self {
            ExternalAgentAccess::Controlled => "Controlled",
            ExternalAgentAccess::Unrestricted => "Unrestricted",
        }
    }
}

impl Default for AssistantModelSelection {
    fn default() -> Self {
        Self {
            provider: "anthropic".to_string(),
            model: "claude-sonnet-4-6".to_string(),
        }
    }
}

/// How aggressively assistant-issued commands auto-run. Combined with each
/// command's `RiskLevel` (declared in the console grammar) to decide whether a
/// call runs immediately or waits for the user. Destructive commands always
/// prompt, in every mode — the non-bypassable floor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ApprovalMode {
    /// Confirm every structure-editing, file-writing, compute, or destructive
    /// command.
    Manual,
    /// Auto-run read-only and in-memory structure edits; confirm file writes,
    /// compute, and destructive ones. The default.
    #[default]
    AutoSafe,
    /// Auto-run everything except destructive commands.
    Auto,
    /// Never execute — the assistant only proposes commands for the user to run.
    Plan,
}

impl ApprovalMode {
    pub fn all() -> [ApprovalMode; 4] {
        [
            ApprovalMode::Manual,
            ApprovalMode::AutoSafe,
            ApprovalMode::Auto,
            ApprovalMode::Plan,
        ]
    }

    /// Full description for a menu row.
    pub fn label(self) -> &'static str {
        match self {
            ApprovalMode::Manual => "Manual — confirm edits, writes, compute & destructive",
            ApprovalMode::AutoSafe => "Auto (safe) — confirm writes, compute & destructive",
            ApprovalMode::Auto => "Auto — confirm destructive only",
            ApprovalMode::Plan => "Plan — propose only, never run",
        }
    }

    /// Compact label for the collapsed picker.
    pub fn short_label(self) -> &'static str {
        match self {
            ApprovalMode::Manual => "Manual",
            ApprovalMode::AutoSafe => "Auto (safe)",
            ApprovalMode::Auto => "Auto",
            ApprovalMode::Plan => "Plan",
        }
    }
}

/// Settings for the in-app LLM assistant. Holds only non-secret defaults: the
/// selection copied into new conversations, effort, per-provider URL overrides,
/// model capabilities, and the command-approval policy. **The API key is
/// never stored here** — it is read from the provider's environment variable at
/// call time (see `frontend::agent::registry`), preserving the
/// no-secrets-in-config invariant SSH already follows.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssistantConfig {
    /// Whether the assistant is usable (the Assistant tab still renders a hint when a
    /// key is missing). On by default.
    pub enabled: bool,
    #[serde(default = "default_auto_diagnose_qm_issues")]
    pub auto_diagnose_qm_issues: bool,
    /// Provider and model copied into each newly-created conversation.
    pub default_selection: AssistantModelSelection,
    /// Reasoning effort; adapters map or drop it per model capability.
    pub effort: crate::io::llm::types::Effort,
    /// Base-URL overrides keyed by provider id. Missing uses the registry default.
    #[serde(default)]
    pub base_urls: std::collections::BTreeMap<String, String>,
    /// Capability overrides keyed by provider, then model id.
    #[serde(default)]
    pub model_effort_overrides:
        std::collections::BTreeMap<String, std::collections::BTreeMap<String, bool>>,
    /// How much of what the assistant proposes auto-runs. `#[serde(default)]` so
    /// an older `settings.json` parses to the default (AutoSafe).
    #[serde(default)]
    pub approval_mode: ApprovalMode,
    #[serde(default)]
    pub external_agent_access: ExternalAgentAccess,
    #[serde(default)]
    pub external_agent_executables: std::collections::BTreeMap<String, String>,
    /// Saved configurations of the custom OpenAI-compatible provider.
    #[serde(default)]
    pub custom_endpoints: Vec<EndpointProfile>,
    /// The profile mirrored by the live custom-provider settings; edits to those
    /// settings are captured back into it.
    #[serde(default)]
    pub active_custom_endpoint: Option<String>,
}

impl Default for AssistantConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            auto_diagnose_qm_issues: true,
            default_selection: AssistantModelSelection::default(),
            effort: crate::io::llm::types::Effort::High,
            base_urls: Default::default(),
            model_effort_overrides: Default::default(),
            approval_mode: ApprovalMode::default(),
            external_agent_access: ExternalAgentAccess::default(),
            external_agent_executables: Default::default(),
            custom_endpoints: Vec::new(),
            active_custom_endpoint: None,
        }
    }
}

impl AssistantConfig {
    pub fn active_endpoint(&self) -> Option<&EndpointProfile> {
        let id = self.active_custom_endpoint.as_deref()?;
        self.custom_endpoints
            .iter()
            .find(|profile| profile.id == id)
    }

    fn custom_model(&self) -> Option<String> {
        (self.default_selection.provider == CUSTOM_ENDPOINT_PROVIDER)
            .then(|| self.default_selection.model.clone())
    }

    /// Copy the live custom-provider settings into the active profile.
    pub fn capture_active_endpoint(&mut self) {
        let Some(id) = self.active_custom_endpoint.clone() else {
            return;
        };
        let base_url = self
            .base_urls
            .get(CUSTOM_ENDPOINT_PROVIDER)
            .cloned()
            .unwrap_or_default();
        let model = self.custom_model();
        let supports_effort = model.as_ref().and_then(|model| {
            self.model_effort_overrides
                .get(CUSTOM_ENDPOINT_PROVIDER)
                .and_then(|models| models.get(model))
                .copied()
        });
        if let Some(profile) = self.custom_endpoints.iter_mut().find(|p| p.id == id) {
            profile.base_url = base_url;
            if let Some(model) = model {
                profile.model = model;
                profile.supports_effort = supports_effort;
            }
        }
    }

    /// Make `id` the active profile and copy it into the live custom-provider
    /// settings. Returns the profile's model, or `None` for an unknown id.
    pub fn apply_endpoint(&mut self, id: &str) -> Option<String> {
        let profile = self.custom_endpoints.iter().find(|p| p.id == id)?.clone();
        if profile.base_url.trim().is_empty() {
            self.base_urls.remove(CUSTOM_ENDPOINT_PROVIDER);
        } else {
            self.base_urls
                .insert(CUSTOM_ENDPOINT_PROVIDER.to_string(), profile.base_url);
        }
        if let Some(supported) = profile.supports_effort {
            self.model_effort_overrides
                .entry(CUSTOM_ENDPOINT_PROVIDER.to_string())
                .or_default()
                .insert(profile.model.clone(), supported);
        }
        self.active_custom_endpoint = Some(profile.id);
        Some(profile.model)
    }

    /// Append an empty profile named `name` and return its id.
    pub fn add_endpoint(&mut self, name: &str, model: &str) -> String {
        let id = (1..)
            .map(|n| format!("p{n}"))
            .find(|id| self.custom_endpoints.iter().all(|p| &p.id != id))
            .unwrap_or_default();
        self.custom_endpoints.push(EndpointProfile {
            id: id.clone(),
            name: name.trim().to_string(),
            base_url: String::new(),
            model: model.to_string(),
            supports_effort: None,
        });
        id
    }
}

fn default_auto_diagnose_qm_issues() -> bool {
    true
}

#[cfg(test)]
mod endpoint_tests {
    use super::*;

    #[test]
    fn old_settings_without_profiles_parse() {
        let mut value = serde_json::to_value(AssistantConfig::default()).unwrap();
        let object = value.as_object_mut().unwrap();
        object.remove("custom_endpoints");
        object.remove("active_custom_endpoint");
        let config: AssistantConfig = serde_json::from_value(value).unwrap();
        assert!(config.custom_endpoints.is_empty());
        assert!(config.active_custom_endpoint.is_none());
    }

    #[test]
    fn switching_profiles_round_trips_live_settings() {
        let mut config = AssistantConfig::default();
        let first = config.add_endpoint("First", "gpt-5.5");
        config.apply_endpoint(&first);
        config.default_selection = AssistantModelSelection {
            provider: CUSTOM_ENDPOINT_PROVIDER.to_string(),
            model: "model-a".to_string(),
        };
        config.base_urls.insert(
            CUSTOM_ENDPOINT_PROVIDER.to_string(),
            "https://a.test/v1".to_string(),
        );
        config.capture_active_endpoint();

        let second = config.add_endpoint("Second", "gpt-5.5");
        assert_ne!(first, second);
        assert_eq!(config.apply_endpoint(&second).as_deref(), Some("gpt-5.5"));
        assert!(!config.base_urls.contains_key(CUSTOM_ENDPOINT_PROVIDER));

        assert_eq!(config.apply_endpoint(&first).as_deref(), Some("model-a"));
        assert_eq!(
            config
                .base_urls
                .get(CUSTOM_ENDPOINT_PROVIDER)
                .map(String::as_str),
            Some("https://a.test/v1")
        );
        assert!(config.apply_endpoint("missing").is_none());
    }
}

#[cfg(test)]
mod qm_tests {
    #[test]
    fn diagnosis_defaults_on_and_explicit_off_round_trips() {
        let mut value = serde_json::to_value(super::AssistantConfig::default()).unwrap();
        value
            .as_object_mut()
            .unwrap()
            .remove("auto_diagnose_qm_issues");
        let mut config: super::AssistantConfig = serde_json::from_value(value).unwrap();
        assert!(config.auto_diagnose_qm_issues);
        config.auto_diagnose_qm_issues = false;
        let saved = serde_json::to_string(&config).unwrap();
        assert!(
            !serde_json::from_str::<super::AssistantConfig>(&saved)
                .unwrap()
                .auto_diagnose_qm_issues
        );
    }
}
