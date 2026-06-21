mod uff;

use anyhow::{Result, anyhow};
use std::{
    collections::BTreeSet,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use crate::domain::{BondType, Structure};

#[derive(Debug, Clone)]
pub struct OptimizationOptions {
    pub max_steps: usize,
    pub gradient_tolerance: f32,
    pub initial_step_size: f32,
    pub max_atom_step: f32,
    pub atoms: AtomOptimizationScope,
    pub cell: CellOptimizationOptions,
    pub max_cell_length_step: f32,
    pub max_cell_angle_step: f32,
    pub max_duration: Duration,
}

#[derive(Debug, Clone, Default)]
pub enum AtomOptimizationScope {
    #[default]
    All,
    Selected(Vec<usize>),
}

impl AtomOptimizationScope {
    pub fn movable_mask(&self, atom_count: usize) -> Vec<bool> {
        match self {
            Self::All => vec![true; atom_count],
            Self::Selected(indices) => {
                let mut mask = vec![false; atom_count];
                for index in indices {
                    if *index < atom_count {
                        mask[*index] = true;
                    }
                }
                mask
            }
        }
    }

    pub fn active_count(&self, atom_count: usize) -> usize {
        match self {
            Self::All => atom_count,
            Self::Selected(indices) => indices
                .iter()
                .copied()
                .filter(|index| *index < atom_count)
                .collect::<BTreeSet<_>>()
                .len(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct CellOptimizationOptions {
    pub a: bool,
    pub b: bool,
    pub c: bool,
    pub alpha: bool,
    pub beta: bool,
    pub gamma: bool,
}

impl CellOptimizationOptions {
    pub fn lengths_only() -> Self {
        Self {
            a: true,
            b: true,
            c: true,
            alpha: false,
            beta: false,
            gamma: false,
        }
    }

    pub fn all() -> Self {
        Self {
            a: true,
            b: true,
            c: true,
            alpha: true,
            beta: true,
            gamma: true,
        }
    }

    pub fn any(self) -> bool {
        self.a || self.b || self.c || self.alpha || self.beta || self.gamma
    }

    pub fn count(self) -> usize {
        [self.a, self.b, self.c, self.alpha, self.beta, self.gamma]
            .into_iter()
            .filter(|enabled| *enabled)
            .count()
    }
}

impl Default for OptimizationOptions {
    fn default() -> Self {
        Self {
            max_steps: 200,
            gradient_tolerance: 1.0e-3,
            initial_step_size: 0.002,
            max_atom_step: 0.05,
            atoms: AtomOptimizationScope::All,
            cell: CellOptimizationOptions::default(),
            max_cell_length_step: 0.05,
            max_cell_angle_step: 0.25,
            max_duration: Duration::from_secs(60),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct OptimizationReport {
    pub initial_energy: f32,
    pub final_energy: f32,
    pub steps: usize,
    pub converged: bool,
    pub stopped: bool,
    pub timed_out: bool,
}

#[derive(Debug, Clone)]
pub struct OptimizationControl {
    cancel_requested: Arc<AtomicBool>,
    started_at: Instant,
    max_duration: Duration,
}

impl OptimizationControl {
    pub fn new(cancel_requested: Arc<AtomicBool>, max_duration: Duration) -> Self {
        Self {
            cancel_requested,
            started_at: Instant::now(),
            max_duration,
        }
    }

    pub fn should_stop(&self) -> bool {
        self.cancel_requested.load(Ordering::Relaxed) || self.timed_out()
    }

    pub fn timed_out(&self) -> bool {
        self.started_at.elapsed() >= self.max_duration
    }
}

#[derive(Debug, Clone)]
pub struct GeometryOptimizer {
    options: OptimizationOptions,
    initial_energy: f32,
    current_energy: f32,
    current_step_size: f32,
    steps: usize,
    converged: bool,
    stopped: bool,
    timed_out: bool,
    finished: bool,
    movable_atoms: Vec<bool>,
    movable_atom_count: usize,
}

impl GeometryOptimizer {
    pub fn new(structure: &Structure, options: OptimizationOptions) -> Result<Self> {
        let initial_energy = uff::energy(structure)?;
        let initial_step_size = options.initial_step_size;
        let movable_atoms = options.atoms.movable_mask(structure.atoms.len());
        let movable_atom_count = options.atoms.active_count(structure.atoms.len());

        Ok(Self {
            options,
            initial_energy,
            current_energy: initial_energy,
            current_step_size: initial_step_size,
            steps: 0,
            converged: false,
            stopped: false,
            timed_out: false,
            finished: false,
            movable_atoms,
            movable_atom_count,
        })
    }

    pub fn step_with_control(
        &mut self,
        structure: &mut Structure,
        control: Option<&OptimizationControl>,
    ) -> Result<bool> {
        if self.finished {
            return Ok(true);
        }
        if let Some(control) = control
            && control.should_stop()
        {
            self.timed_out = control.timed_out();
            self.stopped = !self.timed_out;
            self.finished = true;
            return Ok(true);
        }
        if self.steps >= self.options.max_steps {
            self.finished = true;
            return Ok(true);
        }

        match uff::gradient_step(
            structure,
            uff::GradientStepConfig {
                current_energy: self.current_energy,
                initial_step_size: self.current_step_size,
                max_atom_step: self.options.max_atom_step,
                movable_atoms: &self.movable_atoms,
                movable_atom_count: self.movable_atom_count,
                cell_options: self.options.cell,
                max_cell_length_step: self.options.max_cell_length_step,
                max_cell_angle_step: self.options.max_cell_angle_step,
                gradient_tolerance: self.options.gradient_tolerance,
                control,
            },
        )? {
            uff::GradientStepResult::Accepted { energy, step_size } => {
                self.current_energy = energy;
                self.current_step_size = (step_size * 1.2).min(0.2);
                self.steps += 1;
            }
            uff::GradientStepResult::Converged => {
                self.converged = true;
                self.finished = true;
            }
            uff::GradientStepResult::Stopped { timed_out } => {
                self.timed_out = timed_out;
                self.stopped = !timed_out;
                self.finished = true;
            }
            uff::GradientStepResult::Rejected => {
                self.finished = true;
            }
        }

        Ok(self.finished)
    }

    pub fn report(&self) -> OptimizationReport {
        OptimizationReport {
            initial_energy: self.initial_energy,
            final_energy: self.current_energy,
            steps: self.steps,
            converged: self.converged,
            stopped: self.stopped,
            timed_out: self.timed_out,
        }
    }
}

#[allow(dead_code)]
pub fn optimize_geometry(
    structure: &mut Structure,
    options: OptimizationOptions,
) -> Result<OptimizationReport> {
    let max_duration = options.max_duration;
    let mut optimizer = GeometryOptimizer::new(structure, options)?;
    let control = OptimizationControl::new(Arc::new(AtomicBool::new(false)), max_duration);

    while !optimizer.step_with_control(structure, Some(&control))? {}

    Ok(optimizer.report())
}

pub fn equilibrium_bond_length(first: &str, second: &str, bond_type: BondType) -> Result<f32> {
    uff::bond_length(first, second, bond_type).ok_or_else(|| {
        anyhow!("UFF has no equilibrium bond length for {first}-{second} {bond_type:?}")
    })
}
