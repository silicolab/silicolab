//! Engine-neutral "Run MD" model: presets, recommendation, parameter tiers,
//! validation, and layered assembly.
//!
//! This layer expresses a run as an ordered sequence of physical-intent
//! [`stage::MdStage`]s, recommends a [`preset::PresetId`] and values from the
//! inherited [`system_context::MdSystemContext`], and validates the assembled
//! sequence — all without referencing any engine's input syntax. A simulation
//! engine adapter (e.g. the GROMACS adapter) realizes each resolved stage into
//! concrete engine input; the `raw_passthrough` on a stage is the one place
//! arbitrary engine text rides along.
//!
//! The split is deliberate: this module is the *generic* MD-run layer that any
//! engine reuses, mirroring how [`super::topology`] / [`super::system`] stay
//! engine-neutral while the GROMACS specifics live under `engines::gromacs`.

pub mod merge;
pub mod preset;
pub mod recommend;
pub mod stage;
pub mod system_context;
pub mod validate;

pub use merge::{StageEdits, assemble, family_nonbonded_intent};
pub use preset::{PresetId, PresetParams, ProductionLength};
pub use recommend::{RecNote, Recommendation, recommend, recommended_trajectory_frames};
pub use stage::{
    AnnealSpec, BarostatKind, ConstraintScope, CouplingGroups, DEFAULT_TRAJECTORY_FRAMES, Ensemble,
    MdParameters, MdStage, ParamId, ParamTier, PressureCoupling, PressureShape, RestraintScheme,
    StageKind, StageLength, ThermostatKind,
};
pub use system_context::{
    EffectiveContext, ForceFieldFamily, MdSystemContext, SystemTypeOverrides, ValueSource,
};
pub use validate::{IssueSeverity, ValidationIssue, has_errors, validate};
