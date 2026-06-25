//! Engine-neutral molecular-dynamics model.
//!
//! The data model and engine-neutral builders an MD run rests on: system
//! building, solvation, topology, force-field selection, the stage/parameter
//! model, recommendations, and validation. Both the MD engines (which generate
//! engine input from it) and the MD workflow orchestration build on this layer,
//! so it depends only on `domain`/`io` and carries no engine or workflow
//! dependency.
//!
//! The engine-coupling step — translating this model into a concrete engine
//! stage chain — lives one layer up in
//! [`crate::workflows::molecular_dynamics::protocol`].

pub(crate) mod bonded_graph;
pub mod catalog;
pub mod framework;
pub mod materials;
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
