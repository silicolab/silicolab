//! GROMACS engine integration.
//!
//! Workflows are modelled as **system preparation** ([`prepare_system`])
//! followed by one or more **stages** ([`run_stage`]) -- energy minimization,
//! NVT/NPT equilibration, and production MD all flow through the same code
//! path and differ only in their [`MdpSettings`]. [`render_top`] turns an
//! engine-neutral `MdTopology` into the GROMACS `.top` a run needs; [`analysis`]
//! parses the `.edr`/`.xvg` output of a finished run.

pub mod analysis;
pub mod build;
pub mod carb_topology;
pub mod custom_ff;
pub mod exec;
pub mod forcefield_assets;
pub mod glycoprotein_topology;
pub mod input;
pub mod material;
pub mod nonbonded;
pub mod output;
pub mod realize;
pub mod runner;
pub mod topgen;
pub mod topology;

pub use analysis::{AnalysisContext, Xvg, energy_term_selection, gmx_energy, parse_xvg};
pub use build::{BuildOutcome, BuildRequest, IonOptions, build_system};
pub use input::{
    Annealing, Barostat, FreezeGroup, Integrator, MdpSettings, OutputFrequency, Thermostat,
};
pub use material::{
    FRAMEWORK_FREEZE_GROUP, FrameworkRunHints, MaterialBuildOutcome, MaterialBuildRequest,
    build_material_system, framework_freeze_selection, framework_run_hints,
};
pub use nonbonded::{NonbondedScheme, force_field_block};
pub use realize::stage_specs_from_md_stages;
pub use runner::{
    FileRef, FreezeSelection, GromacsProgress, PrepareSystemRequest, PreparedSystem, StageFileRole,
    StageLinks, StageRequest, StageResult, StageSpec, prepare_system, run_pipeline, run_stage,
};
pub use topgen::render_top;
pub use topology::TopologySource;
