//! Enumerate bonded-interaction terms from a covalent bond graph, the way a
//! force-field topology preprocessor (GROMACS' `pdb2gmx gen_pad`) does.
//!
//! A residue/building-block force field lists each fragment's *bonds* (and a few
//! out-of-plane *impropers*) explicitly, but leaves the *angles* and *proper
//! dihedrals* implicit: they are generated from the connectivity and then
//! parameterized by atom type. The three generators here do exactly that, over
//! whatever bond graph they are given — so terms spanning two residues (a
//! glycosidic linkage) fall out for free when the graph already contains the
//! inter-residue bonds.
//!
//! The GROMACS interaction *function code* is caller-supplied, so one
//! enumeration serves both a CHARMM biomolecule (angle 5, proper 9, 1-4 pair 1)
//! and an OPLS-style framework (angle 1, Ryckaert-Bellemans proper 3). All
//! output is 1-based and de-duplicated.

use std::collections::BTreeSet;

use crate::domain::Bond;

use super::topology::BondedTerm;

/// Build a 0-based neighbor adjacency list from a bond list, each atom's
/// neighbors sorted and de-duplicated. Self-bonds and out-of-range indices are
/// dropped so a malformed bond cannot produce a degenerate term downstream.
pub(crate) fn bond_adjacency(atom_count: usize, bonds: &[Bond]) -> Vec<Vec<usize>> {
    let mut adjacency = vec![Vec::new(); atom_count];
    for bond in bonds {
        if bond.a == bond.b || bond.a >= atom_count || bond.b >= atom_count {
            continue;
        }
        adjacency[bond.a].push(bond.b);
        adjacency[bond.b].push(bond.a);
    }
    for neighbors in &mut adjacency {
        neighbors.sort_unstable();
        neighbors.dedup();
    }
    adjacency
}

/// Index-only bonds from a bond list: 1-based, de-duplicated and
/// orientation-normalized (`a < b`).
pub(crate) fn bonds(bonds: &[Bond], func: i32) -> Vec<BondedTerm> {
    let mut seen = BTreeSet::new();
    for bond in bonds {
        if bond.a == bond.b {
            continue;
        }
        seen.insert((bond.a.min(bond.b), bond.a.max(bond.b)));
    }
    seen.into_iter()
        .map(|(a, b)| BondedTerm {
            atoms: vec![a as u32 + 1, b as u32 + 1],
            func,
        })
        .collect()
}

/// Index-only angles: every pair of bonds sharing a central atom. 1-based, with
/// the two end atoms ordered so each angle appears once.
pub(crate) fn angles(adjacency: &[Vec<usize>], func: i32) -> Vec<BondedTerm> {
    let mut angles = Vec::new();
    for (center, neighbors) in adjacency.iter().enumerate() {
        for i in 0..neighbors.len() {
            for j in (i + 1)..neighbors.len() {
                angles.push(BondedTerm {
                    atoms: vec![
                        neighbors[i] as u32 + 1,
                        center as u32 + 1,
                        neighbors[j] as u32 + 1,
                    ],
                    func,
                });
            }
        }
    }
    angles
}

/// Index-only proper dihedrals: every `i-j-k-l` path over a central bond
/// `(j, k)`. 1-based, de-duplicated. Each central bond is visited once
/// (`j < k`), so a dihedral and its reverse are not both emitted; the `i == l`
/// guard skips the degenerate three-membered-ring closure.
pub(crate) fn proper_dihedrals(adjacency: &[Vec<usize>], func: i32) -> Vec<BondedTerm> {
    let mut seen = BTreeSet::new();
    for (j, neighbors_j) in adjacency.iter().enumerate() {
        for &k in neighbors_j {
            if j >= k {
                continue;
            }
            for &i in neighbors_j {
                if i == k {
                    continue;
                }
                for &l in &adjacency[k] {
                    if l == j || l == i {
                        continue;
                    }
                    seen.insert((i, j, k, l));
                }
            }
        }
    }
    seen.into_iter()
        .map(|(i, j, k, l)| BondedTerm {
            atoms: vec![i as u32 + 1, j as u32 + 1, k as u32 + 1, l as u32 + 1],
            func,
        })
        .collect()
}

