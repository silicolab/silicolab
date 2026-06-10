use std::collections::HashSet;

use anyhow::{Result, bail};
use nalgebra::{Point3, Vector3};

use crate::{
    domain::{BondType, Structure, UnitCell},
    engines::forcefield::{CellOptimizationOptions, OptimizationControl},
};

/// UFF atomic parameters.
///
/// Values follow the published Universal Force Field parameters of Rappe et al.,
/// J. Am. Chem. Soc. 1992, 114, 10024-10035.
#[derive(Debug, Clone, Copy)]
struct UffAtomParameters {
    key: &'static str,
    r1: f32,
    theta0_degrees: f32,
    x1: f32,
    d1: f32,
    z1: f32,
    /// sp3 torsional barrier (Vi in UFF paper)
    v_sp3: f32,
    /// sp2 torsional barrier (Uj in UFF paper)
    v_sp2: f32,
}

#[derive(Debug, Clone, Copy)]
struct TypedAtom {
    params: UffAtomParameters,
}

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

const UFF_PARAMETERS: &[UffAtomParameters] = &[
    UffAtomParameters {
        key: "H_",
        r1: 0.354,
        theta0_degrees: 180.0,
        x1: 2.886,
        d1: 0.044,
        z1: 0.712,
        v_sp3: 0.0,
        v_sp2: 0.0,
    },
    UffAtomParameters {
        key: "C_3",
        r1: 0.757,
        theta0_degrees: 109.47,
        x1: 3.851,
        d1: 0.105,
        z1: 1.912,
        v_sp3: 2.119,
        v_sp2: 2.0,
    },
    UffAtomParameters {
        key: "C_R",
        r1: 0.729,
        theta0_degrees: 120.0,
        x1: 3.851,
        d1: 0.105,
        z1: 1.912,
        v_sp3: 0.0,
        v_sp2: 2.0,
    },
    UffAtomParameters {
        key: "C_2",
        r1: 0.732,
        theta0_degrees: 120.0,
        x1: 3.851,
        d1: 0.105,
        z1: 1.912,
        v_sp3: 0.0,
        v_sp2: 2.0,
    },
    UffAtomParameters {
        key: "C_1",
        r1: 0.706,
        theta0_degrees: 180.0,
        x1: 3.851,
        d1: 0.105,
        z1: 1.912,
        v_sp3: 0.0,
        v_sp2: 2.0,
    },
    UffAtomParameters {
        key: "N_3",
        r1: 0.700,
        theta0_degrees: 106.7,
        x1: 3.660,
        d1: 0.069,
        z1: 2.544,
        v_sp3: 0.45,
        v_sp2: 2.0,
    },
    UffAtomParameters {
        key: "N_R",
        r1: 0.699,
        theta0_degrees: 120.0,
        x1: 3.660,
        d1: 0.069,
        z1: 2.544,
        v_sp3: 0.0,
        v_sp2: 2.0,
    },
    UffAtomParameters {
        key: "N_2",
        r1: 0.685,
        theta0_degrees: 111.2,
        x1: 3.660,
        d1: 0.069,
        z1: 2.544,
        v_sp3: 0.0,
        v_sp2: 2.0,
    },
    UffAtomParameters {
        key: "N_1",
        r1: 0.656,
        theta0_degrees: 180.0,
        x1: 3.660,
        d1: 0.069,
        z1: 2.544,
        v_sp3: 0.0,
        v_sp2: 2.0,
    },
    UffAtomParameters {
        key: "O_3",
        r1: 0.658,
        theta0_degrees: 104.51,
        x1: 3.500,
        d1: 0.060,
        z1: 2.300,
        v_sp3: 0.018,
        v_sp2: 2.0,
    },
    UffAtomParameters {
        key: "O_2",
        r1: 0.634,
        theta0_degrees: 120.0,
        x1: 3.500,
        d1: 0.060,
        z1: 2.300,
        v_sp3: 0.0,
        v_sp2: 2.0,
    },
    UffAtomParameters {
        key: "O_R",
        r1: 0.680,
        theta0_degrees: 110.0,
        x1: 3.500,
        d1: 0.060,
        z1: 2.300,
        v_sp3: 0.0,
        v_sp2: 2.0,
    },
    UffAtomParameters {
        key: "F_",
        r1: 0.668,
        theta0_degrees: 180.0,
        x1: 3.364,
        d1: 0.050,
        z1: 1.735,
        v_sp3: 0.0,
        v_sp2: 2.0,
    },
    UffAtomParameters {
        key: "P_3+5",
        r1: 1.056,
        theta0_degrees: 109.47,
        x1: 4.147,
        d1: 0.305,
        z1: 2.863,
        v_sp3: 0.0,
        v_sp2: 2.0,
    },
    UffAtomParameters {
        key: "S_3+2",
        r1: 1.064,
        theta0_degrees: 92.1,
        x1: 4.035,
        d1: 0.274,
        z1: 2.703,
        v_sp3: 0.0,
        v_sp2: 2.0,
    },
    UffAtomParameters {
        key: "S_3+4",
        r1: 1.049,
        theta0_degrees: 103.2,
        x1: 4.035,
        d1: 0.274,
        z1: 2.703,
        v_sp3: 0.0,
        v_sp2: 2.0,
    },
    UffAtomParameters {
        key: "S_3+6",
        r1: 1.027,
        theta0_degrees: 109.47,
        x1: 4.035,
        d1: 0.274,
        z1: 2.703,
        v_sp3: 0.0,
        v_sp2: 2.0,
    },
    UffAtomParameters {
        key: "S_R",
        r1: 1.077,
        theta0_degrees: 92.2,
        x1: 4.035,
        d1: 0.274,
        z1: 2.703,
        v_sp3: 0.0,
        v_sp2: 2.0,
    },
    UffAtomParameters {
        key: "S_2",
        r1: 0.854,
        theta0_degrees: 120.0,
        x1: 4.035,
        d1: 0.274,
        z1: 2.703,
        v_sp3: 0.0,
        v_sp2: 2.0,
    },
    UffAtomParameters {
        key: "Cl",
        r1: 1.044,
        theta0_degrees: 180.0,
        x1: 3.947,
        d1: 0.227,
        z1: 2.348,
        v_sp3: 0.0,
        v_sp2: 2.0,
    },
    UffAtomParameters {
        key: "Br",
        r1: 1.192,
        theta0_degrees: 180.0,
        x1: 4.189,
        d1: 0.251,
        z1: 2.519,
        v_sp3: 0.0,
        v_sp2: 2.0,
    },
    UffAtomParameters {
        key: "I_",
        r1: 1.382,
        theta0_degrees: 180.0,
        x1: 4.500,
        d1: 0.339,
        z1: 2.650,
        v_sp3: 0.0,
        v_sp2: 2.0,
    },
];

