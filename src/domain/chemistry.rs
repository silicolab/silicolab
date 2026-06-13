use std::collections::HashMap;

use nalgebra::{Point3, Vector3};

use crate::{
    domain::{Atom, Bond, BondType, UnitCell},
    engines::forcefield,
};

#[derive(Debug, Clone, Copy)]
pub struct ElementStyle {
    pub color: Point3<f32>,
    pub covalent_radius: f32,
    pub display_radius: f32,
}

/// Whether an element is commonly present as a free monatomic ion in molecular
/// systems (counter-ions, salt baths, structural metals). Used to classify lone
/// atoms that are not part of a polymer or solvent residue.
pub fn is_monatomic_ion_element(symbol: &str) -> bool {
    matches!(
        symbol,
        "Li" | "Na"
            | "K"
            | "Rb"
            | "Cs"
            | "Mg"
            | "Ca"
            | "Sr"
            | "Ba"
            | "Zn"
            | "Fe"
            | "Cu"
            | "Mn"
            | "Cl"
            | "Br"
            | "I"
            | "F"
    )
}

pub fn element_style(symbol: &str) -> ElementStyle {
    match normalized_symbol(symbol).as_str() {
        "H" => ElementStyle {
            color: Point3::new(0.95, 0.95, 0.95),
            covalent_radius: 0.31,
            display_radius: 0.22,
        },
        "He" => ElementStyle {
            color: Point3::new(0.85, 1.00, 1.00),
            covalent_radius: 0.28,
            display_radius: 0.20,
        },
        "Li" => ElementStyle {
            color: Point3::new(0.80, 0.00, 0.00),
            covalent_radius: 1.28,
            display_radius: 0.44,
        },
        "Be" => ElementStyle {
            color: Point3::new(0.00, 0.50, 0.00),
            covalent_radius: 0.96,
            display_radius: 0.38,
        },
        "B" => ElementStyle {
            color: Point3::new(1.00, 0.71, 0.71),
            covalent_radius: 0.84,
            display_radius: 0.36,
        },
        "C" => ElementStyle {
            color: Point3::new(0.30, 0.30, 0.32),
            covalent_radius: 0.76,
            display_radius: 0.34,
        },
        "N" => ElementStyle {
            color: Point3::new(0.10, 0.25, 0.95),
            covalent_radius: 0.71,
            display_radius: 0.33,
        },
        "O" => ElementStyle {
            color: Point3::new(0.90, 0.05, 0.05),
            covalent_radius: 0.66,
            display_radius: 0.34,
        },
        "F" => ElementStyle {
            color: Point3::new(0.45, 0.90, 0.25),
            covalent_radius: 0.57,
            display_radius: 0.32,
        },
        "Ne" => ElementStyle {
            color: Point3::new(0.70, 0.89, 0.96),
            covalent_radius: 0.58,
            display_radius: 0.30,
        },
        "Na" => ElementStyle {
            color: Point3::new(0.45, 0.35, 0.90),
            covalent_radius: 1.66,
            display_radius: 0.48,
        },
        "Mg" => ElementStyle {
            color: Point3::new(0.00, 0.59, 0.00),
            covalent_radius: 1.41,
            display_radius: 0.46,
        },
        "Al" => ElementStyle {
            color: Point3::new(0.75, 0.65, 0.65),
            covalent_radius: 1.21,
            display_radius: 0.44,
        },
        "Si" => ElementStyle {
            color: Point3::new(0.94, 0.78, 0.63),
            covalent_radius: 1.11,
            display_radius: 0.42,
        },
        "P" => ElementStyle {
            color: Point3::new(1.00, 0.55, 0.10),
            covalent_radius: 1.07,
            display_radius: 0.42,
        },
        "S" => ElementStyle {
            color: Point3::new(0.95, 0.82, 0.10),
            covalent_radius: 1.05,
            display_radius: 0.42,
        },
        "Cl" => ElementStyle {
            color: Point3::new(0.10, 0.75, 0.20),
            covalent_radius: 1.02,
            display_radius: 0.42,
        },
        "Ar" => ElementStyle {
            color: Point3::new(0.50, 0.82, 0.89),
            covalent_radius: 1.06,
            display_radius: 0.38,
        },
        "K" => ElementStyle {
            color: Point3::new(0.56, 0.00, 0.56),
            covalent_radius: 2.03,
            display_radius: 0.52,
        },
        "Ca" => ElementStyle {
            color: Point3::new(0.00, 0.59, 0.00),
            covalent_radius: 1.76,
            display_radius: 0.50,
        },
        "Sc" => ElementStyle {
            color: Point3::new(0.90, 0.90, 0.90),
            covalent_radius: 1.70,
            display_radius: 0.46,
        },
        "Ti" => ElementStyle {
            color: Point3::new(0.75, 0.76, 0.78),
            covalent_radius: 1.36,
            display_radius: 0.46,
        },
        "V" => ElementStyle {
            color: Point3::new(0.65, 0.65, 0.67),
            covalent_radius: 1.25,
            display_radius: 0.44,
        },
        "Cr" => ElementStyle {
            color: Point3::new(0.54, 0.60, 0.78),
            covalent_radius: 1.39,
            display_radius: 0.46,
        },
        "Mn" => ElementStyle {
            color: Point3::new(0.61, 0.48, 0.69),
            covalent_radius: 1.39,
            display_radius: 0.46,
        },
        "Fe" => ElementStyle {
            color: Point3::new(0.88, 0.40, 0.20),
            covalent_radius: 1.32,
            display_radius: 0.44,
        },
        "Co" => ElementStyle {
            color: Point3::new(0.94, 0.48, 0.40),
            covalent_radius: 1.26,
            display_radius: 0.44,
        },
        "Ni" => ElementStyle {
            color: Point3::new(0.31, 0.62, 0.31),
            covalent_radius: 1.24,
            display_radius: 0.44,
        },
        "Cu" => ElementStyle {
            color: Point3::new(0.78, 0.50, 0.20),
            covalent_radius: 1.32,
            display_radius: 0.44,
        },
        "Zn" => ElementStyle {
            color: Point3::new(0.49, 0.50, 0.55),
            covalent_radius: 1.22,
            display_radius: 0.44,
        },
        "Ga" => ElementStyle {
            color: Point3::new(0.75, 0.65, 0.65),
            covalent_radius: 1.22,
            display_radius: 0.44,
        },
        "Ge" => ElementStyle {
            color: Point3::new(0.40, 0.56, 0.56),
            covalent_radius: 1.20,
            display_radius: 0.42,
        },
        "As" => ElementStyle {
            color: Point3::new(0.74, 0.50, 0.89),
            covalent_radius: 1.19,
            display_radius: 0.42,
        },
        "Se" => ElementStyle {
            color: Point3::new(0.78, 0.48, 0.00),
            covalent_radius: 1.20,
            display_radius: 0.42,
        },
        "Br" => ElementStyle {
            color: Point3::new(0.65, 0.16, 0.16),
            covalent_radius: 1.20,
            display_radius: 0.42,
        },
        "Kr" => ElementStyle {
            color: Point3::new(0.36, 0.72, 0.82),
            covalent_radius: 1.16,
            display_radius: 0.38,
        },
        "Rb" => ElementStyle {
            color: Point3::new(0.44, 0.18, 0.69),
            covalent_radius: 2.20,
            display_radius: 0.54,
        },
        "Sr" => ElementStyle {
            color: Point3::new(0.00, 0.59, 0.00),
            covalent_radius: 1.95,
            display_radius: 0.52,
        },
        "Y" => ElementStyle {
            color: Point3::new(0.58, 1.00, 1.00),
            covalent_radius: 1.90,
            display_radius: 0.48,
        },
        "Zr" => ElementStyle {
            color: Point3::new(0.59, 0.58, 0.58),
            covalent_radius: 1.75,
            display_radius: 0.48,
        },
        "Nb" => ElementStyle {
            color: Point3::new(0.45, 0.76, 0.79),
            covalent_radius: 1.64,
            display_radius: 0.46,
        },
        "Mo" => ElementStyle {
            color: Point3::new(0.33, 0.71, 0.71),
            covalent_radius: 1.54,
            display_radius: 0.46,
        },
        "Ru" => ElementStyle {
            color: Point3::new(0.14, 0.56, 0.56),
            covalent_radius: 1.46,
            display_radius: 0.44,
        },
        "Rh" => ElementStyle {
            color: Point3::new(0.04, 0.49, 0.55),
            covalent_radius: 1.42,
            display_radius: 0.44,
        },
        "Pd" => ElementStyle {
            color: Point3::new(0.00, 0.41, 0.52),
            covalent_radius: 1.39,
            display_radius: 0.44,
        },
        "Ag" => ElementStyle {
            color: Point3::new(0.75, 0.75, 0.75),
            covalent_radius: 1.45,
            display_radius: 0.44,
        },
        "Cd" => ElementStyle {
            color: Point3::new(1.00, 0.85, 0.56),
            covalent_radius: 1.44,
            display_radius: 0.46,
        },
        "In" => ElementStyle {
            color: Point3::new(0.65, 0.46, 0.45),
            covalent_radius: 1.42,
            display_radius: 0.46,
        },
        "Sn" => ElementStyle {
            color: Point3::new(0.40, 0.50, 0.50),
            covalent_radius: 1.39,
            display_radius: 0.46,
        },
        "Sb" => ElementStyle {
            color: Point3::new(0.62, 0.39, 0.71),
            covalent_radius: 1.39,
            display_radius: 0.46,
        },
        "Te" => ElementStyle {
            color: Point3::new(0.83, 0.48, 0.00),
            covalent_radius: 1.38,
            display_radius: 0.46,
        },
        "I" => ElementStyle {
            color: Point3::new(0.58, 0.00, 0.58),
            covalent_radius: 1.39,
            display_radius: 0.46,
        },
        "Xe" => ElementStyle {
            color: Point3::new(0.26, 0.62, 0.69),
            covalent_radius: 1.40,
            display_radius: 0.40,
        },
        "Cs" => ElementStyle {
            color: Point3::new(0.44, 0.18, 0.69),
            covalent_radius: 2.44,
            display_radius: 0.56,
        },
        "Ba" => ElementStyle {
            color: Point3::new(0.00, 0.59, 0.00),
            covalent_radius: 2.15,
            display_radius: 0.54,
        },
        "La" => ElementStyle {
            color: Point3::new(0.44, 0.83, 1.00),
            covalent_radius: 2.07,
            display_radius: 0.50,
        },
        "Ce" => ElementStyle {
            color: Point3::new(1.00, 1.00, 0.78),
            covalent_radius: 2.04,
            display_radius: 0.50,
        },
        "W" => ElementStyle {
            color: Point3::new(0.13, 0.58, 0.13),
            covalent_radius: 1.62,
            display_radius: 0.46,
        },
        "Os" => ElementStyle {
            color: Point3::new(0.14, 0.42, 0.42),
            covalent_radius: 1.44,
            display_radius: 0.44,
        },
        "Ir" => ElementStyle {
            color: Point3::new(0.09, 0.33, 0.53),
            covalent_radius: 1.41,
            display_radius: 0.44,
        },
        "Pt" => ElementStyle {
            color: Point3::new(0.82, 0.82, 0.88),
            covalent_radius: 1.36,
            display_radius: 0.44,
        },
        "Au" => ElementStyle {
            color: Point3::new(1.00, 0.84, 0.00),
            covalent_radius: 1.36,
            display_radius: 0.44,
        },
        "Hg" => ElementStyle {
            color: Point3::new(0.72, 0.72, 0.82),
            covalent_radius: 1.32,
            display_radius: 0.44,
        },
        "Tl" => ElementStyle {
            color: Point3::new(0.65, 0.33, 0.30),
            covalent_radius: 1.45,
            display_radius: 0.46,
        },
        "Pb" => ElementStyle {
            color: Point3::new(0.34, 0.35, 0.38),
            covalent_radius: 1.46,
            display_radius: 0.46,
        },
        "Bi" => ElementStyle {
            color: Point3::new(0.62, 0.31, 0.71),
            covalent_radius: 1.48,
            display_radius: 0.46,
        },
        "U" => ElementStyle {
            color: Point3::new(0.00, 0.56, 0.00),
            covalent_radius: 1.96,
            display_radius: 0.50,
        },
        _ => ElementStyle {
            color: Point3::new(0.55, 0.55, 0.60),
            covalent_radius: 0.80,
            display_radius: 0.35,
        },
    }
}

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

