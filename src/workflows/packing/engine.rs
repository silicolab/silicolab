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

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use anyhow::{Result, bail};
use nalgebra::{Point3, Rotation3, Vector3};

use crate::domain::{Structure, UnitCell, chemistry::nearest_periodic_delta};
use crate::workflows::molecular_dynamics::solvation::splitmix64;

use super::assemble::{self, CopyPlacement};
use super::region::{Region, RegionSense};
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

/// Per-species rigid template data: atom offsets relative to the centroid.
struct SpeciesData {
    offsets: Vec<Vector3<f32>>,
    single_atom: bool,
}

/// A placed copy's 6-DOF state.
#[derive(Clone)]
struct CopyState {
    species: usize,
    center: Point3<f32>,
    rotation: Rotation3<f32>,
}

/// A periodic box for the minimum-image overlap path.
struct PeriodicBox {
    origin: Point3<f32>,
    cell: UnitCell,
}

/// Per-copy gradient (rotation is zero for single-atom species).
#[derive(Clone, Copy)]
struct CopyGrad {
    trans: Vector3<f32>,
    rot: Vector3<f32>,
}

/// Per-copy diagnostics from a full evaluation.
#[derive(Clone, Copy)]
struct CopyDiag {
    blame: f32,
    overlap: f32,
    penetration: f32,
}

/// The result of scoring the whole system at the current configuration.
struct Eval {
    penalty: f64,
    max_overlap: f32,
    per_copy: Vec<CopyDiag>,
}

struct Packer {
    request: PackRequest,
    sense: RegionSense,
    species_data: Vec<SpeciesData>,
    copies: Vec<CopyState>,
    fixed_world: Vec<Point3<f32>>,
    periodic: Option<PeriodicBox>,
    /// Confinement box for the `Outside` sense (keeps the exterior bounded).
    confine: Option<Region>,
    /// Axis-aligned seeding domain.
    domain: (Point3<f32>, Point3<f32>),
    tol: f32,
    tol_sq: f32,
    // Scratch state for the current accepted configuration (the gradient's
    // static neighborhood). Rebuilt by `evaluate`.
    world: Vec<Point3<f32>>,
    owner: Vec<u32>,
    list: CellList,
}

impl Packer {
    fn new(request: PackRequest) -> Result<Self> {
        let tol = request.tolerance;
        let sense = request.sense;

        // `Outside` carves a void; a box (or periodic cell) fills its own bounds,
        // leaving no exterior shell to pack into. Only sphere/cylinder voids make
        // sense, so reject the contradiction with a clear message instead of
        // silently packing through the void (periodic) or bailing on zero volume.
        if sense == RegionSense::Outside
            && matches!(request.region, Region::Box { .. } | Region::Cell(_))
        {
            bail!(
                "packing outside the region needs a sphere or cylinder; a box or periodic cell \
                 has no exterior to fill"
            );
        }

        let species_data: Vec<SpeciesData> = request
            .species
            .iter()
            .map(|species| {
                let atoms = &species.molecule.atoms;
                let centroid = centroid(atoms.iter().map(|a| a.position));
                let offsets: Vec<Vector3<f32>> =
                    atoms.iter().map(|a| a.position - centroid).collect();
                SpeciesData {
                    single_atom: offsets.len() <= 1,
                    offsets,
                }
            })
            .collect();

        let fixed_world: Vec<Point3<f32>> = request
            .fixed
            .as_ref()
            .map(|f| f.atoms.iter().map(|a| a.position).collect())
            .unwrap_or_default();

        // Periodic path: a real cell (always for `Region::Cell`) or a box marked
        // periodic. Sphere/cylinder regions are never periodic.
        let periodic = match &request.region {
            Region::Cell(cell) => Some(PeriodicBox {
                origin: Point3::origin(),
                cell: cell.clone(),
            }),
            Region::Box { min, max } if request.periodic => {
                let ext = max - min;
                Some(PeriodicBox {
                    origin: *min,
                    cell: UnitCell::from_parameters(
                        ext.x.max(1.0e-3),
                        ext.y.max(1.0e-3),
                        ext.z.max(1.0e-3),
                        90.0,
                        90.0,
                        90.0,
                    ),
                })
            }
            _ => None,
        };

        // Seeding/confinement domain.
        let domain = match (&request.region, sense) {
            (region, RegionSense::Outside) if periodic.is_none() => request
                .output_cell
                .as_ref()
                .map(cell_bounding_box)
                .unwrap_or_else(|| region.bounding_box()),
            (region, _) => region.bounding_box(),
        };
        let confine = if sense == RegionSense::Outside && periodic.is_none() {
            Some(Region::Box {
                min: domain.0,
                max: domain.1,
            })
        } else {
            None
        };

        // Feasibility: the allowed domain must hold at least one molecule.
        let allowed_volume = allowed_volume(&request.region, domain, sense, periodic.is_some());
        if allowed_volume < tol * tol * tol {
            bail!(
                "the packing region is too small to hold even one molecule at this spacing; \
                 enlarge it or lower the spacing"
            );
        }

        let mut packer = Self {
            sense,
            species_data,
            copies: Vec::new(),
            fixed_world,
            periodic,
            confine,
            domain,
            tol,
            tol_sq: tol * tol,
            world: Vec::new(),
            owner: Vec::new(),
            list: CellList::default(),
            request,
        };
        packer.seed();
        Ok(packer)
    }

