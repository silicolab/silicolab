use anyhow::{Result, anyhow, bail};
use nalgebra::Vector3;

use crate::domain::glycan::{self, Anomer, ProteinAnchor};
use crate::domain::{Biopolymer, BondType, ResidueId, Structure};
use crate::engines::forcefield;
use crate::workflows::assembly::condense::{self, AcceptorSpec, DonorSpec};

use super::builder::tree_to_structure;

pub use crate::domain::glycan::GlycosylationKind;

/// A glycosylated protein, together with what the anchor residue and the
/// notation resolved to.
#[derive(Debug, Clone)]
pub struct Glycosylation {
    pub structure: Structure,
    /// The junction the anchor residue forms.
    pub kind: GlycosylationKind,
    /// Canonical notation for the glycan as built, every anomer stated.
    pub notation: String,
}

/// The junction an anchor residue forms, and the side-chain atom the glycan's
/// reducing end condenses onto.
///
/// The residue alone fixes both: Asn offers an amide nitrogen (ND2), Ser and Thr
/// a hydroxyl oxygen (OG/OG1), and no residue offers each. The glycan plays no
/// part — a given sugar can sit on either junction.
fn anchor_for_residue(residue_name: &str) -> Option<(GlycosylationKind, ProteinAnchor)> {
    match residue_name {
        "ASN" => Some((GlycosylationKind::NLinked, ProteinAnchor::AsnNd2)),
        "SER" => Some((GlycosylationKind::OLinked, ProteinAnchor::SerOg)),
        "THR" => Some((GlycosylationKind::OLinked, ProteinAnchor::ThrOg1)),
        _ => None,
    }
}

/// Which junction a residue forms, for callers that want to show it before
/// committing to a modification.
pub fn glycosylation_kind_for_residue(residue_name: &str) -> Option<GlycosylationKind> {
    anchor_for_residue(residue_name.trim()).map(|(kind, _)| kind)
}

/// Attach a glycan to `anchor`.
///
/// `requested_kind` is an assertion, not an input: the junction is derived from
/// the anchor residue, and a request that disagrees with it is an error. Pass
/// `None` to accept the derivation. `root_anomer` likewise overrides the
/// reducing end's configuration, which is otherwise derived from the junction
/// and the reducing sugar.
pub fn glycosylate_protein(
    protein: &Structure,
    glycan_notation: &str,
    anchor: ResidueId,
    requested_kind: Option<GlycosylationKind>,
    root_anomer: Option<Anomer>,
) -> Result<Glycosylation> {
    let protein_bio = protein
        .biopolymer
        .as_ref()
        .filter(|bio| bio.is_compatible_with_atom_count(protein.atoms.len()))
        .ok_or_else(|| anyhow!("protein has no biopolymer overlay"))?;

    let anchor_residue_index = protein_bio
        .residues
        .iter()
        .position(|residue| residue.id == anchor)
        .ok_or_else(|| anyhow!("anchor residue not found at {anchor:?}"))?;
    let anchor_residue_name = protein_bio.residues[anchor_residue_index]
        .residue_name
        .trim()
        .to_string();

    let (kind, anchor_site) = anchor_for_residue(&anchor_residue_name).ok_or_else(|| {
        anyhow!(
            "{anchor_residue_name} is not a glycosylation anchor residue (expected Asn, Ser or Thr)"
        )
    })?;
    if let Some(requested) = requested_kind
        && requested != kind
    {
        bail!(
            "{anchor_residue_name} is an {} anchor, but {} glycosylation was requested",
            kind.name(),
            requested.name()
        );
    }

    let anchor_atom_name = anchor_site.atom_name();
    let anchor_atom = atom_in_residue(protein_bio, anchor_residue_index, anchor_atom_name)
        .ok_or_else(|| anyhow!("anchor residue is missing atom {anchor_atom_name}"))?;
    let anchor_hydrogen = anchor_hydrogen_atom(protein_bio, anchor_residue_index, kind);
    let anchor_outward =
        anchor_outward_direction(protein, protein_bio, anchor_residue_index, anchor_atom);

    // The reducing end's configuration is fixed by the aglycon it condenses onto,
    // so it is settled here rather than by the notation alone.
    let mut tree = glycan::parse(glycan_notation)?;
    glycan::resolve_root_anomer(&mut tree, Some(kind), root_anomer)?;
    let notation = glycan::to_iupac(&tree);
    let glycan = tree_to_structure(&tree, glycan_notation.to_string())?;
    let glycan_bio = glycan
        .biopolymer
        .as_ref()
        .ok_or_else(|| anyhow!("glycan has no biopolymer overlay"))?;

    let reducing_c1 = reducing_end_anomeric_carbon(&glycan, glycan_bio)
        .ok_or_else(|| anyhow!("glycan has no reducing-end anomeric carbon"))?;
    let leaving_oxygen = glycan_atom_by_name(&glycan, glycan_bio, reducing_c1, "O1");
    let leaving_hydrogen = glycan_atom_by_name(&glycan, glycan_bio, reducing_c1, "HO1");
    let donor_outward = donor_outward_direction(&glycan, reducing_c1, leaving_oxygen);

    let anchor_element = match kind {
        GlycosylationKind::NLinked => "N",
        GlycosylationKind::OLinked => "O",
    };
    let bond_length =
        forcefield::equilibrium_bond_length("C", anchor_element, BondType::Single).unwrap_or(1.45);

    let acceptor = AcceptorSpec {
        anchor_atom,
        remove: anchor_hydrogen.into_iter().collect(),
        outward: anchor_outward,
    };
    let donor = DonorSpec {
        donor_atom: reducing_c1,
        remove: [leaving_oxygen, leaving_hydrogen]
            .into_iter()
            .flatten()
            .collect(),
        outward: donor_outward,
    };

    let structure = condense::attach_fragment(
        protein,
        acceptor,
        &glycan,
        donor,
        bond_length,
        BondType::Single,
        "glycan",
    )?;

    Ok(Glycosylation {
        structure,
        kind,
        notation,
    })
}

