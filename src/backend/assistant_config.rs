//! Persisted settings for the in-app LLM assistant: new-conversation defaults,
//! provider options, and the command-approval policy.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssistantModelSelection {
    pub provider: String,
    pub model: String,
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
}

impl Default for AssistantConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            default_selection: AssistantModelSelection::default(),
            effort: crate::io::llm::types::Effort::High,
            base_urls: Default::default(),
            model_effort_overrides: Default::default(),
            approval_mode: ApprovalMode::default(),
            external_agent_access: ExternalAgentAccess::default(),
            external_agent_executables: Default::default(),
        }
    }
}
