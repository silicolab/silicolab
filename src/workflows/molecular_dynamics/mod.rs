//! Molecular-dynamics workflows.
//!
//! These compose the domain model and the MD engines into the user-facing MD
//! flow, in engine-neutral terms:
//!
//! * [`system`] — the **MD System Builder**: wrap a structure in a periodic
//!   simulation cell.
//! * [`topology`] — the engine-neutral [`topology::MdTopology`] capturing the
//!   system's chemistry (species + nonbonded parameters + composition), built
//!   at system-build time and reused by any engine.
//! * [`protocol`] — the EM → NVT → NPT → production stage chain a run performs.
//!   Callers give physical intent (temperature, time) and it builds the engine
//!   stage chain. This is the deliberate engine-coupling point of this module;
//!   the data layers above it (`topology`, `system`, `solvation`) stay
//!   engine-neutral.
//! * [`solvation`] — geometric, force-field-free solvation: fill the box with
//!   water and add ions ([`solvation::solvate`]), producing element-labelled
//!   coordinates only. A simulation engine parameterizes the system later.
//!
//! Together they realize one flow: the System Builder produces a simulation-
//! ready structure *and* the information a run needs; the simulation stage then
//! has an engine generate every file it requires from that information.

pub mod catalog;
pub mod framework;
pub mod materials;
pub mod protocol;
pub mod run;
pub mod solvation;
pub mod system;
pub mod topology;

pub use catalog::{
    DEFAULT_FORCE_FIELD, FORCE_FIELDS, ForceFieldEntry, SystemContent, classify, force_field_title,
    recommended_force_field,
};
pub use framework::FrameworkMode;
pub use materials::{
    Coverage, CustomTypes, ElementParameterization, FlexibleForceField, MaterialAtomType,
    SolventDefinitions, atom_type, flexible_force_field, framework_coverage, is_framework,
    is_framework_shape, is_framework_with_custom, parameterize_element, solvent_definitions,
    supports_flexible, unparameterized_elements, user_provided_elements,
};

pub use protocol::{
    MdProtocolOptions, STAGE_EM, STAGE_NPT, STAGE_NVT, STAGE_PROD, apply_trajectory_output,
    equilibration_stages, full_protocol, production_stage,
};
pub use run::{
    AnnealSpec, DEFAULT_TRAJECTORY_FRAMES, EffectiveContext, ForceFieldFamily, MdParameters,
    MdStage, MdSystemContext, PresetId, PresetParams, ProductionLength, RecNote, Recommendation,
    RestraintScheme, StageEdits, StageKind, StageLength, SystemTypeOverrides, ValidationIssue,
    ValueSource,
};
pub use solvation::{
    SolvationEstimate, SolvationOptions, SolvationReport, WaterModel, estimate, solvate,
};
pub use system::{
    BoxShape, BoxSizing, DEFAULT_CUTOFF_NM, DEFAULT_PADDING_ANGSTROM, MdSystemConfig,
    MdSystemPreview, MdSystemReport, build_md_system, cell_inradius_angstrom,
    ensure_periodic_cutoff_fits, preview, preview_edges, set_slab_c_axis,
};
pub use topology::{
    BondedParam, BondedTerm, MdTopology, MoleculeAtom, MoleculeRun, MoleculeType, SettleGeometry,
    Species, TopologyDefaults, molecule_name,
};
