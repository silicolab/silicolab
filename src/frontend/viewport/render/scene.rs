use nalgebra::{Point3, Vector3};

use crate::domain::{BondType, Structure, UnitCell};

use super::super::camera::Projector;
use super::{ViewportVisualState, atom_screen_radius, atom_visible};

#[derive(Clone, Copy)]
pub(crate) struct RenderedAtom {
    pub(crate) depth: f32,
    pub(crate) index: usize,
    pub(crate) pos: eframe::egui::Pos2,
    pub(crate) scale: f32,
}

pub(crate) type PickTarget = RenderedAtom;

/// Project just the atom centers into pick targets, skipping bond building and
/// depth sorting. Used by the GPU path, where the heavy geometry lives on the
/// GPU and the CPU only needs atom screen positions for hover/click picking.
pub(crate) fn project_pick_targets(
    structure: &Structure,
    viewport: &Projector,
) -> Vec<RenderedAtom> {
    let mut atoms = structure
        .atoms
        .iter()
        .enumerate()
        .map(|(index, atom)| {
            let projected = viewport.project(atom.position);
            RenderedAtom {
                depth: projected.depth,
                index,
                pos: projected.pos,
                scale: projected.scale,
            }
        })
        .collect::<Vec<_>>();
    atoms.sort_by(|a, b| a.depth.total_cmp(&b.depth));
    atoms
}

pub(crate) fn pick_atom(
    structure: &Structure,
    projected_atoms: &[PickTarget],
    pointer: eframe::egui::Pos2,
    visual_state: &ViewportVisualState,
) -> Option<usize> {
    projected_atoms
        .iter()
        .rev()
        .filter_map(|atom| {
            if !atom_visible(structure, visual_state, atom.index) {
                return None;
            }
            let style =
                crate::domain::chemistry::element_style(&structure.atoms[atom.index].element);
            let radius = atom_screen_radius(style.display_radius, 1.0, atom.scale) + 5.0;
            let distance = atom.pos.distance(pointer);

            (distance <= radius).then_some((atom.index, distance))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(index, _)| index)
}

/// A bond rendered as a world-space line segment, with the atom whose color
/// applies at each end. Camera-independent — used to build GPU cylinder
/// instances that survive rotation without a rebuild. Periodic bonds that cross
/// a cell boundary become two half-segments, each reaching from its atom to the
/// midpoint of the wrapped bond; those
/// halves are flagged `full_bond = false` and drawn as a single cylinder
/// regardless of order. `aromatic_center` is the ring centroid for aromatic
/// bonds, used to offset multi-bond cylinders into the ring plane (so the offset
/// is view-independent).
#[derive(Clone, Copy)]
pub(crate) struct BondWorldSegment {
    pub(crate) start: Point3<f32>,
    pub(crate) end: Point3<f32>,
    pub(crate) start_atom: usize,
    pub(crate) end_atom: usize,
    pub(crate) bond_type: BondType,
    pub(crate) aromatic_center: Option<Point3<f32>>,
    pub(crate) full_bond: bool,
}

pub(crate) fn bond_world_segments(structure: &Structure) -> Vec<BondWorldSegment> {
    let aromatic_centers = aromatic_system_centers(structure);
    let mut segments = Vec::with_capacity(structure.bonds.len());
    for (bond_index, bond) in structure.bonds.iter().enumerate() {
        let start = structure.atoms[bond.a].position;
        let end = structure.atoms[bond.b].position;
        if let Some(cell) = &structure.cell {
            let (delta, crosses_boundary) = periodic_bond_delta(cell, start, end);
            if crosses_boundary {
                segments.push(BondWorldSegment {
                    start,
                    end: Point3::from(start.coords + delta * 0.5),
                    start_atom: bond.a,
                    end_atom: bond.b,
                    bond_type: bond.bond_type,
                    aromatic_center: None,
                    full_bond: false,
                });
                segments.push(BondWorldSegment {
                    start: end,
                    end: Point3::from(end.coords - delta * 0.5),
                    start_atom: bond.b,
                    end_atom: bond.a,
                    bond_type: bond.bond_type,
                    aromatic_center: None,
                    full_bond: false,
                });
                continue;
            }
        }
        segments.push(BondWorldSegment {
            start,
            end,
            start_atom: bond.a,
            end_atom: bond.b,
            bond_type: bond.bond_type,
            aromatic_center: aromatic_centers[bond_index],
            full_bond: true,
        });
    }
    segments
}

fn periodic_bond_delta(
    cell: &UnitCell,
    first: Point3<f32>,
    second: Point3<f32>,
) -> (Vector3<f32>, bool) {
    let first_frac = cell.cartesian_to_fractional(first);
    let second_frac = cell.cartesian_to_fractional(second);
    let mut delta = second_frac - first_frac;
    let shift = Vector3::new(delta.x.round(), delta.y.round(), delta.z.round());
    let crosses_boundary =
        shift.x.abs() > 0.0001 || shift.y.abs() > 0.0001 || shift.z.abs() > 0.0001;

    delta -= shift;

    (
        cell.vectors[0] * delta.x + cell.vectors[1] * delta.y + cell.vectors[2] * delta.z,
        crosses_boundary,
    )
}

fn aromatic_system_centers(structure: &Structure) -> Vec<Option<Point3<f32>>> {
    let mut aromatic_neighbors = vec![Vec::new(); structure.atoms.len()];
    for bond in structure
        .bonds
        .iter()
        .filter(|bond| bond.bond_type == BondType::Aromatic)
    {
        aromatic_neighbors[bond.a].push(bond.b);
        aromatic_neighbors[bond.b].push(bond.a);
    }

    let mut component_for_atom = vec![None; structure.atoms.len()];
    let mut component_centers = Vec::new();

    for atom_index in 0..structure.atoms.len() {
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

        let sum = component_atoms
            .iter()
            .fold(Vector3::zeros(), |acc, &index| {
                acc + structure.atoms[index].position.coords
            });
        component_centers.push(Point3::from(sum / component_atoms.len() as f32));
    }

    structure
        .bonds
        .iter()
        .map(|bond| {
            if bond.bond_type != BondType::Aromatic {
                None
            } else {
                component_for_atom[bond.a].map(|component_index| component_centers[component_index])
            }
        })
        .collect()
}
