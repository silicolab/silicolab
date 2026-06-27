use eframe::egui::{self, Color32, Pos2, Stroke, Vec2};
use nalgebra::{Point3, Vector3};

use crate::domain::{BondType, Structure, UnitCell};
use crate::frontend::AtomSelection;

pub fn status_text(structure: &Structure, selection: &AtomSelection) -> String {
    let mut text = format!(
        "{} | {} atoms | {} bonds",
        if structure.title.is_empty() {
            "Untitled structure"
        } else {
            &structure.title
        },
        structure.atoms.len(),
        structure.bonds.len()
    );

    if let Some(cell) = &structure.cell {
        text.push_str(&format!(
            " | cell {:.2} {:.2} {:.2} A, {:.1} {:.1} {:.1} deg",
            cell.a, cell.b, cell.c, cell.alpha, cell.beta, cell.gamma
        ));
    }

    if !selection.is_empty() {
        text.push_str(&format!(" | selected {}", selection.len()));
    }

    text
}

pub fn bond_geometry_summary(structure: &Structure) -> String {
    if structure.bonds.is_empty() {
        return "no bonds detected".to_string();
    }

    let lengths = structure
        .bonds
        .iter()
        .map(|bond| bond_length(structure, bond.a, bond.b))
        .collect::<Vec<_>>();
    let min_length = lengths.iter().copied().fold(f32::INFINITY, f32::min);
    let max_length = lengths.iter().copied().fold(0.0_f32, f32::max);
    let avg_length = lengths.iter().sum::<f32>() / lengths.len() as f32;
    let angles = bond_angles(structure);

    if angles.is_empty() {
        return format!(
            "bond lengths {:.2}-{:.2} A, avg {:.2} A",
            min_length, max_length, avg_length
        );
    }

    let min_angle = angles.iter().copied().fold(f32::INFINITY, f32::min);
    let max_angle = angles.iter().copied().fold(0.0_f32, f32::max);
    let avg_angle = angles.iter().sum::<f32>() / angles.len() as f32;

    format!(
        "bond lengths {:.2}-{:.2} A, avg {:.2} A; angles {:.1}-{:.1} deg, avg {:.1} deg",
        min_length, max_length, avg_length, min_angle, max_angle, avg_angle
    )
}

fn bond_length(structure: &Structure, a: usize, b: usize) -> f32 {
    match &structure.cell {
        Some(cell) => periodic_delta(
            cell,
            structure.atoms[a].position,
            structure.atoms[b].position,
        )
        .norm(),
        None => nalgebra::distance(&structure.atoms[a].position, &structure.atoms[b].position),
    }
}

fn bond_angles(structure: &Structure) -> Vec<f32> {
    let mut neighbors = vec![Vec::new(); structure.atoms.len()];
    for bond in &structure.bonds {
        neighbors[bond.a].push(bond.b);
        neighbors[bond.b].push(bond.a);
    }

    let mut angles = Vec::new();
    for (center, bonded) in neighbors.iter().enumerate() {
        for i in 0..bonded.len() {
            for j in (i + 1)..bonded.len() {
                let first = delta_from_center(structure, center, bonded[i]);
                let second = delta_from_center(structure, center, bonded[j]);
                let denom = first.norm() * second.norm();
                if denom > 0.0001 {
                    let cosine = (first.dot(&second) / denom).clamp(-1.0, 1.0);
                    angles.push(cosine.acos().to_degrees());
                }
            }
        }
    }

    angles
}

fn delta_from_center(structure: &Structure, center: usize, other: usize) -> Vector3<f32> {
    match &structure.cell {
        Some(cell) => periodic_delta(
            cell,
            structure.atoms[center].position,
            structure.atoms[other].position,
        ),
        None => structure.atoms[other].position - structure.atoms[center].position,
    }
}

fn periodic_delta(cell: &UnitCell, first: Point3<f32>, second: Point3<f32>) -> Vector3<f32> {
    let first_frac = cell.cartesian_to_fractional(first);
    let second_frac = cell.cartesian_to_fractional(second);
    let mut delta = second_frac - first_frac;

    delta.x -= delta.x.round();
    delta.y -= delta.y.round();
    delta.z -= delta.z.round();

    cell.vectors[0] * delta.x + cell.vectors[1] * delta.y + cell.vectors[2] * delta.z
}