    /// Seed each copy onto a jittered regular lattice filling the domain, with a
    /// deterministic random orientation. Species are interleaved so mixtures mix.
    fn seed(&mut self) {
        let species_ids = self.seed_species_order();
        let total = species_ids.len();
        let centers = self.seed_centers(total);

        self.copies = species_ids
            .into_iter()
            .enumerate()
            .map(|(index, species)| {
                let mut rng = Rng::keyed(self.request.seed, 0, index as u64);
                let rotation = if self.species_data[species].single_atom {
                    Rotation3::identity()
                } else {
                    rng.rotation()
                };
                CopyState {
                    species,
                    center: centers[index],
                    rotation,
                }
            })
            .collect();
    }

    /// The species id for each copy, deterministically shuffled so that a
    /// mixture is interleaved across the seed lattice rather than blocked.
    fn seed_species_order(&self) -> Vec<usize> {
        let mut ids: Vec<usize> = Vec::new();
        for (species, spec) in self.request.species.iter().enumerate() {
            ids.extend(std::iter::repeat_n(species, spec.count));
        }
        // Fisher-Yates with the seed stream.
        let mut rng = Rng::keyed(self.request.seed, 1, 0);
        for i in (1..ids.len()).rev() {
            let j = (rng.next_u64() % (i as u64 + 1)) as usize;
            ids.swap(i, j);
        }
        ids
    }

    /// `total` seed centers spread across the allowed domain.
    fn seed_centers(&self, total: usize) -> Vec<Point3<f32>> {
        if let Some(periodic) = &self.periodic {
            return self.seed_centers_periodic(periodic, total);
        }
        // Cartesian jittered lattice over the domain, keeping allowed points.
        let (min, max) = self.domain;
        let ext = max - min;
        let bbox_vol = (ext.x * ext.y * ext.z).max(1.0e-6);
        let allowed_vol = allowed_volume(&self.request.region, self.domain, self.sense, false);
        let frac = (allowed_vol / bbox_vol).clamp(0.02, 1.0);
        let target = ((total as f32) / frac * 1.6).ceil().max(1.0);
        let spacing = (bbox_vol / target).cbrt().max(self.tol * 0.5);
        let counts = [
            (ext.x / spacing).round().max(1.0) as usize,
            (ext.y / spacing).round().max(1.0) as usize,
            (ext.z / spacing).round().max(1.0) as usize,
        ];

        let mut allowed: Vec<Point3<f32>> = Vec::new();
        for i in 0..counts[0] {
            for j in 0..counts[1] {
                for k in 0..counts[2] {
                    let mut rng = Rng::keyed(
                        self.request.seed,
                        2,
                        ((i * counts[1] + j) * counts[2] + k) as u64,
                    );
                    let jitter = |base: f32, n: usize, r: &mut Rng| {
                        ((base + (r.unit() - 0.5) * 0.7) / n as f32).clamp(0.0, 1.0)
                    };
                    let fx = jitter(i as f32 + 0.5, counts[0], &mut rng);
                    let fy = jitter(j as f32 + 0.5, counts[1], &mut rng);
                    let fz = jitter(k as f32 + 0.5, counts[2], &mut rng);
                    let p = Point3::new(min.x + ext.x * fx, min.y + ext.y * fy, min.z + ext.z * fz);
                    if self.allowed(p) {
                        allowed.push(p);
                    }
                }
            }
        }

        self.pick_or_fill(allowed, total)
    }