/// Index-only 1-4 pairs: every atom pair whose *shortest* path through the bond
/// graph is exactly three bonds. Taking the minimum distance (rather than the
/// 1-4 ends of dihedrals) is what keeps a furanose honest — two ring atoms that
/// are 1-4 along one arc but 1-3 across the ring are excluded, never given a
/// spurious 1-4 interaction. 1-based, de-duplicated (`a < b`).
pub(crate) fn one_four_pairs(adjacency: &[Vec<usize>], func: i32) -> Vec<BondedTerm> {
    let mut seen = BTreeSet::new();
    for a in 0..adjacency.len() {
        let d1: BTreeSet<usize> = adjacency[a].iter().copied().collect();
        let mut d2 = BTreeSet::new();
        for &n1 in &adjacency[a] {
            for &n2 in &adjacency[n1] {
                if n2 != a && !d1.contains(&n2) {
                    d2.insert(n2);
                }
            }
        }
        for &n2 in &d2 {
            for &n3 in &adjacency[n2] {
                if n3 != a && !d1.contains(&n3) && !d2.contains(&n3) {
                    seen.insert((a.min(n3), a.max(n3)));
                }
            }
        }
    }
    seen.into_iter()
        .map(|(a, b)| BondedTerm {
            atoms: vec![a as u32 + 1, b as u32 + 1],
            func,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::BondType;

    /// A closed ring of `n` atoms `0-1-…-(n-1)-0`.
    fn ring_bonds(n: usize) -> Vec<Bond> {
        (0..n)
            .map(|i| Bond::with_type(i, (i + 1) % n, BondType::Single))
            .collect()
    }

    fn pairs_set(terms: &[BondedTerm]) -> BTreeSet<(u32, u32)> {
        terms.iter().map(|t| (t.atoms[0], t.atoms[1])).collect()
    }

    #[test]
    fn six_ring_angles_dihedrals_and_para_pairs() {
        let adj = bond_adjacency(6, &ring_bonds(6));
        // One angle per central atom, one proper dihedral per ring bond.
        assert_eq!(angles(&adj, 5).len(), 6);
        assert_eq!(proper_dihedrals(&adj, 9).len(), 6);
        // The only min-distance-3 pairs in a hexagon are the three para pairs.
        let pairs = pairs_set(&one_four_pairs(&adj, 1));
        assert_eq!(pairs, BTreeSet::from([(1, 4), (2, 5), (3, 6)]));
        assert!(one_four_pairs(&adj, 1).iter().all(|t| t.func == 1));
    }

    #[test]
    fn five_ring_has_no_one_four_pairs() {
        // A furanose-like pentagon has diameter two: no atom pair is three bonds
        // apart, so a naive dihedral-endpoint scheme would wrongly pair 1-3 atoms.
        let adj = bond_adjacency(5, &ring_bonds(5));
        assert!(one_four_pairs(&adj, 1).is_empty());
        assert_eq!(angles(&adj, 5).len(), 5);
        assert_eq!(proper_dihedrals(&adj, 9).len(), 5);
    }

    #[test]
    fn open_chain_pairs_count_one_four_only() {
        // Butane skeleton 0-1-2-3-4-5: the 1-4 pairs are (0,3),(1,4),(2,5).
        let bonds_list: Vec<Bond> = (0..5)
            .map(|i| Bond::with_type(i, i + 1, BondType::Single))
            .collect();
        let adj = bond_adjacency(6, &bonds_list);
        let pairs = pairs_set(&one_four_pairs(&adj, 1));
        assert_eq!(pairs, BTreeSet::from([(1, 4), (2, 5), (3, 6)]));
        // A linear chain of six atoms has three internal dihedrals.
        assert_eq!(proper_dihedrals(&adj, 9).len(), 3);
    }

    #[test]
    fn self_and_out_of_range_bonds_are_ignored() {
        // Adjacency is range-checked, so a self-bond and an out-of-range neighbor
        // both drop out, leaving each ring atom with its two true neighbors.
        let mut adj_input = ring_bonds(4);
        adj_input.push(Bond::with_type(0, 0, BondType::Single));
        adj_input.push(Bond::with_type(0, 99, BondType::Single));
        let adj = bond_adjacency(4, &adj_input);
        assert!(adj.iter().all(|n| n.len() == 2));

        // `bonds` has no atom count to range-check against; it only skips the
        // degenerate self-bond, like the topology builders it replaces.
        let mut bond_input = ring_bonds(4);
        bond_input.push(Bond::with_type(0, 0, BondType::Single));
        assert_eq!(bonds(&bond_input, 1).len(), 4);
    }
}
