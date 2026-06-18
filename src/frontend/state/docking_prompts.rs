//! User-editable configuration for the Molecular Docking task panel.
//!
//! The form picks a receptor and ligand entry, a search box (center + size), and
//! the search parameters. The dispatcher resolves the chosen entries into
//! structures and builds the engine request when the user launches the run, so
//! this struct carries no engine types.

/// Draft state for the Molecular Docking panel.
#[derive(Debug, Clone)]
pub struct DockingPrompt {
    /// The receptor entry (kept rigid).
    pub receptor_entry: Option<u64>,
    /// The ligand entry (flexible; the docked molecule).
    pub ligand_entry: Option<u64>,
    /// Search-box center (Å).
    pub box_center: [f32; 3],
    /// Search-box size (Å) on each axis.
    pub box_size: [f32; 3],
    /// Number of independent Monte-Carlo runs (higher = slower, more reliable).
    pub exhaustiveness: u32,
    /// Maximum number of binding modes to return.
    pub num_modes: u32,
    /// Random seed (deterministic search for a fixed seed).
    pub seed: u32,
    /// Score the ligand's input pose only, skipping the search.
    pub score_only: bool,
}

impl Default for DockingPrompt {
    fn default() -> Self {
        Self {
            receptor_entry: None,
            ligand_entry: None,
            box_center: [0.0, 0.0, 0.0],
            // A 22.5 Å cube is a reasonable default that quantizes to Vina's grid.
            box_size: [22.5, 22.5, 22.5],
            exhaustiveness: 8,
            num_modes: 9,
            seed: 0,
            score_only: false,
        }
    }
}
