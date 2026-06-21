use serde::{Deserialize, Serialize};

use crate::domain::Structure;

/// Search configuration. Mirrors the `docking` crate's `DockConfig` so that crate
/// type never reaches silicolab's API edge (the same boundary discipline the QM
/// engine keeps around hartree). `Default` matches the AutoDock Vina CLI defaults.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DockingConfig {
    /// Number of independent Monte-Carlo runs (search thoroughness). Higher is
    /// slower and more reliable.
    pub exhaustiveness: usize,
    /// Maximum number of binding modes to return.
    pub num_modes: usize,
    /// Random seed; the search is deterministic for a fixed seed.
    pub seed: u32,
}

impl Default for DockingConfig {
    fn default() -> Self {
        Self {
            exhaustiveness: 8,
            num_modes: 9,
            seed: 0,
        }
    }
}

/// Whether to run the full Monte-Carlo search or only score the input pose.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DockingKind {
    /// Full docking search (`dock`): returns ranked poses.
    Dock,
    /// Single-point score of the ligand's input pose (`--score_only`).
    ScoreOnly,
}

/// A receptor or ligand input. Either an in-app structure to be prepared
/// (best-effort, approximate) or already-prepared PDBQT text passed through
/// verbatim (trustworthy).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DockingInput {
    /// Prepare PDBQT from this structure (heuristic atom typing + torsion tree).
    /// Boxed because a `Structure` is much larger than the `Pdbqt` variant; it
    /// crosses the wire through the `StructurePayload` bridge, never serde directly.
    Structure(#[serde(with = "crate::payload::structure_serde_boxed")] Box<Structure>),
    /// Already-prepared PDBQT, used as-is.
    Pdbqt(String),
}

/// A complete docking request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DockingRequest {
    pub receptor: DockingInput,
    pub ligand: DockingInput,
    /// Search-box center (Å).
    pub box_center: [f64; 3],
    /// Search-box size (Å).
    pub box_size: [f64; 3],
    pub config: DockingConfig,
    pub kind: DockingKind,
}

/// One docked pose (or the single scored pose for `ScoreOnly`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DockedPose {
    /// Estimated free energy of binding (kcal/mol) — the headline affinity.
    pub affinity: f64,
    /// Final intermolecular energy (kcal/mol).
    pub intermolecular: f64,
    /// Final total internal energy (kcal/mol).
    pub internal: f64,
    /// Torsional free-energy penalty (kcal/mol).
    pub torsional: f64,
    /// The docked ligand conformation, parsed back into a structure.
    #[serde(with = "crate::payload::structure_serde")]
    pub structure: Structure,
    /// The raw pose PDBQT (one `MODEL` block), for saving as a run artifact.
    pub pdbqt: String,
}

/// The completed docking calculation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DockingOutcome {
    /// Poses ranked best affinity first (a single pose for `ScoreOnly`).
    pub poses: Vec<DockedPose>,
    /// Caveats about input preparation; empty when both inputs were already
    /// prepared PDBQT.
    pub notes: Vec<String>,
    /// A pre-formatted, human-readable summary (the affinity table).
    pub summary: String,
}
