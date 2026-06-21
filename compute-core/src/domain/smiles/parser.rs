//! SMILES → [`Sketch`] parser.
//!
//! Scope is the practical organic subset: the bare organic atoms
//! (`B C N O P S F Cl Br I` and aromatic `b c n o p s`), bracket atoms with
//! isotope/charge/explicit-H/atom-class, the bond symbols `- = # : / \`, branches
//! `( )`, ring-closure digits and `%nn`, and dot-disconnected fragments. Chirality
//! (`@`, `@@`) and bond stereo (`/`, `\`) are parsed but discarded — the sketcher
//! does not model stereochemistry in v1.

use anyhow::{Result, bail};

use crate::domain::{
    BondType,
    sketch::{Sketch, seed_layout},
};

/// Parse a SMILES string into a laid-out [`Sketch`].
pub fn parse(input: &str) -> Result<Sketch> {
    let parsed = Parser::new(input).run()?;
    if parsed.atoms.is_empty() {
        bail!("SMILES contained no atoms");
    }
    let mut sketch = parsed.into_sketch();
    seed_layout(&mut sketch);
    Ok(sketch)
}

#[derive(Clone)]
struct ParsedAtom {
    element: String,
    charge: i32,
    aromatic: bool,
    bracket: bool,
    explicit_h: u32,
}

struct ParsedMolecule {
    atoms: Vec<ParsedAtom>,
    bonds: Vec<(usize, usize, BondType)>,
}

impl ParsedMolecule {
    /// Convert to a [`Sketch`]. Bracket atoms keep their written hydrogen count
    /// as a pinned (authoritative) value; coordinates are left at the origin for
    /// the caller to lay out.
    fn into_sketch(self) -> Sketch {
        use crate::domain::sketch::{SketchAtom, SketchBond};
        use nalgebra::Point2;

        let mut sketch = Sketch::new();
        for atom in &self.atoms {
            let mut sketch_atom = SketchAtom::new(atom.element.clone(), Point2::origin());
            sketch_atom.charge = atom.charge;
            // A bracket atom's hydrogen count is authoritative in SMILES: pin it
            // so the valence model is bypassed and `fill_hydrogens` adds exactly
            // this many (handles radicals like `[CH2]` and donors like `[nH]`).
            if atom.bracket {
                sketch_atom.explicit_hydrogens = Some(atom.explicit_h);
            }
            sketch.atoms.push(sketch_atom);
        }
        for &(a, b, order) in &self.bonds {
            sketch.bonds.push(SketchBond { a, b, order });
        }
        sketch
    }
}

struct Parser {
    chars: Vec<char>,
    pos: usize,
    atoms: Vec<ParsedAtom>,
    bonds: Vec<(usize, usize, BondType)>,
    /// Open ring-closure bonds keyed by ring number → (atom index, optional bond).
    rings: std::collections::HashMap<u16, (usize, Option<BondType>)>,
    /// Stack of "previous atom" indices for branch handling.
    branch_stack: Vec<usize>,
    previous: Option<usize>,
    pending_bond: Option<BondType>,
    /// A `.` just severed the chain — do not bond the next atom to the previous.
    broken: bool,
}

impl Parser {
    fn new(input: &str) -> Self {
        Self {
            chars: input.trim().chars().collect(),
            pos: 0,
            atoms: Vec::new(),
            bonds: Vec::new(),
            rings: std::collections::HashMap::new(),
            branch_stack: Vec::new(),
            previous: None,
            pending_bond: None,
            broken: false,
        }
    }

