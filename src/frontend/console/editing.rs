use std::collections::BTreeSet;

use anyhow::{Result, bail};

use super::{DeleteTarget, HydrogenAction};
use crate::{
    domain::{Biopolymer, ChainRecord, ResidueRecord},
    domain::{Bond, Structure},
    frontend::state::AppState,
};

pub(crate) fn hydrogen_command(state: &mut AppState, action: HydrogenAction) -> Result<String> {
    match action {
        HydrogenAction::Add => add_hydrogens_command(state),
    }
}

fn add_hydrogens_command(state: &mut AppState) -> Result<String> {
    if !state.has_active_entry() {
        bail!("hydrogen add requires an open entry");
    }
    let before = state.capture_edit_snapshot();
    let old_atom_count = state.structure().atoms.len();
    let added = state.structure_mut().add_missing_hydrogens();
    attach_added_hydrogens_to_biopolymer(state.structure_mut(), old_atom_count);
    state.mark_structure_changed();
    state.set_source_path(None);
    state
        .ui
        .selection
        .retain_valid(state.structure().atoms.len());
    state.history.push_undo(before);
    Ok(format!("added {added} hydrogen(s)"))
}

pub(crate) fn delete_command(state: &mut AppState, target: DeleteTarget) -> Result<String> {
    match target {
        DeleteTarget::Chain { spec } => {
            let chains = spec
                .split(',')
                .filter_map(|token| token.trim().chars().next())
                .collect::<BTreeSet<_>>();
            if chains.is_empty() {
                bail!("delete chain requires at least one chain id");
            }
            let before = state.capture_edit_snapshot();
            let removed = retain_chains(state.structure_mut(), &chains);
            state.mark_structure_changed();
            state.history.push_undo(before);
            Ok(format!("deleted {removed} atom(s) from chain selection"))
        }
    }
}

fn retain_chains(structure: &mut Structure, deleted_chains: &BTreeSet<char>) -> usize {
    let Some(biopolymer) = structure.biopolymer.clone() else {
        return 0;
    };
    let delete_atom = biopolymer
        .residue_for_atom
        .iter()
        .map(|residue_index| {
            residue_index
                .and_then(|index| biopolymer.residues.get(index))
                .is_some_and(|residue| deleted_chains.contains(&residue.id.chain_id))
        })
        .collect::<Vec<_>>();
    let removed = delete_atom.iter().filter(|delete| **delete).count();
    if removed == 0 {
        return 0;
    }

    let mut remap = vec![None; structure.atoms.len()];
    let mut atoms = Vec::with_capacity(structure.atoms.len() - removed);
    for (index, atom) in structure.atoms.iter().enumerate() {
        if !delete_atom[index] {
            remap[index] = Some(atoms.len());
            atoms.push(atom.clone());
        }
    }
    let bonds = structure
        .bonds
        .iter()
        .filter_map(|bond| {
            Some(Bond {
                a: remap[bond.a]?,
                b: remap[bond.b]?,
                bond_type: bond.bond_type,
            })
        })
        .collect();

    structure.atoms = atoms;
    structure.bonds = bonds;
    structure.biopolymer = biopolymer_after_atom_retain(&biopolymer, &remap);
    removed
}

fn attach_added_hydrogens_to_biopolymer(structure: &mut Structure, old_atom_count: usize) {
    let Some(biopolymer) = &mut structure.biopolymer else {
        return;
    };
    if old_atom_count >= structure.atoms.len()
        || biopolymer.residue_for_atom.len() != old_atom_count
    {
        return;
    }

    biopolymer
        .residue_for_atom
        .resize(structure.atoms.len(), None);
    for atom_index in old_atom_count..structure.atoms.len() {
        let Some(parent_residue) = structure.bonds.iter().find_map(|bond| {
            if bond.a == atom_index && bond.b < old_atom_count {
                biopolymer.residue_for_atom[bond.b]
            } else if bond.b == atom_index && bond.a < old_atom_count {
                biopolymer.residue_for_atom[bond.a]
            } else {
                None
            }
        }) else {
            continue;
        };
        biopolymer.residue_for_atom[atom_index] = Some(parent_residue);
        if let Some(residue) = biopolymer.residues.get_mut(parent_residue) {
            residue.atom_indices.push(atom_index);
        }
    }
}

fn biopolymer_after_atom_retain(
    source: &Biopolymer,
    atom_remap: &[Option<usize>],
) -> Option<Biopolymer> {
    let mut residues = Vec::new();
    let mut residue_remap = vec![None; source.residues.len()];

    for (old_residue_index, residue) in source.residues.iter().enumerate() {
        let atom_indices = residue
            .atom_indices
            .iter()
            .filter_map(|&atom_index| atom_remap.get(atom_index).copied().flatten())
            .collect::<Vec<_>>();
        if atom_indices.is_empty() {
            continue;
        }
        let alpha_carbon = residue
            .alpha_carbon
            .and_then(|atom_index| atom_remap.get(atom_index).copied().flatten());
        residue_remap[old_residue_index] = Some(residues.len());
        residues.push(ResidueRecord {
            id: residue.id.clone(),
            residue_name: residue.residue_name.clone(),
            atom_indices,
            alpha_carbon,
            is_standard_amino_acid: residue.is_standard_amino_acid,
        });
    }

    let chains = source
        .chains
        .iter()
        .filter_map(|chain| {
            let residue_indices = chain
                .residue_indices
                .iter()
                .filter_map(|&index| residue_remap[index])
                .collect::<Vec<_>>();
            (!residue_indices.is_empty()).then_some(ChainRecord {
                id: chain.id,
                residue_indices,
            })
        })
        .collect::<Vec<_>>();

    if residues.is_empty() {
        return None;
    }

    let new_atom_count = atom_remap.iter().filter(|entry| entry.is_some()).count();
    let mut residue_for_atom = vec![None; new_atom_count];
    let mut atom_name_for_atom = vec![None; new_atom_count];
    for (old_atom_index, new_atom_index) in atom_remap.iter().enumerate() {
        let Some(new_atom_index) = new_atom_index else {
            continue;
        };
        residue_for_atom[*new_atom_index] = source
            .residue_for_atom
            .get(old_atom_index)
            .copied()
            .flatten()
            .and_then(|residue_index| residue_remap[residue_index]);
        atom_name_for_atom[*new_atom_index] = source
            .atom_name_for_atom
            .get(old_atom_index)
            .cloned()
            .flatten();
    }

    Some(Biopolymer {
        residues,
        chains,
        secondary_structures: source.secondary_structures.clone(),
        residue_for_atom,
        atom_name_for_atom,
    })
}