fn atom_in_residue(
    biopolymer: &Biopolymer,
    residue_index: usize,
    atom_name: &str,
) -> Option<usize> {
    let residue = biopolymer.residues.get(residue_index)?;
    residue
        .atom_indices
        .iter()
        .copied()
        .find(|&index| biopolymer.atom_name(index) == Some(atom_name))
}

fn anchor_hydrogen_atom(
    biopolymer: &Biopolymer,
    residue_index: usize,
    kind: GlycosylationKind,
) -> Option<usize> {
    let candidates: &[&str] = match kind {
        GlycosylationKind::NLinked => &["HD21", "HD22", "1HD2", "2HD2"],
        GlycosylationKind::OLinked => &["HG", "HG1", "HO", "HOG", "HOG1"],
    };
    candidates
        .iter()
        .find_map(|name| atom_in_residue(biopolymer, residue_index, name))
}

fn anchor_outward_direction(
    structure: &Structure,
    biopolymer: &Biopolymer,
    residue_index: usize,
    anchor_atom: usize,
) -> Vector3<f32> {
    let neighbor = ["CG", "CB", "CA"]
        .iter()
        .find_map(|name| atom_in_residue(biopolymer, residue_index, name));
    match neighbor {
        Some(carbon) => (structure.atoms[anchor_atom].position - structure.atoms[carbon].position)
            .try_normalize(1.0e-4)
            .unwrap_or_else(Vector3::z),
        None => Vector3::z(),
    }
}

fn reducing_end_anomeric_carbon(structure: &Structure, biopolymer: &Biopolymer) -> Option<usize> {
    let root_index = biopolymer.residues.iter().position(|residue| {
        crate::domain::biopolymer::is_carbohydrate_residue(&residue.residue_name)
    })?;
    let root = &biopolymer.residues[root_index];
    root.atom_indices.iter().copied().find(|&index| {
        index < structure.atoms.len()
            && biopolymer
                .atom_name(index)
                .map(glycan::is_anomeric_carbon)
                .unwrap_or(false)
    })
}

fn glycan_atom_by_name(
    structure: &Structure,
    biopolymer: &Biopolymer,
    anomeric_carbon: usize,
    name: &str,
) -> Option<usize> {
    let residue_index = (*biopolymer.residue_for_atom.get(anomeric_carbon)?)?;
    let residue = biopolymer.residues.get(residue_index)?;
    residue
        .atom_indices
        .iter()
        .copied()
        .find(|&index| index < structure.atoms.len() && biopolymer.atom_name(index) == Some(name))
}

