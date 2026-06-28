//! Make a PTM-modified protein `Structure` MD-ready by renaming the modified
//! residue to its native CHARMM36 name.
//!
//! The structural PTM builders ([`crate::workflows::ptm`]) weld an idealized
//! modifying group on as extra atoms in their own residue(s), joined to the host
//! by a junction bond. The bundled CHARMM36 force field parameterizes these
//! modifications as *whole* modified residues (SEP/TPO/PTR, ALY, MLZ/MLY/M3L),
//! so — unlike a glycan, which is a separately typed moleculetype merged in with
//! an approximate junction patch — the correct, fabrication-free path is to fold
//! the modifying group into the host residue, relabel it and its atoms to the
//! native rtp names, and let `pdb2gmx` build the residue from the rtp. `pdb2gmx`
//! is invoked with `-ignh`, so modifying-group hydrogens are rebuilt from the
//! rtp/hdb and need not match here; only heavy atoms are name-checked.

use std::collections::{HashMap, HashSet, VecDeque};

use anyhow::{Result, anyhow, bail};

use crate::domain::modification::PtmKind;
use crate::domain::ptm_patches::{self, PtmResidue};
use crate::domain::{ProteinAnchor, ResidueId, Structure};

use super::forcefield_assets::{self, CarbTopologyDatabase};

/// A PTM-modified structure made MD-ready: every modifying-group atom folded
/// into the host residue and relabeled to the native CHARMM36 residue's rtp
/// names, ready for `pdb2gmx -ignh` to parameterize from `aminoacids.rtp`.
#[derive(Debug, Clone)]
pub struct PtmPreparation {
    pub structure: Structure,
    pub native_residue: String,
    /// Integral net charge of the native residue, summed from the rtp.
    pub net_charge: i32,
}

/// Rename `residue`'s PTM into its native CHARMM36 modified residue, loading the
/// bundled `aminoacids.rtp` database.
pub fn prepare_ptm_residue(
    structure: &Structure,
    residue: ResidueId,
    kind: PtmKind,
    anchor: ProteinAnchor,
) -> Result<PtmPreparation> {
    let database = forcefield_assets::charmm36_ptm_database()?;
    prepare_ptm_residue_with(structure, residue, kind, anchor, &database)
}

/// As [`prepare_ptm_residue`], against a preloaded rtp database.
pub fn prepare_ptm_residue_with(
    structure: &Structure,
    residue: ResidueId,
    kind: PtmKind,
    anchor: ProteinAnchor,
    database: &CarbTopologyDatabase,
) -> Result<PtmPreparation> {
    let ptm = ptm_patches::native_ptm_residue(kind, anchor)?;
    let native = ptm.charmm_name;

    let bio = structure
        .biopolymer
        .as_ref()
        .ok_or_else(|| anyhow!("a PTM topology needs a biopolymer overlay"))?;
    if !bio.is_compatible_with_atom_count(structure.atoms.len()) {
        bail!("the biopolymer overlay does not cover every atom");
    }

    let host = bio
        .residues
        .iter()
        .position(|record| record.id == residue)
        .ok_or_else(|| anyhow!("host residue {residue:?} not found"))?;

    let anchor_atom = bio.residues[host]
        .atom_indices
        .iter()
        .copied()
        .find(|&index| bio.atom_name(index) == Some(ptm.anchor_atom))
        .ok_or_else(|| anyhow!("host residue is missing anchor atom {}", ptm.anchor_atom))?;

    let modifying = collect_modifying_atoms(structure, host, anchor_atom);
    if modifying.is_empty() {
        bail!(
            "no modifying-group atoms joined to anchor {}",
            ptm.anchor_atom
        );
    }
    let modifying_heavy: Vec<usize> = modifying
        .iter()
        .copied()
        .filter(|&index| !is_hydrogen(structure, index))
        .collect();

    let host_names: HashSet<String> = bio.residues[host]
        .atom_indices
        .iter()
        .filter_map(|&index| bio.atom_name(index).map(str::to_string))
        .collect();

    let assigned = assign_rtp_names(
        structure,
        anchor_atom,
        &ptm,
        &modifying_heavy,
        &host_names,
        database,
    )?;

    let prepared = fold_into_host(structure, host, native, &modifying, &assigned);
    verify_residue(&prepared, host, native, &ptm, anchor_atom, database)?;

    let net_charge = rtp_net_charge(database, native);
    Ok(PtmPreparation {
        structure: prepared,
        native_residue: native.to_string(),
        net_charge,
    })
}

fn is_hydrogen(structure: &Structure, index: usize) -> bool {
    structure
        .atoms
        .get(index)
        .map(|atom| atom.element.eq_ignore_ascii_case("H"))
        .unwrap_or(false)
}

