use super::*;

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Instant,
};

use anyhow::Result;
use nalgebra::{Point3, Rotation3, Vector3};

use super::super::{PackProgress, PackResult};

impl Packer {
    /// Run the optimizer, streaming progress and honoring cancel/timeout.
    pub(crate) fn optimize(
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
}
