//! Widom test-particle insertion: the excess chemical potential and the
//! infinite-dilution solubility of a probe molecule in a dense host, from the
//! Boltzmann factor of its interaction energy at random trial positions.

use anyhow::{Result, bail};
use nalgebra::{Matrix3, Point3, Quaternion, Rotation3, UnitQuaternion, Vector3};

use crate::domain::{Structure, Trajectory, UnitCell};
use crate::md::solvation::splitmix64;
use crate::md::topology::MdTopology;

/// Boltzmann constant in kJ/(mol·K).
pub const KB_KJ_PER_MOL_K: f64 = 8.314_462_618e-3;
/// Coulomb constant in kJ·Å/(mol·e²).
const COULOMB_KJ_A_PER_MOL_E2: f64 = 1_389.354_58;
/// cm³(STP) per mole of ideal gas.
const CM3_STP_PER_MOL: f64 = 22_414.0;
const PA_PER_CMHG: f64 = 1_333.223_87;

/// Nonbonded parameters of one atom, in Å and kJ/mol.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NonbondedAtom {
    pub sigma: f64,
    pub epsilon: f64,
    pub charge: f64,
}

/// What to insert, where, and how often.
#[derive(Debug, Clone)]
pub struct WidomSpec {
    /// Probe atom positions (Å) in any frame; the centroid is moved to the trial point.
    pub probe_positions: Vec<Point3<f32>>,
    pub probe: Vec<NonbondedAtom>,
    pub temperature_k: f64,
    pub insertions: usize,
    pub seed: u64,
    /// Pair cutoff (Å); Lennard-Jones is truncated there with an analytic
    /// tail correction, and Coulomb is shifted-force to vanish there.
    pub cutoff: f64,
}

#[derive(Debug, Clone)]
pub struct WidomReport {
    /// `⟨exp(−ΔU/kT)⟩`, the infinite-dilution Henry factor (dimensionless).
    pub boltzmann_mean: f64,
    /// Standard error of the mean Boltzmann factor over the insertions.
    pub boltzmann_error: f64,
    /// Excess chemical potential `−kT ln⟨exp(−ΔU/kT)⟩` in kJ/mol.
    pub mu_excess_kj_mol: f64,
    /// Solubility coefficient in cm³(STP)/(cm³·cmHg), ideal-gas phase assumed.
    pub solubility_cm3stp_per_cm3_cmhg: f64,
    /// Fraction of insertions with a favourable energy (ΔU < 0).
    pub favourable_fraction: f64,
    pub insertions: usize,
    pub frames: usize,
}

/// Per-atom nonbonded parameters of a structure described by `topology`, in
/// coordinate order: the composition's molecule runs expanded atom by atom.
pub fn host_parameters(topology: &MdTopology, atom_count: usize) -> Result<Vec<NonbondedAtom>> {
    let mut atoms = Vec::with_capacity(atom_count);
    for run in &topology.composition {
        let Some(molecule) = topology.molecule(&run.molecule) else {
            bail!(
                "the topology lists molecule `{}` but does not define it",
                run.molecule
            );
        };
        for _ in 0..run.count {
            for atom in &molecule.atoms {
                let Some(species) = topology.species.iter().find(|s| s.element == atom.species)
                else {
                    bail!(
                        "atom type `{}` of molecule `{}` has no nonbonded parameters",
                        atom.species,
                        run.molecule
                    );
                };
                atoms.push(NonbondedAtom {
                    sigma: species.sigma_angstrom as f64,
                    epsilon: species.epsilon_kj_mol as f64,
                    charge: atom.charge as f64,
                });
            }
        }
    }
    if atoms.len() != atom_count {
        bail!(
            "the topology describes {} atoms but the structure has {atom_count}",
            atoms.len()
        );
    }
    Ok(atoms)
}

/// Insert the probe into the structure as it stands.
pub fn widom_insertion(
    structure: &Structure,
    host: &[NonbondedAtom],
    spec: &WidomSpec,
) -> Result<WidomReport> {
    let cell = periodic_cell(structure)?;
    let positions: Vec<Point3<f32>> = structure.atoms.iter().map(|a| a.position).collect();
    let mut accumulator = Accumulator::default();
    sample_frame(&cell, &positions, host, spec, spec.seed, &mut accumulator)?;
    Ok(accumulator.report(spec, 1))
}