/// All atoms reachable from the host anchor without re-entering the host
/// residue — the welded modifying group(s). Starting at the side-chain anchor
/// keeps backbone (and thus the rest of a chain) out of the traversal.
fn collect_modifying_atoms(structure: &Structure, host: usize, anchor_atom: usize) -> Vec<usize> {
    let bio = structure.biopolymer.as_ref().expect("checked by caller");
    let mut visited: HashSet<usize> = HashSet::new();
    let mut collected = Vec::new();
    let mut queue = VecDeque::from([anchor_atom]);
    while let Some(current) = queue.pop_front() {
        for neighbor in bonded_neighbors(structure, current) {
            if visited.contains(&neighbor) {
                continue;
            }
            let in_host = bio.residue_for_atom.get(neighbor).and_then(|r| *r) == Some(host);
            if in_host {
                continue;
            }
            visited.insert(neighbor);
            collected.push(neighbor);
            queue.push_back(neighbor);
        }
    }
    collected.sort_unstable();
    collected
}

fn bonded_neighbors(structure: &Structure, atom: usize) -> Vec<usize> {
    structure
        .bonds
        .iter()
        .filter_map(|bond| {
            if bond.a == atom {
                Some(bond.b)
            } else if bond.b == atom {
                Some(bond.a)
            } else {
                None
            }
        })
        .collect()
}

/// Map each modifying heavy atom to its native rtp atom name by matching the
/// welded group's bond graph against the residue's rtp `[ bonds ]` graph,
/// element by element, outward from the anchor. Greedy matching suffices: the
/// groups are small trees and same-element rtp siblings (the phosphate oxygens,
/// the lysine methyls) are interchangeable.
fn assign_rtp_names(
    structure: &Structure,
    anchor_atom: usize,
    ptm: &PtmResidue,
    modifying_heavy: &[usize],
    host_names: &HashSet<String>,
    database: &CarbTopologyDatabase,
) -> Result<HashMap<usize, String>> {
    let rtp_adj = rtp_adjacency(database, ptm.charmm_name);
    let heavy: HashSet<usize> = modifying_heavy.iter().copied().collect();

    let mut assigned: HashMap<usize, String> = HashMap::new();
    let mut rtp_used: HashSet<String> = HashSet::from([ptm.anchor_atom.to_string()]);
    let mut queue: VecDeque<(usize, String)> =
        VecDeque::from([(anchor_atom, ptm.anchor_atom.to_string())]);

    while let Some((atom, rtp_name)) = queue.pop_front() {
        let mut neighbors: Vec<usize> = bonded_neighbors(structure, atom)
            .into_iter()
            .filter(|index| heavy.contains(index) && !assigned.contains_key(index))
            .collect();
        neighbors.sort_unstable();

        let candidates = rtp_adj.get(&rtp_name).cloned().unwrap_or_default();
        for atom_index in neighbors {
            let element = structure.atoms[atom_index].element.as_str();
            let pick = candidates.iter().find(|name| {
                !rtp_used.contains(*name)
                    && !host_names.contains(*name)
                    && element_matches(name, element)
                    && database
                        .typing
                        .contains_key(&(ptm.charmm_name.to_string(), (*name).clone()))
            });
            let Some(name) = pick.cloned() else {
                continue;
            };
            rtp_used.insert(name.clone());
            assigned.insert(atom_index, name.clone());
            queue.push_back((atom_index, name));
        }
    }

    for &index in modifying_heavy {
        if !assigned.contains_key(&index) {
            let name = structure
                .biopolymer
                .as_ref()
                .and_then(|bio| bio.atom_name(index))
                .unwrap_or("?");
            bail!(
                "requires force-field assets: modifying atom {name} has no match in CHARMM36 \
                 residue {}",
                ptm.charmm_name
            );
        }
    }
    Ok(assigned)
}

fn rtp_adjacency(database: &CarbTopologyDatabase, residue: &str) -> HashMap<String, Vec<String>> {
    let mut adjacency: HashMap<String, Vec<String>> = HashMap::new();
    let Some(bonds) = database.bonds.get(residue) else {
        return adjacency;
    };
    for [a, b] in bonds {
        if is_inter_residue(a) || is_inter_residue(b) {
            continue;
        }
        adjacency.entry(a.clone()).or_default().push(b.clone());
        adjacency.entry(b.clone()).or_default().push(a.clone());
    }
    adjacency
}

fn is_inter_residue(name: &str) -> bool {
    name.starts_with('+') || name.starts_with('-')
}

