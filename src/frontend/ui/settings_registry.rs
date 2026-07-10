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
//! structure (General ▸ Appearance / Startup & Projects; Compute ▸ Compute
//! targets / Built-in engines / Defaults for new jobs / Monitoring; Representation
//! ▸ Base / Cartoon / Surface / Color Schemes; Assistant; Advanced ▸
//! Configuration). The compute-targets editor, built-in engines, and Advanced
//! meta-settings are wrapped wholesale as [`Control::Custom`] rather than
//! rebuilt; the Representation page lives in `settings_representation`. A group
//! may nest a collapsible [`Subgroup`] (e.g. Advanced's "Danger zone"). The modal
//! (`settings_modal`) renders one category at a time from these descriptors, or a
//! flat cross-category list while a search is active.
//!
//! Layout: [`schema`] holds the descriptor/control types; [`accessors`] the
//! General-group read/change/reset function pointers; [`custom`] the
//! `Control::Custom` editors (assistant, paths, Advanced meta-settings);
//! [`catalog`] assembles the descriptors into the registry; and [`render`] turns
//! them into widgets.
//!
//! [`AppAction`]: crate::frontend::actions::AppAction

mod accessors;
mod catalog;
mod custom;
pub(crate) mod hardware;
mod render;
mod schema;

pub(crate) use accessors::*;
pub use catalog::*;
pub(crate) use custom::*;
pub use render::*;
pub use schema::*;
