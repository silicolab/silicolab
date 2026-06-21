//! Provider-agnostic LLM transport layer.
//!
//! Pure transport + per-provider protocol: no `AppState`, no egui. The agent
//! loop (`frontend/agent`) depends only on [`types`] and the [`provider`] trait;
//! a provider adapter ([`anthropic`], [`openai_compat`]) translates to and from
//! vendor JSON entirely behind that boundary.

pub mod anthropic;
pub mod openai_compat;
pub mod provider;
pub mod retry;
pub mod types;
