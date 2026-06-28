//! Persisted settings for the in-app LLM assistant: provider/model selection and
//! the command-approval policy. Re-exported from [`super::config`].

use serde::{Deserialize, Serialize};

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

/// Settings for the in-app LLM assistant. Holds only non-secret selection: the
/// provider id, model, effort, an optional `base_url` override for
/// OpenAI-compatible providers, and the command-approval policy. **The API key is
/// never stored here** — it is read from the provider's environment variable at
/// call time (see `frontend::agent::registry`), preserving the
/// no-secrets-in-config invariant SSH already follows.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssistantConfig {
    /// Whether the assistant is usable (the Assistant tab still renders a hint when a
    /// key is missing). On by default.
    pub enabled: bool,
    /// Active provider id, keyed into `frontend::agent::registry::PROVIDERS`.
    pub provider: String,
    /// Active model id within the selected provider.
    pub model: String,
    /// Reasoning effort; adapters map or drop it per model capability.
    pub effort: crate::io::llm::types::Effort,
    /// Base-URL override for OpenAI-compatible providers. `None` uses
    /// the provider's registry default. Non-secret.
    #[serde(default)]
    pub base_url: Option<String>,
    /// Per-model override for whether the active OpenAI-compatible model accepts
    /// a reasoning-effort knob. `None` uses the registry heuristic (known model
    /// → its declared capability; unknown / free-typed id → assume yes); `Some`
    /// pins it. Lets users point a custom endpoint at a reasoning model the
    /// built-in table can't know about — or silence the picker for one that
    /// rejects the knob. Reset when the model or provider changes. Non-secret.
    #[serde(default)]
    pub effort_override: Option<bool>,
    /// How much of what the assistant proposes auto-runs. `#[serde(default)]` so
    /// an older `settings.json` parses to the default (AutoSafe).
    #[serde(default)]
    pub approval_mode: ApprovalMode,
}

impl Default for AssistantConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            provider: "anthropic".to_string(),
            // Sonnet 4.6: cheaper/faster than Opus, very strong tool use — the
            // recommended default driver (Opus 4.8 remains selectable).
            model: "claude-sonnet-4-6".to_string(),
            effort: crate::io::llm::types::Effort::High,
            base_url: None,
            effort_override: None,
            approval_mode: ApprovalMode::default(),
        }
    }
}