    fn run(mut self) -> Result<ParsedMolecule> {
        while self.pos < self.chars.len() {
            let c = self.chars[self.pos];
            match c {
                '(' => {
                    let Some(previous) = self.previous else {
                        bail!("'(' with no preceding atom");
                    };
                    self.branch_stack.push(previous);
                    self.pos += 1;
                }
                ')' => {
                    self.previous = self.branch_stack.pop();
                    if self.previous.is_none() {
                        bail!("unbalanced ')'");
                    }
                    self.pos += 1;
                }
                '-' | '=' | '#' | ':' | '/' | '\\' => {
                    self.pending_bond = Some(bond_symbol(c));
                    self.pos += 1;
                }
                '.' => {
                    self.broken = true;
                    self.pending_bond = None;
                    self.pos += 1;
                }
                '%' => {
                    self.pos += 1;
                    let number = self.read_two_digit_ring()?;
                    self.handle_ring_closure(number)?;
                }
                '0'..='9' => {
                    let number = (c as u8 - b'0') as u16;
                    self.pos += 1;
                    self.handle_ring_closure(number)?;
                }
                '[' => {
                    let atom = self.read_bracket_atom()?;
                    self.push_atom(atom);
                }
                c if c.is_whitespace() => {
                    self.pos += 1;
                }
                _ => {
                    let atom = self.read_bare_atom()?;
                    self.push_atom(atom);
                }
            }
        }

        if !self.branch_stack.is_empty() {
            bail!("unbalanced '('");
        }
        if let Some((number, _)) = self.rings.iter().next() {
            bail!("unclosed ring bond {number}");
        }

        Ok(ParsedMolecule {
            atoms: self.atoms,
            bonds: self.bonds,
        })
    }

    fn push_atom(&mut self, atom: ParsedAtom) {
        let index = self.atoms.len();
        let aromatic = atom.aromatic;
        self.atoms.push(atom);
        if let Some(previous) = self.previous
            && !self.broken
        {
            let order = self.pending_bond.unwrap_or_else(|| {
                if aromatic && self.atoms[previous].aromatic {
                    BondType::Aromatic
                } else {
                    BondType::Single
                }
            });
            self.add_bond_checked(previous, index, order);
        }
        self.previous = Some(index);
        self.pending_bond = None;
        self.broken = false;
    }

    fn add_bond_checked(&mut self, a: usize, b: usize, order: BondType) {
        if a != b
            && !self
                .bonds
                .iter()
                .any(|(x, y, _)| (*x == a && *y == b) || (*x == b && *y == a))
        {
            self.bonds.push((a, b, order));
        }
    }

    fn handle_ring_closure(&mut self, number: u16) -> Result<()> {
        let Some(current) = self.previous else {
            bail!("ring bond {number} with no preceding atom");
        };
        let bond = self.pending_bond.take();
        if let Some((other, other_bond)) = self.rings.remove(&number) {
            let order = bond.or(other_bond).unwrap_or_else(|| {
                if self.atoms[current].aromatic && self.atoms[other].aromatic {
                    BondType::Aromatic
                } else {
                    BondType::Single
                }
            });
            self.add_bond_checked(other, current, order);
        } else {
            self.rings.insert(number, (current, bond));
        }
        Ok(())
    }

    fn read_two_digit_ring(&mut self) -> Result<u16> {
        let tens = self.next_digit()?;
        let ones = self.next_digit()?;
        Ok(tens * 10 + ones)
    }

    fn next_digit(&mut self) -> Result<u16> {
        let Some(c) = self.chars.get(self.pos) else {
            bail!("expected digit in '%' ring number");
        };
        let Some(value) = c.to_digit(10) else {
            bail!("expected digit in '%' ring number, found '{c}'");
        };
        self.pos += 1;
        Ok(value as u16)
    }

    fn read_bare_atom(&mut self) -> Result<ParsedAtom> {
        // Two-letter organic atoms first.
        let two: String = self.chars[self.pos..].iter().take(2).collect::<String>();
        if two == "Cl" || two == "Br" {
            self.pos += 2;
            return Ok(ParsedAtom {
                element: two,
                charge: 0,
                aromatic: false,
                bracket: false,
                explicit_h: 0,
            });
        }
        let c = self.chars[self.pos];
        let (element, aromatic) = match c {
            'B' | 'C' | 'N' | 'O' | 'P' | 'S' | 'F' | 'I' => (c.to_string(), false),
            'b' | 'c' | 'n' | 'o' | 'p' | 's' => (c.to_uppercase().to_string(), true),
            '*' => ("C".to_string(), false), // wildcard → treat as carbon
            _ => bail!("unexpected character '{c}' in SMILES"),
        };
        self.pos += 1;
        Ok(ParsedAtom {
            element,
            charge: 0,
            aromatic,
            bracket: false,
            explicit_h: 0,
        })
    }

