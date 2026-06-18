//! Writing [`Structure`]s to PDBQT, including the docking preparation step.
//!
//! Two outputs are produced here:
//!
//! * [`to_pdbqt`] — a rigid PDBQT (atom records only), used for the generic
//!   structure-format export and for the receptor input.
//! * [`prepare_ligand_pdbqt`] — a flexible ligand PDBQT carrying the
//!   `ROOT`/`BRANCH`/`TORSDOF` torsion tree the search needs.
//!
//! The chemistry is best-effort (see [`super::typing`]): non-polar hydrogens are
//! merged, AutoDock types are derived from the element + bond graph, and rotatable
//! bonds are the non-ring single bonds between two non-terminal heavy atoms — the
//! same definition the crate uses when counting torsional degrees of freedom.

use anyhow::{Result, bail};

use crate::domain::{BondType, Structure};

use super::typing::{AdAssignment, Neighbors, atom_line, classify, is_hydrogen, neighbors_of};

/// A prepared PDBQT input plus provenance about how reliable the preparation is.
pub struct PreparedPdbqt {
    /// The PDBQT text, ready to hand to `docking::api::dock`.
    pub text: String,
    /// Active torsions (`TORSDOF`); 0 for a rigid receptor.
    pub torsions: usize,
    /// Human-readable caveats about the (approximate) preparation.
    pub notes: Vec<String>,
}

/// One kept atom: the non-polar hydrogens have already been dropped.
struct Kept {
    original: usize,
    element: String,
    position: [f32; 3],
    charge: f32,
    ad: &'static str,
    /// A polar hydrogen (`HD`); not counted as a heavy atom.
    hydrogen: bool,
}

/// Type every atom, dropping merged non-polar hydrogens. Errors on an unsupported
/// element so preparation fails loudly rather than emitting an invalid type.
fn collect_kept(structure: &Structure, neighbors: &Neighbors) -> Result<(Vec<Kept>, Vec<String>)> {
    if structure.atoms.is_empty() {
        bail!("structure has no atoms to prepare");
    }
    let mut kept = Vec::new();
    let mut any_hydrogen = false;
    for (index, atom) in structure.atoms.iter().enumerate() {
        if is_hydrogen(&atom.element) {
            any_hydrogen = true;
        }
        match classify(structure, neighbors, index)? {
            AdAssignment::DropHydrogen => {}
            AdAssignment::Emit(ad) => kept.push(Kept {
                original: index,
                element: atom.element.clone(),
                position: [atom.position.x, atom.position.y, atom.position.z],
                charge: atom.charge,
                ad,
                hydrogen: ad == "HD",
            }),
        }
    }
    if kept.is_empty() {
        bail!("structure has no atoms left after preparation");
    }

    let mut notes = Vec::new();
    if !any_hydrogen {
        notes.push(
            "structure has no explicit hydrogens; hydrogen-bond donor typing will be \
             incomplete — add hydrogens before docking for best accuracy"
                .to_string(),
        );
    }
    Ok((kept, notes))
}

fn write_atoms<'a>(out: &mut String, kept: impl Iterator<Item = (usize, &'a Kept)>) {
    for (serial0, atom) in kept {
        out.push_str(&atom_line(
            serial0 + 1,
            &atom.element,
            atom.position,
            atom.charge,
            atom.ad,
        ));
        out.push('\n');
    }
}

/// Serialize a structure as a rigid PDBQT (atom records only). Used for the
/// generic `.pdbqt` export and for receptor input.
pub fn to_pdbqt(structure: &Structure) -> Result<String> {
    let neighbors = neighbors_of(structure);
    let (kept, _notes) = collect_kept(structure, &neighbors)?;
    let mut out = String::new();
    let title = structure.title.trim();
    out.push_str(&format!(
        "REMARK  silicolab PDBQT export: {}\n",
        if title.is_empty() { "structure" } else { title }
    ));
    write_atoms(&mut out, kept.iter().enumerate());
    out.push_str("END\n");
    Ok(out)
}

