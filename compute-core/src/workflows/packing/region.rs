//! Geometric regions a [`crate::workflows::packing::pack`] run fills.
//!
//! A [`Region`] is a pure geometric shape (box, periodic cell, sphere, or
//! cylinder). The packer drives molecules onto the *allowed side* of the
//! region's surface, selected by a [`RegionSense`]: `Inside` packs within the
//! shape (the common liquid/droplet case); `Outside` carves the shape out as a
//! void, packing in the complement (the engine bounds the exterior by the
//! region's axis-aligned bounding box, or the result cell when one is given).
//!
//! Both [`Region::penetration`] (how far a point must move to comply) and
//! [`Region::volume_angstrom3`] are exact, so they double as the packer's
//! boundary penalty and its feasibility pre-check.

use nalgebra::{Point3, Vector3};

use crate::domain::UnitCell;

/// A geometric region to pack molecules into. Lengths are in angstrom.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum Region {
    /// An axis-aligned box spanning `min..=max`.
    Box { min: Point3<f32>, max: Point3<f32> },
    /// A periodic unit cell. Distances use the minimum image and atoms wrap, so
    /// the cell has no boundary to penetrate (the periodic-boundary path).
    Cell(UnitCell),
    /// A ball of the given radius about `center`.
    Sphere { center: Point3<f32>, radius: f32 },
    /// A finite cylinder: `radius` about the line through `center` along `axis`,
    /// extending `length / 2` either side of `center`.
    Cylinder {
        center: Point3<f32>,
        axis: Vector3<f32>,
        radius: f32,
        length: f32,
    },
}

/// Which side of a [`Region`]'s surface molecules are kept on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RegionSense {
    /// Pack inside the shape (the usual case).
    #[default]
    Inside,
    /// Pack outside the shape — carve it out as a void.
    Outside,
}

impl Region {
    /// How far (angstrom) the point `p` must move to lie on the allowed side of
    /// this region for `sense`. `0.0` once it complies. This is the magnitude
    /// the packer squares into its boundary penalty, so it is a true Euclidean
    /// distance-to-comply for convex shapes.
    pub fn penetration(&self, p: Point3<f32>, sense: RegionSense) -> f32 {
        match self {
            Region::Box { min, max } => box_penetration(*min, *max, p, sense),
            // A periodic cell wraps every coordinate, so there is no surface to
            // penetrate regardless of sense.
            Region::Cell(_) => 0.0,
            Region::Sphere { center, radius } => {
                let dist = (p - center).norm();
                match sense {
                    RegionSense::Inside => (dist - radius).max(0.0),
                    RegionSense::Outside => (radius - dist).max(0.0),
                }
            }
            Region::Cylinder {
                center,
                axis,
                radius,
                length,
            } => cylinder_penetration(*center, *axis, *radius, *length, p, sense),
        }
    }

    /// Whether `p` already lies on the allowed side for `sense`.
    pub fn contains(&self, p: Point3<f32>, sense: RegionSense) -> bool {
        self.penetration(p, sense) <= 0.0
    }

    /// The region's volume in cubic angstrom.
    pub fn volume_angstrom3(&self) -> f32 {
        match self {
            Region::Box { min, max } => {
                let d = max - min;
                (d.x.max(0.0)) * (d.y.max(0.0)) * (d.z.max(0.0))
            }
            Region::Cell(cell) => {
                let [a, b, c] = cell.vectors;
                a.dot(&b.cross(&c)).abs()
            }
            Region::Sphere { radius, .. } => (4.0 / 3.0) * std::f32::consts::PI * radius.powi(3),
            Region::Cylinder { radius, length, .. } => {
                std::f32::consts::PI * radius * radius * length.max(0.0)
            }
        }
    }

