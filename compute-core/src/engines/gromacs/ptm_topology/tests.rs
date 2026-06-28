use super::*;
use crate::domain::biopolymer::{Biopolymer, ChainRecord, ResidueRecord};
use crate::domain::modification::MethylDegree;
use crate::domain::structure::{Atom, Bond, BondType};
use crate::workflows::ptm::{acetylate_protein, methylate_protein, phosphorylate_protein};
use nalgebra::Point3;

fn database() -> CarbTopologyDatabase {
    forcefield_assets::charmm36_ptm_database().expect("bundled aminoacids.rtp parses")
}

fn target() -> ResidueId {
    ResidueId::new('A', 1, ' ')
}

/// Build a single-residue protein with backbone metadata so the PTM builders and
/// the rename can resolve the side-chain anchor.
fn single_residue(
    name: &str,
    atoms: &[(&str, &str, [f32; 3])],
    bonds: &[(usize, usize)],
) -> Structure {
    let structure_atoms: Vec<Atom> = atoms
        .iter()
        .map(|(_, element, position)| Atom {
            element: element.to_string(),
            position: Point3::new(position[0], position[1], position[2]),
            charge: 0.0,
        })
        .collect();
    let structure_bonds: Vec<Bond> = bonds
        .iter()
        .map(|&(a, b)| Bond::with_type(a, b, BondType::Single))
        .collect();
    let names: Vec<Option<String>> = atoms.iter().map(|(n, _, _)| Some(n.to_string())).collect();
    let find = |needle: &str| atoms.iter().position(|(n, _, _)| *n == needle);
    let residue = ResidueRecord {
        id: target(),
        residue_name: name.to_string(),
        atom_indices: (0..atoms.len()).collect(),
        alpha_carbon: find("CA"),
        backbone_nitrogen: find("N"),
        backbone_carbon: find("C"),
        backbone_oxygen: find("O"),
        is_standard_amino_acid: true,
    };
    let biopolymer = Biopolymer {
        residues: vec![residue],
        chains: vec![ChainRecord {
            id: 'A',
            residue_indices: vec![0],
        }],
        secondary_structures: Vec::new(),
        residue_for_atom: vec![Some(0); atoms.len()],
        atom_name_for_atom: names,
    };
    let mut structure = Structure::with_bonds(name.to_string(), structure_atoms, structure_bonds);
    structure.biopolymer = Some(biopolymer);
    structure
}

fn ser() -> Structure {
    single_residue(
        "SER",
        &[
            ("N", "N", [0.0, 0.0, 0.0]),
            ("CA", "C", [1.45, 0.0, 0.0]),
            ("C", "C", [2.9, 0.0, 0.0]),
            ("O", "O", [3.6, 1.0, 0.0]),
            ("CB", "C", [1.45, 1.5, 0.0]),
            ("OG", "O", [2.6, 2.4, 0.0]),
            ("HG", "H", [3.4, 2.0, 0.0]),
        ],
        &[(0, 1), (1, 2), (2, 3), (1, 4), (4, 5), (5, 6)],
    )
}

fn lys() -> Structure {
    single_residue(
        "LYS",
        &[
            ("N", "N", [0.0, 0.0, 0.0]),
            ("CA", "C", [1.45, 0.0, 0.0]),
            ("C", "C", [2.9, 0.0, 0.0]),
            ("O", "O", [3.6, 1.0, 0.0]),
            ("CB", "C", [1.45, 1.5, 0.0]),
            ("CG", "C", [2.9, 2.0, 0.0]),
            ("CD", "C", [3.5, 3.3, 0.0]),
            ("CE", "C", [4.9, 3.5, 0.0]),
            ("NZ", "N", [5.5, 4.8, 0.0]),
            ("HZ1", "H", [6.5, 4.8, 0.0]),
            ("HZ2", "H", [5.0, 5.6, 0.0]),
            ("HZ3", "H", [5.5, 4.0, 0.8]),
        ],
        &[
            (0, 1),
            (1, 2),
            (2, 3),
            (1, 4),
            (4, 5),
            (5, 6),
            (6, 7),
            (7, 8),
            (8, 9),
            (8, 10),
            (8, 11),
        ],
    )
}

fn folded_residue_name(prep: &PtmPreparation) -> &str {
    prep.structure
        .biopolymer
        .as_ref()
        .unwrap()
        .residues
        .iter()
        .find(|r| r.id == target())
        .unwrap()
        .residue_name
        .as_str()
}

fn heavy_atoms_resolve(prep: &PtmPreparation, db: &CarbTopologyDatabase) {
    let bio = prep.structure.biopolymer.as_ref().unwrap();
    let host = bio.residues.iter().find(|r| r.id == target()).unwrap();
    for &index in &host.atom_indices {
        if prep.structure.atoms[index]
            .element
            .eq_ignore_ascii_case("H")
        {
            continue;
        }
        let name = bio.atom_name(index).unwrap();
        assert!(
            db.typing
                .contains_key(&(prep.native_residue.clone(), name.to_string())),
            "{}.{name} must resolve to a CHARMM typing (no orphan heavy atom)",
            prep.native_residue
        );
    }
}

