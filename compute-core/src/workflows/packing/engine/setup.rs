use super::*;

use anyhow::{Result, bail};
use nalgebra::{Point3, Rotation3, Vector3};

use crate::domain::UnitCell;

use super::super::region::{Region, RegionSense};

impl Packer {
    pub(crate) fn new(request: PackRequest) -> Result<Self> {
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
    pub(crate) fn random_allowed_point(&self, rng: &mut Rng) -> Point3<f32> {
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
}
