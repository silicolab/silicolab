//! Resolving the host-side attachment point for a PTM: the anchor heavy atom on
//! a residue (or chain terminus), the hydrogen the incoming group displaces, and
//! the outward bond direction. Produces the [`AcceptorSpec`] the shared condense
//! path consumes, mirroring how `glycoprotein` resolves a glycosylation anchor.

use anyhow::{Result, anyhow};
use nalgebra::Vector3;

use crate::domain::biopolymer::Biopolymer;
use crate::domain::{ProteinAnchor, ResidueId, Structure};
use crate::workflows::assembly::condense::AcceptorSpec;

/// Resolve a side-chain anchor: locate the anchor heavy atom on `residue`, the
/// hydrogen bonded to it (displaced by the modifying group), and the outward
/// direction from the bonded heavy neighbor.
pub fn resolve_acceptor(
    protein: &Structure,
    residue: ResidueId,
    anchor: ProteinAnchor,
) -> Result<AcceptorSpec> {
    let bio = biopolymer(protein)?;
    let residue_index = bio
        .residues
        .iter()
        .position(|record| record.id == residue)
        .ok_or_else(|| anyhow!("anchor residue {residue:?} not found"))?;
    let anchor_atom = atom_in_residue(bio, residue_index, anchor.atom_name())
        .ok_or_else(|| anyhow!("residue is missing anchor atom {}", anchor.atom_name()))?;
    let hydrogen = bonded_hydrogen(protein, anchor_atom).ok_or_else(|| {
        anyhow!(
            "anchor atom {} has no hydrogen to displace",
            anchor.atom_name()
        )
    })?;
    Ok(AcceptorSpec {
        anchor_atom,
        remove: vec![hydrogen],
        outward: outward_from_heavy_neighbor(protein, anchor_atom),
    })
}

/// Resolve the N-terminus targeted by `residue` — the alpha-amino N of its
/// chain's first residue and one of its hydrogens. `residue` must itself be that
/// first residue (the terminus atom name `N` is shared by every residue, so the
/// caller names the residue and this confirms it is the chain head).
pub fn resolve_n_terminus(protein: &Structure, target: ResidueId) -> Result<AcceptorSpec> {
    let bio = biopolymer(protein)?;
    let chain_id = target.chain_id;
    let chain = bio
        .chains
        .iter()
        .find(|chain| chain.id == chain_id)
        .ok_or_else(|| anyhow!("chain {chain_id} not found"))?;
    let first = *chain
        .residue_indices
        .first()
        .ok_or_else(|| anyhow!("chain {chain_id} has no residues"))?;
    let residue = &bio.residues[first];
    if residue.id != target {
        return Err(anyhow!(
            "{target:?} is not the N-terminal residue of chain {chain_id}"
        ));
    }
    let nitrogen = residue
        .backbone_nitrogen
        .or_else(|| atom_in_residue(bio, first, "N"))
        .ok_or_else(|| anyhow!("N-terminal residue has no backbone nitrogen"))?;
    let hydrogen = bonded_hydrogen(protein, nitrogen)
        .ok_or_else(|| anyhow!("N-terminus has no amino hydrogen to displace"))?;
    let outward = match residue.alpha_carbon {
        Some(alpha) => (protein.atoms[nitrogen].position - protein.atoms[alpha].position)
            .try_normalize(1.0e-4)
            .unwrap_or_else(Vector3::z),
        None => outward_from_heavy_neighbor(protein, nitrogen),
    };
    Ok(AcceptorSpec {
        anchor_atom: nitrogen,
        remove: vec![hydrogen],
        outward,
    })
}

fn biopolymer(protein: &Structure) -> Result<&Biopolymer> {
    protein
        .biopolymer
        .as_ref()
        .filter(|bio| bio.is_compatible_with_atom_count(protein.atoms.len()))
        .ok_or_else(|| anyhow!("protein has no biopolymer overlay"))
}

fn atom_in_residue(bio: &Biopolymer, residue_index: usize, name: &str) -> Option<usize> {
    bio.residues
        .get(residue_index)?
        .atom_indices
        .iter()
        .copied()
        .find(|&index| bio.atom_name(index) == Some(name))
}

fn neighbors(protein: &Structure, atom: usize) -> impl Iterator<Item = usize> + '_ {
    protein.bonds.iter().filter_map(move |bond| {
        if bond.a == atom {
            Some(bond.b)
        } else if bond.b == atom {
            Some(bond.a)
        } else {
            None
        }
    })
}

fn bonded_hydrogen(protein: &Structure, atom: usize) -> Option<usize> {
    neighbors(protein, atom)
        .find(|&index| protein.atoms.get(index).is_some_and(|a| a.element == "H"))
}

