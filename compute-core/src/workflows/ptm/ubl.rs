//! Structural ubiquitination / SUMOylation / NEDDylation: attach a whole
//! ubiquitin-like (UBL) protein to a target lysine through an isopeptide (amide)
//! bond. The UBL — ubiquitin, SUMO1, or NEDD8 — is bundled as a canonical
//! heavy-atom template (cleaned from an RCSB deposition); its C-terminal diGly
//! carbonyl carbon is the donor, the host Lys NZ the acceptor, and the one shared
//! [`condense`](crate::workflows::assembly::condense) path welds them.

use anyhow::{Context, Result, anyhow, bail};
use nalgebra::Vector3;

use crate::domain::biopolymer::{Biopolymer, ResidueRecord};
use crate::domain::modification::UblKind;
use crate::domain::{BondType, ProteinAnchor, ResidueId, Structure};
use crate::engines::forcefield;
use crate::io::formats::pdb::parse_pdb;
use crate::workflows::assembly::condense::{self, DonorSpec};

use super::host;

// Canonical UBL structures, cleaned to a single heavy-atom protein chain (waters,
// ions, hydrogens, and CRYST1/CONECT stripped) and ending in the resolved
// C-terminal diGly. Provenance is recorded in assets/ubl/README plus each file's
// TITLE: ubiquitin 1UBQ chain A, SUMO1 2N1V model 1 chain A (mature form), NEDD8
// 1NDD chain B (the conformer that resolves the Gly-Gly tail with its OXT).
const UBIQUITIN_PDB: &str = include_str!("../../../../assets/ubl/ubiquitin.pdb");
const SUMO1_PDB: &str = include_str!("../../../../assets/ubl/sumo1.pdb");
const NEDD8_PDB: &str = include_str!("../../../../assets/ubl/nedd8.pdb");

/// Short label for a UBL kind, used in titles and error context.
fn ubl_label(kind: UblKind) -> &'static str {
    match kind {
        UblKind::Ubiquitin => "ubiquitin",
        UblKind::Sumo => "sumo",
        UblKind::Nedd8 => "nedd8",
    }
}

/// The bundled, canonical template for a UBL kind, parsed into a [`Structure`]
/// with a biopolymer overlay (residue/chain/atom-name records the condense path
/// requires).
pub fn ubl_template(kind: UblKind) -> Result<Structure> {
    let text = match kind {
        UblKind::Ubiquitin => UBIQUITIN_PDB,
        UblKind::Sumo => SUMO1_PDB,
        UblKind::Nedd8 => NEDD8_PDB,
    };
    let structure =
        parse_pdb(text).with_context(|| format!("parsing bundled {} template", ubl_label(kind)))?;
    if structure.biopolymer.is_none() {
        bail!(
            "bundled {} template has no biopolymer overlay",
            ubl_label(kind)
        );
    }
    Ok(structure)
}

/// Conjugate a UBL onto `residue`'s Lys NZ via an isopeptide bond: the UBL's
/// C-terminal Gly carbonyl carbon bonds to the lysine ε-amino nitrogen, the
/// C-terminal hydroxyl (OXT) and one NZ hydrogen leaving as the amide forms.
/// `ubl_override` supplies a caller-provided UBL structure in place of the
/// bundled template (still typed by `ubl` for labeling).
pub fn ubiquitinate_protein(
    protein: &Structure,
    residue: ResidueId,
    ubl: UblKind,
    ubl_override: Option<&Structure>,
) -> Result<Structure> {
    let bundled;
    let template = match ubl_override {
        Some(structure) => structure,
        None => {
            bundled = ubl_template(ubl)?;
            &bundled
        }
    };

    let donor = ubl_c_terminus_donor(template)?;
    let acceptor = host::resolve_acceptor(protein, residue, ProteinAnchor::LysNz)?;
    let bond_length =
        forcefield::equilibrium_bond_length("N", "C", BondType::Single).unwrap_or(1.34);

    condense::attach_fragment(
        protein,
        acceptor,
        template,
        donor,
        bond_length,
        BondType::Single,
        ubl_label(ubl),
    )
}

/// Build the donor spec for a UBL's C-terminus: its last residue's backbone
/// carbonyl carbon, the terminal hydroxyl (OXT plus any hydrogen on it) that
/// leaves on amide formation, and the outward direction from carbon toward OXT.
fn ubl_c_terminus_donor(template: &Structure) -> Result<DonorSpec> {
    let bio = template
        .biopolymer
        .as_ref()
        .filter(|bio| bio.is_compatible_with_atom_count(template.atoms.len()))
        .ok_or_else(|| anyhow!("UBL template has no biopolymer overlay"))?;

    let last_chain = bio
        .chains
        .last()
        .ok_or_else(|| anyhow!("UBL template has no chains"))?;
    let &residue_index = last_chain
        .residue_indices
        .last()
        .ok_or_else(|| anyhow!("UBL template chain has no residues"))?;
    let residue = &bio.residues[residue_index];

    let carbonyl = residue
        .backbone_carbon
        .or_else(|| atom_named(bio, residue, "C"))
        .ok_or_else(|| anyhow!("UBL C-terminal residue has no carbonyl carbon"))?;
    let oxt = atom_named(bio, residue, "OXT")
        .ok_or_else(|| anyhow!("UBL C-terminus has no OXT hydroxyl to displace"))?;

    let mut remove = vec![oxt];
    if let Some(hydrogen) = bonded_hydrogen(template, oxt) {
        remove.push(hydrogen);
    }

    let outward = (template.atoms[oxt].position - template.atoms[carbonyl].position)
        .try_normalize(1.0e-4)
        .unwrap_or_else(Vector3::z);

    Ok(DonorSpec {
        donor_atom: carbonyl,
        remove,
        outward,
    })
}