/// Insert the probe into each listed trajectory frame and pool the samples.
pub fn widom_insertion_over_frames(
    structure: &Structure,
    trajectory: &Trajectory,
    host: &[NonbondedAtom],
    frames: &[usize],
    spec: &WidomSpec,
) -> Result<WidomReport> {
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
    let mut accumulator = Accumulator::default();
    for (index, &frame) in frames.iter().enumerate() {
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
        let seed = splitmix64(spec.seed ^ (index as u64 + 1));
        sample_frame(&cell, &positions, host, spec, seed, &mut accumulator)?;
    }
    Ok(accumulator.report(spec, frames.len()))
}

#[derive(Default)]
struct Accumulator {
    sum: f64,
    sum_sq: f64,
    favourable: usize,
    count: usize,
}

impl Accumulator {
    fn report(&self, spec: &WidomSpec, frames: usize) -> WidomReport {
        let n = self.count.max(1) as f64;
        let mean = self.sum / n;
        let variance = (self.sum_sq / n - mean * mean).max(0.0);
        let error = if self.count > 1 {
            (variance / (n - 1.0)).sqrt()
        } else {
            0.0
        };
        let kt = KB_KJ_PER_MOL_K * spec.temperature_k;
        let gas_constant_j = KB_KJ_PER_MOL_K * 1000.0;
        WidomReport {
            boltzmann_mean: mean,
            boltzmann_error: error,
            mu_excess_kj_mol: if mean > 0.0 {
                -kt * mean.ln()
            } else {
                f64::INFINITY
            },
            solubility_cm3stp_per_cm3_cmhg: mean * CM3_STP_PER_MOL * 1.0e-6 * PA_PER_CMHG
                / (gas_constant_j * spec.temperature_k),
            favourable_fraction: self.favourable as f64 / n,
            insertions: self.count,
            frames,
        }
    }
}

fn periodic_cell(structure: &Structure) -> Result<UnitCell> {
    match &structure.cell {
        Some(cell) if !cell.is_placeholder() => Ok(cell.clone()),
        _ => bail!(
            "Widom insertion needs a periodic cell; build or pack the system into a box first"
        ),
    }
}

fn sample_frame(
    cell: &UnitCell,
    positions: &[Point3<f32>],
    host: &[NonbondedAtom],
    spec: &WidomSpec,
    seed: u64,
    accumulator: &mut Accumulator,
) -> Result<()> {
    if host.len() != positions.len() {
        bail!(
            "{} host parameters for {} atoms",
            host.len(),
            positions.len()
        );
    }
    if spec.probe.len() != spec.probe_positions.len() || spec.probe.is_empty() {
        bail!("the probe needs one parameter set per atom");
    }
    if spec.insertions == 0 {
        bail!("at least one insertion is needed");
    }
    if spec.temperature_k <= 0.0 {
        bail!("the temperature must be positive");
    }
    let grid = CellList::new(cell, positions, spec.cutoff)?;
    let kt = KB_KJ_PER_MOL_K * spec.temperature_k;
    let tail = lennard_jones_tail(&spec.probe, host, grid.volume(), spec.cutoff);
    let centroid = spec
        .probe_positions
        .iter()
        .fold(Vector3::zeros(), |sum, p| sum + p.coords)
        / spec.probe_positions.len() as f32;
    let local: Vec<Vector3<f64>> = spec
        .probe_positions
        .iter()
        .map(|p| (p.coords - centroid).cast::<f64>())
        .collect();
    let mut rng = seed;
    let mut next = || {
        rng = splitmix64(rng);
        (rng >> 11) as f64 / (1u64 << 53) as f64
    };
    let cutoff_sq = spec.cutoff * spec.cutoff;
    for _ in 0..spec.insertions {
        let origin = cell_point(cell, next(), next(), next());
        let rotation = random_rotation(next(), next(), next());
        let mut energy = 0.0;
        'probe: for (atom, offset) in spec.probe.iter().zip(&local) {
            let point = origin + rotation * offset;
            for neighbour in grid.neighbours(&point) {
                let delta = grid.minimum_image(point - positions[neighbour].coords.cast::<f64>());
                let r_sq = delta.norm_squared();
                if r_sq >= cutoff_sq {
                    continue;
                }
                energy += pair_energy(atom, &host[neighbour], r_sq, spec.cutoff);
                if energy > 50.0 * kt {
                    break 'probe;
                }
            }
        }
        energy += tail;
        let boltzmann = if energy > 50.0 * kt {
            0.0
        } else {
            (-energy / kt).exp()
        };
        accumulator.sum += boltzmann;
        accumulator.sum_sq += boltzmann * boltzmann;
        if energy < 0.0 {
            accumulator.favourable += 1;
        }
        accumulator.count += 1;
    }
    Ok(())
}

