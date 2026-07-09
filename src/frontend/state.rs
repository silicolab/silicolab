//! Frontend application state: the dockable workbench layout, workspace/sidebar
//! state, per-atom drawing styles, the task launch prompts (QM/optimization, MD,
//! disorder), the Settings drafts, and the top-level [`UiState`]/[`AppState`].
//!
//! The module is split into cohesive submodules and re-exported flat here so the
//! rest of the frontend continues to reference every type as `state::Name`.

mod app;
mod atom_style;
mod chart;
mod disorder_prompts;
mod dock;
mod docking_prompts;
mod engine_drafts;
mod execution;
mod export_prompt;
mod layout;
mod md_prompts;
mod ptm_prompts;
mod qm_prompts;

pub use app::*;
pub use atom_style::*;
pub use chart::*;
pub use disorder_prompts::*;
pub use dock::*;
pub use docking_prompts::*;
pub use engine_drafts::*;
pub use execution::*;
pub use export_prompt::*;
pub use layout::*;
pub use md_prompts::*;
pub use ptm_prompts::*;
pub use qm_prompts::*;

#[cfg(test)]
mod tests;
