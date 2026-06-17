use super::*;

use anyhow::Result;
use nalgebra::{Point3, Vector3};

use crate::{
    domain::{Structure, UnitCell},
    engines::forcefield::{CellOptimizationOptions, OptimizationControl},
};

#[derive(Debug, Clone)]
struct OptimizationGradient {
    atoms: Vec<Vector3<f32>>,
    cell: Option<CellGradient>,
    cell_parameter_count: usize,
}

#[derive(Debug, Clone, Copy, Default)]
struct CellGradient {
    a: f32,
    b: f32,
    c: f32,
    alpha: f32,
    beta: f32,
    gamma: f32,
}

#[derive(Debug, Clone, Copy)]
struct CellParameters {
    a: f32,
    b: f32,
    c: f32,
    alpha: f32,
    beta: f32,
    gamma: f32,
}

#[derive(Debug, Clone)]
struct OptimizationState {
    positions: Vec<Point3<f32>>,
    cell: Option<UnitCell>,
}

pub(crate) enum GradientStepResult {
    Accepted { energy: f32, step_size: f32 },
    Converged,
    Stopped { timed_out: bool },
    Rejected,
}

pub(crate) struct GradientStepConfig<'a> {
    pub current_energy: f32,
    pub initial_step_size: f32,
    pub max_atom_step: f32,
    pub movable_atoms: &'a [bool],
    pub movable_atom_count: usize,
    pub cell_options: CellOptimizationOptions,
    pub max_cell_length_step: f32,
    pub max_cell_angle_step: f32,
    pub gradient_tolerance: f32,
    pub control: Option<&'a OptimizationControl>,
}

pub(crate) fn gradient_step(
    structure: &mut Structure,
    config: GradientStepConfig<'_>,
) -> Result<GradientStepResult> {
    let GradientStepConfig {
        current_energy,
        initial_step_size,
        max_atom_step,
        movable_atoms,
        movable_atom_count,
        cell_options,
        max_cell_length_step,
        max_cell_angle_step,
        gradient_tolerance,
        control,
    } = config;
    let Some(gradient) =
        numerical_gradient_with_control(structure, movable_atoms, cell_options, control)?
    else {
        return Ok(GradientStepResult::Stopped {
            timed_out: control.is_some_and(OptimizationControl::timed_out),
        });
    };
    if gradient.rms(movable_atom_count) < gradient_tolerance {
        return Ok(GradientStepResult::Converged);
    }

    let original_state = OptimizationState::from_structure(structure);
    let mut step_size = suggested_step_size(&gradient, initial_step_size, max_atom_step);

    for _ in 0..12 {
        if let Some(control) = control
            && control.should_stop()
        {
            return Ok(GradientStepResult::Stopped {
                timed_out: control.timed_out(),
            });
        }
        apply_gradient_step(
            structure,
            &gradient,
            step_size,
            max_atom_step,
            movable_atoms,
            max_cell_length_step,
            max_cell_angle_step,
        );
        let trial_energy = energy(structure)?;

        if trial_energy < current_energy {
            return Ok(GradientStepResult::Accepted {
                energy: trial_energy,
                step_size,
            });
        }

        original_state.restore(structure);
        step_size *= 0.5;
    }

    Ok(GradientStepResult::Rejected)
}

fn numerical_gradient_with_control(
    structure: &mut Structure,
    movable_atoms: &[bool],
    cell_options: CellOptimizationOptions,
    control: Option<&OptimizationControl>,
) -> Result<Option<OptimizationGradient>> {
    let epsilon = 1.0e-3;
    let mut atoms = vec![Vector3::zeros(); structure.atoms.len()];

    for atom_index in 0..structure.atoms.len() {
        if !movable_atoms.get(atom_index).copied().unwrap_or(false) {
            continue;
        }
        if let Some(control) = control
            && control.should_stop()
        {
            return Ok(None);
        }
        for (axis, atom_gradient) in atoms[atom_index].as_mut_slice().iter_mut().enumerate() {
            if let Some(control) = control
                && control.should_stop()
            {
                return Ok(None);
            }
            structure.atoms[atom_index].position[axis] += epsilon;
            let plus = energy(structure)?;
            structure.atoms[atom_index].position[axis] -= 2.0 * epsilon;
            let minus = energy(structure)?;
            structure.atoms[atom_index].position[axis] += epsilon;

            *atom_gradient = (plus - minus) / (2.0 * epsilon);
        }
    }

    let cell = if cell_options.any() && structure.cell.is_some() {
        match numerical_cell_gradient(structure, cell_options, control)? {
            Some(gradient) => Some(gradient),
            None => return Ok(None),
        }
    } else {
        None
    };

    Ok(Some(OptimizationGradient {
        atoms,
        cell,
        cell_parameter_count: cell_options.count(),
    }))
}

