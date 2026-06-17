use super::*;

use std::collections::HashSet;

use anyhow::Result;
use nalgebra::{Point3, Vector3};

use crate::domain::{BondType, Structure, UnitCell};

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

pub(crate) fn atom_delta(structure: &Structure, first: usize, second: usize) -> Vector3<f32> {
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

fn ordered_pair(first: usize, second: usize) -> (usize, usize) {
    if first < second {
        (first, second)
    } else {
        (second, first)
    }
}