/// Prepare a rigid receptor PDBQT from a structure.
pub fn prepare_receptor_pdbqt(structure: &Structure) -> Result<PreparedPdbqt> {
    let neighbors = neighbors_of(structure);
    let (kept, notes) = collect_kept(structure, &neighbors)?;
    let mut text = String::new();
    text.push_str("REMARK  silicolab prepared receptor (rigid)\n");
    write_atoms(&mut text, kept.iter().enumerate());
    text.push_str("END\n");
    Ok(PreparedPdbqt {
        text,
        torsions: 0,
        notes,
    })
}

/// Prepare a flexible ligand PDBQT, building the `ROOT`/`BRANCH` torsion tree from
/// the rotatable bonds.
pub fn prepare_ligand_pdbqt(structure: &Structure) -> Result<PreparedPdbqt> {
    let neighbors = neighbors_of(structure);
    let (kept, notes) = collect_kept(structure, &neighbors)?;

    // Adjacency restricted to kept atoms, indexed in kept space.
    let original_to_kept = {
        let mut map = vec![usize::MAX; structure.atoms.len()];
        for (k, atom) in kept.iter().enumerate() {
            map[atom.original] = k;
        }
        map
    };
    let mut adj: Vec<Vec<(usize, BondType)>> = vec![Vec::new(); kept.len()];
    for bond in &structure.bonds {
        let (a, b) = (original_to_kept[bond.a], original_to_kept[bond.b]);
        if a != usize::MAX && b != usize::MAX {
            adj[a].push((b, bond.bond_type));
            adj[b].push((a, bond.bond_type));
        }
    }

    let heavy_degree = |k: usize| adj[k].iter().filter(|(j, _)| !kept[*j].hydrogen).count();

    // Rotatable bonds: non-ring single bonds between two non-terminal heavy atoms.
    let mut rotatable: Vec<(usize, usize)> = Vec::new();
    for a in 0..kept.len() {
        for &(b, bond_type) in &adj[a] {
            if a >= b {
                continue;
            }
            if bond_type != BondType::Single || kept[a].hydrogen || kept[b].hydrogen {
                continue;
            }
            if heavy_degree(a) < 2 || heavy_degree(b) < 2 {
                continue;
            }
            if is_bridge(&adj, a, b) {
                rotatable.push((a, b));
            }
        }
    }

    // Fragments: connected components after removing the rotatable bonds.
    let rotatable_set: std::collections::HashSet<(usize, usize)> =
        rotatable.iter().copied().collect();
    let frag_of = components(&adj, &rotatable_set, kept.len());
    let fragment_count = frag_of.iter().copied().max().map(|m| m + 1).unwrap_or(0);
    let mut fragments: Vec<Vec<usize>> = vec![Vec::new(); fragment_count];
    for (k, &f) in frag_of.iter().enumerate() {
        fragments[f].push(k);
    }

    // Fragment adjacency via the rotatable bonds: (neighbor_fragment, from_atom, to_atom).
    let mut frag_adj: Vec<Vec<(usize, usize, usize)>> = vec![Vec::new(); fragment_count];
    for &(a, b) in &rotatable {
        let (fa, fb) = (frag_of[a], frag_of[b]);
        frag_adj[fa].push((fb, a, b));
        frag_adj[fb].push((fa, b, a));
    }

    // Root at the largest fragment (most atoms) — the conventional anchor.
    let root = (0..fragment_count)
        .max_by_key(|&f| fragments[f].len())
        .unwrap_or(0);

    let mut out = String::new();
    out.push_str("REMARK  silicolab prepared ligand\n");
    let mut serial_of = vec![0usize; kept.len()];
    let mut next_serial = 1usize;
    let mut visited = vec![false; fragment_count];
    visited[root] = true;

    out.push_str("ROOT\n");
    for &k in &fragments[root] {
        serial_of[k] = next_serial;
        next_serial += 1;
        push_atom(&mut out, &kept[k], serial_of[k]);
    }
    out.push_str("ENDROOT\n");

    for &(child, from_atom, to_atom) in &frag_adj[root] {
        if !visited[child] {
            emit_branch(
                &mut out,
                &kept,
                &fragments,
                &frag_adj,
                &mut serial_of,
                &mut next_serial,
                &mut visited,
                from_atom,
                to_atom,
                child,
            );
        }
    }
    out.push_str(&format!("TORSDOF {}\n", rotatable.len()));

    if visited.iter().any(|v| !v) {
        bail!(
            "ligand appears to be more than one disconnected molecule; \
             dock a single connected ligand"
        );
    }

    Ok(PreparedPdbqt {
        text: out,
        torsions: rotatable.len(),
        notes,
    })
}

