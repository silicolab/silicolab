//! Schema-driven settings registry.
//!
//! Each user setting is described once as a [`SettingDescriptor`] and the
//! Settings UI is *generated* from those descriptors rather than hand-coded per
//! control. This keeps the single-mutator invariant intact: a descriptor only
//! declares how to **read** the current value and which [`AppAction`] to
//! **emit** on change — the mutation itself still happens in
//! `dispatcher.rs::dispatch`. Controls carry plain function pointers (not
//! closures), so they cannot capture and smuggle in a mutation path.
//!
//! The whole Settings panel is sourced here: a two-level category → group
//! structure (General ▸ Appearance / Startup & Projects; Representation ▸ Base /
//! Cartoon / Surface / Color Schemes; Engines; Tasks; Advanced ▸
//! Configuration). The Engines editor and the Advanced meta-settings are wrapped
//! wholesale as [`Control::Custom`] rather than rebuilt; the Representation page
//! lives in `settings_representation`. The modal (`settings_modal`) renders one
//! category at a time from these descriptors, or a flat cross-category list
//! while a search is active.
//!
//! Layout: [`schema`] holds the descriptor/control types; [`accessors`] the
//! General-group read/change/reset function pointers; [`custom`] the
//! `Control::Custom` editors (assistant, paths, Advanced meta-settings); and
//! [`render`] builds the registry and turns descriptors into widgets.
//!
//! [`AppAction`]: crate::frontend::actions::AppAction

mod accessors;
mod custom;
mod render;
mod schema;

pub(crate) use accessors::*;
pub(crate) use custom::*;
pub use render::*;
pub use schema::*;
