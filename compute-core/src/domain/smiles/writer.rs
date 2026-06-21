//! [`Sketch`] → SMILES writer.
//!
//! Walks the bond graph with a DFS, emitting a (non-canonical) SMILES string:
//! ring-closure (back) edges get digit labels, branches are parenthesised, and
//! charged / non-organic atoms are written in bracket form with an explicit
//! hydrogen count.

use crate::domain::{BondType, sketch::Sketch};

/// Emit a (non-canonical) SMILES string for a sketch.
pub fn to_smiles(sketch: &Sketch) -> String {
    Writer::new(sketch).run()
}

struct Writer<'a> {
    sketch: &'a Sketch,
    aromatic: Vec<bool>,
    /// Heavy-atom indices (hydrogens are folded into neighbours).
    heavy: Vec<bool>,
    visited: Vec<bool>,
    /// Bonds that form the spanning tree (recursed into); the rest are emitted
    /// as ring closures.
    tree_bond: Vec<bool>,
    /// Per-atom assigned ring-closure labels: (label, bond).
    ring_labels: Vec<Vec<(u16, BondType)>>,
    next_ring: u16,
}

impl<'a> Writer<'a> {
    fn new(sketch: &'a Sketch) -> Self {
        let n = sketch.atoms.len();
        let aromatic = (0..n)
            .map(|i| {
                sketch
                    .neighbors(i)
                    .iter()
                    .any(|(_, order)| *order == BondType::Aromatic)
            })
            .collect();
        let heavy = sketch
            .atoms
            .iter()
            .map(|atom| atom.element != "H")
            .collect();
        Self {
            sketch,
            aromatic,
            heavy,
            visited: vec![false; n],
            tree_bond: vec![false; sketch.bonds.len()],
            ring_labels: vec![Vec::new(); n],
            next_ring: 1,
        }
    }

    fn run(mut self) -> String {
        self.assign_rings();
        let mut output = String::new();
        let mut first = true;
        for start in 0..self.sketch.atoms.len() {
            if self.visited[start] || !self.heavy[start] {
                continue;
            }
            if !first {
                output.push('.');
            }
            first = false;
            self.write_atom(start, None, &mut output);
        }
        output
    }

    /// Find ring-closure (back) edges via DFS and assign each a digit.
    fn assign_rings(&mut self) {
        let n = self.sketch.atoms.len();
        let mut seen = vec![false; n];
        for start in 0..n {
            if seen[start] || !self.heavy[start] {
                continue;
            }
            // Iterative DFS recording tree edges; remaining heavy-heavy bonds are
            // ring closures.
            let mut stack = vec![start];
            seen[start] = true;
            while let Some(current) = stack.pop() {
                for (neighbor, bond_index) in self.heavy_neighbors(current) {
                    if !seen[neighbor] {
                        seen[neighbor] = true;
                        self.tree_bond[bond_index] = true;
                        stack.push(neighbor);
                    }
                }
            }
        }
        // Heavy–heavy bonds that are not tree edges close a ring.
        for (bond_index, bond) in self.sketch.bonds.iter().enumerate() {
            if self.tree_bond[bond_index] || !self.heavy[bond.a] || !self.heavy[bond.b] {
                continue;
            }
            let label = self.next_ring;
            self.next_ring += 1;
            self.ring_labels[bond.a].push((label, bond.order));
            self.ring_labels[bond.b].push((label, bond.order));
        }
    }

    fn heavy_neighbors(&self, atom: usize) -> Vec<(usize, usize)> {
        self.sketch
            .bonds
            .iter()
            .enumerate()
            .filter_map(|(index, bond)| {
                let other = if bond.a == atom {
                    bond.b
                } else if bond.b == atom {
                    bond.a
                } else {
                    return None;
                };
                self.heavy[other].then_some((other, index))
            })
            .collect()
    }

    fn write_atom(&mut self, atom: usize, incoming: Option<BondType>, output: &mut String) {
        self.visited[atom] = true;
        if let Some(order) = incoming
            && let Some(symbol) = self.bond_symbol(order, atom)
        {
            output.push(symbol);
        }
        output.push_str(&self.atom_token(atom));
        for (label, _) in self.ring_labels[atom].clone() {
            if label >= 10 {
                output.push_str(&format!("%{label}"));
            } else {
                output.push_str(&label.to_string());
            }
        }

        // Tree children: unvisited heavy neighbours reached via a spanning-tree
        // edge (ring-closure bonds are emitted as digits, not recursed into).
        let children: Vec<(usize, BondType)> = self
            .sketch
            .bonds
            .iter()
            .enumerate()
            .filter_map(|(index, bond)| {
                if !self.tree_bond[index] {
                    return None;
                }
                let other = if bond.a == atom {
                    bond.b
                } else if bond.b == atom {
                    bond.a
                } else {
                    return None;
                };
                (self.heavy[other] && !self.visited[other]).then_some((other, bond.order))
            })
            .collect();

        let count = children.len();
        for (index, (child, order)) in children.into_iter().enumerate() {
            if self.visited[child] {
                continue;
            }
            let last = index + 1 == count;
            if last {
                self.write_atom(child, Some(order), output);
            } else {
                output.push('(');
                self.write_atom(child, Some(order), output);
                output.push(')');
            }
        }
    }

    /// The bond symbol to emit, or `None` to leave it implicit. `atom` is the
    /// bond's near endpoint being written.
    fn bond_symbol(&self, order: BondType, atom: usize) -> Option<char> {
        match order {
            BondType::Double => Some('='),
            BondType::Triple => Some('#'),
            BondType::Aromatic => None,
            BondType::Single => {
                // A single bond between two aromatic atoms must be explicit, or
                // it would be read as aromatic.
                if self.aromatic[atom] { Some('-') } else { None }
            }
        }
    }

    fn atom_token(&self, atom: usize) -> String {
        let data = &self.sketch.atoms[atom];
        let symbol = if self.aromatic[atom] {
            data.element.to_lowercase()
        } else {
            data.element.clone()
        };
        let organic = matches!(
            data.element.as_str(),
            "B" | "C" | "N" | "O" | "P" | "S" | "F" | "Cl" | "Br" | "I"
        );
        if data.charge == 0 && organic {
            return symbol;
        }
        // Bracket form with explicit H count and charge.
        let explicit_h = self.bracket_hydrogens(atom);
        let mut token = format!("[{symbol}");
        match explicit_h {
            0 => {}
            1 => token.push('H'),
            n => token.push_str(&format!("H{n}")),
        }
        match data.charge.cmp(&0) {
            std::cmp::Ordering::Greater => {
                token.push('+');
                if data.charge > 1 {
                    token.push_str(&data.charge.to_string());
                }
            }
            std::cmp::Ordering::Less => {
                token.push('-');
                if data.charge < -1 {
                    token.push_str(&data.charge.abs().to_string());
                }
            }
            std::cmp::Ordering::Equal => {}
        }
        token.push(']');
        token
    }

    /// Hydrogens to write inside a bracket: explicit H neighbours plus the
    /// (pinned or valence-implicit) count.
    fn bracket_hydrogens(&self, atom: usize) -> u32 {
        let explicit = self
            .sketch
            .neighbors(atom)
            .iter()
            .filter(|(other, _)| self.sketch.atoms[*other].element == "H")
            .count() as u32;
        explicit + self.sketch.implicit_hydrogens(atom)
    }
}