fn donor_outward_direction(
    structure: &Structure,
    anomeric_carbon: usize,
    leaving_oxygen: Option<usize>,
) -> Vector3<f32> {
    match leaving_oxygen {
        Some(oxygen) => (structure.atoms[oxygen].position
            - structure.atoms[anomeric_carbon].position)
            .try_normalize(1.0e-4)
            .unwrap_or_else(Vector3::z),
        None => Vector3::z(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::glycan::{Aglycon, infer_attachment};
    use crate::domain::{Atom, AtomCategory, Bond, ChainRecord, ResidueRecord};
    use crate::workflows::glycan::builder::glycan_to_structure;
    use nalgebra::Point3;

    fn atom(element: &str, x: f32, y: f32, z: f32) -> Atom {
        Atom {
            element: element.to_string(),
            position: Point3::new(x, y, z),
            charge: 0.0,
        }
    }

    fn asn_structure() -> Structure {
        let atoms = vec![
            atom("N", 0.0, 0.0, 0.0),
            atom("C", 1.45, 0.0, 0.0),
            atom("C", 2.9, 0.0, 0.0),
            atom("O", 3.6, 1.0, 0.0),
            atom("C", 3.6, -1.2, 0.0),
            atom("O", 3.0, -2.3, 0.0),
            atom("N", 4.9, -1.2, 0.0),
            atom("H", 5.4, -2.0, 0.0),
        ];
        let names = ["N", "CA", "CB", "O", "CG", "OD1", "ND2", "HD21"];
        let bonds = vec![
            Bond::with_type(0, 1, BondType::Single),
            Bond::with_type(1, 2, BondType::Single),
            Bond::with_type(1, 3, BondType::Single),
            Bond::with_type(2, 4, BondType::Single),
            Bond::with_type(4, 5, BondType::Single),
            Bond::with_type(4, 6, BondType::Single),
            Bond::with_type(6, 7, BondType::Single),
        ];
        let residue = ResidueRecord {
            id: ResidueId::new('A', 1, ' '),
            residue_name: "ASN".to_string(),
            atom_indices: (0..atoms.len()).collect(),
            alpha_carbon: Some(1),
            backbone_nitrogen: Some(0),
            backbone_carbon: Some(2),
            backbone_oxygen: Some(3),
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
            atom_name_for_atom: names.iter().map(|n| Some(n.to_string())).collect(),
        };
        let mut structure = Structure::with_bonds("asn".to_string(), atoms, bonds);
        structure.biopolymer = Some(biopolymer);
        structure
    }

    fn ser_structure() -> Structure {
        let atoms = vec![
            atom("N", 0.0, 0.0, 0.0),
            atom("C", 1.45, 0.0, 0.0),
            atom("C", 2.9, 0.0, 0.0),
            atom("O", 3.6, 1.0, 0.0),
            atom("O", 3.6, -1.2, 0.0),
            atom("H", 4.5, -1.2, 0.0),
        ];
        let names = ["N", "CA", "CB", "O", "OG", "HG"];
        let bonds = vec![
            Bond::with_type(0, 1, BondType::Single),
            Bond::with_type(1, 2, BondType::Single),
            Bond::with_type(1, 3, BondType::Single),
            Bond::with_type(2, 4, BondType::Single),
            Bond::with_type(4, 5, BondType::Single),
        ];
        let residue = ResidueRecord {
            id: ResidueId::new('A', 1, ' '),
            residue_name: "SER".to_string(),
            atom_indices: (0..atoms.len()).collect(),
            alpha_carbon: Some(1),
            backbone_nitrogen: Some(0),
            backbone_carbon: Some(2),
            backbone_oxygen: Some(3),
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
            atom_name_for_atom: names.iter().map(|n| Some(n.to_string())).collect(),
        };
        let mut structure = Structure::with_bonds("ser".to_string(), atoms, bonds);
        structure.biopolymer = Some(biopolymer);
        structure
    }

    fn junction_present(structure: &Structure) -> bool {
        let bio = structure.biopolymer.as_ref().unwrap();
        structure.bonds.iter().any(|bond| {
            let na = bio.atom_name(bond.a);
            let nb = bio.atom_name(bond.b);
            (na == Some("ND2") && nb == Some("C1")) || (na == Some("C1") && nb == Some("ND2"))
        })
    }

    fn o_junction_present(structure: &Structure) -> bool {
        let bio = structure.biopolymer.as_ref().unwrap();
        structure.bonds.iter().any(|bond| {
            let na = bio.atom_name(bond.a);
            let nb = bio.atom_name(bond.b);
            (na == Some("OG") && nb == Some("C1")) || (na == Some("C1") && nb == Some("OG"))
        })
    }

    #[test]
    fn glycosylates_asn_with_glcnac() {
        let protein = asn_structure();
        let glycan = glycan_to_structure("GlcNAc", None).expect("glycan");
        let glycan_atom_count = glycan.atoms.len();

        let glycosylation =
            glycosylate_protein(&protein, "GlcNAc", ResidueId::new('A', 1, ' '), None, None)
                .expect("glycosylation");
        assert_eq!(
            glycosylation.kind,
            GlycosylationKind::NLinked,
            "Asn derives an N-linked junction"
        );
        let result = glycosylation.structure;

        assert_eq!(
            result.atoms.len(),
            protein.atoms.len() - 1 + glycan_atom_count - 2
        );
        assert!(junction_present(&result), "ND2-C1 junction bond present");

        let carbohydrate = (0..result.atoms.len())
            .filter(|&i| result.atom_category(i) == AtomCategory::Carbohydrate)
            .count();
        assert!(carbohydrate > 0, "glycan atoms classify Carbohydrate");

        let attachment = infer_attachment(&result).expect("attachment inferred");
        match attachment {
            Aglycon::NLinked { asn, anchor_atom } => {
                assert_eq!(asn, ResidueId::new('A', 1, ' '));
                assert_eq!(anchor_atom, "ND2");
            }
            other => panic!("expected N-linked, got {other:?}"),
        }
    }

    #[test]
    fn glycosylates_ser_with_glcnac_o_linked() {
        let protein = ser_structure();
        let glycan = glycan_to_structure("GlcNAc", None).expect("glycan");
        let glycan_atom_count = glycan.atoms.len();

        let glycosylation =
            glycosylate_protein(&protein, "GlcNAc", ResidueId::new('A', 1, ' '), None, None)
                .expect("glycosylation");
        assert_eq!(
            glycosylation.kind,
            GlycosylationKind::OLinked,
            "Ser derives an O-linked junction"
        );
        let result = glycosylation.structure;

        assert_eq!(
            result.atoms.len(),
            protein.atoms.len() - 1 + glycan_atom_count - 2
        );
        assert!(o_junction_present(&result), "OG-C1 junction bond present");

        let attachment = infer_attachment(&result).expect("attachment inferred");
        match attachment {
            Aglycon::OLinked {
                ser_thr,
                anchor_atom,
            } => {
                assert_eq!(ser_thr, ResidueId::new('A', 1, ' '));
                assert_eq!(anchor_atom, "OG");
            }
            other => panic!("expected O-linked, got {other:?}"),
        }
    }

    /// Mucin-type O-glycosylation is alpha-GalNAc. Nobody states that: the anchor
    /// residue implies the junction, and the junction implies the configuration.
    #[test]
    fn o_linked_galnac_is_built_alpha() {
        let result = glycosylate_protein(
            &ser_structure(),
            "GalNAc",
            ResidueId::new('A', 1, ' '),
            None,
            None,
        )
        .expect("glycosylation");

        assert_eq!(result.notation, "GalNAc(a1-O)");
        let bio = result.structure.biopolymer.as_ref().unwrap();
        assert!(
            bio.residues.iter().any(|r| r.residue_name == "A2G"),
            "expected alpha-GalNAc (A2G), got {:?}",
            bio.residues
                .iter()
                .map(|r| &r.residue_name)
                .collect::<Vec<_>>()
        );
    }

    /// The same sugar attached to Asn would be beta, and is refused outright:
    /// N-glycans are GlcNAc-linked.
    #[test]
    fn n_linked_refuses_a_galnac_reducing_end() {
        let err = glycosylate_protein(
            &asn_structure(),
            "GalNAc",
            ResidueId::new('A', 1, ' '),
            None,
            None,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("GlcNAc"), "{err}");
    }

    /// The anchor residue alone fixes the junction — the glycan has no say.
    #[test]
    fn the_junction_is_derived_from_the_anchor_residue() {
        assert_eq!(
            glycosylation_kind_for_residue("ASN"),
            Some(GlycosylationKind::NLinked)
        );
        assert_eq!(
            glycosylation_kind_for_residue("SER"),
            Some(GlycosylationKind::OLinked)
        );
        assert_eq!(
            glycosylation_kind_for_residue("THR"),
            Some(GlycosylationKind::OLinked)
        );
        assert_eq!(glycosylation_kind_for_residue("ALA"), None);
    }

    /// A stated kind is an assertion against the residue, not an input that can
    /// steer it.
    #[test]
    fn a_kind_contradicting_the_anchor_residue_is_refused() {
        let err = glycosylate_protein(
            &ser_structure(),
            "GlcNAc",
            ResidueId::new('A', 1, ' '),
            Some(GlycosylationKind::NLinked),
            None,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("SER"), "{err}");
        assert!(
            err.contains("O-linked") && err.contains("N-linked"),
            "{err}"
        );

        // Agreeing with the residue is accepted.
        assert!(
            glycosylate_protein(
                &ser_structure(),
                "GlcNAc",
                ResidueId::new('A', 1, ' '),
                Some(GlycosylationKind::OLinked),
                None,
            )
            .is_ok()
        );
    }

    #[test]
    fn a_residue_that_cannot_carry_a_glycan_is_refused() {
        let mut protein = ser_structure();
        let bio = protein.biopolymer.as_mut().unwrap();
        bio.residues[0].residue_name = "ALA".to_string();

        let err = glycosylate_protein(&protein, "GlcNAc", ResidueId::new('A', 1, ' '), None, None)
            .unwrap_err()
            .to_string();
        assert!(err.contains("not a glycosylation anchor"), "{err}");
    }

    /// The override replaces the derivation, so an alpha N-linked GlcNAc — which
    /// the canonical table would never produce — is buildable when asked for.
    #[test]
    fn an_explicit_root_anomer_overrides_the_derivation() {
        let derived = glycosylate_protein(
            &asn_structure(),
            "GlcNAc",
            ResidueId::new('A', 1, ' '),
            None,
            None,
        )
        .expect("derived");
        let forced = glycosylate_protein(
            &asn_structure(),
            "GlcNAc",
            ResidueId::new('A', 1, ' '),
            None,
            Some(Anomer::Alpha),
        )
        .expect("forced");

        let ccd = |s: &Structure| -> Vec<String> {
            s.biopolymer
                .as_ref()
                .unwrap()
                .residues
                .iter()
                .filter(|r| !r.is_standard_amino_acid)
                .map(|r| r.residue_name.clone())
                .collect()
        };
        assert_eq!(
            ccd(&derived.structure),
            vec!["NAG"],
            "beta-GlcNAc by derivation"
        );
        assert_eq!(
            ccd(&forced.structure),
            vec!["NDG"],
            "alpha-GlcNAc by override"
        );
    }

    /// A reducing-end configuration that contradicts the aglycon is an error, not
    /// a silent override.
    #[test]
    fn a_contradicting_reducing_anomer_is_refused() {
        let err = glycosylate_protein(
            &asn_structure(),
            "aGlcNAc",
            ResidueId::new('A', 1, ' '),
            None,
            None,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("beta") && err.contains("alpha"), "{err}");
    }

    #[test]
    fn junction_round_trips_through_pdb() {
        let protein = asn_structure();
        let result =
            glycosylate_protein(&protein, "GlcNAc", ResidueId::new('A', 1, ' '), None, None)
                .expect("glycosylation")
                .structure;

        let serialized = crate::io::formats::pdb::to_pdb(&result).expect("serialize glycoprotein");
        assert!(
            serialized.lines().any(|line| line.starts_with("LINK")),
            "to_pdb emits LINK for the junction"
        );

        let reparsed =
            crate::io::formats::pdb::parse_pdb(&serialized).expect("reparse glycoprotein");
        assert!(
            junction_present(&reparsed),
            "ND2-C1 junction survives the PDB round trip"
        );
    }
}
