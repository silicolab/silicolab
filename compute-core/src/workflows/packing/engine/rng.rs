use nalgebra::{Point3, Rotation3, Vector3};

use crate::domain::UnitCell;
use crate::workflows::molecular_dynamics::solvation::splitmix64;

use super::super::region::{Region, RegionSense};

/// A SplitMix64 stream reusing the crate's `splitmix64` finalizer, keyed
/// deterministically on `(seed, salt, index)`.
pub(crate) struct Rng {
    state: u64,
}

impl Rng {
    pub(crate) fn keyed(seed: u64, salt: u64, index: u64) -> Self {
        let state = splitmix64(seed)
            ^ splitmix64(salt.wrapping_mul(0x9E37_79B9_7F4A_7C15))
            ^ splitmix64(index.wrapping_add(0xD1B5_4A32_D192_ED03));
        Self { state }
    }

    pub(crate) fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        splitmix64(self.state)
    }

    /// A uniform f32 in `[0, 1)`.
    pub(crate) fn unit(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / ((1u64 << 24) as f32)
    }

    pub(crate) fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.unit()
    }

    pub(crate) fn rotation(&mut self) -> Rotation3<f32> {
        use std::f32::consts::TAU;
        Rotation3::from_euler_angles(
            self.range(0.0, TAU),
            self.range(0.0, TAU),
            self.range(0.0, TAU),
        )
    }
}

pub(crate) fn centroid(points: impl Iterator<Item = Point3<f32>>) -> Point3<f32> {
    let mut sum = Vector3::zeros();
    let mut count = 0u32;
    for p in points {
        sum += p.coords;
        count += 1;
    }
    if count == 0 {
        Point3::origin()
    } else {
        Point3::from(sum / count as f32)
    }
}

pub(crate) fn clamp_norm(v: Vector3<f32>, max: f32) -> Vector3<f32> {
    let norm = v.norm();
    if norm > max && norm > 0.0 {
        v * (max / norm)
    } else {
        v
    }
}

pub(crate) fn cell_bounding_box(cell: &UnitCell) -> (Point3<f32>, Point3<f32>) {
    let mut min = Point3::new(f32::INFINITY, f32::INFINITY, f32::INFINITY);
    let mut max = Point3::new(f32::NEG_INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY);
    for corner in cell.corners() {
        for i in 0..3 {
            min[i] = min[i].min(corner[i]);
            max[i] = max[i].max(corner[i]);
        }
    }
    (min, max)
}

/// The volume of the allowed placement domain (Å³).
pub(crate) fn allowed_volume(
    region: &Region,
    domain: (Point3<f32>, Point3<f32>),
    sense: RegionSense,
    periodic: bool,
) -> f32 {
    if periodic {
        return region.volume_angstrom3();
    }
    match sense {
        RegionSense::Inside => region.volume_angstrom3(),
        RegionSense::Outside => {
            let ext = domain.1 - domain.0;
            let bbox = (ext.x * ext.y * ext.z).max(0.0);
            (bbox - region.volume_angstrom3()).max(0.0)
        }
    }
}