fn atom_named(bio: &Biopolymer, residue: &ResidueRecord, name: &str) -> Option<usize> {
    residue
        .atom_indices
        .iter()
        .copied()
        .find(|&index| bio.atom_name(index) == Some(name))
}

fn bonded_hydrogen(structure: &Structure, atom: usize) -> Option<usize> {
    structure.bonds.iter().find_map(|bond| {
        let other = if bond.a == atom {
            bond.b
        } else if bond.b == atom {
            bond.a
        } else {
            return None;
        };
        structure
            .atoms
            .get(other)
            .filter(|a| a.element == "H")
            .map(|_| other)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflows::ptm::testkit::{self, sidechain_backbone, single_residue};

    fn lysine() -> Structure {
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
            sidechain_backbone(),
        )
    }

    fn target() -> ResidueId {
        ResidueId::new('A', 1, ' ')
    }

    #[test]
    fn templates_load_with_expected_residue_counts() {
        for (kind, residues) in [
            (UblKind::Ubiquitin, 76usize),
            (UblKind::Sumo, 97),
            (UblKind::Nedd8, 76),
        ] {
            let template = ubl_template(kind).expect("template loads");
            let bio = template.biopolymer.as_ref().expect("overlay");
            assert_eq!(
                bio.residues.len(),
                residues,
                "{} residue count",
                ubl_label(kind)
            );
            // The C-terminal residue is the attaching diGly, carbonyl + OXT intact.
            let donor = ubl_c_terminus_donor(&template).expect("C-terminus resolves");
            assert_eq!(template.atoms[donor.donor_atom].element, "C");
        }
    }

    #[test]
    fn ubiquitinates_lysine_through_isopeptide_bond() {
        let protein = lysine();
        let template = ubl_template(UblKind::Ubiquitin).expect("template");
        let donor = ubl_c_terminus_donor(&template).expect("donor");

        let result = ubiquitinate_protein(&protein, target(), UblKind::Ubiquitin, None)
            .expect("conjugation");

        // host loses one NZ hydrogen; UBL loses its leaving group (OXT [+ H]).
        assert_eq!(
            result.atoms.len(),
            protein.atoms.len() - 1 + template.atoms.len() - donor.remove.len()
        );
        assert!(
            testkit::junction(&result, "NZ", "C"),
            "isopeptide NZ–C bond formed"
        );
        assert!(
            testkit::residue_has_atom(&result, target(), "NZ"),
            "Lys NZ intact"
        );
    }

    #[test]
    fn host_and_ubl_chains_get_distinct_ids() {
        let protein = lysine();
        let result = ubiquitinate_protein(&protein, target(), UblKind::Ubiquitin, None)
            .expect("conjugation");
        let bio = result.biopolymer.as_ref().expect("overlay");

        let ids: Vec<char> = bio.chains.iter().map(|chain| chain.id).collect();
        assert!(ids.len() >= 2, "host and UBL contribute separate chains");
        let mut unique = ids.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(
            unique.len(),
            ids.len(),
            "all chain ids are distinct: {ids:?}"
        );
        assert!(ids.contains(&'A'), "host chain A preserved");

        // The new chain's residues carry the new id, not the template's original.
        let ubl_chain = bio.chains.last().expect("ubl chain");
        for &residue_index in &ubl_chain.residue_indices {
            assert_eq!(bio.residues[residue_index].id.chain_id, ubl_chain.id);
        }
    }

    #[test]
    fn ubl_override_replaces_the_bundled_template() {
        let protein = lysine();
        // A caller-supplied UBL: reuse the bundled NEDD8 as an override while the
        // kind is labeled Sumo, proving the override structure is what attaches.
        let override_ubl = ubl_template(UblKind::Nedd8).expect("override template");
        let result = ubiquitinate_protein(&protein, target(), UblKind::Sumo, Some(&override_ubl))
            .expect("conjugation with override");
        assert_eq!(
            result.atoms.len(),
            protein.atoms.len() - 1 + override_ubl.atoms.len() - 1
        );
        assert!(
            testkit::junction(&result, "NZ", "C"),
            "isopeptide bond formed"
        );
    }
}