    fn seed_centers_periodic(&self, periodic: &PeriodicBox, total: usize) -> Vec<Point3<f32>> {
        let per_axis = (total as f32).cbrt().ceil().max(1.0) as usize;
        let mut centers: Vec<Point3<f32>> = Vec::with_capacity(per_axis.pow(3));
        for i in 0..per_axis {
            for j in 0..per_axis {
                for k in 0..per_axis {
                    let mut rng = Rng::keyed(
                        self.request.seed,
                        3,
                        ((i * per_axis + j) * per_axis + k) as u64,
                    );
                    let f = |idx: usize, r: &mut Rng| {
                        ((idx as f32 + 0.5) / per_axis as f32
                            + (r.unit() - 0.5) * 0.4 / per_axis as f32)
                            .rem_euclid(1.0)
                    };
                    let fx = f(i, &mut rng);
                    let fy = f(j, &mut rng);
                    let fz = f(k, &mut rng);
                    centers.push(
                        periodic.origin + periodic.cell.fractional_to_cartesian(fx, fy, fz).coords,
                    );
                }
            }
        }
        self.pick_or_fill(centers, total)
    }

    /// Pick `total` centers spread across `candidates`, filling any shortfall
    /// with rejection-sampled allowed points.
    fn pick_or_fill(&self, candidates: Vec<Point3<f32>>, total: usize) -> Vec<Point3<f32>> {
        let mut centers = Vec::with_capacity(total);
        if candidates.len() >= total && total > 0 {
            for index in 0..total {
                let pick = (index * candidates.len()) / total;
                centers.push(candidates[pick]);
            }
        } else {
            centers.extend(candidates);
            let mut rng = Rng::keyed(self.request.seed, 4, 0);
            while centers.len() < total {
                centers.push(self.random_allowed_point(&mut rng));
            }
        }
        centers
    }

    /// A rejection-sampled point on the allowed side of the region.
    fn random_allowed_point(&self, rng: &mut Rng) -> Point3<f32> {
        if let Some(periodic) = &self.periodic {
            let p = periodic.origin
                + periodic
                    .cell
                    .fractional_to_cartesian(rng.unit(), rng.unit(), rng.unit())
                    .coords;
            return p;
        }
        let (min, max) = self.domain;
        let mut fallback = Point3::from((min.coords + max.coords) * 0.5);
        for _ in 0..64 {
            let p = Point3::new(
                rng.range(min.x, max.x),
                rng.range(min.y, max.y),
                rng.range(min.z, max.z),
            );
            fallback = p;
            if self.allowed(p) {
                return p;
            }
        }
        fallback
    }

    /// Whether a candidate center lies on the allowed side of the region.
    fn allowed(&self, p: Point3<f32>) -> bool {
        if self.periodic.is_some() {
            return true;
        }
        if !self.request.region.contains(p, self.sense) {
            return false;
        }
        match &self.confine {
            Some(confine) => confine.contains(p, RegionSense::Inside),
            None => true,
        }
    }