fn push_atom(out: &mut String, atom: &Kept, serial: usize) {
    out.push_str(&atom_line(
        serial,
        &atom.element,
        atom.position,
        atom.charge,
        atom.ad,
    ));
    out.push('\n');
}

#[allow(clippy::too_many_arguments)]
fn emit_branch(
    out: &mut String,
    kept: &[Kept],
    fragments: &[Vec<usize>],
    frag_adj: &[Vec<(usize, usize, usize)>],
    serial_of: &mut [usize],
    next_serial: &mut usize,
    visited: &mut [bool],
    from_atom: usize,
    to_atom: usize,
    child: usize,
) {
    visited[child] = true;
    // The connecting ("to") atom is listed first inside the branch.
    serial_of[to_atom] = *next_serial;
    *next_serial += 1;
    let from_serial = serial_of[from_atom];
    let to_serial = serial_of[to_atom];
    out.push_str(&format!("BRANCH {from_serial:>3} {to_serial:>3}\n"));
    push_atom(out, &kept[to_atom], to_serial);
    for &k in &fragments[child] {
        if k != to_atom {
            serial_of[k] = *next_serial;
            *next_serial += 1;
            push_atom(out, &kept[k], serial_of[k]);
        }
    }
    for &(grandchild, gc_from, gc_to) in &frag_adj[child] {
        if !visited[grandchild] {
            emit_branch(
                out,
                kept,
                fragments,
                frag_adj,
                serial_of,
                next_serial,
                visited,
                gc_from,
                gc_to,
                grandchild,
            );
        }
    }
    out.push_str(&format!("ENDBRANCH {from_serial:>3} {to_serial:>3}\n"));
}

/// Whether the bond `a-b` is a bridge (its removal disconnects `a` from `b`),
/// i.e. it lies in no ring. Ligands are small, so a per-bond DFS is fine.
fn is_bridge(adj: &[Vec<(usize, BondType)>], a: usize, b: usize) -> bool {
    let mut stack = vec![a];
    let mut seen = vec![false; adj.len()];
    seen[a] = true;
    while let Some(node) = stack.pop() {
        for &(next, _) in &adj[node] {
            // Skip the direct a-b edge; any other path to `b` means it is in a ring.
            if (node == a && next == b) || (node == b && next == a) {
                continue;
            }
            if !seen[next] {
                if next == b {
                    return false;
                }
                seen[next] = true;
                stack.push(next);
            }
        }
    }
    true
}

/// Connected-component id per atom, traversing only bonds that are not rotatable.
fn components(
    adj: &[Vec<(usize, BondType)>],
    rotatable: &std::collections::HashSet<(usize, usize)>,
    n: usize,
) -> Vec<usize> {
    let mut comp = vec![usize::MAX; n];
    let mut next_comp = 0;
    for start in 0..n {
        if comp[start] != usize::MAX {
            continue;
        }
        comp[start] = next_comp;
        let mut stack = vec![start];
        while let Some(node) = stack.pop() {
            for &(neighbor, _) in &adj[node] {
                let edge = if node < neighbor {
                    (node, neighbor)
                } else {
                    (neighbor, node)
                };
                if rotatable.contains(&edge) {
                    continue;
                }
                if comp[neighbor] == usize::MAX {
                    comp[neighbor] = next_comp;
                    stack.push(neighbor);
                }
            }
        }
        next_comp += 1;
    }
    comp
}