fn numerical_cell_gradient(
    structure: &mut Structure,
    cell_options: CellOptimizationOptions,
    control: Option<&OptimizationControl>,
) -> Result<Option<CellGradient>> {
    let length_epsilon = 1.0e-3;
    let angle_epsilon = 1.0e-3;
    let original_state = OptimizationState::from_structure(structure);
    let cell = original_state.cell.as_ref().expect("cell");
    let params = CellParameters::from_cell(cell);
    let fractional = fractional_positions(structure, cell);
    let mut gradient = CellGradient::default();

    if cell_options.a {
        if let Some(control) = control
            && control.should_stop()
        {
            return Ok(None);
        }
        gradient.a = cell_parameter_gradient(structure, &params, &fractional, 0, length_epsilon)?;
    }
    if cell_options.b {
        if let Some(control) = control
            && control.should_stop()
        {
            return Ok(None);
        }
        gradient.b = cell_parameter_gradient(structure, &params, &fractional, 1, length_epsilon)?;
    }
    if cell_options.c {
        if let Some(control) = control
            && control.should_stop()
        {
            return Ok(None);
        }
        gradient.c = cell_parameter_gradient(structure, &params, &fractional, 2, length_epsilon)?;
    }
    if cell_options.alpha {
        if let Some(control) = control
            && control.should_stop()
        {
            return Ok(None);
        }
        gradient.alpha =
            cell_parameter_gradient(structure, &params, &fractional, 3, angle_epsilon)?;
    }
    if cell_options.beta {
        if let Some(control) = control
            && control.should_stop()
        {
            return Ok(None);
        }
        gradient.beta = cell_parameter_gradient(structure, &params, &fractional, 4, angle_epsilon)?;
    }
    if cell_options.gamma {
        if let Some(control) = control
            && control.should_stop()
        {
            return Ok(None);
        }
        gradient.gamma =
            cell_parameter_gradient(structure, &params, &fractional, 5, angle_epsilon)?;
    }

    original_state.restore(structure);

    Ok(Some(gradient))
}

fn cell_parameter_gradient(
    structure: &mut Structure,
    params: &CellParameters,
    fractional: &[Vector3<f32>],
    parameter_index: usize,
    epsilon: f32,
) -> Result<f32> {
    let Some(plus_params) = params.with_delta(parameter_index, epsilon) else {
        return Ok(0.0);
    };
    apply_cell_parameters(structure, plus_params, fractional);
    let plus = energy(structure)?;

    let Some(minus_params) = params.with_delta(parameter_index, -epsilon) else {
        apply_cell_parameters(structure, *params, fractional);
        return Ok(0.0);
    };
    apply_cell_parameters(structure, minus_params, fractional);
    let minus = energy(structure)?;
    apply_cell_parameters(structure, *params, fractional);

    Ok((plus - minus) / (2.0 * epsilon))
}

fn apply_gradient_step(
    structure: &mut Structure,
    gradient: &OptimizationGradient,
    step_size: f32,
    max_atom_step: f32,
    movable_atoms: &[bool],
    max_cell_length_step: f32,
    max_cell_angle_step: f32,
) {
    for (index, (atom, grad)) in structure.atoms.iter_mut().zip(&gradient.atoms).enumerate() {
        if !movable_atoms.get(index).copied().unwrap_or(false) {
            continue;
        }
        let mut displacement = -grad * step_size;
        let norm = displacement.norm();
        if norm > max_atom_step {
            displacement *= max_atom_step / norm;
        }

        atom.position += displacement;
    }

    if let (Some(cell_gradient), Some(cell)) = (gradient.cell, structure.cell.clone()) {
        let fractional = fractional_positions(structure, &cell);
        let params = CellParameters::from_cell(&cell);
        let next = params.stepped(
            cell_gradient,
            step_size,
            max_cell_length_step,
            max_cell_angle_step,
        );
        apply_cell_parameters(structure, next, &fractional);
    }
}

fn fractional_positions(structure: &Structure, cell: &UnitCell) -> Vec<Vector3<f32>> {
    structure
        .atoms
        .iter()
        .map(|atom| cell.cartesian_to_fractional(atom.position))
        .collect()
}

