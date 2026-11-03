//! Probe-accessible free volume of a periodic cell by grid insertion: a grid
//! point is free when no atom's van der Waals sphere, grown by the probe
//! radius, covers it.

use anyhow::{Result, bail};
use nalgebra::{Matrix3, Point3, Vector3};

use super::van_der_waals_radius;
use crate::domain::{Structure, Trajectory, UnitCell, chemistry::atomic_mass};

#[derive(Debug, Clone, Copy)]
pub struct FreeVolumeSpec {
    /// Radius (Å) of the spherical probe; `0` gives the plain unoccupied fraction.
    pub probe_radius: f32,
    /// Grid spacing (Å); the fraction's resolution is a few percent at 0.5 Å.
    pub grid_spacing: f32,
}

impl Default for FreeVolumeSpec {
    fn default() -> Self {
        Self {
            probe_radius: 1.0,
            grid_spacing: 0.5,
        }
    }
}

#[derive(Debug, Clone)]
pub struct FreeVolumeReport {
    pub cell_volume_a3: f64,
    pub free_volume_a3: f64,
    /// Free volume as a fraction of the cell volume, averaged over the frames.
    pub fraction: f64,
    pub probe_radius: f32,
    pub grid_points: usize,
    pub frames: usize,
    pub density_g_cm3: f64,
}

/// Free volume of the structure as it stands (its own coordinates and cell).
pub fn free_volume(structure: &Structure, spec: FreeVolumeSpec) -> Result<FreeVolumeReport> {
    let cell = periodic_cell(structure)?;
    let positions: Vec<Point3<f32>> = structure.atoms.iter().map(|a| a.position).collect();
    let radii = probe_radii(structure, spec.probe_radius);
    let (free, points) = free_points(&cell, &positions, &radii, spec.grid_spacing)?;
    let volume = cell_volume(&cell);
    Ok(report(
        structure,
        spec,
        volume,
        free as f64 / points as f64,
        points,
        1,
    ))
}

/// Free volume averaged over `frames` of a trajectory of the structure, using
/// each frame's box when the trajectory carries one.
pub fn free_volume_over_frames(
    structure: &Structure,
    trajectory: &Trajectory,
    frames: &[usize],
    spec: FreeVolumeSpec,
) -> Result<FreeVolumeReport> {
    if trajectory.natoms() != structure.atoms.len() {
        bail!(
            "the trajectory has {} atoms but the structure has {}",
            trajectory.natoms(),
            structure.atoms.len()
        );
    }
    if frames.is_empty() {
        bail!("no frames selected");
    }
    let radii = probe_radii(structure, spec.probe_radius);
    let mut fraction_sum = 0.0;
    let mut volume_sum = 0.0;
    let mut points_total = 0;
    for &frame in frames {
        if frame >= trajectory.frame_count() {
            bail!(
                "frame {frame} is out of range (the trajectory has {} frames)",
                trajectory.frame_count()
            );
        }
        let cell = match trajectory.box_vectors(frame) {
            Some(vectors) => UnitCell::from_vectors(vectors),
            None => periodic_cell(structure)?,
        };
        let positions: Vec<Point3<f32>> = (0..trajectory.natoms())
            .map(|atom| trajectory.position(frame, atom))
            .collect();
        let (free, points) = free_points(&cell, &positions, &radii, spec.grid_spacing)?;
        fraction_sum += free as f64 / points as f64;
        volume_sum += cell_volume(&cell);
        points_total = points;
    }
    let n = frames.len() as f64;
    Ok(report(
        structure,
        spec,
        volume_sum / n,
        fraction_sum / n,
        points_total,
        frames.len(),
    ))
}

fn report(
    structure: &Structure,
    spec: FreeVolumeSpec,
    volume: f64,
    fraction: f64,
    grid_points: usize,
    frames: usize,
) -> FreeVolumeReport {
    let mass_u: f64 = structure
        .atoms
        .iter()
        .map(|atom| atomic_mass(&atom.element).unwrap_or(0.0) as f64)
        .sum();
    const U_TO_G_OVER_A3_TO_CM3: f64 = 1.660_539;
    FreeVolumeReport {
        cell_volume_a3: volume,
        free_volume_a3: volume * fraction,
        fraction,
        probe_radius: spec.probe_radius,
        grid_points,
        frames,
        density_g_cm3: if volume > 0.0 {
            mass_u * U_TO_G_OVER_A3_TO_CM3 / volume
        } else {
            0.0
        },
    }
}

