//! Mean-squared displacement of molecular centres of mass and the self-diffusion
//! coefficient from its linear regime.

use anyhow::{Result, bail};
use nalgebra::{Matrix3, Vector3};

use crate::domain::{Structure, Trajectory, chemistry::atomic_mass};

/// What to average over and where to fit.
#[derive(Debug, Clone)]
pub struct MsdSpec {
    /// Molecules (atom index lists) whose centres of mass are tracked.
    pub molecules: Vec<Vec<usize>>,
    /// Longest lag to evaluate, in frames (default: half the trajectory).
    pub max_lag_frames: Option<usize>,
    /// Fraction of the lag range where the Einstein fit starts and ends.
    pub fit_window: (f64, f64),
}

/// MSD curve plus the Einstein-relation fit.
#[derive(Debug, Clone)]
pub struct DiffusionReport {
    /// Lag times (ps), one per MSD point, starting at the first frame spacing.
    pub lag_ps: Vec<f64>,
    /// Mean-squared displacement (Å²) at each lag.
    pub msd: Vec<f64>,
    /// Self-diffusion coefficient `D = slope / 6` in cm²/s.
    pub diffusion_cm2_s: f64,
    /// Lag range (ps) the fit used.
    pub fit_range_ps: (f64, f64),
    /// Log–log slope of the MSD over the fit window: 1 is Fickian, below 1 is
    /// sub-diffusive (the sorbate is still caged), 2 is ballistic.
    pub exponent: f64,
    pub molecules: usize,
    pub frames: usize,
    /// False when no box was available, so wrapped coordinates were used as is.
    pub unwrapped: bool,
}

const ANGSTROM2_PER_PS_TO_CM2_PER_S: f64 = 1.0e-4;

/// Multiple-time-origin MSD of the molecules' centres of mass.
///
/// Coordinates are unwrapped frame to frame under the minimum-image convention
/// of each frame's box (falling back to the structure's cell), so a trajectory
/// that GROMACS wrapped into the box still yields the true displacement.
pub fn mean_squared_displacement(
    trajectory: &Trajectory,
    structure: &Structure,
    spec: &MsdSpec,
) -> Result<DiffusionReport> {
    let frames = trajectory.frame_count();
    if frames < 3 {
        bail!("the trajectory has {frames} frames; MSD needs at least 3");
    }
    if trajectory.natoms() != structure.atoms.len() {
        bail!(
            "the trajectory has {} atoms but the structure has {}",
            trajectory.natoms(),
            structure.atoms.len()
        );
    }
    if spec.molecules.is_empty() {
        bail!("no molecules selected");
    }
    let (fit_from, fit_to) = spec.fit_window;
    if !(0.0..1.0).contains(&fit_from) || !(fit_from..=1.0).contains(&fit_to) || fit_to <= fit_from
    {
        bail!("the fit window must satisfy 0 <= from < to <= 1 (got {fit_from}..{fit_to})");
    }

    let masses: Vec<f64> = structure
        .atoms
        .iter()
        .map(|atom| atomic_mass(&atom.element).unwrap_or(12.0) as f64)
        .collect();
    let fallback_box = structure
        .cell
        .as_ref()
        .filter(|cell| !cell.is_placeholder())
        .map(|cell| cell.vectors);
    let unwrapped = fallback_box.is_some() || trajectory.box_vectors(0).is_some();

    let centres = centres_of_mass(trajectory, &masses, &spec.molecules, fallback_box);

    let max_lag = spec
        .max_lag_frames
        .unwrap_or(frames / 2)
        .clamp(2, frames - 1);
    let dt = frame_spacing(trajectory)?;

    let molecules = spec.molecules.len();
    let mut msd = Vec::with_capacity(max_lag);
    let mut lag_ps = Vec::with_capacity(max_lag);
    for lag in 1..=max_lag {
        let mut sum = 0.0f64;
        let mut count = 0usize;
        for origin in 0..frames - lag {
            let base = origin * molecules;
            let later = (origin + lag) * molecules;
            for m in 0..molecules {
                sum += (centres[later + m] - centres[base + m]).norm_squared();
                count += 1;
            }
        }
        msd.push(sum / count as f64);
        lag_ps.push(lag as f64 * dt);
    }

    let first = ((max_lag as f64 * fit_from).floor() as usize).min(max_lag - 2);
    let last = ((max_lag as f64 * fit_to).ceil() as usize).clamp(first + 2, max_lag);
    let window = first..last;
    let slope = linear_slope(&lag_ps[window.clone()], &msd[window.clone()]);
    let exponent = {
        let logs: Vec<(f64, f64)> = window
            .clone()
            .filter(|&i| msd[i] > 0.0)
            .map(|i| (lag_ps[i].ln(), msd[i].ln()))
            .collect();
        let xs: Vec<f64> = logs.iter().map(|p| p.0).collect();
        let ys: Vec<f64> = logs.iter().map(|p| p.1).collect();
        linear_slope(&xs, &ys)
    };

    Ok(DiffusionReport {
        fit_range_ps: (lag_ps[first], lag_ps[last - 1]),
        diffusion_cm2_s: slope / 6.0 * ANGSTROM2_PER_PS_TO_CM2_PER_S,
        exponent,
        lag_ps,
        msd,
        molecules,
        frames,
        unwrapped,
    })
}

