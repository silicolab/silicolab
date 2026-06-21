use super::*;

use std::collections::HashMap;

use nalgebra::{Point3, Vector3};

use crate::{
    domain::{Atom, Bond, BondType, UnitCell},
    engines::forcefield,
};

pub fn infer_bonds_with_cell(atoms: &[Atom], cell: Option<&UnitCell>) -> Vec<Bond> {
    // A placeholder `1×1×1` cell is not a real lattice; treating it as periodic
    // would bond every atom to a neighbor's image. Ignore it here so no caller
    // can accidentally over-bond, regardless of which loader produced it.
    let cell = cell.filter(|cell| !cell.is_placeholder());
    if cell.is_none() {
        return infer_nonperiodic_bonds(atoms);
    }

    let mut candidates = Vec::new();

    for i in 0..atoms.len() {
        // Monatomic ions (Na+, Cl-, K+, …) are not covalently bonded to anything;
        // their large covalent radii would otherwise spuriously bond them to
        // nearby solvent/solute atoms in a solvated system.
        if is_monatomic_ion_element(&atoms[i].element) {
            continue;
        }
        for j in (i + 1)..atoms.len() {
            if is_monatomic_ion_element(&atoms[j].element) {
                continue;
            }
            let first = element_style(&atoms[i].element);
            let second = element_style(&atoms[j].element);
            let max_distance = (first.covalent_radius + second.covalent_radius) * 1.25;
            let distance = match cell {
                Some(cell) => {
                    nearest_periodic_delta(cell, atoms[i].position, atoms[j].position).norm()
                }
                None => nalgebra::distance(&atoms[i].position, &atoms[j].position),
            };

            if distance <= max_distance {
                candidates.push((i, j, distance));
            }
        }
    }

    assign_bond_orders(atoms, &candidates)
}

fn infer_nonperiodic_bonds(atoms: &[Atom]) -> Vec<Bond> {
    let Some(bucket_size) = max_bond_cutoff(atoms) else {
        return Vec::new();
    };
    let mut buckets: HashMap<(i32, i32, i32), Vec<usize>> = HashMap::new();

    for (index, atom) in atoms.iter().enumerate() {
        buckets
            .entry(spatial_bucket(atom.position, bucket_size))
            .or_default()
            .push(index);
    }

    let mut candidates = Vec::new();
    for (i, atom) in atoms.iter().enumerate() {
        // See `infer_bonds_with_cell`: never bond monatomic ions.
        if is_monatomic_ion_element(&atom.element) {
            continue;
        }
        let bucket = spatial_bucket(atom.position, bucket_size);
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    let neighbor_bucket = (bucket.0 + dx, bucket.1 + dy, bucket.2 + dz);
                    let Some(indices) = buckets.get(&neighbor_bucket) else {
                        continue;
                    };

                    for &j in indices {
                        if j <= i {
                            continue;
                        }
                        if is_monatomic_ion_element(&atoms[j].element) {
                            continue;
                        }

                        let first = element_style(&atoms[i].element);
                        let second = element_style(&atoms[j].element);
                        let max_distance = (first.covalent_radius + second.covalent_radius) * 1.25;
                        let distance = nalgebra::distance(&atoms[i].position, &atoms[j].position);
                        if distance <= max_distance {
                            candidates.push((i, j, distance));
                        }
                    }
                }
            }
        }
    }

    assign_bond_orders(atoms, &candidates)
}

fn max_bond_cutoff(atoms: &[Atom]) -> Option<f32> {
    atoms
        .iter()
        .map(|atom| element_style(&atom.element).covalent_radius)
        .reduce(f32::max)
        .map(|radius| radius * 2.0 * 1.25)
}

fn spatial_bucket(position: Point3<f32>, bucket_size: f32) -> (i32, i32, i32) {
    (
        (position.x / bucket_size).floor() as i32,
        (position.y / bucket_size).floor() as i32,
        (position.z / bucket_size).floor() as i32,
    )
}

