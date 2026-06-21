use super::*;

use nalgebra::Point3;

use crate::domain::{Structure, chemistry::nearest_periodic_delta};

use super::super::PackReport;
use super::super::assemble::{self, CopyPlacement};
use super::super::region::RegionSense;

impl Packer {
    /// Rebuild the static neighborhood (`world`, `owner`, `list`) for the current
    /// configuration and score the whole system.
    pub(crate) fn evaluate(&mut self) -> Eval {
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

    /// The boundary penetration and its weighted squared penalty for a point.
    pub(crate) fn boundary(&self, p: Point3<f32>) -> (f32, f64) {
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

    pub(crate) fn dist_sq(&self, a: Point3<f32>, b: Point3<f32>) -> f32 {
        match &self.periodic {
            Some(periodic) => nearest_periodic_delta(&periodic.cell, a, b).norm_squared(),
            None => (a - b).norm_squared(),
        }
    }

    /// Wrap copy centers back into the periodic box (keeps molecules whole).
    pub(crate) fn wrap_centers(&mut self) {
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
    pub(crate) fn rebuild_world(&mut self) {
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

    pub(crate) fn assemble_current(&self) -> Structure {
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

    pub(crate) fn report(
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
