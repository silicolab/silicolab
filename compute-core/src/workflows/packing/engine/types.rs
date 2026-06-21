use super::*;

use nalgebra::{Point3, Rotation3, Vector3};

use crate::domain::UnitCell;

use super::super::PackRequest;
use super::super::region::{Region, RegionSense};

/// Per-species rigid template data: atom offsets relative to the centroid.
pub(crate) struct SpeciesData {
    pub offsets: Vec<Vector3<f32>>,
    pub single_atom: bool,
}

/// A placed copy's 6-DOF state.
#[derive(Clone)]
pub(crate) struct CopyState {
    pub species: usize,
    pub center: Point3<f32>,
    pub rotation: Rotation3<f32>,
}

/// A periodic box for the minimum-image overlap path.
pub(crate) struct PeriodicBox {
    pub origin: Point3<f32>,
    pub cell: UnitCell,
}

/// Per-copy gradient (rotation is zero for single-atom species).
#[derive(Clone, Copy)]
pub(crate) struct CopyGrad {
    pub trans: Vector3<f32>,
    pub rot: Vector3<f32>,
}

/// Per-copy diagnostics from a full evaluation.
#[derive(Clone, Copy)]
pub(crate) struct CopyDiag {
    pub blame: f32,
    pub overlap: f32,
    pub penetration: f32,
}

/// The result of scoring the whole system at the current configuration.
pub(crate) struct Eval {
    pub penalty: f64,
    pub max_overlap: f32,
    pub per_copy: Vec<CopyDiag>,
}

pub(crate) struct Packer {
    pub request: PackRequest,
    pub sense: RegionSense,
    pub species_data: Vec<SpeciesData>,
    pub copies: Vec<CopyState>,
    pub fixed_world: Vec<Point3<f32>>,
    pub periodic: Option<PeriodicBox>,
    /// Confinement box for the `Outside` sense (keeps the exterior bounded).
    pub confine: Option<Region>,
    /// Axis-aligned seeding domain.
    pub domain: (Point3<f32>, Point3<f32>),
    pub tol: f32,
    pub tol_sq: f32,
    // Scratch state for the current accepted configuration (the gradient's
    // static neighborhood). Rebuilt by `evaluate`.
    pub world: Vec<Point3<f32>>,
    pub owner: Vec<u32>,
    pub list: CellList,
}
