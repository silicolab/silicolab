//! The packing engine: rigid-body gradient descent against a smooth
//! overlap-penalty objective, with a cell-list, incremental numeric gradient,
//! backtracking line search, and worst-copy random restarts.
//!
//! Each copy is a rigid body with 6 DOF (translation + an axis-angle rotation
//! re-anchored each step); single-atom species drop the degenerate rotation
//! DOF. The objective is
//!
//! ```text
//! f(x) = Σ_{close pairs, different copy} ((tol² − d²)/tol²)²  +  w · Σ_i penetration(atom_i)²
//! ```
//!
//! The overlap term has compact support (exactly 0 once `d ≥ tol`), which makes
//! the cell-list exact and the incremental gradient cheap: perturbing one copy
//! only re-scores its cell-list neighborhood. The optimizer scaffold mirrors the
//! forcefield optimizer's numeric-gradient + 12-step backtracking line search
//! (`engines/forcefield/uff.rs`). All randomness flows through the crate's
//! `splitmix64` PRNG keyed on `(seed, restart, copy)`, so a packing is
//! bit-for-bit reproducible.

mod cell_list;
mod optimize;
mod rng;
mod scoring;
mod setup;
mod types;

pub(crate) use cell_list::*;
pub(crate) use rng::*;
pub(crate) use types::*;

#[cfg(test)]
mod tests;

use std::{
    sync::{Arc, atomic::AtomicBool},
    time::Duration,
};

use anyhow::{Result, bail};

use crate::domain::UnitCell;

use super::assemble;
use super::region::Region;
use super::{PackProgress, PackReport, PackRequest, PackResult};

/// Sentinel copy id for immovable fixed/obstacle atoms.
const FIXED: u32 = u32::MAX;
/// Finite-difference epsilon for translation (Å) and rotation (rad).
const EPSILON: f32 = 1.0e-3;
/// Maximum backtracking halvings per step (matches the forcefield optimizer).
const MAX_BACKTRACK: usize = 12;
/// Initial line-search step size, and the cap it may grow to.
const INITIAL_STEP: f32 = 0.5;
const MAX_STEP: f32 = 5.0;
/// How often to stream an intermediate structure to the caller.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(75);
/// A copy counts as "placed" once its worst residual overlap and boundary
/// penetration are both within this slack (Å).
const PLACE_TOL: f32 = 0.2;
/// Relative weight of the boundary penalty against the (normalized) overlap term.
const REGION_WEIGHT: f32 = 1.0;

/// Entry point used by [`super::pack`].
pub(crate) fn run(
    request: PackRequest,
    cancel: Arc<AtomicBool>,
    progress: impl FnMut(PackProgress) -> Result<()>,
) -> Result<PackResult> {
    if request.tolerance <= 0.0 {
        bail!("packing tolerance must be a positive distance");
    }
    for species in &request.species {
        if species.count > 0 && species.molecule.atoms.is_empty() {
            bail!(
                "cannot pack \"{}\": the molecule has no atoms",
                species.molecule.title
            );
        }
    }
    if request.region.volume_angstrom3() <= 0.0 {
        bail!("the packing region has zero volume; give it nonzero dimensions");
    }

    let total: usize = request.species.iter().map(|species| species.count).sum();
    if total == 0 {
        // Nothing to pack: hand back the obstacle (or an empty structure).
        let cell = result_cell(&request);
        let structure = assemble::assemble(
            &request.species,
            &[],
            request.fixed.as_ref(),
            cell,
            "Disordered system",
        );
        let report = PackReport {
            placed: vec![0; request.species.len()],
            requested: request.species.iter().map(|s| s.count).collect(),
            restarts_used: 0,
            final_penalty: 0.0,
            max_overlap: 0.0,
            converged: true,
            timed_out: false,
            steps: 0,
        };
        return Ok(PackResult { structure, report });
    }

    let packer = Packer::new(request)?;
    packer.optimize(cancel, progress)
}

/// The cell stamped on the result: an explicit `output_cell`, else the periodic
/// cell when the region carries one. A box packed periodically is meaningless
/// without its cell (the minimum-image spacing the packer engineered would read
/// as cross-edge clashes), so its cell is stamped even if `output_cell` is unset.
fn result_cell(request: &PackRequest) -> Option<UnitCell> {
    if let Some(cell) = &request.output_cell {
        return Some(cell.clone());
    }
    match &request.region {
        Region::Cell(cell) => Some(cell.clone()),
        Region::Box { min, max } if request.periodic => {
            let ext = max - min;
            Some(UnitCell::from_parameters(
                ext.x.max(1.0e-3),
                ext.y.max(1.0e-3),
                ext.z.max(1.0e-3),
                90.0,
                90.0,
                90.0,
            ))
        }
        _ => None,
    }
}
