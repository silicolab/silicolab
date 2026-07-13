use super::*;

use std::borrow::Cow;
use std::f32::consts::PI;

use nalgebra::Point3;

use crate::domain::{
    Atom, PdbAtomAnnotation, ResidueId, SecondaryStructureKind, SecondaryStructureSpan, Structure,
    build_biopolymer,
};

/// Single-chain structure with one ALA residue per residue along an ideal
/// α-helix Cα trace (1.5 Å rise, 100° turn, 2.3 Å radius) and no HELIX/SHEET
/// records. Each residue carries a full N/CA/C backbone: the amide N and
/// carbonyl C sit on the Cα–Cα midpoints, so consecutive C(i)/N(i+1) coincide —
/// a zero-length peptide "bond" that keeps the ribbon one continuous fragment.
/// Only the Cα positions carry the helix geometry the ribbon sweep reads.
fn helix_structure(residues: usize) -> Structure {
    let ca: Vec<Point3<f32>> = (0..residues)
        .map(|i| {
            let angle = 100.0 * PI / 180.0 * i as f32;
            Point3::new(2.3 * angle.cos(), 2.3 * angle.sin(), 1.5 * i as f32)
        })
        .collect();

    let mut atoms = Vec::with_capacity(residues * 3);
    let mut annotations = Vec::with_capacity(residues * 3);
    for i in 0..residues {
        let previous = if i > 0 { ca[i - 1] } else { ca[i] };
        let next = if i + 1 < residues { ca[i + 1] } else { ca[i] };
        let nitrogen = Point3::from((previous.coords + ca[i].coords) * 0.5);
        let carbon = Point3::from((ca[i].coords + next.coords) * 0.5);
        for (atom_name, element, position) in
            [("N", "N", nitrogen), ("CA", "C", ca[i]), ("C", "C", carbon)]
        {
            atoms.push(Atom {
                element: element.to_string(),
                position,
                charge: 0.0,
            });
            annotations.push(PdbAtomAnnotation {
                atom_name: atom_name.to_string(),
                residue_name: "ALA".to_string(),
                chain_id: 'A',
                residue_seq: i as i32 + 1,
                insertion_code: ' ',
            });
        }
    }
    let mut structure = Structure::with_bonds("helix", atoms, Vec::new());
    structure.biopolymer = build_biopolymer(&annotations, Vec::new());
    structure
}

#[test]
fn cartoon_derives_secondary_structure_when_records_absent() {
    let structure = helix_structure(12);
    let biopolymer = structure.biopolymer.as_ref().expect("biopolymer");
    assert!(biopolymer.secondary_structures.is_empty());

    let resolved = resolve_secondary_structures(&structure, biopolymer);
    let chain_id = biopolymer.chains[0].id;
    let helix_residues = biopolymer
        .residues
        .iter()
        .filter(|residue| {
            residue_cartoon_kind(residue, resolved.as_ref(), chain_id) == CartoonSegmentKind::Helix
        })
        .count();
    assert!(
        helix_residues >= 8,
        "expected the helix to be drawn as helix ribbon"
    );
}

#[test]
fn cartoon_prefers_explicit_secondary_structure() {
    let mut structure = helix_structure(12);
    let biopolymer = structure.biopolymer.as_mut().expect("biopolymer");
    biopolymer.secondary_structures = vec![SecondaryStructureSpan {
        kind: SecondaryStructureKind::Sheet,
        start: ResidueId::new('A', 1, ' '),
        end: ResidueId::new('A', 12, ' '),
    }];

    let biopolymer = structure.biopolymer.as_ref().expect("biopolymer");
    let resolved = resolve_secondary_structures(&structure, biopolymer);
    // Helical geometry, but the explicit sheet record is used verbatim.
    assert!(matches!(resolved, Cow::Borrowed(_)));
    assert!(
        resolved
            .iter()
            .all(|span| span.kind == SecondaryStructureKind::Sheet)
    );
}
