//! Radial distribution function between two atom selections under periodic
//! boundaries, averaged over trajectory frames.

use anyhow::{Result, bail};
use nalgebra::{Matrix3, Point3, Vector3};

use crate::domain::{Structure, Trajectory, UnitCell};

#[derive(Debug, Clone)]
pub struct RdfSpec {
    pub first: Vec<usize>,
    pub second: Vec<usize>,
    /// Largest separation (Å) tabulated; must fit the minimum image.
    pub r_max: f64,
    pub bin_width: f64,
}

#[derive(Debug, Clone)]
pub struct RdfReport {
    /// Bin centres (Å).
    pub r: Vec<f64>,
    /// `g(r)` per bin.
    pub g: Vec<f64>,
    /// Running coordination number: atoms of the second selection within `r`
    /// of one atom of the first.
    pub coordination: Vec<f64>,
    pub frames: usize,
    pub pairs_per_frame: usize,
}

/// `g(r)` of the structure as it stands.
pub fn radial_distribution(structure: &Structure, spec: &RdfSpec) -> Result<RdfReport> {
    let cell = periodic_cell(structure)?;
    let positions: Vec<Point3<f32>> = structure.atoms.iter().map(|a| a.position).collect();
    let mut histogram = Histogram::new(spec)?;
    histogram.add_frame(&cell, &positions, spec)?;
    Ok(histogram.report(spec))
}

/// `g(r)` averaged over the listed trajectory frames with their boxes.
pub fn radial_distribution_over_frames(
    structure: &Structure,
    trajectory: &Trajectory,
    frames: &[usize],
    spec: &RdfSpec,
) -> Result<RdfReport> {
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
    let mut histogram = Histogram::new(spec)?;
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
        histogram.add_frame(&cell, &positions, spec)?;
    }
    Ok(histogram.report(spec))
}

fn periodic_cell(structure: &Structure) -> Result<UnitCell> {
    match &structure.cell {
        Some(cell) if !cell.is_placeholder() => Ok(cell.clone()),
        _ => bail!("a radial distribution needs a periodic cell"),
    }
}

struct Histogram {
    counts: Vec<f64>,
    frames: usize,
    /// Sum over frames of `N_second / V`, the second selection's number density.
    density_sum: f64,
}

impl Histogram {
    fn new(spec: &RdfSpec) -> Result<Self> {
        if spec.bin_width <= 0.0 || spec.r_max <= spec.bin_width {
            bail!("the bin width must be positive and smaller than r_max");
        }
        if spec.first.is_empty() || spec.second.is_empty() {
            bail!("both selections need at least one atom");
        }
        Ok(Self {
            counts: vec![0.0; (spec.r_max / spec.bin_width).ceil() as usize],
            frames: 0,
            density_sum: 0.0,
        })
    }

    fn add_frame(
        &mut self,
        cell: &UnitCell,
        positions: &[Point3<f32>],
        spec: &RdfSpec,
    ) -> Result<()> {
        let basis = Matrix3::from_columns(&[
            cell.vectors[0].cast::<f64>(),
            cell.vectors[1].cast::<f64>(),
            cell.vectors[2].cast::<f64>(),
        ]);
        let volume = basis.determinant().abs();
        let Some(inverse) = basis.try_inverse().filter(|_| volume > 1.0e-6) else {
            bail!("the cell is degenerate");
        };
        let widths = [
            volume / basis.column(1).cross(&basis.column(2)).norm(),
            volume / basis.column(2).cross(&basis.column(0)).norm(),
            volume / basis.column(0).cross(&basis.column(1)).norm(),
        ];
        if widths.iter().any(|width| *width < 2.0 * spec.r_max) {
            bail!(
                "r_max {} Å exceeds half the cell width; use a smaller --rmax",
                spec.r_max
            );
        }
        let minimum_image = |delta: Vector3<f64>| -> Vector3<f64> {
            let fractional = inverse * delta;
            basis * fractional.map(|f| f - f.round())
        };
        let n = positions.len();
        for &a in &spec.first {
            for &b in &spec.second {
                if a == b || a >= n || b >= n {
                    continue;
                }
                let delta = minimum_image(
                    positions[b].coords.cast::<f64>() - positions[a].coords.cast::<f64>(),
                );
                let r = delta.norm();
                if r < spec.r_max {
                    self.counts[(r / spec.bin_width) as usize] += 1.0;
                }
            }
        }
        self.density_sum += spec.second.len() as f64 / volume;
        self.frames += 1;
        Ok(())
    }

    fn report(&self, spec: &RdfSpec) -> RdfReport {
        let frames = self.frames.max(1) as f64;
        let density = self.density_sum / frames;
        let n_first = spec.first.len() as f64;
        let mut r = Vec::with_capacity(self.counts.len());
        let mut g = Vec::with_capacity(self.counts.len());
        let mut coordination = Vec::with_capacity(self.counts.len());
        let mut running = 0.0;
        for (bin, count) in self.counts.iter().enumerate() {
            let inner = bin as f64 * spec.bin_width;
            let outer = inner + spec.bin_width;
            let shell = 4.0 / 3.0 * std::f64::consts::PI * (outer.powi(3) - inner.powi(3));
            let per_frame = count / frames;
            running += per_frame / n_first;
            r.push(inner + 0.5 * spec.bin_width);
            g.push(per_frame / (n_first * density * shell));
            coordination.push(running);
        }
        RdfReport {
            r,
            g,
            coordination,
            frames: self.frames,
            pairs_per_frame: spec.first.len() * spec.second.len(),
        }
    }
}

/// The curve as CSV (`r_angstrom,g,coordination`).
pub fn rdf_csv(report: &RdfReport) -> String {
    let mut text = String::from("r_angstrom,g,coordination\n");
    for ((r, g), n) in report.r.iter().zip(&report.g).zip(&report.coordination) {
        text.push_str(&format!("{r:.3},{g:.4},{n:.4}\n"));
    }
    text
}