/// Lennard-Jones (Lorentz–Berthelot) plus shifted-force Coulomb energy of one
/// pair. A bare `q q / r` truncated per atom leaves tens of kJ/mol from every
/// partial charge whose bonded partner lies just outside the sphere; the
/// shifted-force form goes to zero with zero slope at the cutoff instead.
fn pair_energy(a: &NonbondedAtom, b: &NonbondedAtom, r_sq: f64, cutoff: f64) -> f64 {
    let sigma = 0.5 * (a.sigma + b.sigma);
    let epsilon = (a.epsilon * b.epsilon).sqrt();
    let mut energy = 0.0;
    if epsilon > 0.0 && sigma > 0.0 {
        let s6 = (sigma * sigma / r_sq).powi(3);
        energy += 4.0 * epsilon * (s6 * s6 - s6);
    }
    if a.charge != 0.0 && b.charge != 0.0 {
        let r = r_sq.sqrt();
        let shifted = 1.0 / r - 1.0 / cutoff + (r - cutoff) / (cutoff * cutoff);
        energy += COULOMB_KJ_A_PER_MOL_E2 * a.charge * b.charge * shifted;
    }
    energy
}

/// Energy (kJ/mol) the truncated Lennard-Jones sum misses beyond the cutoff,
/// assuming a uniform host there: per probe–host pair
/// `(16π/3V) ε σ³ [(σ/rc)⁹/3 − (σ/rc)³]`, the full pair sum for one inserted
/// particle rather than the halved per-particle share of a total energy.
/// Without it μ_ex is too high by a few kJ/mol for an aromatic probe at a 10 Å
/// cutoff, and inconsistent with an MD run that applied the matching
/// dispersion correction.
fn lennard_jones_tail(
    probe: &[NonbondedAtom],
    host: &[NonbondedAtom],
    volume: f64,
    cutoff: f64,
) -> f64 {
    let mut tail = 0.0;
    for a in probe {
        for b in host {
            let sigma = 0.5 * (a.sigma + b.sigma);
            let epsilon = (a.epsilon * b.epsilon).sqrt();
            if epsilon <= 0.0 || sigma <= 0.0 {
                continue;
            }
            let s3 = (sigma / cutoff).powi(3);
            tail += epsilon * sigma.powi(3) * (s3 * s3 * s3 / 3.0 - s3);
        }
    }
    16.0 * std::f64::consts::PI / (3.0 * volume) * tail
}

fn cell_point(cell: &UnitCell, fa: f64, fb: f64, fc: f64) -> Vector3<f64> {
    cell.vectors[0].cast::<f64>() * fa
        + cell.vectors[1].cast::<f64>() * fb
        + cell.vectors[2].cast::<f64>() * fc
}

/// A uniformly distributed random rotation (Shoemake's method).
fn random_rotation(u1: f64, u2: f64, u3: f64) -> Rotation3<f64> {
    let (a, b) = ((1.0 - u1).sqrt(), u1.sqrt());
    let (t1, t2) = (
        2.0 * std::f64::consts::PI * u2,
        2.0 * std::f64::consts::PI * u3,
    );
    UnitQuaternion::new_normalize(Quaternion::new(
        b * t2.cos(),
        a * t1.sin(),
        a * t1.cos(),
        b * t2.sin(),
    ))
    .to_rotation_matrix()
}

