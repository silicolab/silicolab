use super::*;

use nalgebra::Vector3;

use crate::{
    domain::{Atom, Bond, BondType},
    engines::forcefield,
};

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

fn hydrogen_bond_length(element: &str) -> f32 {
    forcefield::equilibrium_bond_length(element, "H", BondType::Single).unwrap_or(1.0)
}
