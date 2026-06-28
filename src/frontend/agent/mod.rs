//! The provider-agnostic agent: session state, the poll-driven turn loop, the
//! tool surface, and the data-driven provider registry. Depends on the neutral
//! `io/llm` boundary and on `AppState`; the UI (`ui/panel_bodies` Assistant tab) and
//! settings (`ui/settings_registry` Assistant) sit above it.

pub mod loop_driver;
pub mod registry;
pub mod session;
pub mod tools;

#[cfg(test)]
mod mock;

pub use loop_driver::{
    always_allow_command, always_allow_risk, approve_tool_call, cancel_agent, cancel_agent_job,
    clear_stored_key, delete_assistant_conversation, fetch_models, gated_pending, impact_hint,
    new_assistant_conversation, poll_agent_jobs, poll_agent_turn, poll_model_fetch,
    refresh_key_status, reject_tool_call, remove_queued_agent_input, rename_assistant_conversation,
    send_agent_message, set_approval_mode, set_assistant_api_key, set_assistant_base_url,
    set_assistant_effort, set_assistant_effort_supported, set_assistant_enabled,
    switch_assistant_conversation, switch_provider_model,
};
pub use session::{AgentSession, AssistantConversationId, ModelFetchStatus, TranscriptEntry};