/// Host atoms binned by fractional coordinate so a trial point sees only the
/// atoms within a cutoff. Each axis is split into cells no thinner than the
/// cutoff measured perpendicular to the other two axes, so the 27-cell
/// neighbourhood always covers the cutoff sphere (also for triclinic cells).
struct CellList {
    basis: Matrix3<f64>,
    inverse: Matrix3<f64>,
    counts: [usize; 3],
    cells: Vec<Vec<usize>>,
}

impl CellList {
    fn new(cell: &UnitCell, positions: &[Point3<f32>], cutoff: f64) -> Result<Self> {
        if cutoff <= 0.0 {
            bail!("the cutoff must be positive");
        }
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
        if widths.iter().any(|width| *width < 2.0 * cutoff) {
            bail!(
                "the cell is thinner than twice the {cutoff} Å cutoff in some direction; use a \
                 larger box or a shorter --cutoff"
            );
        }
        let counts = widths.map(|width| ((width / cutoff).floor() as usize).clamp(1, 64));
        let mut list = Self {
            basis,
            inverse,
            counts,
            cells: vec![Vec::new(); counts[0] * counts[1] * counts[2]],
        };
        for (index, position) in positions.iter().enumerate() {
            let cell_index = list.cell_of(&position.coords.cast::<f64>());
            list.cells[cell_index].push(index);
        }
        Ok(list)
    }

    fn fractional_cell(&self, point: &Vector3<f64>) -> [i64; 3] {
        let fractional = self.inverse * point;
        let mut out = [0i64; 3];
        for axis in 0..3 {
            let f = fractional[axis] - fractional[axis].floor();
            out[axis] = ((f * self.counts[axis] as f64).floor() as i64)
                .clamp(0, self.counts[axis] as i64 - 1);
        }
        out
    }

    fn cell_of(&self, point: &Vector3<f64>) -> usize {
        let [i, j, k] = self.fractional_cell(point);
        self.flat(i, j, k)
    }

    fn flat(&self, i: i64, j: i64, k: i64) -> usize {
        let i = i.rem_euclid(self.counts[0] as i64) as usize;
        let j = j.rem_euclid(self.counts[1] as i64) as usize;
        let k = k.rem_euclid(self.counts[2] as i64) as usize;
        (i * self.counts[1] + j) * self.counts[2] + k
    }

    /// Indices of the host atoms in the 27 cells around `point`, each once.
    fn neighbours(&self, point: &Vector3<f64>) -> impl Iterator<Item = usize> + '_ {
        let [i, j, k] = self.fractional_cell(point);
        let mut seen = std::collections::BTreeSet::new();
        for di in -1..=1 {
            for dj in -1..=1 {
                for dk in -1..=1 {
                    seen.insert(self.flat(i + di, j + dj, k + dk));
                }
            }
        }
        seen.into_iter()
            .flat_map(move |cell| self.cells[cell].iter().copied())
    }

    fn volume(&self) -> f64 {
        self.basis.determinant().abs()
    }

    fn minimum_image(&self, delta: Vector3<f64>) -> Vector3<f64> {
        let fractional = self.inverse * delta;
        self.basis * fractional.map(|f| f - f.round())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tail_correction_matches_the_uniform_host_integral() {
        let atom = NonbondedAtom {
            sigma: 3.4,
            epsilon: 1.0,
            charge: 0.0,
        };
        let volume = 8000.0;
        let cutoff = 10.0;
        let tail = lennard_jones_tail(&[atom], &[atom; 100], volume, cutoff);
        // ρ ∫_rc^∞ 4ε[(σ/r)¹² − (σ/r)⁶] 4πr² dr, integrated numerically.
        let density = 100.0 / volume;
        let steps = 200_000;
        let upper = 400.0;
        let dr = (upper - cutoff) / steps as f64;
        let integral: f64 = (0..steps)
            .map(|i| {
                let r = cutoff + (i as f64 + 0.5) * dr;
                let s6 = (3.4 / r).powi(6);
                4.0 * (s6 * s6 - s6) * 4.0 * std::f64::consts::PI * r * r * dr
            })
            .sum();
        assert!(tail < 0.0);
        assert!(
            (tail - density * integral).abs() < 1e-4 * tail.abs(),
            "{tail} vs {}",
            density * integral
        );
    }
}
