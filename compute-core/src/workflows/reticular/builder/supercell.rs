use nalgebra::Vector3;

use crate::domain::{Atom, Bond, Structure, UnitCell};
use crate::workflows::reticular::recipe::ReticularBuildSpec;

use super::honeycomb::add_bond;

pub(super) fn supercell_repeats(spec: &ReticularBuildSpec) -> [u32; 3] {
    let mut repeats = spec.supercell;
    let period = spec.stacking_period();
    if period > 1 {
        repeats[2] = repeats[2].max(1).div_ceil(period);
    }
    repeats
}

pub(super) fn expand_supercell(structure: &Structure, supercell: [u32; 3]) -> Structure {
    let Some(cell) = &structure.cell else {
        return structure.clone();
    };
    let nx = supercell[0].max(1);
    let ny = supercell[1].max(1);
    let nz = supercell[2].max(1);
    let source_atom_count = structure.atoms.len();
    let expanded_cell = UnitCell::from_parameters(
        cell.a * nx as f32,
        cell.b * ny as f32,
        cell.c * nz as f32,
        cell.alpha,
        cell.beta,
        cell.gamma,
    );
    let mut atoms = Vec::new();
    let mut bonds = Vec::new();

    for ix in 0..nx {
        for iy in 0..ny {
            for iz in 0..nz {
                for atom in &structure.atoms {
                    let frac = cell.cartesian_to_fractional(atom.position);
                    let expanded_frac = Vector3::new(
                        (frac.x + ix as f32) / nx as f32,
                        (frac.y + iy as f32) / ny as f32,
                        (frac.z + iz as f32) / nz as f32,
                    );

                    atoms.push(Atom {
                        element: atom.element.clone(),
                        position: expanded_cell.fractional_to_cartesian(
                            expanded_frac.x,
                            expanded_frac.y,
                            expanded_frac.z,
                        ),
                        charge: atom.charge,
                    });
                }
            }
        }
    }

    for ix in 0..nx {
        for iy in 0..ny {
            for iz in 0..nz {
                for bond in &structure.bonds {
                    let shift = bond_cell_shift(structure, cell, bond);
                    let jx = (ix as i32 + shift.x).rem_euclid(nx as i32) as u32;
                    let jy = (iy as i32 + shift.y).rem_euclid(ny as i32) as u32;
                    let jz = (iz as i32 + shift.z).rem_euclid(nz as i32) as u32;
                    let a = expanded_index(ix, iy, iz, bond.a, ny, nz, source_atom_count);
                    let b = expanded_index(jx, jy, jz, bond.b, ny, nz, source_atom_count);

                    add_bond(&mut bonds, a, b, bond.bond_type);
                }
            }
        }
    }

    Structure::with_cell_and_bonds(structure.title.clone(), atoms, bonds, expanded_cell)
}

fn bond_cell_shift(structure: &Structure, cell: &UnitCell, bond: &Bond) -> CellShift {
    let first = cell.cartesian_to_fractional(structure.atoms[bond.a].position);
    let second = cell.cartesian_to_fractional(structure.atoms[bond.b].position);
    let delta = second - first;

    CellShift {
        x: -delta.x.round() as i32,
        y: -delta.y.round() as i32,
        z: -delta.z.round() as i32,
    }
}

struct CellShift {
    x: i32,
    y: i32,
    z: i32,
}

fn expanded_index(
    ix: u32,
    iy: u32,
    iz: u32,
    atom: usize,
    ny: u32,
    nz: u32,
    source_atom_count: usize,
) -> usize {
    (((ix * ny + iy) * nz + iz) as usize * source_atom_count) + atom
}