fn assign_bond_orders(atoms: &[Atom], candidates: &[(usize, usize, f32)]) -> Vec<Bond> {
    let aromatic_atoms = aromatic_candidate_atoms(atoms, candidates);
    let mut remaining_valence = atoms
        .iter()
        .map(|atom| typical_valence(&atom.element).unwrap_or(1) as f32)
        .collect::<Vec<_>>();
    let mut bonds = Vec::new();

    for &(a, b, _) in candidates {
        if aromatic_atoms[a] && aromatic_atoms[b] {
            bonds.push(Bond::with_type(a, b, BondType::Aromatic));
            remaining_valence[a] -= 1.5;
            remaining_valence[b] -= 1.5;
        }
    }

    let mut non_aromatic = candidates
        .iter()
        .copied()
        .filter(|(a, b, _)| !(aromatic_atoms[*a] && aromatic_atoms[*b]))
        .collect::<Vec<_>>();
    non_aromatic.sort_by(|a, b| a.2.total_cmp(&b.2));

    for (a, b, distance) in non_aromatic {
        let bond_type = best_bond_type_for_distance(
            &atoms[a].element,
            &atoms[b].element,
            distance,
            remaining_valence[a],
            remaining_valence[b],
        );
        let order = bond_order_value(bond_type);

        bonds.push(Bond::with_type(a, b, bond_type));
        remaining_valence[a] -= order;
        remaining_valence[b] -= order;
    }

    bonds
}

fn aromatic_candidate_atoms(atoms: &[Atom], candidates: &[(usize, usize, f32)]) -> Vec<bool> {
    let mut degree = vec![0_usize; atoms.len()];
    let mut has_aromatic_length_neighbor = vec![false; atoms.len()];

    for &(a, b, distance) in candidates {
        degree[a] += 1;
        degree[b] += 1;

        if is_aromatic_element(&atoms[a].element)
            && is_aromatic_element(&atoms[b].element)
            && (1.32..=1.44).contains(&distance)
        {
            has_aromatic_length_neighbor[a] = true;
            has_aromatic_length_neighbor[b] = true;
        }
    }

    atoms
        .iter()
        .enumerate()
        .map(|(index, atom)| {
            is_aromatic_element(&atom.element)
                && degree[index] >= 2
                && has_aromatic_length_neighbor[index]
        })
        .collect()
}

fn best_bond_type_for_distance(
    first: &str,
    second: &str,
    distance: f32,
    first_remaining_valence: f32,
    second_remaining_valence: f32,
) -> BondType {
    [BondType::Triple, BondType::Double, BondType::Single]
        .into_iter()
        .filter(|bond_type| {
            let order = bond_order_value(*bond_type);
            first_remaining_valence + 0.01 >= order && second_remaining_valence + 0.01 >= order
        })
        .filter_map(|bond_type| {
            forcefield::equilibrium_bond_length(first, second, bond_type)
                .ok()
                .map(|ideal| (bond_type, (distance - ideal).abs()))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(bond_type, _)| bond_type)
        .unwrap_or(BondType::Single)
}

pub(crate) fn bonded_neighbors(atom_count: usize, bonds: &[Bond]) -> Vec<Vec<(usize, BondType)>> {
    let mut neighbors = vec![Vec::new(); atom_count];
    for bond in bonds {
        neighbors[bond.a].push((bond.b, bond.bond_type));
        neighbors[bond.b].push((bond.a, bond.bond_type));
    }
    neighbors
}

/// The minimum-image displacement from `first` to `second` under the cell's
/// periodicity: the fractional separation is wrapped into `[-0.5, 0.5]` on each
/// axis before converting back to Cartesian. Used for periodic distance tests
/// (bond inference, and the disordered-system packer's overlap term).
pub(crate) fn nearest_periodic_delta(
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