    /// Run the optimizer, streaming progress and honoring cancel/timeout.
    fn optimize(
        mut self,
        cancel: Arc<AtomicBool>,
        mut progress: impl FnMut(PackProgress) -> Result<()>,
    ) -> Result<PackResult> {
        let started = Instant::now();
        let tolerance = self.request.limits.penalty_tolerance as f64;
        let mut step_size = INITIAL_STEP;
        let mut steps = 0usize;
        let mut restarts = 0usize;
        let mut timed_out = false;
        let mut last_progress = started;

        let mut current = self.evaluate();

        loop {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            if started.elapsed() >= self.request.limits.max_duration {
                timed_out = true;
                break;
            }
            if current.penalty <= tolerance {
                break;
            }
            if steps >= self.request.limits.max_steps {
                break;
            }

            let gradient = self.gradient();
            match self.line_search(&gradient, step_size, current.penalty) {
                Some((eval, used_step)) => {
                    if self.periodic.is_some() {
                        self.wrap_centers();
                    }
                    current = eval;
                    step_size = (used_step * 1.2).min(MAX_STEP);
                    steps += 1;
                }
                None => {
                    // Stalled. Re-seed the worst copies and continue, if allowed.
                    if restarts < self.request.limits.max_restarts {
                        self.reseed_worst(&current.per_copy, restarts);
                        restarts += 1;
                        step_size = INITIAL_STEP;
                        current = self.evaluate();
                        steps += 1;
                    } else {
                        break;
                    }
                }
            }

            if last_progress.elapsed() >= PROGRESS_INTERVAL {
                let structure = self.assemble_current();
                let report = self.report(&current, steps, restarts, false, timed_out);
                progress(PackProgress { structure, report })?;
                last_progress = Instant::now();
            }
        }

        let converged = current.penalty <= tolerance;
        let structure = self.assemble_current();
        let report = self.report(&current, steps, restarts, converged, timed_out);
        Ok(PackResult { structure, report })
    }

    /// Rebuild the static neighborhood (`world`, `owner`, `list`) for the current
    /// configuration and score the whole system.
    fn evaluate(&mut self) -> Eval {
        self.rebuild_world();
        self.list
            .rebuild(&self.world, self.tol, self.periodic.as_ref());

        let copy_count = self.copies.len();
        let mut per_copy = vec![
            CopyDiag {
                blame: 0.0,
                overlap: 0.0,
                penetration: 0.0,
            };
            copy_count
        ];
        let mut penalty = 0.0f64;
        let mut max_overlap = 0.0f32;
        let mut buf: Vec<u32> = Vec::new();

        for a in 0..self.world.len() {
            let pa = self.world[a];
            let owner_a = self.owner[a];

            // Overlap pairs (count each once via a < b; skip same owner).
            self.list.query(pa, &mut buf);
            for &b in &buf {
                let b = b as usize;
                if b <= a {
                    continue;
                }
                if owner_a == self.owner[b] {
                    continue;
                }
                let d_sq = self.dist_sq(pa, self.world[b]);
                if d_sq < self.tol_sq {
                    let t = (self.tol_sq - d_sq) / self.tol_sq;
                    let contribution = (t * t) as f64;
                    penalty += contribution;
                    let depth = self.tol - d_sq.sqrt();
                    max_overlap = max_overlap.max(depth);
                    if owner_a != FIXED {
                        let diag = &mut per_copy[owner_a as usize];
                        diag.blame += contribution as f32;
                        diag.overlap = diag.overlap.max(depth);
                    }
                    let owner_b = self.owner[b];
                    if owner_b != FIXED {
                        let diag = &mut per_copy[owner_b as usize];
                        diag.blame += contribution as f32;
                        diag.overlap = diag.overlap.max(depth);
                    }
                }
            }

            // Boundary penalty for packed atoms.
            if owner_a != FIXED {
                let (pen, term) = self.boundary(pa);
                penalty += term;
                let diag = &mut per_copy[owner_a as usize];
                diag.blame += term as f32;
                diag.penetration = diag.penetration.max(pen);
            }
        }

        Eval {
            penalty,
            max_overlap,
            per_copy,
        }
    }

    /// The numeric gradient (central differences) over every copy's DOF,
    /// evaluated incrementally against the static neighborhood.
    fn gradient(&self) -> Vec<CopyGrad> {
        let mut gradient = vec![
            CopyGrad {
                trans: Vector3::zeros(),
                rot: Vector3::zeros(),
            };
            self.copies.len()
        ];
        let mut buf: Vec<u32> = Vec::new();

        for (c, grad) in gradient.iter_mut().enumerate() {
            let copy = &self.copies[c];
            let single_atom = self.species_data[copy.species].single_atom;

            for axis in 0..3 {
                let mut delta = Vector3::zeros();
                delta[axis] = EPSILON;
                let plus = self.copy_penalty(c, copy.center + delta, copy.rotation, &mut buf);
                let minus = self.copy_penalty(c, copy.center - delta, copy.rotation, &mut buf);
                grad.trans[axis] = ((plus - minus) / (2.0 * EPSILON as f64)) as f32;
            }

            if !single_atom {
                for axis in 0..3 {
                    let mut axis_angle = Vector3::zeros();
                    axis_angle[axis] = EPSILON;
                    let rot_plus = Rotation3::new(axis_angle) * copy.rotation;
                    let rot_minus = Rotation3::new(-axis_angle) * copy.rotation;
                    let plus = self.copy_penalty(c, copy.center, rot_plus, &mut buf);
                    let minus = self.copy_penalty(c, copy.center, rot_minus, &mut buf);
                    grad.rot[axis] = ((plus - minus) / (2.0 * EPSILON as f64)) as f32;
                }
            }
        }

        gradient
    }