/// Element of an rtp atom name: its leading alphabetic character (CHARMM names
/// lead with the element, e.g. `O1P`/`OH` → O, `CH3`/`CM1` → C, `NZ` → N).
fn element_matches(rtp_name: &str, element: &str) -> bool {
    let rtp_element = rtp_name.chars().find(|c| c.is_ascii_alphabetic());
    let structure_element = element.chars().next();
    match (rtp_element, structure_element) {
        (Some(a), Some(b)) => a.eq_ignore_ascii_case(&b),
        _ => false,
    }
}

/// Build the renamed structure: relabel modifying heavy atoms, rename the host
/// residue to the native name, fold every modifying atom into it, and drop the
/// now-empty modifying residue records.
fn fold_into_host(
    structure: &Structure,
    host: usize,
    native: &str,
    modifying: &[usize],
    assigned: &HashMap<usize, String>,
) -> Structure {
    let mut prepared = structure.clone();
    let bio = prepared
        .biopolymer
        .as_mut()
        .expect("biopolymer checked by caller");

    bio.residues[host].residue_name = native.to_string();

    let removed: HashSet<usize> = modifying
        .iter()
        .filter_map(|&index| bio.residue_for_atom.get(index).and_then(|r| *r))
        .filter(|&residue| residue != host)
        .collect();

    for (&index, name) in assigned {
        if let Some(slot) = bio.atom_name_for_atom.get_mut(index) {
            *slot = Some(name.clone());
        }
    }
    for &index in modifying {
        if let Some(slot) = bio.residue_for_atom.get_mut(index) {
            *slot = Some(host);
        }
    }
    bio.residues[host].atom_indices.extend_from_slice(modifying);
    bio.residues[host].atom_indices.sort_unstable();
    bio.residues[host].atom_indices.dedup();

    compact_residues(bio, &removed);
    prepared
}

fn compact_residues(bio: &mut crate::domain::biopolymer::Biopolymer, removed: &HashSet<usize>) {
    let mut remap: HashMap<usize, usize> = HashMap::new();
    let mut kept = Vec::new();
    for (old, residue) in std::mem::take(&mut bio.residues).into_iter().enumerate() {
        if removed.contains(&old) {
            continue;
        }
        remap.insert(old, kept.len());
        kept.push(residue);
    }
    bio.residues = kept;
    for slot in bio.residue_for_atom.iter_mut() {
        if let Some(old) = *slot {
            *slot = remap.get(&old).copied();
        }
    }
    for chain in bio.chains.iter_mut() {
        chain.residue_indices = chain
            .residue_indices
            .iter()
            .filter_map(|old| remap.get(old).copied())
            .collect();
    }
}

/// Confirm every heavy atom of the folded residue resolves to a CHARMM typing
/// entry (no orphan), and the junction bond is present in both the structure and
/// the residue's own rtp definition.
fn verify_residue(
    prepared: &Structure,
    host: usize,
    native: &str,
    ptm: &PtmResidue,
    anchor_atom: usize,
    database: &CarbTopologyDatabase,
) -> Result<()> {
    let bio = prepared.biopolymer.as_ref().expect("set in fold_into_host");
    let host_record = &bio.residues[host];
    for &index in &host_record.atom_indices {
        if is_hydrogen(prepared, index) {
            continue;
        }
        let name = bio
            .atom_name(index)
            .ok_or_else(|| anyhow!("folded atom {index} has no name"))?;
        if !database
            .typing
            .contains_key(&(native.to_string(), name.to_string()))
        {
            bail!("requires force-field assets: {native}.{name} has no CHARMM typing");
        }
    }

    let partner_present = prepared.bonds.iter().any(|bond| {
        let other = if bond.a == anchor_atom {
            Some(bond.b)
        } else if bond.b == anchor_atom {
            Some(bond.a)
        } else {
            None
        };
        other
            .and_then(|index| bio.atom_name(index))
            .map(|name| ptm.junction_partners.contains(&name))
            .unwrap_or(false)
    });
    if !partner_present {
        bail!(
            "the junction bond {}-{:?} is missing",
            ptm.anchor_atom,
            ptm.junction_partners
        );
    }

    let rtp_has_junction = database.bonds.get(native).is_some_and(|bonds| {
        bonds.iter().any(|[a, b]| {
            (a == ptm.anchor_atom && ptm.junction_partners.contains(&b.as_str()))
                || (b == ptm.anchor_atom && ptm.junction_partners.contains(&a.as_str()))
        })
    });
    if !rtp_has_junction {
        bail!(
            "CHARMM36 residue {native} does not list the {} junction bond",
            ptm.anchor_atom
        );
    }
    Ok(())
}

fn rtp_net_charge(database: &CarbTopologyDatabase, residue: &str) -> i32 {
    let sum: f32 = database
        .typing
        .iter()
        .filter(|((res, _), _)| res == residue)
        .map(|(_, typing)| typing.charge)
        .sum();
    sum.round() as i32
}

#[cfg(test)]
mod tests;
