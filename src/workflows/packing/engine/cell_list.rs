use super::*;

use nalgebra::{Point3, Vector3};

use crate::domain::UnitCell;

/// A flat-grid cell-list keyed on `tol`-sized buckets, supporting a Euclidean
/// grid and a periodic (fractional, wrapped) grid.
#[derive(Default)]
pub(crate) struct CellList {
    periodic: bool,
    cell: Option<UnitCell>,
    origin: Point3<f32>,
    bucket: f32,
    dims: [i32; 3],
    buckets: Vec<Vec<u32>>,
}

impl CellList {
    pub(crate) fn rebuild(
        &mut self,
        world: &[Point3<f32>],
        tol: f32,
        periodic: Option<&PeriodicBox>,
    ) {
        self.bucket = tol.max(1.0e-3);
        match periodic {
            Some(pb) => {
                self.periodic = true;
                self.cell = Some(pb.cell.clone());
                self.dims = [
                    (pb.cell.vectors[0].norm() / self.bucket).floor().max(1.0) as i32,
                    (pb.cell.vectors[1].norm() / self.bucket).floor().max(1.0) as i32,
                    (pb.cell.vectors[2].norm() / self.bucket).floor().max(1.0) as i32,
                ];
                self.origin = pb.origin;
            }
            None => {
                self.periodic = false;
                self.cell = None;
                let (mut min, mut max) = (
                    Point3::new(f32::INFINITY, f32::INFINITY, f32::INFINITY),
                    Point3::new(f32::NEG_INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY),
                );
                for p in world {
                    for i in 0..3 {
                        min[i] = min[i].min(p[i]);
                        max[i] = max[i].max(p[i]);
                    }
                }
                if world.is_empty() {
                    min = Point3::origin();
                    max = Point3::origin();
                }
                // Pad one bucket so the -1..=1 sweep never needs clamping logic.
                self.origin = min - Vector3::new(self.bucket, self.bucket, self.bucket);
                self.dims = [
                    (((max[0] - min[0]) / self.bucket).floor() as i32) + 3,
                    (((max[1] - min[1]) / self.bucket).floor() as i32) + 3,
                    (((max[2] - min[2]) / self.bucket).floor() as i32) + 3,
                ];
            }
        }

        let total = (self.dims[0] * self.dims[1] * self.dims[2]).max(1) as usize;
        if self.buckets.len() != total {
            self.buckets = vec![Vec::new(); total];
        } else {
            for bucket in &mut self.buckets {
                bucket.clear();
            }
        }
        for (slot, p) in world.iter().enumerate() {
            let (bx, by, bz) = self.bucket_of(*p);
            let index = self.linear(bx, by, bz);
            self.buckets[index].push(slot as u32);
        }
    }

    fn bucket_of(&self, p: Point3<f32>) -> (i32, i32, i32) {
        if self.periodic {
            let cell = self.cell.as_ref().expect("periodic cell");
            let rel = p - self.origin;
            let frac = cell.cartesian_to_fractional(Point3::from(rel));
            let coord =
                |f: f32, n: i32| ((f.rem_euclid(1.0) * n as f32).floor() as i32).clamp(0, n - 1);
            (
                coord(frac.x, self.dims[0]),
                coord(frac.y, self.dims[1]),
                coord(frac.z, self.dims[2]),
            )
        } else {
            let coord = |v: f32, origin: f32, n: i32| {
                (((v - origin) / self.bucket).floor() as i32).clamp(0, n - 1)
            };
            (
                coord(p.x, self.origin.x, self.dims[0]),
                coord(p.y, self.origin.y, self.dims[1]),
                coord(p.z, self.origin.z, self.dims[2]),
            )
        }
    }

    fn linear(&self, bx: i32, by: i32, bz: i32) -> usize {
        ((bx * self.dims[1] + by) * self.dims[2] + bz) as usize
    }

    /// Fill `out` with the slot ids in the 3×3×3 neighborhood of `p`. Periodic
    /// neighbor buckets wrap; duplicate buckets (tiny grids) are de-duplicated.
    pub(crate) fn query(&self, p: Point3<f32>, out: &mut Vec<u32>) {
        out.clear();
        let (bx, by, bz) = self.bucket_of(p);
        let mut visited: Vec<usize> = Vec::with_capacity(27);
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    let (nx, ny, nz) = if self.periodic {
                        (
                            (bx + dx).rem_euclid(self.dims[0]),
                            (by + dy).rem_euclid(self.dims[1]),
                            (bz + dz).rem_euclid(self.dims[2]),
                        )
                    } else {
                        let (nx, ny, nz) = (bx + dx, by + dy, bz + dz);
                        if nx < 0
                            || ny < 0
                            || nz < 0
                            || nx >= self.dims[0]
                            || ny >= self.dims[1]
                            || nz >= self.dims[2]
                        {
                            continue;
                        }
                        (nx, ny, nz)
                    };
                    let index = self.linear(nx, ny, nz);
                    if visited.contains(&index) {
                        continue;
                    }
                    visited.push(index);
                    out.extend_from_slice(&self.buckets[index]);
                }
            }
        }
    }
}