    /// The penalty contributed by copy `c` at a candidate pose: its boundary
    /// terms plus its overlaps with every *other* copy/fixed atom (read from the
    /// static neighborhood, whose other-copy positions are unchanged).
    fn copy_penalty(
        &self,
        c: usize,
        center: Point3<f32>,
        rotation: Rotation3<f32>,
        buf: &mut Vec<u32>,
    ) -> f64 {
        let species = &self.species_data[self.copies[c].species];
        let mut penalty = 0.0f64;
        for offset in &species.offsets {
            let p = center + rotation * offset;
            penalty += self.boundary(p).1;
            self.list.query(p, buf);
            for &slot in buf.iter() {
                if self.owner[slot as usize] == c as u32 {
                    continue;
                }
                let d_sq = self.dist_sq(p, self.world[slot as usize]);
                if d_sq < self.tol_sq {
                    let t = (self.tol_sq - d_sq) / self.tol_sq;
                    penalty += (t * t) as f64;
                }
            }
        }
        penalty
    }

    /// Backtracking line search down the negative gradient. Applies the step from
    /// the saved configuration, accepting the first that lowers the total
    /// penalty; otherwise halves and retries up to [`MAX_BACKTRACK`] times.
    fn line_search(
        &mut self,
        gradient: &[CopyGrad],
        step_size: f32,
        current_penalty: f64,
    ) -> Option<(Eval, f32)> {
        let saved = self.copies.clone();
        let mut step = step_size;
        for _ in 0..MAX_BACKTRACK {
            self.apply_step(&saved, gradient, step);
            let eval = self.evaluate();
            if eval.penalty < current_penalty {
                return Some((eval, step));
            }
            step *= 0.5;
        }
        self.copies.clone_from(&saved);
        None
    }

    /// Move every copy by `-step · gradient` from the saved configuration, with
    /// per-copy translation and rotation caps.
    fn apply_step(&mut self, saved: &[CopyState], gradient: &[CopyGrad], step: f32) {
        let max_trans = self.request.limits.max_translation_step;
        let max_rot = self.request.limits.max_rotation_step;
        for c in 0..self.copies.len() {
            let single_atom = self.species_data[saved[c].species].single_atom;
            let trans_delta = clamp_norm(gradient[c].trans * -step, max_trans);
            self.copies[c].center = saved[c].center + trans_delta;
            self.copies[c].rotation = if single_atom {
                saved[c].rotation
            } else {
                let rot_delta = clamp_norm(gradient[c].rot * -step, max_rot);
                Rotation3::new(rot_delta) * saved[c].rotation
            };
        }
    }