/// The common time step (ps) between frames. Every lag is a whole number of
/// frames, so frame times must be finite, increasing, and evenly spaced (to 1%,
/// plus the rounding of an `f32` time); a concatenated run with a repeated or
/// skipped frame is refused rather than fitted with a wrong time axis.
fn frame_spacing(trajectory: &Trajectory) -> Result<f64> {
    let frames = trajectory.frame_count();
    let times: Vec<f64> = (0..frames).map(|f| trajectory.time(f) as f64).collect();
    if let Some(frame) = times.iter().position(|t| !t.is_finite()) {
        bail!("frame {frame} has no finite time; MSD needs the frame times");
    }
    let dt = (times[frames - 1] - times[0]) / (frames - 1) as f64;
    for (frame, pair) in times.windows(2).enumerate() {
        let step = pair[1] - pair[0];
        if step <= 0.0 {
            bail!(
                "frame times do not increase at frame {} ({} ps after {} ps); remove repeated \
                 frames (e.g. at a restart boundary) before computing the MSD",
                frame + 1,
                pair[1],
                pair[0]
            );
        }
        let rounding = 4.0 * f32::EPSILON as f64 * pair[0].abs().max(pair[1].abs());
        if (step - dt).abs() > 0.01 * dt + rounding {
            bail!(
                "frames are not evenly spaced: {step} ps between frames {frame} and {} against \
                 {dt:.4} ps on average; MSD lags count frames, so resample the trajectory to \
                 a fixed interval first",
                frame + 1
            );
        }
    }
    Ok(dt)
}

/// Centre of mass of every molecule in every frame (`frame * molecules + m`),
/// after unwrapping each atom's path through the periodic box.
fn centres_of_mass(
    trajectory: &Trajectory,
    masses: &[f64],
    molecules: &[Vec<usize>],
    fallback_box: Option<[Vector3<f32>; 3]>,
) -> Vec<Vector3<f64>> {
    let frames = trajectory.frame_count();
    let tracked: Vec<usize> = molecules.iter().flatten().copied().collect();
    let mut previous: Vec<Vector3<f64>> = tracked
        .iter()
        .map(|&atom| trajectory.position(0, atom).coords.cast::<f64>())
        .collect();
    let mut unwrapped = previous.clone();
    let mut centres = Vec::with_capacity(frames * molecules.len());

    let push_centres = |positions: &[Vector3<f64>], centres: &mut Vec<Vector3<f64>>| {
        let mut cursor = 0;
        for molecule in molecules {
            let mut weighted = Vector3::zeros();
            let mut total = 0.0;
            for &atom in molecule {
                let mass = masses.get(atom).copied().unwrap_or(1.0);
                weighted += positions[cursor] * mass;
                total += mass;
                cursor += 1;
            }
            centres.push(if total > 0.0 {
                weighted / total
            } else {
                weighted
            });
        }
    };
    push_centres(&unwrapped, &mut centres);

    for frame in 1..frames {
        let cell = trajectory.box_vectors(frame).or(fallback_box);
        let image = cell.and_then(MinimumImage::new);
        for (slot, &atom) in tracked.iter().enumerate() {
            let current = trajectory.position(frame, atom).coords.cast::<f64>();
            let mut step = current - previous[slot];
            if let Some(image) = &image {
                step = image.shortest(step);
            }
            unwrapped[slot] += step;
            previous[slot] = current;
        }
        push_centres(&unwrapped, &mut centres);
    }
    centres
}

/// Minimum-image correction for a (possibly triclinic) box.
struct MinimumImage {
    basis: Matrix3<f64>,
    inverse: Matrix3<f64>,
}

impl MinimumImage {
    fn new(vectors: [Vector3<f32>; 3]) -> Option<Self> {
        let basis = Matrix3::from_columns(&[
            vectors[0].cast::<f64>(),
            vectors[1].cast::<f64>(),
            vectors[2].cast::<f64>(),
        ]);
        if basis.determinant().abs() < 1.0e-6 {
            return None;
        }
        let inverse = basis.try_inverse()?;
        Some(Self { basis, inverse })
    }

    fn shortest(&self, displacement: Vector3<f64>) -> Vector3<f64> {
        let fractional = self.inverse * displacement;
        let wrapped = fractional.map(|f| f - f.round());
        self.basis * wrapped
    }
}

fn linear_slope(xs: &[f64], ys: &[f64]) -> f64 {
    let n = xs.len().min(ys.len());
    if n < 2 {
        return 0.0;
    }
    let mean_x = xs[..n].iter().sum::<f64>() / n as f64;
    let mean_y = ys[..n].iter().sum::<f64>() / n as f64;
    let mut sxx = 0.0;
    let mut sxy = 0.0;
    for i in 0..n {
        let dx = xs[i] - mean_x;
        sxx += dx * dx;
        sxy += dx * (ys[i] - mean_y);
    }
    if sxx > 0.0 { sxy / sxx } else { 0.0 }
}

/// The MSD curve as CSV (`lag_ps,msd_angstrom2`).
pub fn msd_csv(report: &DiffusionReport) -> String {
    let mut text = String::from("lag_ps,msd_angstrom2\n");
    for (lag, msd) in report.lag_ps.iter().zip(&report.msd) {
        text.push_str(&format!("{lag:.4},{msd:.4}\n"));
    }
    text
}
