//! Generic fragment condensation: weld a fragment onto a host structure by
//! deleting the leaving groups on both sides, placing the fragment with the
//! shared geometry primitive, and bonding the donor atom to the host anchor.
//! Shared by glycosylation and the PTM fragment builders.

use std::collections::{HashMap, HashSet};

use anyhow::{Result, anyhow};
use nalgebra::Vector3;

use super::stitch::{self, AcceptorSite, DonorSite};
use crate::domain::glycan::TemplateAtom;
use crate::domain::{Atom, Biopolymer, Bond, BondType, ChainRecord, ResidueRecord, Structure};

/// Host-side attachment point: the anchor atom the fragment bonds to, the host
/// atoms to delete (a leaving hydrogen), and the outward bond direction.
pub struct AcceptorSpec {
    pub anchor_atom: usize,
    pub remove: Vec<usize>,
    pub outward: Vector3<f32>,
}

/// Fragment-side attachment point: the donor atom that bonds to the host, the
/// fragment atoms to delete (the leaving group), and the outward bond direction.
pub struct DonorSpec {
    pub donor_atom: usize,
    pub remove: Vec<usize>,
    pub outward: Vector3<f32>,
}

/// Bond `fragment`'s donor atom to `host`'s anchor atom, dropping the leaving
/// atoms named in each spec, and return the merged structure. The fragment is
/// rigidly placed so its donor sits one `bond_length` out along the anchor's
/// outward direction.
pub fn attach_fragment(
    host: &Structure,
    acceptor: AcceptorSpec,
    fragment: &Structure,
    donor: DonorSpec,
    bond_length: f32,
    bond_type: BondType,
    title_suffix: &str,
) -> Result<Structure> {
    let host_bio = host
        .biopolymer
        .as_ref()
        .ok_or_else(|| anyhow!("host has no biopolymer overlay"))?;
    let fragment_bio = fragment
        .biopolymer
        .as_ref()
        .ok_or_else(|| anyhow!("fragment has no biopolymer overlay"))?;
    let anchor_position = host
        .atoms
        .get(acceptor.anchor_atom)
        .ok_or_else(|| anyhow!("anchor atom index is out of range"))?
        .position;

    let template_atoms: Vec<TemplateAtom> = fragment
        .atoms
        .iter()
        .enumerate()
        .map(|(index, atom)| TemplateAtom {
            name: fragment_bio
                .atom_name(index)
                .map(str::to_string)
                .unwrap_or_default(),
            element: atom.element.clone(),
            position: atom.position,
        })
        .collect();
    let child_bonds: Vec<(usize, usize)> = fragment.bonds.iter().map(|b| (b.a, b.b)).collect();

    let placement = stitch::place_fragment(
        &template_atoms,
        &child_bonds,
        DonorSite {
            anomeric_atom: donor.donor_atom,
            outward: donor.outward,
        },
        AcceptorSite {
            oxygen_atom: acceptor.anchor_atom,
            outward: acceptor.outward,
        },
        anchor_position,
        bond_length,
        acceptor.outward,
    );

    merge(
        host,
        host_bio,
        fragment,
        fragment_bio,
        &placement,
        &acceptor,
        &donor,
        bond_type,
        title_suffix,
    )
}