fn periodic_cell(structure: &Structure) -> Result<UnitCell> {
    match &structure.cell {
        Some(cell) if !cell.is_placeholder() => Ok(cell.clone()),
        _ => bail!("free volume needs a periodic cell; build or pack the system into a box first"),
    }
}

fn probe_radii(structure: &Structure, probe: f32) -> Vec<f32> {
    structure
        .atoms
        .iter()
        .map(|atom| van_der_waals_radius(&atom.element) + probe)
        .collect()
}

fn cell_volume(cell: &UnitCell) -> f64 {
    Matrix3::from_columns(&[
        cell.vectors[0].cast::<f64>(),
        cell.vectors[1].cast::<f64>(),
        cell.vectors[2].cast::<f64>(),
    ])
    .determinant()
    .abs()
}

/// Count the grid points not inside any sphere; returns `(free, total)`.
///
/// Each sphere marks the grid points inside its fractional bounding box, with
/// indices wrapped into the cell, so a sphere crossing a face covers both sides.
fn free_points(
    cell: &UnitCell,
    positions: &[Point3<f32>],
    radii: &[f32],
    spacing: f32,
) -> Result<(usize, usize)> {
    if spacing.is_nan() || spacing <= 0.0 {
        bail!("the grid spacing must be positive");
    }
    let basis = Matrix3::from_columns(&cell.vectors);
    let Some(inverse) = basis.try_inverse() else {
        bail!("the cell is degenerate");
    };
    let counts = [cell.a, cell.b, cell.c].map(|length| ((length / spacing).ceil() as usize).max(1));
    const MAX_POINTS: usize = 200_000_000;
    let total = counts[0]
        .checked_mul(counts[1])
        .and_then(|points| points.checked_mul(counts[2]))
        .filter(|&points| points <= MAX_POINTS)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "a {spacing} Å grid needs {}×{}×{} points for this cell, more than {MAX_POINTS}; \
                 use a coarser --grid or a smaller cell",
                counts[0],
                counts[1],
                counts[2]
            )
        })?;
    let mut occupied = vec![false; total];
    // Fractional half-widths of a sphere's bounding box along each axis.
    let reach = |radius: f32| -> [f32; 3] {
        let rows = [inverse.row(0), inverse.row(1), inverse.row(2)];
        rows.map(|row| radius * row.norm())
    };
    let step = [
        1.0 / counts[0] as f32,
        1.0 / counts[1] as f32,
        1.0 / counts[2] as f32,
    ];

    for (position, &radius) in positions.iter().zip(radii) {
        let fractional = inverse * position.coords;
        let fractional = fractional.map(|f| f - f.floor());
        let centre = basis * fractional;
        let extent = reach(radius);
        let radius_sq = radius * radius;
        let lo = |axis: usize| ((fractional[axis] - extent[axis]) / step[axis]).floor() as i64;
        let hi = |axis: usize| ((fractional[axis] + extent[axis]) / step[axis]).ceil() as i64;
        for i in lo(0)..=hi(0) {
            let fx = i as f32 * step[0];
            let ix = i.rem_euclid(counts[0] as i64) as usize;
            for j in lo(1)..=hi(1) {
                let fy = j as f32 * step[1];
                let iy = j.rem_euclid(counts[1] as i64) as usize;
                for k in lo(2)..=hi(2) {
                    let fz = k as f32 * step[2];
                    let iz = k.rem_euclid(counts[2] as i64) as usize;
                    let point = basis * Vector3::new(fx, fy, fz);
                    if (point - centre).norm_squared() <= radius_sq {
                        occupied[(ix * counts[1] + iy) * counts[2] + iz] = true;
                    }
                }
            }
        }
    }
    let free = occupied.iter().filter(|&&taken| !taken).count();
    Ok((free, total))
}
