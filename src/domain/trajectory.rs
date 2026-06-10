//! A decoded molecular-dynamics trajectory: a sequence of coordinate frames
//! over a fixed set of atoms.

use nalgebra::Point3;

use crate::domain::Atom;

/// A molecular-dynamics trajectory: one set of atom coordinates per frame.
///
/// Coordinates are in Angstrom (converted from GROMACS' nanometers on read) to
/// match [`crate::domain::Structure`]. Every frame shares the same `natoms`. To
/// bound memory on large solvated systems the coordinates live in a single flat
/// arena rather than a `Vec<Vec<_>>`.
#[derive(Debug, Clone)]
pub struct Trajectory {
    natoms: usize,
    /// `frame_count * natoms * 3` floats, frame-major then atom-major (x, y, z).
    coords: Vec<f32>,
    /// Simulation time (ps) of each frame; `len() == frame_count`.
    times: Vec<f32>,
    /// Frame stride applied while reading: `1` means every source frame was
    /// kept, `> 1` means the reader subsampled to stay under its memory budget.
    stride: usize,
    /// Number of frames present in the source file before any subsampling.
    source_frame_count: usize,
}

impl Trajectory {
    /// Assemble a trajectory from a flat coordinate arena and per-frame times.
    ///
    /// `coords.len()` must equal `times.len() * natoms * 3`. `stride` /
    /// `source_frame_count` record any subsampling the reader applied.
    pub fn from_parts(
        natoms: usize,
        coords: Vec<f32>,
        times: Vec<f32>,
        stride: usize,
        source_frame_count: usize,
    ) -> Self {
        debug_assert_eq!(coords.len(), times.len() * natoms * 3);
        Self {
            natoms,
            coords,
            times,
            stride: stride.max(1),
            source_frame_count,
        }
    }

    pub fn natoms(&self) -> usize {
        self.natoms
    }

    pub fn frame_count(&self) -> usize {
        self.times.len()
    }

    pub fn is_empty(&self) -> bool {
        self.times.is_empty()
    }

    /// Simulation time (ps) of `frame`, or `0.0` if out of range.
    pub fn time(&self, frame: usize) -> f32 {
        self.times.get(frame).copied().unwrap_or(0.0)
    }

    /// `1` when every source frame was kept; `> 1` when the reader subsampled.
    pub fn stride(&self) -> usize {
        self.stride
    }

    /// Frames present in the source file before subsampling.
    pub fn source_frame_count(&self) -> usize {
        self.source_frame_count
    }

    /// Position of `atom` in `frame` (Angstrom). Returns the origin if either
    /// index is out of range.
    pub fn position(&self, frame: usize, atom: usize) -> Point3<f32> {
        if frame >= self.frame_count() || atom >= self.natoms {
            return Point3::origin();
        }
        let base = (frame * self.natoms + atom) * 3;
        Point3::new(
            self.coords[base],
            self.coords[base + 1],
            self.coords[base + 2],
        )
    }

    /// Overwrite the positions of `atoms` with `frame`'s coordinates, leaving
    /// every other field (element, charge) untouched. Atoms beyond the
    /// trajectory's `natoms` are left unchanged.
    pub fn apply_frame(&self, frame: usize, atoms: &mut [Atom]) {
        if frame >= self.frame_count() {
            return;
        }
        let base = frame * self.natoms * 3;
        for (i, atom) in atoms.iter_mut().enumerate().take(self.natoms) {
            let offset = base + i * 3;
            atom.position = Point3::new(
                self.coords[offset],
                self.coords[offset + 1],
                self.coords[offset + 2],
            );
        }
    }
}