    fn read_bracket_atom(&mut self) -> Result<ParsedAtom> {
        self.pos += 1; // consume '['
        // Optional isotope digits — ignored.
        while self.chars.get(self.pos).is_some_and(|c| c.is_ascii_digit()) {
            self.pos += 1;
        }
        let (element, aromatic) = self.read_bracket_symbol()?;
        if !crate::domain::chemistry::is_element_symbol(&element) {
            bail!("unknown element `{element}` in bracket atom");
        }

        let mut explicit_h = 0u32;
        let mut charge = 0i32;

        while let Some(&c) = self.chars.get(self.pos) {
            match c {
                ']' => {
                    self.pos += 1;
                    return Ok(ParsedAtom {
                        element,
                        charge,
                        aromatic,
                        bracket: true,
                        explicit_h,
                    });
                }
                '@' => {
                    // Chirality (`@`, `@@`, `@TH1`, …) — consume and discard.
                    self.pos += 1;
                    while self
                        .chars
                        .get(self.pos)
                        .is_some_and(|c| *c == '@' || c.is_ascii_alphanumeric())
                    {
                        // Stop before the closing bracket handled above.
                        if self.chars[self.pos] == ']' {
                            break;
                        }
                        // Only consume chirality tokens, not H/charge — bail out
                        // once we hit those.
                        if matches!(self.chars[self.pos], 'H' | '+' | '-' | ':') {
                            break;
                        }
                        self.pos += 1;
                    }
                }
                'H' => {
                    self.pos += 1;
                    explicit_h = self.read_optional_count(1) as u32;
                }
                '+' => {
                    charge += self.read_charge_run('+');
                }
                '-' => {
                    charge -= self.read_charge_run('-');
                }
                ':' => {
                    // Atom class — `:` followed by digits, discarded.
                    self.pos += 1;
                    while self.chars.get(self.pos).is_some_and(|c| c.is_ascii_digit()) {
                        self.pos += 1;
                    }
                }
                _ => bail!("unexpected character '{c}' inside bracket atom"),
            }
        }
        bail!("unterminated bracket atom")
    }

    fn read_bracket_symbol(&mut self) -> Result<(String, bool)> {
        let Some(&first) = self.chars.get(self.pos) else {
            bail!("empty bracket atom");
        };
        if first.is_ascii_uppercase() {
            self.pos += 1;
            // A following lowercase letter forms a two-letter element symbol,
            // except 'H' which is the hydrogen-count token.
            if let Some(&second) = self.chars.get(self.pos)
                && second.is_ascii_lowercase()
                && second != 'h'
            {
                self.pos += 1;
                return Ok((format!("{first}{second}"), false));
            }
            Ok((first.to_string(), false))
        } else if first.is_ascii_lowercase() {
            // Aromatic atom: single (b,c,n,o,p,s) or two-letter (se, as).
            self.pos += 1;
            if let Some(&second) = self.chars.get(self.pos)
                && second.is_ascii_lowercase()
                && matches!((first, second), ('s', 'e') | ('a', 's'))
            {
                self.pos += 1;
                let symbol = format!("{}{second}", first.to_uppercase());
                return Ok((symbol, true));
            }
            Ok((first.to_uppercase().to_string(), true))
        } else {
            bail!("expected element symbol in bracket atom, found '{first}'")
        }
    }

    /// Read an optional integer count after `H`/`+`/`-`, defaulting to `default`.
    fn read_optional_count(&mut self, default: i32) -> i32 {
        if self.chars.get(self.pos).is_some_and(|c| c.is_ascii_digit()) {
            let mut value = 0i32;
            while let Some(&c) = self.chars.get(self.pos) {
                let Some(digit) = c.to_digit(10) else { break };
                value = value * 10 + digit as i32;
                self.pos += 1;
            }
            value
        } else {
            default
        }
    }

    /// Handle a charge run: a sign followed by either a digit count (`+2`) or
    /// repeats of the same sign (`++`). Assumes the leading sign is at `pos`.
    fn read_charge_run(&mut self, sign: char) -> i32 {
        self.pos += 1; // consume the leading sign
        if self.chars.get(self.pos).is_some_and(|c| c.is_ascii_digit()) {
            return self.read_optional_count(1);
        }
        let mut magnitude = 1;
        while self.chars.get(self.pos) == Some(&sign) {
            magnitude += 1;
            self.pos += 1;
        }
        magnitude
    }
}

fn bond_symbol(c: char) -> BondType {
    match c {
        '=' => BondType::Double,
        '#' => BondType::Triple,
        ':' => BondType::Aromatic,
        // '-', '/', '\\' all map to a single bond (stereo discarded).
        _ => BondType::Single,
    }
}