fn apply_cell_parameters(
    structure: &mut Structure,
    params: CellParameters,
    fractional: &[Vector3<f32>],
) {
    let cell = UnitCell::from_parameters(
        params.a,
        params.b,
        params.c,
        params.alpha,
        params.beta,
        params.gamma,
    );

    for (atom, frac) in structure.atoms.iter_mut().zip(fractional) {
        atom.position = cell.fractional_to_cartesian(frac.x, frac.y, frac.z);
    }
    structure.cell = Some(cell);
}

impl CellGradient {
    fn squared_norm(self) -> f32 {
        self.a * self.a
            + self.b * self.b
            + self.c * self.c
            + self.alpha * self.alpha
            + self.beta * self.beta
            + self.gamma * self.gamma
    }
}

impl OptimizationGradient {
    fn rms(&self, movable_atom_count: usize) -> f32 {
        let mut squared_sum = self.atoms.iter().map(Vector3::norm_squared).sum::<f32>();
        let mut count = movable_atom_count.max(1) as f32;

        if let Some(cell) = self.cell {
            squared_sum += cell.squared_norm();
            count += self.cell_parameter_count as f32;
        }

        (squared_sum / count).sqrt()
    }

    fn max_atom_norm(&self) -> f32 {
        self.atoms.iter().map(Vector3::norm).fold(0.0_f32, f32::max)
    }
}

fn suggested_step_size(
    gradient: &OptimizationGradient,
    current_step_size: f32,
    max_atom_step: f32,
) -> f32 {
    let max_gradient = gradient.max_atom_norm();
    if max_gradient <= 1.0e-6 || max_atom_step <= 0.0 {
        return current_step_size.clamp(1.0e-6, 0.2);
    }

    let gradient_limited = 0.8 * max_atom_step / max_gradient;
    current_step_size.max(gradient_limited).clamp(1.0e-6, 0.2)
}

impl CellParameters {
    fn from_cell(cell: &UnitCell) -> Self {
        Self {
            a: cell.a,
            b: cell.b,
            c: cell.c,
            alpha: cell.alpha,
            beta: cell.beta,
            gamma: cell.gamma,
        }
    }

    fn with_delta(self, parameter_index: usize, delta: f32) -> Option<Self> {
        let mut next = self;
        match parameter_index {
            0 => next.a += delta,
            1 => next.b += delta,
            2 => next.c += delta,
            3 => next.alpha += delta,
            4 => next.beta += delta,
            5 => next.gamma += delta,
            _ => return None,
        }

        next.is_valid().then_some(next)
    }

    fn stepped(
        self,
        gradient: CellGradient,
        step_size: f32,
        max_length_step: f32,
        max_angle_step: f32,
    ) -> Self {
        let mut next = self;
        next.a += limited_step(-gradient.a * step_size, max_length_step);
        next.b += limited_step(-gradient.b * step_size, max_length_step);
        next.c += limited_step(-gradient.c * step_size, max_length_step);
        next.alpha += limited_step(-gradient.alpha * step_size, max_angle_step);
        next.beta += limited_step(-gradient.beta * step_size, max_angle_step);
        next.gamma += limited_step(-gradient.gamma * step_size, max_angle_step);

        next.a = next.a.max(0.5);
        next.b = next.b.max(0.5);
        next.c = next.c.max(0.5);
        next.alpha = next.alpha.clamp(20.0, 160.0);
        next.beta = next.beta.clamp(20.0, 160.0);
        next.gamma = next.gamma.clamp(20.0, 160.0);
        next
    }

    fn is_valid(self) -> bool {
        self.a > 0.5
            && self.b > 0.5
            && self.c > 0.5
            && (20.0..160.0).contains(&self.alpha)
            && (20.0..160.0).contains(&self.beta)
            && (20.0..160.0).contains(&self.gamma)
    }
}

impl OptimizationState {
    fn from_structure(structure: &Structure) -> Self {
        Self {
            positions: structure.atoms.iter().map(|atom| atom.position).collect(),
            cell: structure.cell.clone(),
        }
    }

    fn restore(&self, structure: &mut Structure) {
        for (atom, position) in structure.atoms.iter_mut().zip(&self.positions) {
            atom.position = *position;
        }
        structure.cell = self.cell.clone();
    }
}

fn limited_step(value: f32, limit: f32) -> f32 {
    value.clamp(-limit, limit)
}
