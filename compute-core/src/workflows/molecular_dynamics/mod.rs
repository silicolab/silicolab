//! Molecular-dynamics workflow: the engine-coupling step.
//!
//! The engine-neutral MD model — system building, topology, solvation, force
//! fields, the stage/parameter model, recommendations, and validation — now
//! lives one layer down in [`crate::md`]. This module keeps only [`protocol`],
//! which translates that model into a concrete engine stage chain, and
//! re-exports the `md` model so existing `workflows::molecular_dynamics::…`
//! paths keep resolving for the GUI and other callers.

pub mod protocol;

// Glob (not a named list) on purpose: external callers — including the GUI crate —
// reach the model through this facade by both item name and submodule path
// (`molecular_dynamics::run::…`, `::solvation::…`). The glob re-exports `md`'s
// submodules too, which a named list would not; do not narrow it.
pub use crate::md::*;
pub use protocol::{
    MdProtocolOptions, STAGE_EM, STAGE_NPT, STAGE_NVT, STAGE_PROD, apply_trajectory_output,
    equilibration_stages, full_protocol, production_stage,
};