    /// Re-seed the worst-offending copies into the least-crowded candidate pose,
    /// with a fresh random orientation. Keyed on `(seed, restart, copy)`.
    fn reseed_worst(&mut self, per_copy: &[CopyDiag], restart: usize) {
        // A failed line search leaves the neighborhood reflecting its last
        // rejected trial; rebuild it from the current (restored) configuration so
        // `local_crowding` scores candidate poses against the real positions.
        self.rebuild_world();
        self.list
            .rebuild(&self.world, self.tol, self.periodic.as_ref());

        let count = self.copies.len();
        let worst_count = ((count as f32) * 0.1).ceil().max(1.0) as usize;
        let mut order: Vec<usize> = (0..count).collect();
        order.sort_by(|&a, &b| {
            per_copy[b]
                .blame
                .partial_cmp(&per_copy[a].blame)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        for &c in order.iter().take(worst_count) {
            if per_copy[c].blame <= 0.0 {
                break;
            }
            let mut rng = Rng::keyed(self.request.seed, 100 + restart as u64, c as u64);
            // Sample candidate poses; keep the one with the least local crowding.
            let mut best = self.copies[c].center;
            let mut best_crowding = f32::INFINITY;
            for _ in 0..16 {
                let candidate = self.random_allowed_point(&mut rng);
                let crowding = self.local_crowding(candidate, c);
                if crowding < best_crowding {
                    best_crowding = crowding;
                    best = candidate;
                }
            }
            self.copies[c].center = best;
            self.copies[c].rotation = if self.species_data[self.copies[c].species].single_atom {
                Rotation3::identity()
            } else {
                rng.rotation()
            };
        }
    }

    /// A crowding score for placing copy `c`'s centroid at `p`: the number of
    /// other atoms within one tolerance of it (read from the static list).
    fn local_crowding(&self, p: Point3<f32>, c: usize) -> f32 {
        let mut buf: Vec<u32> = Vec::new();
        self.list.query(p, &mut buf);
        let mut crowding = 0.0f32;
        for &slot in &buf {
            if self.owner[slot as usize] == c as u32 {
                continue;
            }
            if self.dist_sq(p, self.world[slot as usize]) < self.tol_sq {
                crowding += 1.0;
            }
        }
        crowding
    }

    /// The boundary penetration and its weighted squared penalty for a point.
    fn boundary(&self, p: Point3<f32>) -> (f32, f64) {
        if self.periodic.is_some() {
            return (0.0, 0.0);
        }
        let pen = self.request.region.penetration(p, self.sense);
        let mut worst = pen;
        let mut term = (pen * pen) as f64;
        if let Some(confine) = &self.confine {
            let cpen = confine.penetration(p, RegionSense::Inside);
            worst = worst.max(cpen);
            term += (cpen * cpen) as f64;
        }
        (worst, REGION_WEIGHT as f64 * term)
    }

    fn dist_sq(&self, a: Point3<f32>, b: Point3<f32>) -> f32 {
        match &self.periodic {
            Some(periodic) => nearest_periodic_delta(&periodic.cell, a, b).norm_squared(),
            None => (a - b).norm_squared(),
        }
    }

    /// Wrap copy centers back into the periodic box (keeps molecules whole).
    fn wrap_centers(&mut self) {
        let Some(periodic) = &self.periodic else {
            return;
        };
        for copy in &mut self.copies {
            let rel = copy.center - periodic.origin;
            let mut frac = periodic.cell.cartesian_to_fractional(Point3::from(rel));
            frac.x = frac.x.rem_euclid(1.0);
            frac.y = frac.y.rem_euclid(1.0);
            frac.z = frac.z.rem_euclid(1.0);
            copy.center = periodic.origin
                + periodic
                    .cell
                    .fractional_to_cartesian(frac.x, frac.y, frac.z)
                    .coords;
        }
    }

    /// World positions of one copy's atoms in template order.
    fn copy_positions(&self, copy: &CopyState) -> Vec<Point3<f32>> {
        self.species_data[copy.species]
            .offsets
            .iter()
            .map(|offset| copy.center + copy.rotation * offset)
            .collect()
    }

    /// Rebuild the flat `world`/`owner` arrays (copies first, then fixed atoms).
    fn rebuild_world(&mut self) {
        let total: usize = self
            .copies
            .iter()
            .map(|c| self.species_data[c.species].offsets.len())
            .sum();
        self.world.clear();
        self.world.reserve(total + self.fixed_world.len());
        self.owner.clear();
        self.owner.reserve(total + self.fixed_world.len());
        for (index, copy) in self.copies.iter().enumerate() {
            for offset in &self.species_data[copy.species].offsets {
                self.world.push(copy.center + copy.rotation * offset);
                self.owner.push(index as u32);
            }
        }
        for &p in &self.fixed_world {
            self.world.push(p);
            self.owner.push(FIXED);
        }
    }

    fn assemble_current(&self) -> Structure {
        let copies: Vec<CopyPlacement> = self
            .copies
            .iter()
            .map(|copy| CopyPlacement {
                species_index: copy.species,
                positions: self.copy_positions(copy),
            })
            .collect();
        assemble::assemble(
            &self.request.species,
            &copies,
            self.request.fixed.as_ref(),
            result_cell(&self.request),
            "Disordered system",
        )
    }

    fn report(
        &self,
        eval: &Eval,
        steps: usize,
        restarts: usize,
        converged: bool,
        timed_out: bool,
    ) -> PackReport {
        let mut placed = vec![0usize; self.request.species.len()];
        for (copy, diag) in self.copies.iter().zip(&eval.per_copy) {
            if diag.overlap <= PLACE_TOL && diag.penetration <= PLACE_TOL {
                placed[copy.species] += 1;
            }
        }
        PackReport {
            placed,
            requested: self.request.species.iter().map(|s| s.count).collect(),
            restarts_used: restarts,
            final_penalty: eval.penalty as f32,
            max_overlap: eval.max_overlap,
            converged,
            timed_out,
            steps,
        }
    }
}

/// A flat-grid cell-list keyed on `tol`-sized buckets, supporting a Euclidean
/// grid and a periodic (fractional, wrapped) grid.
#[derive(Default)]
struct CellList {
    periodic: bool,
    cell: Option<UnitCell>,
    origin: Point3<f32>,
    bucket: f32,
    dims: [i32; 3],
    buckets: Vec<Vec<u32>>,
}

impl CellList {
    fn rebuild(&mut self, world: &[Point3<f32>], tol: f32, periodic: Option<&PeriodicBox>) {
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
    fn query(&self, p: Point3<f32>, out: &mut Vec<u32>) {
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

/// A SplitMix64 stream reusing the crate's `splitmix64` finalizer, keyed
/// deterministically on `(seed, salt, index)`.
struct Rng {
    state: u64,
}

impl Rng {
    fn keyed(seed: u64, salt: u64, index: u64) -> Self {
        let state = splitmix64(seed)
            ^ splitmix64(salt.wrapping_mul(0x9E37_79B9_7F4A_7C15))
            ^ splitmix64(index.wrapping_add(0xD1B5_4A32_D192_ED03));
        Self { state }
    }

    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        splitmix64(self.state)
    }

    /// A uniform f32 in `[0, 1)`.
    fn unit(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / ((1u64 << 24) as f32)
    }

    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.unit()
    }

    fn rotation(&mut self) -> Rotation3<f32> {
        use std::f32::consts::TAU;
        Rotation3::from_euler_angles(
            self.range(0.0, TAU),
            self.range(0.0, TAU),
            self.range(0.0, TAU),
        )
    }
}

fn centroid(points: impl Iterator<Item = Point3<f32>>) -> Point3<f32> {
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

fn clamp_norm(v: Vector3<f32>, max: f32) -> Vector3<f32> {
    let norm = v.norm();
    if norm > max && norm > 0.0 {
        v * (max / norm)
    } else {
        v
    }
}

fn cell_bounding_box(cell: &UnitCell) -> (Point3<f32>, Point3<f32>) {
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
fn allowed_volume(
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Atom, Structure};
    use crate::workflows::packing::{PackLimits, PackSpecies};

    fn argon() -> Structure {
        Structure::new(
            "Ar",
            vec![Atom {
                element: "Ar".to_string(),
                position: Point3::origin(),
                charge: 0.0,
            }],
        )
    }

    fn box_region(edge: f32) -> Region {
        Region::Box {
            min: Point3::origin(),
            max: Point3::new(edge, edge, edge),
        }
    }

    fn base_request(species: Vec<PackSpecies>, region: Region) -> PackRequest {
        PackRequest {
            species,
            region,
            sense: RegionSense::Inside,
            tolerance: 2.0,
            periodic: false,
            seed: 1,
            fixed: None,
            output_cell: None,
            limits: PackLimits {
                max_duration: Duration::from_secs(5),
                ..PackLimits::default()
            },
        }
    }

    fn no_progress(_: PackProgress) -> Result<()> {
        Ok(())
    }

    fn min_pair_distance(structure: &Structure, residue_size: usize) -> f32 {
        let mut min = f32::INFINITY;
        let atoms = &structure.atoms;
        for i in 0..atoms.len() {
            for j in (i + 1)..atoms.len() {
                // Skip intra-molecule pairs (same residue block).
                if i / residue_size == j / residue_size {
                    continue;
                }
                let d = (atoms[i].position - atoms[j].position).norm();
                min = min.min(d);
            }
        }
        min
    }

    #[test]
    fn overlapping_single_atoms_separate_to_tolerance() {
        let request = base_request(
            vec![PackSpecies {
                molecule: argon(),
                count: 8,
            }],
            box_region(12.0),
        );
        let result = run(request, Arc::new(AtomicBool::new(false)), no_progress).unwrap();
        assert_eq!(result.structure.atoms.len(), 8);
        let min = min_pair_distance(&result.structure, 1);
        assert!(
            min >= 2.0 - PLACE_TOL,
            "atoms still overlap: min distance {min:.3}"
        );
    }

    #[test]
    fn same_seed_is_bit_for_bit_reproducible() {
        let make = || {
            base_request(
                vec![PackSpecies {
                    molecule: argon(),
                    count: 10,
                }],
                box_region(15.0),
            )
        };
        let a = run(make(), Arc::new(AtomicBool::new(false)), no_progress).unwrap();
        let b = run(make(), Arc::new(AtomicBool::new(false)), no_progress).unwrap();
        assert_eq!(a.structure.atoms.len(), b.structure.atoms.len());
        for (x, y) in a.structure.atoms.iter().zip(&b.structure.atoms) {
            assert_eq!(x.position, y.position, "packing is not deterministic");
        }
    }

    #[test]
    fn different_seed_gives_a_different_packing() {
        let mut request = base_request(
            vec![PackSpecies {
                molecule: argon(),
                count: 10,
            }],
            box_region(15.0),
        );
        let a = run(
            request.clone(),
            Arc::new(AtomicBool::new(false)),
            no_progress,
        )
        .unwrap();
        request.seed = 2;
        let b = run(request, Arc::new(AtomicBool::new(false)), no_progress).unwrap();
        let differs = a
            .structure
            .atoms
            .iter()
            .zip(&b.structure.atoms)
            .any(|(x, y)| x.position != y.position);
        assert!(differs, "different seeds produced identical packings");
    }

    #[test]
    fn all_atoms_land_inside_the_region() {
        let request = base_request(
            vec![PackSpecies {
                molecule: argon(),
                count: 12,
            }],
            box_region(14.0),
        );
        let result = run(request, Arc::new(AtomicBool::new(false)), no_progress).unwrap();
        for atom in &result.structure.atoms {
            let p = atom.position;
            assert!(
                (-PLACE_TOL..=14.0 + PLACE_TOL).contains(&p.x)
                    && (-PLACE_TOL..=14.0 + PLACE_TOL).contains(&p.y)
                    && (-PLACE_TOL..=14.0 + PLACE_TOL).contains(&p.z),
                "atom escaped the box: {p:?}"
            );
        }
    }

    #[test]
    fn tiny_region_is_rejected() {
        let request = base_request(
            vec![PackSpecies {
                molecule: argon(),
                count: 1,
            }],
            box_region(0.5),
        );
        let err = run(request, Arc::new(AtomicBool::new(false)), no_progress).unwrap_err();
        assert!(err.to_string().contains("too small"));
    }

    #[test]
    fn periodic_box_result_carries_a_cell_without_output_cell() {
        let mut request = base_request(
            vec![PackSpecies {
                molecule: argon(),
                count: 6,
            }],
            box_region(16.0),
        );
        request.periodic = true;
        request.output_cell = None;
        let result = run(request, Arc::new(AtomicBool::new(false)), no_progress).unwrap();
        // A periodic box must stamp its cell, or the min-image spacing reads as
        // cross-edge clashes downstream.
        assert!(result.structure.cell.is_some());
    }

    #[test]
    fn outside_a_box_is_rejected() {
        let mut request = base_request(
            vec![PackSpecies {
                molecule: argon(),
                count: 4,
            }],
            box_region(20.0),
        );
        request.sense = RegionSense::Outside;
        let err = run(request, Arc::new(AtomicBool::new(false)), no_progress).unwrap_err();
        assert!(err.to_string().contains("sphere or cylinder"), "got: {err}");
    }

    #[test]
    fn zero_count_is_a_no_op() {
        let request = base_request(
            vec![PackSpecies {
                molecule: argon(),
                count: 0,
            }],
            box_region(10.0),
        );
        let result = run(request, Arc::new(AtomicBool::new(false)), no_progress).unwrap();
        assert!(result.structure.atoms.is_empty());
        assert!(result.report.converged);
    }
}