fn outward_from_heavy_neighbor(protein: &Structure, atom: usize) -> Vector3<f32> {
    let heavy = neighbors(protein, atom)
        .find(|&index| protein.atoms.get(index).is_some_and(|a| a.element != "H"));
    match heavy {
        Some(neighbor) => (protein.atoms[atom].position - protein.atoms[neighbor].position)
            .try_normalize(1.0e-4)
            .unwrap_or_else(Vector3::z),
        None => Vector3::z(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Atom, Bond, BondType, ChainRecord, ResidueRecord};
    use nalgebra::Point3;

    fn atom(element: &str, x: f32, y: f32, z: f32) -> Atom {
        Atom {
            element: element.to_string(),
            position: Point3::new(x, y, z),
            charge: 0.0,
        }
    }

    fn ser_structure() -> Structure {
        let atoms = vec![
            atom("N", 0.0, 0.0, 0.0),
            atom("C", 1.45, 0.0, 0.0),
            atom("C", 2.9, 0.0, 0.0),
            atom("O", 3.6, 1.0, 0.0),
            atom("C", 3.6, -1.2, 0.0),
            atom("O", 4.9, -1.2, 0.0),
            atom("H", 5.4, -2.0, 0.0),
            atom("H", -0.6, 0.7, 0.2),
        ];
        let names = ["N", "CA", "C", "O", "CB", "OG", "HG", "H"];
        let bonds = vec![
            Bond::with_type(0, 1, BondType::Single),
            Bond::with_type(1, 2, BondType::Single),
            Bond::with_type(2, 3, BondType::Single),
            Bond::with_type(1, 4, BondType::Single),
            Bond::with_type(4, 5, BondType::Single),
            Bond::with_type(5, 6, BondType::Single),
            Bond::with_type(0, 7, BondType::Single),
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
            atom_name_for_atom: names.iter().map(|name| Some(name.to_string())).collect(),
        };
        let mut structure = Structure::with_bonds("ser".to_string(), atoms, bonds);
        structure.biopolymer = Some(biopolymer);
        structure
    }

    #[test]
    fn resolves_serine_hydroxyl_anchor() {
        let protein = ser_structure();
        let spec = resolve_acceptor(&protein, ResidueId::new('A', 1, ' '), ProteinAnchor::SerOg)
            .expect("anchor resolves");
        let bio = protein.biopolymer.as_ref().unwrap();
        assert_eq!(bio.atom_name(spec.anchor_atom), Some("OG"));
        assert_eq!(spec.remove.len(), 1);
        assert_eq!(bio.atom_name(spec.remove[0]), Some("HG"));
        // Outward points OG away from its CB neighbor.
        assert!(spec.outward.x > 0.5, "outward leaves the side chain");
    }

    #[test]
    fn resolves_n_terminus_by_chain_position() {
        let protein = ser_structure();
        let spec =
            resolve_n_terminus(&protein, ResidueId::new('A', 1, ' ')).expect("terminus resolves");
        let bio = protein.biopolymer.as_ref().unwrap();
        assert_eq!(bio.atom_name(spec.anchor_atom), Some("N"));
        assert_eq!(bio.atom_name(spec.remove[0]), Some("H"));
        assert!(
            spec.outward.x < 0.0,
            "outward points away from the alpha carbon"
        );
    }

    #[test]
    fn n_terminus_rejects_non_head_residue() {
        let protein = ser_structure();
        let result = resolve_n_terminus(&protein, ResidueId::new('A', 2, ' '));
        assert!(
            result.is_err(),
            "only the chain's first residue is the N-terminus"
        );
    }

    #[test]
    fn missing_anchor_atom_errors() {
        let protein = ser_structure();
        let result = resolve_acceptor(&protein, ResidueId::new('A', 1, ' '), ProteinAnchor::TyrOh);
        assert!(result.is_err(), "serine has no tyrosine OH");
    }

    /// The resolver and a fragment must compose through the one shared condense
    /// path: both host and fragment carry a biopolymer overlay, so welding a
    /// phosphate onto Ser-OG forms the OG–P bond and drops the leaving atoms.
    #[test]
    fn phosphate_welds_onto_serine_through_condense() {
        use crate::workflows::assembly::condense::{self, DonorSpec};
        use crate::workflows::ptm::fragments;

        let protein = ser_structure();
        let acceptor =
            resolve_acceptor(&protein, ResidueId::new('A', 1, ' '), ProteinAnchor::SerOg)
                .expect("anchor resolves");
        let fragment = fragments::phosphate();
        let donor = DonorSpec {
            donor_atom: fragment.donor,
            remove: fragment.leaving.clone(),
            outward: fragment.outward,
        };

        let result = condense::attach_fragment(
            &protein,
            acceptor,
            &fragment.structure,
            donor,
            1.6,
            BondType::Single,
            "phospho",
        )
        .expect("condense welds the fragment");

        assert_eq!(
            result.atoms.len(),
            protein.atoms.len() - 1 + fragment.structure.atoms.len() - 2
        );
        let bio = result.biopolymer.as_ref().expect("merged overlay");
        let junction = result.bonds.iter().any(|bond| {
            let (a, b) = (bio.atom_name(bond.a), bio.atom_name(bond.b));
            (a == Some("OG") && b == Some("P")) || (a == Some("P") && b == Some("OG"))
        });
        assert!(junction, "OG–P phosphoester bond formed");
    }

    #[test]
    fn welded_fragment_avoids_host_chain_collision() {
        use crate::workflows::assembly::condense::{self, DonorSpec};
        use crate::workflows::ptm::fragments;

        let protein = ser_structure();
        let acceptor =
            resolve_acceptor(&protein, ResidueId::new('A', 1, ' '), ProteinAnchor::SerOg)
                .expect("anchor resolves");
        let fragment = fragments::phosphate();
        let donor = DonorSpec {
            donor_atom: fragment.donor,
            remove: fragment.leaving.clone(),
            outward: fragment.outward,
        };
        let result = condense::attach_fragment(
            &protein,
            acceptor,
            &fragment.structure,
            donor,
            1.6,
            BondType::Single,
            "phospho",
        )
        .expect("condense welds the fragment");

        let bio = result.biopolymer.as_ref().expect("merged overlay");
        assert_eq!(bio.residues.len(), 2);
        assert_ne!(
            bio.residues[0].id, bio.residues[1].id,
            "fragment must not duplicate the host residue id"
        );
    }
}