pub(super) fn color32(point: Point3<f32>) -> Color32 {
    Color32::from_rgb(
        (point.x.clamp(0.0, 1.0) * 255.0) as u8,
        (point.y.clamp(0.0, 1.0) * 255.0) as u8,
        (point.z.clamp(0.0, 1.0) * 255.0) as u8,
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn draw_aromatic_bond(
    painter: &egui::Painter,
    start: Pos2,
    end: Pos2,
    aromatic_center: Option<Pos2>,
    color: Color32,
    main_width: f32,
    accent_width: f32,
    offset: f32,
) {
    painter.line_segment([start, end], Stroke::new(main_width, color));
    let perp = inward_perpendicular_offset(start, end, offset, aromatic_center);
    draw_dashed_segment(
        painter,
        start + perp,
        end + perp,
        6.0,
        4.0,
        Stroke::new(accent_width, color),
    );
}

fn draw_dashed_segment(
    painter: &egui::Painter,
    start: Pos2,
    end: Pos2,
    dash_length: f32,
    gap_length: f32,
    stroke: Stroke,
) {
    let segment = end - start;
    let length = segment.length();
    if length <= f32::EPSILON {
        return;
    }

    let direction = segment / length;
    let mut cursor = 0.0;
    while cursor < length {
        let dash_end = (cursor + dash_length).min(length);
        painter.line_segment(
            [start + direction * cursor, start + direction * dash_end],
            stroke,
        );
        cursor += dash_length + gap_length;
    }
}

pub(super) fn perpendicular_offset(start: Pos2, end: Pos2, offset: f32) -> Vec2 {
    let delta = end - start;
    if delta.length_sq() <= f32::EPSILON {
        Vec2::ZERO
    } else {
        Vec2::new(-delta.y, delta.x).normalized() * offset
    }
}

pub(super) fn trimmed_segment(
    start: Pos2,
    end: Pos2,
    start_trim: f32,
    end_trim: f32,
) -> Option<(Pos2, Pos2)> {
    let delta = end - start;
    let length = delta.length();
    if length <= start_trim + end_trim || length <= f32::EPSILON {
        return None;
    }

    let direction = delta / length;
    Some((start + direction * start_trim, end - direction * end_trim))
}

fn inward_perpendicular_offset(
    start: Pos2,
    end: Pos2,
    offset: f32,
    aromatic_center: Option<Pos2>,
) -> Vec2 {
    let perp = perpendicular_offset(start, end, offset);
    let Some(center) = aromatic_center else {
        return perp;
    };

    let midpoint = start + (end - start) * 0.5;
    let center_delta = center - midpoint;
    if perp.dot(center_delta) >= 0.0 {
        perp
    } else {
        -perp
    }
}

pub(super) fn aromatic_component_centers(
    bonds: &[(usize, usize, crate::domain::BondType)],
    atom_positions: &[Pos2],
) -> Vec<Option<Pos2>> {
    let mut aromatic_neighbors = vec![Vec::new(); atom_positions.len()];
    for &(a, b, _) in bonds
        .iter()
        .filter(|(_, _, bond_type)| *bond_type == BondType::Aromatic)
    {
        aromatic_neighbors[a].push(b);
        aromatic_neighbors[b].push(a);
    }

    let mut component_for_atom = vec![None; atom_positions.len()];
    let mut component_centers = Vec::new();

    for atom_index in 0..atom_positions.len() {
        if aromatic_neighbors[atom_index].is_empty() || component_for_atom[atom_index].is_some() {
            continue;
        }

        let mut stack = vec![atom_index];
        let mut component_atoms = Vec::new();
        component_for_atom[atom_index] = Some(component_centers.len());

        while let Some(current) = stack.pop() {
            component_atoms.push(current);
            for &neighbor in &aromatic_neighbors[current] {
                if component_for_atom[neighbor].is_none() {
                    component_for_atom[neighbor] = Some(component_centers.len());
                    stack.push(neighbor);
                }
            }
        }

        let sum = component_atoms.iter().fold(Vec2::ZERO, |acc, &index| {
            acc + atom_positions[index].to_vec2()
        });
        let center = sum / component_atoms.len() as f32;
        component_centers.push(Pos2::new(center.x, center.y));
    }

    bonds
        .iter()
        .map(|&(a, _, bond_type)| {
            if bond_type != BondType::Aromatic {
                None
            } else {
                component_for_atom[a].map(|component_index| component_centers[component_index])
            }
        })
        .collect()
}