    /// The region's axis-aligned bounding box. Seeds the packer's start lattice
    /// and bounds the placement domain for [`RegionSense::Outside`].
    pub fn bounding_box(&self) -> (Point3<f32>, Point3<f32>) {
        match self {
            Region::Box { min, max } => (*min, *max),
            Region::Cell(cell) => {
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
            Region::Sphere { center, radius } => {
                let r = Vector3::new(*radius, *radius, *radius);
                (center - r, center + r)
            }
            Region::Cylinder {
                center,
                axis,
                radius,
                length,
            } => {
                let axis_hat = normalized_axis(*axis);
                let half = 0.5 * length;
                let e0 = center - axis_hat * half;
                let e1 = center + axis_hat * half;
                let mut min = Point3::origin();
                let mut max = Point3::origin();
                for i in 0..3 {
                    // Radial half-extent projected onto axis i (exact AABB of a
                    // finite cylinder): radius * sqrt(1 - axis_hat[i]^2).
                    let radial = radius * (1.0 - axis_hat[i] * axis_hat[i]).max(0.0).sqrt();
                    min[i] = e0[i].min(e1[i]) - radial;
                    max[i] = e0[i].max(e1[i]) + radial;
                }
                (min, max)
            }
        }
    }
}

fn box_penetration(min: Point3<f32>, max: Point3<f32>, p: Point3<f32>, sense: RegionSense) -> f32 {
    match sense {
        RegionSense::Inside => {
            // Per-axis overshoot past the nearer face; the Euclidean norm of the
            // overshoot vector is the distance from an exterior point to the box.
            let over = Vector3::new(
                (min.x - p.x).max(p.x - max.x).max(0.0),
                (min.y - p.y).max(p.y - max.y).max(0.0),
                (min.z - p.z).max(p.z - max.z).max(0.0),
            );
            over.norm()
        }
        RegionSense::Outside => {
            let inside = (0..3).all(|i| p[i] >= min[i] && p[i] <= max[i]);
            if inside {
                // Smallest single-axis push needed to exit through a face.
                (0..3)
                    .map(|i| (p[i] - min[i]).min(max[i] - p[i]))
                    .fold(f32::INFINITY, f32::min)
            } else {
                0.0
            }
        }
    }
}

fn cylinder_penetration(
    center: Point3<f32>,
    axis: Vector3<f32>,
    radius: f32,
    length: f32,
    p: Point3<f32>,
    sense: RegionSense,
) -> f32 {
    let axis_hat = normalized_axis(axis);
    let half = 0.5 * length;
    let d = p - center;
    let axial = d.dot(&axis_hat);
    let radial = (d - axis_hat * axial).norm();
    match sense {
        RegionSense::Inside => {
            // Radial and axial directions are orthogonal, so the distance to the
            // solid finite cylinder is the norm of the two per-direction
            // overshoots (correct even at the rim corner).
            let radial_over = (radial - radius).max(0.0);
            let axial_over = (axial.abs() - half).max(0.0);
            (radial_over * radial_over + axial_over * axial_over).sqrt()
        }
        RegionSense::Outside => {
            if radial < radius && axial.abs() < half {
                (radius - radial).min(half - axial.abs())
            } else {
                0.0
            }
        }
    }
}

/// A safe unit axis: falls back to +z for a degenerate (near-zero) input so a
/// malformed cylinder still has a well-defined geometry instead of NaNs.
fn normalized_axis(axis: Vector3<f32>) -> Vector3<f32> {
    axis.try_normalize(1.0e-6)
        .unwrap_or_else(|| Vector3::new(0.0, 0.0, 1.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f32, b: f32) {
        assert!((a - b).abs() < 1.0e-4, "expected {b}, got {a}");
    }

    #[test]
    fn box_inside_penetration_is_distance_to_box() {
        let region = Region::Box {
            min: Point3::new(0.0, 0.0, 0.0),
            max: Point3::new(10.0, 10.0, 10.0),
        };
        // Interior point: zero.
        approx(
            region.penetration(Point3::new(5.0, 5.0, 5.0), RegionSense::Inside),
            0.0,
        );
        // On a face: zero.
        approx(
            region.penetration(Point3::new(0.0, 5.0, 5.0), RegionSense::Inside),
            0.0,
        );
        // Outside on one axis: the single-axis overshoot.
        approx(
            region.penetration(Point3::new(13.0, 5.0, 5.0), RegionSense::Inside),
            3.0,
        );
        // Outside a corner: the diagonal distance.
        approx(
            region.penetration(Point3::new(13.0, 14.0, 5.0), RegionSense::Inside),
            5.0,
        );
    }

    #[test]
    fn box_outside_penetration_is_exit_depth() {
        let region = Region::Box {
            min: Point3::new(0.0, 0.0, 0.0),
            max: Point3::new(10.0, 10.0, 10.0),
        };
        // Outside the box already complies.
        approx(
            region.penetration(Point3::new(-1.0, 5.0, 5.0), RegionSense::Outside),
            0.0,
        );
        // Inside: push to the nearest face (x = 8 is 2 from the x=10 face).
        approx(
            region.penetration(Point3::new(8.0, 5.0, 5.0), RegionSense::Outside),
            2.0,
        );
    }

    #[test]
    fn sphere_penetration_both_senses() {
        let region = Region::Sphere {
            center: Point3::new(0.0, 0.0, 0.0),
            radius: 5.0,
        };
        approx(
            region.penetration(Point3::new(3.0, 0.0, 0.0), RegionSense::Inside),
            0.0,
        );
        approx(
            region.penetration(Point3::new(8.0, 0.0, 0.0), RegionSense::Inside),
            3.0,
        );
        approx(
            region.penetration(Point3::new(2.0, 0.0, 0.0), RegionSense::Outside),
            3.0,
        );
        approx(
            region.penetration(Point3::new(8.0, 0.0, 0.0), RegionSense::Outside),
            0.0,
        );
    }

    #[test]
    fn cylinder_penetration_radial_and_axial() {
        let region = Region::Cylinder {
            center: Point3::new(0.0, 0.0, 0.0),
            axis: Vector3::new(0.0, 0.0, 1.0),
            radius: 4.0,
            length: 10.0, // spans z in [-5, 5]
        };
        // Inside.
        approx(
            region.penetration(Point3::new(1.0, 1.0, 2.0), RegionSense::Inside),
            0.0,
        );
        // Radially outside by 2.
        approx(
            region.penetration(Point3::new(6.0, 0.0, 0.0), RegionSense::Inside),
            2.0,
        );
        // Axially past the cap by 3.
        approx(
            region.penetration(Point3::new(0.0, 0.0, 8.0), RegionSense::Inside),
            3.0,
        );
        // Past the rim corner: sqrt(2^2 + 3^2).
        approx(
            region.penetration(Point3::new(6.0, 0.0, 8.0), RegionSense::Inside),
            (4.0f32 + 9.0).sqrt(),
        );
    }

    #[test]
    fn volumes_match_closed_form() {
        approx(
            Region::Box {
                min: Point3::new(0.0, 0.0, 0.0),
                max: Point3::new(2.0, 3.0, 4.0),
            }
            .volume_angstrom3(),
            24.0,
        );
        approx(
            Region::Sphere {
                center: Point3::origin(),
                radius: 3.0,
            }
            .volume_angstrom3(),
            (4.0 / 3.0) * std::f32::consts::PI * 27.0,
        );
        approx(
            Region::Cylinder {
                center: Point3::origin(),
                axis: Vector3::new(0.0, 0.0, 1.0),
                radius: 2.0,
                length: 5.0,
            }
            .volume_angstrom3(),
            std::f32::consts::PI * 4.0 * 5.0,
        );
    }

    #[test]
    fn cell_region_has_no_boundary_and_full_volume() {
        let cell = UnitCell::from_parameters(10.0, 12.0, 14.0, 90.0, 90.0, 90.0);
        let region = Region::Cell(cell);
        // No surface to penetrate.
        approx(
            region.penetration(Point3::new(100.0, 0.0, 0.0), RegionSense::Inside),
            0.0,
        );
        approx(region.volume_angstrom3(), 10.0 * 12.0 * 14.0);
        let (min, max) = region.bounding_box();
        approx(min.x, 0.0);
        approx(max.x, 10.0);
        approx(max.z, 14.0);
    }

    #[test]
    fn sphere_bounding_box_encloses_it() {
        let region = Region::Sphere {
            center: Point3::new(1.0, 2.0, 3.0),
            radius: 5.0,
        };
        let (min, max) = region.bounding_box();
        approx(min.x, -4.0);
        approx(max.z, 8.0);
    }

    #[test]
    fn cylinder_bounding_box_is_tight_for_z_axis() {
        let region = Region::Cylinder {
            center: Point3::origin(),
            axis: Vector3::new(0.0, 0.0, 1.0),
            radius: 2.0,
            length: 10.0,
        };
        let (min, max) = region.bounding_box();
        approx(min.x, -2.0);
        approx(max.x, 2.0);
        approx(min.z, -5.0);
        approx(max.z, 5.0);
    }
}
