//! The provider-agnostic agent: session state, the poll-driven turn loop, the
//! tool surface, and the data-driven provider registry. Depends on the neutral
//! `io/llm` boundary and on `AppState`; the UI (`ui/bottom_panel` Chat tab) and
//! settings (`ui/settings_registry` Assistant) sit above it.

pub mod loop_driver;
pub mod registry;
pub mod session;
pub mod tools;

#[cfg(test)]
mod mock;

pub use loop_driver::{
    approve_tool_call, cancel_agent, clear_assistant_api_key, poll_agent_heavy, poll_agent_turn,
    refresh_key_status, reject_tool_call, send_agent_message, set_assistant_api_key,
    set_assistant_base_url, set_assistant_effort, set_assistant_enabled, switch_provider_model,
};
pub use session::{AgentPhase, AgentSession, TranscriptEntry};
