//! Turn placed rigid copies into one combined [`Structure`].
//!
//! Fixed/obstacle atoms come first (so their indices are stable), then each
//! packed copy's atoms in template order. Every copy's bonds are replicated with
//! its atom indices offset by the running base — the same offset trick as
//! [`Structure::make_supercell`] — and each copy gets an [`AppendedResidue`] so
//! `atom_category` / cartoon / water-detection keep working on the result (via
//! [`extend_biopolymer_coverage`], exactly as solvation does).

use nalgebra::Point3;

use crate::domain::{
    AppendedResidue, Atom, Bond, Structure, UnitCell, chemistry, extend_biopolymer_coverage,
};

use super::PackSpecies;

/// One placed copy: which species it is and the world positions of its atoms in
/// template order. The engine computes these (it needs them for overlap
/// scoring), so assembly never re-derives geometry — guaranteeing the streamed /
/// final structure matches exactly what was scored.
pub(crate) struct CopyPlacement {
    pub species_index: usize,
    pub positions: Vec<Point3<f32>>,
}

/// Assemble the fixed obstacle (if any) and all placed copies into one combined
/// structure, stamping `output_cell` and attaching per-copy residue metadata.
pub(crate) fn assemble(
    species: &[PackSpecies],
    copies: &[CopyPlacement],
    fixed: Option<&Structure>,
    output_cell: Option<UnitCell>,
    title: &str,
) -> Structure {
    let mut atoms: Vec<Atom> = Vec::new();
    let mut bonds: Vec<Bond> = Vec::new();
    let mut appended: Vec<AppendedResidue> = Vec::new();

    // Fixed/obstacle atoms first; their bonds are already 0-based at the front.
    let fixed_atom_count = fixed.map(|f| f.atoms.len()).unwrap_or(0);
    if let Some(fixed) = fixed {
        atoms.extend(fixed.atoms.iter().cloned());
        bonds.extend(fixed.bonds.iter().cloned());
    }

    for (copy_index, copy) in copies.iter().enumerate() {
        let template = &species[copy.species_index].molecule;
        debug_assert_eq!(
            copy.positions.len(),
            template.atoms.len(),
            "placement atom count must match its template",
        );
        let residue_name = residue_label(&template.title);
        let base = atoms.len();
        let mut residue_atoms = Vec::with_capacity(template.atoms.len());
        for (index, position) in copy.positions.iter().enumerate() {
            let source = &template.atoms[index];
            atoms.push(Atom {
                element: source.element.clone(),
                position: *position,
                charge: source.charge,
            });
            residue_atoms.push((base + index, atom_label(&source.element, index)));
        }
        for bond in &template.bonds {
            bonds.push(Bond::with_type(
                base + bond.a,
                base + bond.b,
                bond.bond_type,
            ));
        }
        appended.push(AppendedResidue {
            residue_name,
            chain_id: 'X',
            sequence_number: copy_index as i32 + 1,
            atoms: residue_atoms,
        });
    }

    let total_atom_count = atoms.len();
    let biopolymer = extend_biopolymer_coverage(
        fixed.and_then(|f| f.biopolymer.as_ref()),
        fixed_atom_count,
        total_atom_count,
        &appended,
    );

    Structure {
        title: title.to_string(),
        atoms,
        bonds,
        cell: output_cell,
        biopolymer,
    }
}

/// A short residue label from a molecule's title: up to three uppercase
/// alphanumerics, falling back to `MOL` when the title has none. A title like
/// `"water"` becomes `WAT`, which the water detector recognizes as solvent.
fn residue_label(title: &str) -> String {
    let label: String = title
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(3)
        .collect::<String>()
        .to_ascii_uppercase();
    if label.is_empty() {
        "MOL".to_string()
    } else {
        label
    }
}