#[allow(clippy::too_many_arguments)]
fn merge(
    host: &Structure,
    host_bio: &Biopolymer,
    fragment: &Structure,
    fragment_bio: &Biopolymer,
    placement: &stitch::FragmentPlacement,
    acceptor: &AcceptorSpec,
    donor: &DonorSpec,
    bond_type: BondType,
    title_suffix: &str,
) -> Result<Structure> {
    let removed_fragment: Vec<bool> = (0..fragment.atoms.len())
        .map(|index| donor.remove.contains(&index))
        .collect();
    let removed_host: Vec<bool> = (0..host.atoms.len())
        .map(|index| acceptor.remove.contains(&index))
        .collect();

    let mut atoms: Vec<Atom> = Vec::new();
    let mut atom_names: Vec<Option<String>> = Vec::new();
    let mut residue_for_atom: Vec<Option<usize>> = Vec::new();
    let mut host_remap = vec![usize::MAX; host.atoms.len()];

    for (index, atom) in host.atoms.iter().enumerate() {
        if removed_host[index] {
            continue;
        }
        host_remap[index] = atoms.len();
        atoms.push(atom.clone());
        atom_names.push(host_bio.atom_name(index).map(str::to_string));
        residue_for_atom.push(*host_bio.residue_for_atom.get(index).unwrap_or(&None));
    }

    let host_residue_count = host_bio.residues.len();
    let mut fragment_remap = vec![usize::MAX; fragment.atoms.len()];
    for (index, placed) in placement.atoms.iter().enumerate() {
        if removed_fragment[index] {
            continue;
        }
        fragment_remap[index] = atoms.len();
        atoms.push(Atom {
            element: placed.element.clone(),
            position: placed.position,
            charge: fragment.atoms[index].charge,
        });
        atom_names.push(fragment_bio.atom_name(index).map(str::to_string));
        let residue = fragment_bio
            .residue_for_atom
            .get(index)
            .and_then(|r| *r)
            .map(|r| r + host_residue_count);
        residue_for_atom.push(residue);
    }

    let mut bonds: Vec<Bond> = Vec::new();
    for bond in &host.bonds {
        let (a, b) = (
            host_remap.get(bond.a).copied().unwrap_or(usize::MAX),
            host_remap.get(bond.b).copied().unwrap_or(usize::MAX),
        );
        if a != usize::MAX && b != usize::MAX {
            bonds.push(Bond::with_type(a, b, bond.bond_type));
        }
    }
    for bond in &fragment.bonds {
        let (a, b) = (
            fragment_remap.get(bond.a).copied().unwrap_or(usize::MAX),
            fragment_remap.get(bond.b).copied().unwrap_or(usize::MAX),
        );
        if a != usize::MAX && b != usize::MAX {
            bonds.push(Bond::with_type(a, b, bond.bond_type));
        }
    }

    let junction_host = host_remap[acceptor.anchor_atom];
    let junction_donor = fragment_remap[donor.donor_atom];
    if junction_host == usize::MAX || junction_donor == usize::MAX {
        return Err(anyhow!("junction atoms were removed during merge"));
    }
    bonds.push(Bond::with_type(junction_host, junction_donor, bond_type));

    let (chains, chain_id_map) = merge_chains(host_bio, fragment_bio, host_residue_count);

    let mut residues = host_bio.residues.clone();
    for residue in &mut residues {
        remap_residue(residue, &host_remap);
    }
    for fragment_residue in &fragment_bio.residues {
        let mut residue = fragment_residue.clone();
        remap_residue(&mut residue, &fragment_remap);
        if let Some(&new_chain) = chain_id_map.get(&residue.id.chain_id) {
            residue.id.chain_id = new_chain;
        }
        residues.push(residue);
    }

    let biopolymer = Biopolymer {
        residues,
        chains,
        secondary_structures: host_bio.secondary_structures.clone(),
        residue_for_atom,
        atom_name_for_atom: atom_names,
    };

    let title = format!("{}+{}", host.title, title_suffix);
    let mut structure = Structure::with_bonds(title, atoms, bonds);
    structure.biopolymer = Some(biopolymer);
    structure.cell = host.cell.clone();

    Ok(structure)
}

fn remap_residue(residue: &mut ResidueRecord, remap: &[usize]) {
    residue.atom_indices = residue
        .atom_indices
        .iter()
        .filter_map(|&old| remap.get(old).copied())
        .filter(|&new| new != usize::MAX)
        .collect();
    residue.alpha_carbon = remap_optional(residue.alpha_carbon, remap);
    residue.backbone_nitrogen = remap_optional(residue.backbone_nitrogen, remap);
    residue.backbone_carbon = remap_optional(residue.backbone_carbon, remap);
    residue.backbone_oxygen = remap_optional(residue.backbone_oxygen, remap);
}

fn remap_optional(index: Option<usize>, remap: &[usize]) -> Option<usize> {
    let old = index?;
    let new = remap.get(old).copied()?;
    if new == usize::MAX { None } else { Some(new) }
}

/// Append the fragment's chains to the host's, returning the merged chains and
/// the fragment chain-id remapping applied (so the caller can keep fragment
/// residue ids in step). A fragment chain whose id collides with one already in
/// use — the host's, or an earlier fragment chain — is reassigned the next free
/// id so welded residues never duplicate a host chain+residue id; otherwise the
/// id is kept verbatim.
fn merge_chains(
    host_bio: &Biopolymer,
    fragment_bio: &Biopolymer,
    host_residue_count: usize,
) -> (Vec<ChainRecord>, HashMap<char, char>) {
    let mut chains = host_bio.chains.clone();
    let mut chain_id_map: HashMap<char, char> = HashMap::new();
    let mut used: HashSet<char> = host_bio.chains.iter().map(|chain| chain.id).collect();

    for fragment_chain in &fragment_bio.chains {
        let id = if used.contains(&fragment_chain.id) {
            let fresh = next_free_chain_id(&used);
            chain_id_map.insert(fragment_chain.id, fresh);
            fresh
        } else {
            fragment_chain.id
        };
        used.insert(id);
        chains.push(ChainRecord {
            id,
            residue_indices: fragment_chain
                .residue_indices
                .iter()
                .map(|&index| index + host_residue_count)
                .collect(),
        });
    }
    (chains, chain_id_map)
}

/// The first chain id (A–Z, a–z, 0–9) not already in `used`, falling back to `?`
/// only when every printable single-character id is taken.
fn next_free_chain_id(used: &HashSet<char>) -> char {
    ('A'..='Z')
        .chain('a'..='z')
        .chain('0'..='9')
        .find(|id| !used.contains(id))
        .unwrap_or('?')
}