pub fn energy(structure: &Structure) -> Result<f32> {
    let typed = typed_atoms(structure)?;
    let neighbors = bonded_neighbors(structure);
    let exclusions = nonbonded_exclusions(&neighbors);

    let mut total = 0.0;
    total += bond_energy(structure, &typed);
    total += angle_energy(structure, &typed, &neighbors);
    total += torsion_energy(structure, &typed, &neighbors);
    total += vdw_energy(structure, &typed, &exclusions);

    Ok(total)
}

pub fn bond_length(first: &str, second: &str, bond_type: BondType) -> Option<f32> {
    let first = default_parameters_for_element(first)?;
    let second = default_parameters_for_element(second)?;

    Some(bond_equilibrium_distance(first, second, bond_type))
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

fn bond_energy(structure: &Structure, typed: &[TypedAtom]) -> f32 {
    structure
        .bonds
        .iter()
        .map(|bond| {
            let distance = atom_delta(structure, bond.a, bond.b).norm().max(0.0001);
            let params_a = typed[bond.a].params;
            let params_b = typed[bond.b].params;
            let equilibrium = bond_equilibrium_distance(params_a, params_b, bond.bond_type);
            let force_constant = 664.12 * params_a.z1 * params_b.z1 / equilibrium.powi(3);
            let delta = distance - equilibrium;

            0.5 * force_constant * delta * delta
        })
        .sum()
}

fn angle_energy(
    structure: &Structure,
    typed: &[TypedAtom],
    neighbors: &[Vec<(usize, BondType)>],
) -> f32 {
    let mut total = 0.0;

    for (center, bonded) in neighbors.iter().enumerate() {
        for i in 0..bonded.len() {
            for j in (i + 1)..bonded.len() {
                let first = bonded[i].0;
                let second = bonded[j].0;
                let first_vec = atom_delta(structure, center, first);
                let second_vec = atom_delta(structure, center, second);
                let denominator = first_vec.norm() * second_vec.norm();
                if denominator <= 0.0001 {
                    continue;
                }

                let theta = (first_vec.dot(&second_vec) / denominator)
                    .clamp(-1.0, 1.0)
                    .acos();
                let theta0 = typed[center].params.theta0_degrees.to_radians();
                let delta = theta - theta0;
                let force_constant = 80.0;

                total += 0.5 * force_constant * delta * delta;
            }
        }
    }

    total
}

fn vdw_energy(
    structure: &Structure,
    typed: &[TypedAtom],
    exclusions: &HashSet<(usize, usize)>,
) -> f32 {
    let mut total = 0.0;

    for i in 0..structure.atoms.len() {
        for j in (i + 1)..structure.atoms.len() {
            if exclusions.contains(&(i, j)) {
                continue;
            }

            let distance = atom_delta(structure, i, j).norm().max(0.2);
            if distance > 10.0 {
                continue;
            }

            let xij = (typed[i].params.x1 * typed[j].params.x1).sqrt();
            let dij = (typed[i].params.d1 * typed[j].params.d1).sqrt();
            let ratio = xij / distance;
            let ratio6 = ratio.powi(6);

            total += dij * (ratio6 * ratio6 - 2.0 * ratio6);
        }
    }

    total
}

fn torsion_energy(
    structure: &Structure,
    typed: &[TypedAtom],
    neighbors: &[Vec<(usize, BondType)>],
) -> f32 {
    let mut total = 0.0;

    // Iterate over all central bonds (j-k)
    for j in 0..structure.atoms.len() {
        for (k, _) in &neighbors[j] {
            let k = *k;
            if j >= k {
                continue; // Avoid double counting
            }

            // Skip if either atom is sp hybridized (linear)
            let j_type = typed[j].params.key;
            let k_type = typed[k].params.key;
            if j_type.ends_with("_1") || k_type.ends_with("_1") {
                continue;
            }

            // Get neighbors of j (excluding k) and k (excluding j)
            let j_neighbors: Vec<usize> = neighbors[j]
                .iter()
                .filter(|(n, _)| *n != k)
                .map(|(n, _)| *n)
                .collect();
            let k_neighbors: Vec<usize> = neighbors[k]
                .iter()
                .filter(|(n, _)| *n != j)
                .map(|(n, _)| *n)
                .collect();

            if j_neighbors.is_empty() || k_neighbors.is_empty() {
                continue;
            }

            // Determine hybridization of j and k
            let j_is_sp2 = j_type.ends_with("_R") || j_type.ends_with("_2");
            let k_is_sp2 = k_type.ends_with("_R") || k_type.ends_with("_2");

            // UFF torsion parameters based on hybridization
            let (n, phi0_degrees, v_phi) = if j_is_sp2 && k_is_sp2 {
                // sp2-sp2: n=2, phi0=180, high barrier
                let v = 5.0
                    * (typed[j].params.v_sp2 * typed[k].params.v_sp2).sqrt()
                    * (1.0 + 4.18 * (1.5_f32).ln());
                (2.0_f32, 180.0_f32, v)
            } else if j_is_sp2 || k_is_sp2 {
                // sp2-sp3: n=6, phi0=0
                let v = if j_is_sp2 {
                    (typed[j].params.v_sp2 * typed[k].params.v_sp3).sqrt()
                } else {
                    (typed[j].params.v_sp3 * typed[k].params.v_sp2).sqrt()
                };
                (6.0_f32, 0.0_f32, v)
            } else {
                // sp3-sp3: n=3, phi0=180 (or 90 for group 16)
                let v = (typed[j].params.v_sp3 * typed[k].params.v_sp3).sqrt();
                let j_is_group16 =
                    matches!(structure.atoms[j].element.as_str(), "O" | "S" | "Se" | "Te");
                let k_is_group16 =
                    matches!(structure.atoms[k].element.as_str(), "O" | "S" | "Se" | "Te");
                if j_is_group16 && k_is_group16 {
                    (2.0_f32, 90.0_f32, v)
                } else {
                    (3.0_f32, 180.0_f32, v)
                }
            };

            if v_phi.abs() < 1.0e-6 {
                continue;
            }

            let phi0 = phi0_degrees.to_radians();
            let cos_n_phi0 = (n * phi0).cos();

            // Sum over all combinations of i and l
            for &i in &j_neighbors {
                for &l in &k_neighbors {
                    let phi = dihedral_angle(structure, i, j, k, l);
                    if phi.is_nan() {
                        continue;
                    }

                    total += 0.5 * v_phi * (1.0 - cos_n_phi0 * (n * phi).cos());
                }
            }
        }
    }

    total
}

fn dihedral_angle(structure: &Structure, i: usize, j: usize, k: usize, l: usize) -> f32 {
    let r_ij = atom_delta(structure, j, i);
    let r_jk = atom_delta(structure, k, j);
    let r_kl = atom_delta(structure, l, k);

    let n1 = r_ij.cross(&r_jk);
    let n2 = r_jk.cross(&r_kl);

    let n1_norm = n1.norm();
    let n2_norm = n2.norm();
    let r_jk_norm = r_jk.norm();

    if n1_norm < 1.0e-6 || n2_norm < 1.0e-6 || r_jk_norm < 1.0e-6 {
        return f32::NAN;
    }

    let n1_unit = n1 / n1_norm;
    let n2_unit = n2 / n2_norm;
    let r_jk_unit = r_jk / r_jk_norm;

    let cos_phi = n1_unit.dot(&n2_unit).clamp(-1.0, 1.0);
    let sin_phi = r_jk_unit.dot(&n1_unit.cross(&n2_unit));

    sin_phi.atan2(cos_phi)
}

fn bond_equilibrium_distance(
    first: UffAtomParameters,
    second: UffAtomParameters,
    bond_type: BondType,
) -> f32 {
    let bond_order = match bond_type {
        BondType::Single => 1.0_f32,
        BondType::Double => 2.0,
        BondType::Triple => 3.0,
        BondType::Aromatic => 1.5,
    };
    let bond_order_correction = -0.1332 * (first.r1 + second.r1) * bond_order.ln();

    first.r1 + second.r1 + bond_order_correction
}

fn nonbonded_exclusions(neighbors: &[Vec<(usize, BondType)>]) -> HashSet<(usize, usize)> {
    let mut exclusions = HashSet::new();

    for (atom, bonded) in neighbors.iter().enumerate() {
        for (neighbor, _) in bonded {
            exclusions.insert(ordered_pair(atom, *neighbor));

            for (second_neighbor, _) in &neighbors[*neighbor] {
                if *second_neighbor != atom {
                    exclusions.insert(ordered_pair(atom, *second_neighbor));
                }
            }
        }
    }

    exclusions
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

fn typed_atoms(structure: &Structure) -> Result<Vec<TypedAtom>> {
    let neighbors = bonded_neighbors(structure);

    structure
        .atoms
        .iter()
        .enumerate()
        .map(|(index, _)| {
            let key = uff_type_for_atom(structure, &neighbors, index)?;
            let params = parameter_by_key(key)
                .ok_or_else(|| anyhow::anyhow!("missing UFF parameters for atom type {key}"))?;

            Ok(TypedAtom { params })
        })
        .collect()
}

fn bonded_neighbors(structure: &Structure) -> Vec<Vec<(usize, BondType)>> {
    let mut neighbors = vec![Vec::new(); structure.atoms.len()];

    for bond in &structure.bonds {
        neighbors[bond.a].push((bond.b, bond.bond_type));
        neighbors[bond.b].push((bond.a, bond.bond_type));
    }

    neighbors
}

fn uff_type_for_atom(
    structure: &Structure,
    neighbors: &[Vec<(usize, BondType)>],
    atom_index: usize,
) -> Result<&'static str> {
    let element = structure.atoms[atom_index].element.as_str();
    let atom_neighbors = &neighbors[atom_index];

    match element {
        "H" => Ok("H_"),
        "C" if has_bond_type(atom_neighbors, BondType::Aromatic) => Ok("C_R"),
        "C" if has_bond_type(atom_neighbors, BondType::Triple) => Ok("C_1"),
        "C" if has_bond_type(atom_neighbors, BondType::Double) => Ok("C_2"),
        "C" => Ok("C_3"),
        "N" if has_bond_type(atom_neighbors, BondType::Aromatic) => Ok("N_R"),
        "N" if has_bond_type(atom_neighbors, BondType::Triple) => Ok("N_1"),
        "N" if has_bond_type(atom_neighbors, BondType::Double) => Ok("N_2"),
        "N" => Ok("N_3"),
        "O" if has_bond_type(atom_neighbors, BondType::Aromatic) => Ok("O_R"),
        "O" if has_bond_type(atom_neighbors, BondType::Double) => Ok("O_2"),
        "O" => Ok("O_3"),
        "F" => Ok("F_"),
        "P" => Ok("P_3+5"),
        "S" if has_bond_type(atom_neighbors, BondType::Aromatic) => Ok("S_R"),
        "S" if has_bond_type(atom_neighbors, BondType::Double) => Ok("S_2"),
        "S" if atom_neighbors.len() >= 4 => Ok("S_3+6"),
        "S" if atom_neighbors.len() == 3 => Ok("S_3+4"),
        "S" => Ok("S_3+2"),
        "Cl" => Ok("Cl"),
        "Br" => Ok("Br"),
        "I" => Ok("I_"),
        _ => bail!("unsupported element for UFF atom typing: {element}"),
    }
}

fn has_bond_type(neighbors: &[(usize, BondType)], bond_type: BondType) -> bool {
    neighbors.iter().any(|(_, ty)| *ty == bond_type)
}

fn parameter_by_key(key: &str) -> Option<UffAtomParameters> {
    UFF_PARAMETERS
        .iter()
        .copied()
        .find(|params| params.key == key)
}

fn atom_delta(structure: &Structure, first: usize, second: usize) -> Vector3<f32> {
    match &structure.cell {
        Some(cell) => nearest_periodic_delta(
            cell,
            structure.atoms[first].position,
            structure.atoms[second].position,
        ),
        None => structure.atoms[second].position - structure.atoms[first].position,
    }
}

fn nearest_periodic_delta(
    cell: &UnitCell,
    first: Point3<f32>,
    second: Point3<f32>,
) -> Vector3<f32> {
    let first_frac = cell.cartesian_to_fractional(first);
    let second_frac = cell.cartesian_to_fractional(second);
    let mut delta = second_frac - first_frac;

    delta.x -= delta.x.round();
    delta.y -= delta.y.round();
    delta.z -= delta.z.round();

    cell.vectors[0] * delta.x + cell.vectors[1] * delta.y + cell.vectors[2] * delta.z
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

fn default_parameters_for_element(element: &str) -> Option<UffAtomParameters> {
    let key = match element {
        "H" => "H_",
        "C" => "C_3",
        "N" => "N_3",
        "O" => "O_3",
        "F" => "F_",
        "P" => "P_3+5",
        "S" => "S_3+2",
        "Cl" => "Cl",
        "Br" => "Br",
        "I" => "I_",
        _ => return None,
    };

    parameter_by_key(key)
}

fn ordered_pair(first: usize, second: usize) -> (usize, usize) {
    if first < second {
        (first, second)
    } else {
        (second, first)
    }
}

#[cfg(test)]
mod tests {
    use nalgebra::Point3;

    use crate::{
        domain::{Atom, Bond, BondType, Structure, UnitCell},
        engines::forcefield::{
            AtomOptimizationScope, CellOptimizationOptions, OptimizationOptions, optimize_geometry,
        },
    };

    use super::energy;

    #[test]
    fn uff_optimization_shortens_stretched_hydrogen_bond() {
        let mut structure = Structure::with_bonds(
            "stretched hydrogen",
            vec![
                Atom {
                    element: "H".to_string(),
                    position: Point3::new(0.0, 0.0, 0.0),
                    charge: 0.0,
                },
                Atom {
                    element: "H".to_string(),
                    position: Point3::new(1.5, 0.0, 0.0),
                    charge: 0.0,
                },
            ],
            vec![Bond::with_type(0, 1, BondType::Single)],
        );
        let initial_distance =
            nalgebra::distance(&structure.atoms[0].position, &structure.atoms[1].position);
        let report = optimize_geometry(&mut structure, OptimizationOptions::default()).unwrap();
        let final_distance =
            nalgebra::distance(&structure.atoms[0].position, &structure.atoms[1].position);

        assert!(report.final_energy < report.initial_energy);
        assert!(report.steps > 0);
        assert!(report.converged || report.final_energy < report.initial_energy);
        assert!(final_distance < initial_distance);
        assert!((final_distance - 0.708).abs() < 0.05);
    }

    #[test]
    fn uff_optimization_opens_water_angle_toward_tetrahedral_oxygen() {
        let mut structure = Structure::with_bonds(
            "water",
            vec![
                Atom {
                    element: "O".to_string(),
                    position: Point3::new(0.0, 0.0, 0.0),
                    charge: 0.0,
                },
                Atom {
                    element: "H".to_string(),
                    position: Point3::new(0.96, 0.0, 0.0),
                    charge: 0.0,
                },
                Atom {
                    element: "H".to_string(),
                    position: Point3::new(0.0, 0.96, 0.0),
                    charge: 0.0,
                },
            ],
            vec![
                Bond::with_type(0, 1, BondType::Single),
                Bond::with_type(0, 2, BondType::Single),
            ],
        );
        let initial_error = (h_o_h_angle(&structure) - 104.51).abs();

        optimize_geometry(&mut structure, OptimizationOptions::default()).unwrap();
        let final_error = (h_o_h_angle(&structure) - 104.51).abs();

        assert!(final_error < initial_error);
    }

    #[test]
    fn uff_vdw_repulsion_separates_unbonded_hydrogens() {
        let mut structure = Structure::with_bonds(
            "unbonded hydrogens",
            vec![
                Atom {
                    element: "H".to_string(),
                    position: Point3::new(-0.25, 0.0, 0.0),
                    charge: 0.0,
                },
                Atom {
                    element: "H".to_string(),
                    position: Point3::new(0.25, 0.0, 0.0),
                    charge: 0.0,
                },
            ],
            Vec::new(),
        );
        let initial_energy = energy(&structure).unwrap();
        let initial_distance =
            nalgebra::distance(&structure.atoms[0].position, &structure.atoms[1].position);

        optimize_geometry(&mut structure, OptimizationOptions::default()).unwrap();
        let final_energy = energy(&structure).unwrap();
        let final_distance =
            nalgebra::distance(&structure.atoms[0].position, &structure.atoms[1].position);

        assert!(final_energy < initial_energy);
        assert!(final_distance > initial_distance);
    }

    #[test]
    fn uff_partial_optimization_freezes_unselected_atoms() {
        let mut structure = Structure::with_bonds(
            "partial hydrogen stretch",
            vec![
                Atom {
                    element: "H".to_string(),
                    position: Point3::new(0.0, 0.0, 0.0),
                    charge: 0.0,
                },
                Atom {
                    element: "H".to_string(),
                    position: Point3::new(1.5, 0.0, 0.0),
                    charge: 0.0,
                },
            ],
            vec![Bond::with_type(0, 1, BondType::Single)],
        );
        let fixed_before = structure.atoms[0].position;
        let movable_before = structure.atoms[1].position;

        let report = optimize_geometry(
            &mut structure,
            OptimizationOptions {
                atoms: AtomOptimizationScope::Selected(vec![1]),
                ..OptimizationOptions::default()
            },
        )
        .unwrap();

        assert!(report.final_energy < report.initial_energy);
        assert_eq!(structure.atoms[0].position, fixed_before);
        assert_ne!(structure.atoms[1].position, movable_before);
    }

    #[test]
    fn uff_optimization_uses_periodic_bond_across_cell_boundary() {
        let cell = UnitCell::from_parameters(10.0, 10.0, 10.0, 90.0, 90.0, 90.0);
        let mut structure = Structure::with_cell_and_bonds(
            "periodic hydrogen bond",
            vec![
                Atom {
                    element: "H".to_string(),
                    position: Point3::new(0.1, 0.0, 0.0),
                    charge: 0.0,
                },
                Atom {
                    element: "H".to_string(),
                    position: Point3::new(9.9, 0.0, 0.0),
                    charge: 0.0,
                },
            ],
            vec![Bond::with_type(0, 1, BondType::Single)],
            cell,
        );
        let initial_distance = periodic_distance(&structure, 0, 1);

        let report = optimize_geometry(&mut structure, OptimizationOptions::default()).unwrap();
        let final_distance = periodic_distance(&structure, 0, 1);

        assert!(report.final_energy < report.initial_energy);
        assert!(final_distance > initial_distance);
        assert!((final_distance - 0.708).abs() < 0.05);
    }

    #[test]
    fn uff_partial_periodic_optimization_freezes_unselected_atoms() {
        let cell = UnitCell::from_parameters(10.0, 10.0, 10.0, 90.0, 90.0, 90.0);
        let mut structure = Structure::with_cell_and_bonds(
            "partial periodic hydrogen bond",
            vec![
                Atom {
                    element: "H".to_string(),
                    position: Point3::new(0.1, 0.0, 0.0),
                    charge: 0.0,
                },
                Atom {
                    element: "H".to_string(),
                    position: Point3::new(9.9, 0.0, 0.0),
                    charge: 0.0,
                },
            ],
            vec![Bond::with_type(0, 1, BondType::Single)],
            cell,
        );
        let fixed_before = structure.atoms[0].position;
        let initial_distance = periodic_distance(&structure, 0, 1);

        let report = optimize_geometry(
            &mut structure,
            OptimizationOptions {
                atoms: AtomOptimizationScope::Selected(vec![1]),
                ..OptimizationOptions::default()
            },
        )
        .unwrap();

        assert!(report.final_energy < report.initial_energy);
        assert_eq!(structure.atoms[0].position, fixed_before);
        assert!(periodic_distance(&structure, 0, 1) > initial_distance);
    }

    #[test]
    fn uff_optimization_can_adjust_periodic_cell_length() {
        let cell = UnitCell::from_parameters(1.0, 10.0, 10.0, 90.0, 90.0, 90.0);
        let mut structure = Structure::with_cell_and_bonds(
            "cell length relaxation",
            vec![
                Atom {
                    element: "H".to_string(),
                    position: Point3::new(0.0, 0.0, 0.0),
                    charge: 0.0,
                },
                Atom {
                    element: "H".to_string(),
                    position: Point3::new(0.5, 0.0, 0.0),
                    charge: 0.0,
                },
            ],
            vec![Bond::with_type(0, 1, BondType::Single)],
            cell,
        );
        let initial_cell_a = structure.cell.as_ref().expect("cell").a;
        let initial_distance = periodic_distance(&structure, 0, 1);

        let report = optimize_geometry(
            &mut structure,
            OptimizationOptions {
                cell: CellOptimizationOptions {
                    a: true,
                    ..CellOptimizationOptions::default()
                },
                max_atom_step: 0.0,
                ..OptimizationOptions::default()
            },
        )
        .unwrap();
        let final_cell_a = structure.cell.as_ref().expect("cell").a;
        let final_distance = periodic_distance(&structure, 0, 1);

        assert!(report.final_energy < report.initial_energy);
        assert!(final_cell_a > initial_cell_a);
        assert!(final_distance > initial_distance);
        assert!((final_distance - 0.708).abs() < 0.05);
    }

    fn h_o_h_angle(structure: &Structure) -> f32 {
        let first = structure.atoms[1].position - structure.atoms[0].position;
        let second = structure.atoms[2].position - structure.atoms[0].position;

        (first.dot(&second) / (first.norm() * second.norm()))
            .clamp(-1.0, 1.0)
            .acos()
            .to_degrees()
    }

    fn periodic_distance(structure: &Structure, first: usize, second: usize) -> f32 {
        let cell = structure.cell.as_ref().expect("cell");
        let mut delta = cell.cartesian_to_fractional(structure.atoms[second].position)
            - cell.cartesian_to_fractional(structure.atoms[first].position);
        delta.x -= delta.x.round();
        delta.y -= delta.y.round();
        delta.z -= delta.z.round();

        (cell.vectors[0] * delta.x + cell.vectors[1] * delta.y + cell.vectors[2] * delta.z).norm()
    }
}