/// A per-residue atom name: the element symbol plus a 1-based index, e.g. `C1`.
fn atom_label(element: &str, index_in_residue: usize) -> String {
    format!(
        "{}{}",
        chemistry::normalized_symbol(element).to_ascii_uppercase(),
        index_in_residue + 1
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{AtomCategory, BondType};

    fn diatomic(title: &str) -> Structure {
        Structure::with_bonds(
            title,
            vec![
                Atom {
                    element: "C".to_string(),
                    position: Point3::new(0.0, 0.0, 0.0),
                    charge: 0.0,
                },
                Atom {
                    element: "O".to_string(),
                    position: Point3::new(1.2, 0.0, 0.0),
                    charge: 0.0,
                },
            ],
            vec![Bond::with_type(0, 1, BondType::Double)],
        )
    }

    #[test]
    fn single_copy_reproduces_the_template() {
        let template = diatomic("carbon monoxide");
        let species = vec![PackSpecies {
            molecule: template.clone(),
            count: 1,
        }];
        let copies = vec![CopyPlacement {
            species_index: 0,
            positions: template.atoms.iter().map(|a| a.position).collect(),
        }];

        let result = assemble(&species, &copies, None, None, "packed");

        assert_eq!(result.atoms.len(), 2);
        assert_eq!(result.atoms[0].element, "C");
        assert_eq!(result.atoms[1].element, "O");
        assert_eq!(result.atoms[1].position, Point3::new(1.2, 0.0, 0.0));
        // Bond preserved with the original (offset-by-0) indices and type.
        assert_eq!(result.bonds.len(), 1);
        assert_eq!((result.bonds[0].a, result.bonds[0].b), (0, 1));
        assert_eq!(result.bonds[0].bond_type, BondType::Double);
        assert!(result.cell.is_none());
        // The copy is covered by a residue, so it classifies as a ligand.
        let biopolymer = result.biopolymer.as_ref().expect("biopolymer");
        assert!(biopolymer.is_compatible_with_atom_count(result.atoms.len()));
        assert_eq!(result.atom_category(0), AtomCategory::Ligand);
    }

    #[test]
    fn copies_offset_bonds_and_count_atoms() {
        let template = diatomic("co");
        let species = vec![PackSpecies {
            molecule: template.clone(),
            count: 3,
        }];
        let copies: Vec<CopyPlacement> = (0..3)
            .map(|c| CopyPlacement {
                species_index: 0,
                positions: template
                    .atoms
                    .iter()
                    .map(|a| a.position + nalgebra::Vector3::new(10.0 * c as f32, 0.0, 0.0))
                    .collect(),
            })
            .collect();

        let result = assemble(&species, &copies, None, None, "packed");

        assert_eq!(result.atoms.len(), 6);
        assert_eq!(result.bonds.len(), 3);
        // Third copy's bond is offset by 2*2 = 4.
        assert_eq!((result.bonds[2].a, result.bonds[2].b), (4, 5));
    }

    #[test]
    fn fixed_obstacle_comes_first_and_offsets_copies() {
        let template = diatomic("co");
        let fixed = Structure::new(
            "obstacle",
            vec![Atom {
                element: "Fe".to_string(),
                position: Point3::new(0.0, 0.0, 0.0),
                charge: 0.0,
            }],
        );
        let species = vec![PackSpecies {
            molecule: template.clone(),
            count: 1,
        }];
        let copies = vec![CopyPlacement {
            species_index: 0,
            positions: template.atoms.iter().map(|a| a.position).collect(),
        }];

        let result = assemble(&species, &copies, Some(&fixed), None, "packed");

        assert_eq!(result.atoms.len(), 3);
        assert_eq!(result.atoms[0].element, "Fe");
        // Copy bond offset past the one fixed atom.
        assert_eq!(result.bonds.len(), 1);
        assert_eq!((result.bonds[0].a, result.bonds[0].b), (1, 2));
    }

    #[test]
    fn residue_label_falls_back_and_truncates() {
        assert_eq!(residue_label("water"), "WAT");
        assert_eq!(residue_label("benzene"), "BEN");
        assert_eq!(residue_label("123-abc"), "123");
        assert_eq!(residue_label("***"), "MOL");
    }
}