fn no_modifying_residues_remain(prep: &PtmPreparation) {
    let bio = prep.structure.biopolymer.as_ref().unwrap();
    assert_eq!(
        bio.residues.len(),
        1,
        "modifying residue records folded away"
    );
    // Every atom maps to the single surviving residue.
    assert!(
        bio.residue_for_atom.iter().all(|r| *r == Some(0)),
        "every atom belongs to the folded residue"
    );
}

fn junction_bond_present(prep: &PtmPreparation, anchor: &str, partner: &str) {
    let bio = prep.structure.biopolymer.as_ref().unwrap();
    let present = prep.structure.bonds.iter().any(|bond| {
        let a = bio.atom_name(bond.a);
        let b = bio.atom_name(bond.b);
        (a == Some(anchor) && b == Some(partner)) || (a == Some(partner) && b == Some(anchor))
    });
    assert!(present, "junction bond {anchor}-{partner} must be present");
}

#[test]
fn phospho_serine_renames_to_sep_and_is_md_ready() {
    let protein = phosphorylate_protein(&ser(), target(), ProteinAnchor::SerOg).unwrap();
    let db = database();
    let prep = prepare_ptm_residue_with(
        &protein,
        target(),
        PtmKind::Phosphoryl,
        ProteinAnchor::SerOg,
        &db,
    )
    .expect("phospho-Ser prepares");

    assert_eq!(prep.native_residue, "SEP");
    assert_eq!(folded_residue_name(&prep), "SEP");
    assert_eq!(prep.net_charge, -1, "phospho-Ser is monoanionic");
    heavy_atoms_resolve(&prep, &db);
    no_modifying_residues_remain(&prep);
    junction_bond_present(&prep, "OG", "P");
}

#[test]
fn acetyl_lysine_renames_to_aly_and_is_md_ready() {
    let protein = acetylate_protein(&lys(), target(), false).unwrap();
    let db = database();
    let prep = prepare_ptm_residue_with(
        &protein,
        target(),
        PtmKind::Acetyl { n_terminal: false },
        ProteinAnchor::LysNz,
        &db,
    )
    .expect("acetyl-Lys prepares");

    assert_eq!(prep.native_residue, "ALY");
    assert_eq!(prep.net_charge, 0, "acetyl-Lys is neutral");
    heavy_atoms_resolve(&prep, &db);
    no_modifying_residues_remain(&prep);
    junction_bond_present(&prep, "NZ", "CH");
}

#[test]
fn trimethyl_lysine_renames_to_m3l_and_is_md_ready() {
    let protein =
        methylate_protein(&lys(), target(), ProteinAnchor::LysNz, MethylDegree::Tri).unwrap();
    let db = database();
    let prep = prepare_ptm_residue_with(
        &protein,
        target(),
        PtmKind::Methyl {
            degree: MethylDegree::Tri,
        },
        ProteinAnchor::LysNz,
        &db,
    )
    .expect("trimethyl-Lys prepares");

    assert_eq!(prep.native_residue, "M3L");
    assert_eq!(prep.net_charge, 1, "trimethyl-Lys is cationic");
    heavy_atoms_resolve(&prep, &db);
    no_modifying_residues_remain(&prep);
    // All three methyl carbons fold in with distinct rtp names.
    let bio = prep.structure.biopolymer.as_ref().unwrap();
    for methyl in ["CM1", "CM2", "CM3"] {
        assert!(
            (0..prep.structure.atoms.len()).any(|i| bio.atom_name(i) == Some(methyl)),
            "methyl carbon {methyl} must be present after fold"
        );
    }
}

#[test]
fn monomethyl_lysine_renames_to_mlz() {
    let protein =
        methylate_protein(&lys(), target(), ProteinAnchor::LysNz, MethylDegree::Mono).unwrap();
    let db = database();
    let prep = prepare_ptm_residue_with(
        &protein,
        target(),
        PtmKind::Methyl {
            degree: MethylDegree::Mono,
        },
        ProteinAnchor::LysNz,
        &db,
    )
    .expect("monomethyl-Lys prepares");
    assert_eq!(prep.native_residue, "MLZ");
    assert_eq!(prep.net_charge, 1);
    heavy_atoms_resolve(&prep, &db);
    junction_bond_present(&prep, "NZ", "CM");
}

#[test]
fn deferred_lipidation_gates_with_requires_force_field_assets() {
    use crate::domain::modification::AcylKind;
    let db = database();
    // A cysteine carrying nothing; the gate fires from the kind/anchor mapping
    // before any structural work, so a bare residue is enough.
    let err = prepare_ptm_residue_with(
        &lys(),
        target(),
        PtmKind::Acyl(AcylKind::Palmitoyl),
        ProteinAnchor::CysSg,
        &db,
    )
    .unwrap_err();
    assert!(
        err.to_string().contains("requires force-field assets"),
        "deferred PTM must gate clearly, got {err}"
    );
}