pub fn add_missing_hydrogens(atoms: &mut Vec<Atom>, bonds: &mut Vec<Bond>) -> usize {
    let mut added = 0;
    let original_atom_count = atoms.len();
    let neighbors = bonded_neighbors(original_atom_count, bonds);

    for atom_index in 0..original_atom_count {
        let Some(target_valence) = typical_valence(&atoms[atom_index].element) else {
            continue;
        };
        let current_valence = neighbors[atom_index]
            .iter()
            .map(|(_, bond_type)| bond_order_value(*bond_type))
            .sum::<f32>();
        if current_valence + 0.1 >= target_valence as f32 {
            continue;
        }

        let missing = (target_valence as f32 - current_valence).round().max(0.0) as usize;
        let directions = hydrogen_directions(atoms, &neighbors, atom_index, missing);
        for direction in directions {
            let h_index = atoms.len();
            atoms.push(Atom {
                element: "H".to_string(),
                position: atoms[atom_index].position
                    + direction * hydrogen_bond_length(&atoms[atom_index].element),
                charge: 0.0,
            });
            bonds.push(Bond::with_type(atom_index, h_index, BondType::Single));
            added += 1;
        }
    }

    added
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

fn bonded_neighbors(atom_count: usize, bonds: &[Bond]) -> Vec<Vec<(usize, BondType)>> {
    let mut neighbors = vec![Vec::new(); atom_count];
    for bond in bonds {
        neighbors[bond.a].push((bond.b, bond.bond_type));
        neighbors[bond.b].push((bond.a, bond.bond_type));
    }
    neighbors
}

fn hydrogen_directions(
    atoms: &[Atom],
    neighbors: &[Vec<(usize, BondType)>],
    atom_index: usize,
    count: usize,
) -> Vec<Vector3<f32>> {
    if count == 0 {
        return Vec::new();
    }

    let center = atoms[atom_index].position;
    let existing = neighbors[atom_index]
        .iter()
        .filter_map(|(neighbor, _)| (atoms[*neighbor].position - center).try_normalize(0.0001))
        .collect::<Vec<_>>();

    if existing.is_empty() {
        return default_hydrogen_directions(count);
    }

    if is_sp2_like_center(&neighbors[atom_index]) {
        return sp2_hydrogen_directions(&existing, count);
    }

    sp3_hydrogen_directions(&existing, count)
}

fn sp2_hydrogen_directions(existing: &[Vector3<f32>], count: usize) -> Vec<Vector3<f32>> {
    if existing.len() >= 2 {
        let sum = existing
            .iter()
            .copied()
            .fold(Vector3::zeros(), |acc, dir| acc + dir);
        let primary = (-sum).try_normalize(0.0001).unwrap_or_else(Vector3::x);
        return vec![primary; count];
    }

    let axis = Vector3::z_axis();
    let first = existing[0];
    let candidates = [
        rotation_around(axis.into_inner(), 120.0) * first,
        rotation_around(axis.into_inner(), -120.0) * first,
    ];

    candidates
        .into_iter()
        .cycle()
        .take(count)
        .map(|dir| dir.try_normalize(0.0001).unwrap_or_else(Vector3::x))
        .collect()
}

fn sp3_hydrogen_directions(existing: &[Vector3<f32>], count: usize) -> Vec<Vector3<f32>> {
    if existing.len() == 1 && count == 3 {
        return tetrahedral_tripod_around(existing[0]);
    }

    let fallback = [
        Vector3::new(1.0, 1.0, 1.0).normalize(),
        Vector3::new(1.0, -1.0, -1.0).normalize(),
        Vector3::new(-1.0, 1.0, -1.0).normalize(),
        Vector3::new(-1.0, -1.0, 1.0).normalize(),
    ];
    let sum = existing
        .iter()
        .copied()
        .fold(Vector3::zeros(), |acc, dir| acc + dir);
    let primary = (-sum).try_normalize(0.0001).unwrap_or_else(Vector3::x);
    let mut directions = vec![primary];

    for fallback_dir in fallback {
        if directions.len() >= count {
            break;
        }
        if existing
            .iter()
            .chain(directions.iter())
            .all(|dir| dir.dot(&fallback_dir).abs() < 0.85)
        {
            directions.push(fallback_dir);
        }
    }

    while directions.len() < count {
        directions.push(primary);
    }

    directions
}

fn tetrahedral_tripod_around(existing: Vector3<f32>) -> Vec<Vector3<f32>> {
    let axis_to_existing = existing.try_normalize(0.0001).unwrap_or_else(Vector3::x);
    let away = -axis_to_existing;
    let reference = if away.cross(&Vector3::z()).norm() > 0.0001 {
        Vector3::z()
    } else {
        Vector3::y()
    };
    let u = away.cross(&reference).normalize();
    let v = away.cross(&u).normalize();
    let axial_component = 1.0 / 3.0;
    let radial_component = (1.0_f32 - axial_component * axial_component).sqrt();

    [0.0_f32, 120.0, 240.0]
        .into_iter()
        .map(|degrees| {
            let radians = degrees.to_radians();
            (away * axial_component + (u * radians.cos() + v * radians.sin()) * radial_component)
                .normalize()
        })
        .collect()
}

fn default_hydrogen_directions(count: usize) -> Vec<Vector3<f32>> {
    [
        Vector3::new(1.0, 1.0, 1.0).normalize(),
        Vector3::new(1.0, -1.0, -1.0).normalize(),
        Vector3::new(-1.0, 1.0, -1.0).normalize(),
        Vector3::new(-1.0, -1.0, 1.0).normalize(),
    ]
    .into_iter()
    .cycle()
    .take(count)
    .collect()
}

fn is_sp2_like_center(neighbors: &[(usize, BondType)]) -> bool {
    neighbors
        .iter()
        .any(|(_, bond_type)| matches!(bond_type, BondType::Aromatic | BondType::Double))
}

fn rotation_around(axis: Vector3<f32>, degrees: f32) -> nalgebra::Rotation3<f32> {
    nalgebra::Rotation3::from_axis_angle(&nalgebra::Unit::new_normalize(axis), degrees.to_radians())
}

fn typical_valence(element: &str) -> Option<usize> {
    match normalized_symbol(element).as_str() {
        "C" => Some(4),
        "N" => Some(3),
        "O" => Some(2),
        "P" => Some(3),
        "S" => Some(2),
        "F" | "Cl" | "Br" | "I" | "H" => Some(1),
        _ => None,
    }
}

fn is_aromatic_element(element: &str) -> bool {
    matches!(normalized_symbol(element).as_str(), "C" | "N" | "O" | "S")
}

fn bond_order_value(bond_type: BondType) -> f32 {
    match bond_type {
        BondType::Single => 1.0,
        BondType::Double => 2.0,
        BondType::Triple => 3.0,
        BondType::Aromatic => 1.5,
    }
}

fn hydrogen_bond_length(element: &str) -> f32 {
    forcefield::equilibrium_bond_length(element, "H", BondType::Single).unwrap_or(1.0)
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

pub fn normalized_symbol(symbol: &str) -> String {
    let mut chars = symbol.trim().chars();
    let Some(first) = chars.next() else {
        return String::new();
    };

    let mut normalized = first.to_uppercase().collect::<String>();

    if let Some(second) = chars.next() {
        normalized.push_str(&second.to_lowercase().collect::<String>());
    }

    normalized
}

/// Every element symbol (H–Og), in proper case.
const ELEMENT_SYMBOLS: &[&str] = &[
    "H", "He", "Li", "Be", "B", "C", "N", "O", "F", "Ne", "Na", "Mg", "Al", "Si", "P", "S", "Cl",
    "Ar", "K", "Ca", "Sc", "Ti", "V", "Cr", "Mn", "Fe", "Co", "Ni", "Cu", "Zn", "Ga", "Ge", "As",
    "Se", "Br", "Kr", "Rb", "Sr", "Y", "Zr", "Nb", "Mo", "Tc", "Ru", "Rh", "Pd", "Ag", "Cd", "In",
    "Sn", "Sb", "Te", "I", "Xe", "Cs", "Ba", "La", "Ce", "Pr", "Nd", "Pm", "Sm", "Eu", "Gd", "Tb",
    "Dy", "Ho", "Er", "Tm", "Yb", "Lu", "Hf", "Ta", "W", "Re", "Os", "Ir", "Pt", "Au", "Hg", "Tl",
    "Pb", "Bi", "Po", "At", "Rn", "Fr", "Ra", "Ac", "Th", "Pa", "U", "Np", "Pu", "Am", "Cm", "Bk",
    "Cf", "Es", "Fm", "Md", "No", "Lr", "Rf", "Db", "Sg", "Bh", "Hs", "Mt", "Ds", "Rg", "Cn", "Nh",
    "Fl", "Mc", "Lv", "Ts", "Og",
];

/// Whether `symbol` is a real chemical element (case-insensitive).
pub fn is_element_symbol(symbol: &str) -> bool {
    let normalized = normalized_symbol(symbol);
    ELEMENT_SYMBOLS.contains(&normalized.as_str())
}

/// Standard atomic weights (u), parallel to [`ELEMENT_SYMBOLS`]. Conventional
/// IUPAC values; the longest-lived isotope's mass is used for elements with no
/// stable form. Used to convert a target mass density to a molecule count when
/// packing a disordered system.
const ATOMIC_MASSES_U: &[f32] = &[
    1.008, 4.0026, 6.94, 9.0122, 10.81, 12.011, 14.007, 15.999, 18.998, 20.180, 22.990, 24.305,
    26.982, 28.085, 30.974, 32.06, 35.45, 39.948, 39.098, 40.078, 44.956, 47.867, 50.942, 51.996,
    54.938, 55.845, 58.933, 58.693, 63.546, 65.38, 69.723, 72.630, 74.922, 78.971, 79.904, 83.798,
    85.468, 87.62, 88.906, 91.224, 92.906, 95.95, 98.0, 101.07, 102.91, 106.42, 107.87, 112.41,
    114.82, 118.71, 121.76, 127.60, 126.90, 131.29, 132.91, 137.33, 138.91, 140.12, 140.91, 144.24,
    145.0, 150.36, 151.96, 157.25, 158.93, 162.50, 164.93, 167.26, 168.93, 173.05, 174.97, 178.49,
    180.95, 183.84, 186.21, 190.23, 192.22, 195.08, 196.97, 200.59, 204.38, 207.2, 208.98, 209.0,
    210.0, 222.0, 223.0, 226.0, 227.0, 232.04, 231.04, 238.03, 237.0, 244.0, 243.0, 247.0, 247.0,
    251.0, 252.0, 257.0, 258.0, 259.0, 262.0, 267.0, 268.0, 269.0, 270.0, 269.0, 278.0, 281.0,
    282.0, 285.0, 286.0, 289.0, 289.0, 293.0, 294.0, 294.0,
];

const _: () = assert!(
    ATOMIC_MASSES_U.len() == ELEMENT_SYMBOLS.len(),
    "atomic-mass table must stay parallel to the element-symbol table",
);

/// The standard atomic weight (u) of an element, or `None` if `symbol` is not a
/// recognized element. Case-insensitive.
pub fn atomic_mass(symbol: &str) -> Option<f32> {
    let normalized = normalized_symbol(symbol);
    ELEMENT_SYMBOLS
        .iter()
        .position(|candidate| *candidate == normalized)
        .map(|index| ATOMIC_MASSES_U[index])
}

#[cfg(test)]
mod tests {
    use nalgebra::Point3;

    use crate::domain::{Atom, BondType, Structure, UnitCell};

    use super::{add_missing_hydrogens, infer_bonds_with_cell};

    #[test]
    fn infers_periodic_bond_across_cell_boundary() {
        let cell = UnitCell::from_parameters(10.0, 10.0, 10.0, 90.0, 90.0, 90.0);
        let atoms = vec![
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
        ];

        assert_eq!(infer_bonds_with_cell(&atoms, Some(&cell)).len(), 1);
    }

    #[test]
    fn monatomic_ions_are_never_bonded() {
        // A sodium ion 2.0 Å from a water oxygen sits well inside the naive
        // (1.66 + 0.66) * 1.25 ≈ 2.9 Å covalent cutoff, yet must not bond.
        let atoms = vec![
            Atom {
                element: "Na".to_string(),
                position: Point3::new(0.0, 0.0, 0.0),
                charge: 0.0,
            },
            Atom {
                element: "O".to_string(),
                position: Point3::new(2.0, 0.0, 0.0),
                charge: 0.0,
            },
            Atom {
                element: "H".to_string(),
                position: Point3::new(2.0, 0.97, 0.0),
                charge: 0.0,
            },
        ];

        // Non-periodic and periodic paths both skip the ion; only the O–H bond
        // survives.
        let bonds = infer_bonds_with_cell(&atoms, None);
        assert_eq!(bonds.len(), 1);
        assert!(bonds.iter().all(|bond| bond.a != 0 && bond.b != 0));

        let cell = UnitCell::from_parameters(20.0, 20.0, 20.0, 90.0, 90.0, 90.0);
        let periodic = infer_bonds_with_cell(&atoms, Some(&cell));
        assert_eq!(periodic.len(), 1);
        assert!(periodic.iter().all(|bond| bond.a != 0 && bond.b != 0));
    }

    #[test]
    fn infers_carbon_carbon_double_bond_from_distance() {
        let atoms = vec![
            Atom {
                element: "C".to_string(),
                position: Point3::new(0.0, 0.0, 0.0),
                charge: 0.0,
            },
            Atom {
                element: "C".to_string(),
                position: Point3::new(1.34, 0.0, 0.0),
                charge: 0.0,
            },
        ];
        let bonds = infer_bonds_with_cell(&atoms, None);

        assert_eq!(bonds[0].bond_type, BondType::Double);
    }

    #[test]
    fn infers_aromatic_bonds_in_benzene_like_ring() {
        let atoms = (0..6)
            .map(|index| {
                let angle = index as f32 * std::f32::consts::TAU / 6.0;

                Atom {
                    element: "C".to_string(),
                    position: Point3::new(angle.cos() * 1.39, angle.sin() * 1.39, 0.0),
                    charge: 0.0,
                }
            })
            .collect::<Vec<_>>();
        let bonds = infer_bonds_with_cell(&atoms, None);

        assert_eq!(bonds.len(), 6);
        assert!(
            bonds
                .iter()
                .all(|bond| bond.bond_type == BondType::Aromatic)
        );
    }

    #[test]
    fn adds_missing_hydrogens_to_methane_carbon() {
        let mut structure = Structure::with_bonds(
            "carbon",
            vec![Atom {
                element: "C".to_string(),
                position: Point3::origin(),
                charge: 0.0,
            }],
            Vec::new(),
        );
        let added = add_missing_hydrogens(&mut structure.atoms, &mut structure.bonds);

        assert_eq!(added, 4);
        assert_eq!(structure.atoms.len(), 5);
        assert_eq!(structure.bonds.len(), 4);
    }

    #[test]
    fn adds_aromatic_hydrogen_with_sp2_angle() {
        let atoms = toluene_heavy_atoms();
        let bonds = infer_bonds_with_cell(&atoms, None);
        let mut structure = Structure::with_bonds("toluene heavy atoms", atoms, bonds);
        let added = add_missing_hydrogens(&mut structure.atoms, &mut structure.bonds);
        let phenyl_carbon = 2;
        let methyl_bearing_carbon = 1;
        let h_index = structure
            .bonds
            .iter()
            .find_map(|bond| {
                if bond.a == phenyl_carbon && structure.atoms[bond.b].element == "H" {
                    Some(bond.b)
                } else if bond.b == phenyl_carbon && structure.atoms[bond.a].element == "H" {
                    Some(bond.a)
                } else {
                    None
                }
            })
            .expect("phenyl C-H");
        let ring_substituent = structure.atoms[methyl_bearing_carbon].position
            - structure.atoms[phenyl_carbon].position;
        let hydrogen = structure.atoms[h_index].position - structure.atoms[phenyl_carbon].position;
        let angle = (ring_substituent.dot(&hydrogen) / (ring_substituent.norm() * hydrogen.norm()))
            .clamp(-1.0, 1.0)
            .acos()
            .to_degrees();

        assert_eq!(added, 8);
        assert!((angle - 120.0).abs() < 5.0, "angle was {angle}");
    }

    #[test]
    fn adds_toluene_methyl_hydrogens_with_tetrahedral_angles() {
        let atoms = toluene_heavy_atoms();
        let bonds = infer_bonds_with_cell(&atoms, None);
        let mut structure = Structure::with_bonds("toluene heavy atoms", atoms, bonds);
        let added = add_missing_hydrogens(&mut structure.atoms, &mut structure.bonds);
        let methyl_carbon = 0;
        let phenyl_carbon = 1;
        let methyl_hydrogens = bonded_hydrogens(&structure, methyl_carbon);

        assert_eq!(added, 8);
        assert_eq!(methyl_hydrogens.len(), 3);

        for hydrogen in &methyl_hydrogens {
            let angle = bond_angle(&structure, phenyl_carbon, methyl_carbon, *hydrogen);

            assert!((angle - 109.47).abs() < 5.0, "C(Ph)-C-H angle was {angle}");
        }

        for i in 0..methyl_hydrogens.len() {
            for j in (i + 1)..methyl_hydrogens.len() {
                let angle = bond_angle(
                    &structure,
                    methyl_hydrogens[i],
                    methyl_carbon,
                    methyl_hydrogens[j],
                );

                assert!((angle - 109.47).abs() < 5.0, "H-C-H angle was {angle}");
            }
        }
    }

    fn toluene_heavy_atoms() -> Vec<Atom> {
        vec![
            Atom {
                element: "C".to_string(),
                position: Point3::new(2.221095, 0.033319, -0.027866),
                charge: 0.0,
            },
            Atom {
                element: "C".to_string(),
                position: Point3::new(0.721422, 0.009962, -0.039183),
                charge: 0.0,
            },
            Atom {
                element: "C".to_string(),
                position: Point3::new(0.032134, -1.212_05, 0.024559),
                charge: 0.0,
            },
            Atom {
                element: "C".to_string(),
                position: Point3::new(-1.366126, -1.230666, 0.051405),
                charge: 0.0,
            },
            Atom {
                element: "C".to_string(),
                position: Point3::new(-2.083336, -0.031206, 0.027755),
                charge: 0.0,
            },
            Atom {
                element: "C".to_string(),
                position: Point3::new(-1.403292, 1.189092, -0.017693),
                charge: 0.0,
            },
            Atom {
                element: "C".to_string(),
                position: Point3::new(-0.005096, 1.211892, -0.044659),
                charge: 0.0,
            },
        ]
    }

    fn bonded_hydrogens(structure: &Structure, atom_index: usize) -> Vec<usize> {
        structure
            .bonds
            .iter()
            .filter_map(|bond| {
                if bond.a == atom_index && structure.atoms[bond.b].element == "H" {
                    Some(bond.b)
                } else if bond.b == atom_index && structure.atoms[bond.a].element == "H" {
                    Some(bond.a)
                } else {
                    None
                }
            })
            .collect()
    }

    fn bond_angle(structure: &Structure, first: usize, center: usize, second: usize) -> f32 {
        let first = structure.atoms[first].position - structure.atoms[center].position;
        let second = structure.atoms[second].position - structure.atoms[center].position;

        (first.dot(&second) / (first.norm() * second.norm()))
            .clamp(-1.0, 1.0)
            .acos()
            .to_degrees()
    }
}
