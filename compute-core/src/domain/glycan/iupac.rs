use anyhow::{Result, bail};

use super::dictionary::{self, MonosaccharideEntry};
use super::{Anomer, GlycanResidue, GlycanTree, Linkage, NodeId};

pub fn parse(input: &str) -> Result<GlycanTree> {
    let units = Scanner::new(input).run()?;
    build_tree(units)
}

#[derive(Debug, Clone)]
enum Unit {
    Residue {
        entry: MonosaccharideEntry,
        linkage: Option<Linkage>,
    },
    BranchOpen,
    BranchClose,
}

struct Scanner {
    chars: Vec<char>,
    pos: usize,
    units: Vec<Unit>,
}

impl Scanner {
    fn new(input: &str) -> Self {
        Self {
            chars: input.trim().chars().collect(),
            pos: 0,
            units: Vec::new(),
        }
    }

    fn run(mut self) -> Result<Vec<Unit>> {
        let mut depth = 0i32;
        while self.pos < self.chars.len() {
            let c = self.chars[self.pos];
            match c {
                '[' => {
                    depth += 1;
                    self.units.push(Unit::BranchOpen);
                    self.pos += 1;
                }
                ']' => {
                    depth -= 1;
                    if depth < 0 {
                        bail!("unbalanced ']' in glycan notation");
                    }
                    self.units.push(Unit::BranchClose);
                    self.pos += 1;
                }
                c if c.is_whitespace() => {
                    self.pos += 1;
                }
                '(' => {
                    bail!("'(' linkage with no preceding monosaccharide");
                }
                _ => {
                    let name = self.read_name()?;
                    let entry = match dictionary::lookup(&name) {
                        Some(entry) => entry,
                        None => bail!(
                            "unrecognized monosaccharide `{name}`; supported tokens: {}",
                            dictionary::supported_tokens().join(", ")
                        ),
                    };
                    let linkage = if self.chars.get(self.pos) == Some(&'(') {
                        Some(self.read_linkage()?)
                    } else {
                        None
                    };
                    self.units.push(Unit::Residue { entry, linkage });
                }
            }
        }
        if depth != 0 {
            bail!("unbalanced '[' in glycan notation");
        }
        if self.units.is_empty() {
            bail!("glycan notation contained no monosaccharides");
        }
        Ok(self.units)
    }

    fn read_name(&mut self) -> Result<String> {
        let start = self.pos;
        while let Some(&c) = self.chars.get(self.pos) {
            if c.is_ascii_alphanumeric() {
                self.pos += 1;
            } else {
                break;
            }
        }
        if self.pos == start {
            bail!(
                "unexpected character '{}' in glycan notation",
                self.chars[self.pos]
            );
        }
        Ok(self.chars[start..self.pos].iter().collect())
    }

    fn read_linkage(&mut self) -> Result<Linkage> {
        self.pos += 1;
        let anomer = match self.chars.get(self.pos) {
            Some('a') => Anomer::Alpha,
            Some('b') => Anomer::Beta,
            Some('?') => Anomer::Unknown,
            other => bail!("expected anomer 'a'/'b'/'?' in linkage, found {other:?}"),
        };
        self.pos += 1;
        let child_pos = self.read_position()?;
        if self.chars.get(self.pos) != Some(&'-') {
            bail!("expected '-' between linkage positions");
        }
        self.pos += 1;
        let parent_pos = self.read_position()?;
        if self.chars.get(self.pos) != Some(&')') {
            bail!("expected ')' to close linkage");
        }
        self.pos += 1;
        Ok(Linkage {
            anomer,
            child_pos,
            parent_pos,
        })
    }

    fn read_position(&mut self) -> Result<u8> {
        match self.chars.get(self.pos) {
            Some('?') => {
                self.pos += 1;
                Ok(0)
            }
            Some(c) if c.is_ascii_digit() => {
                let value = (*c as u8) - b'0';
                self.pos += 1;
                Ok(value)
            }
            other => bail!("expected digit or '?' for linkage position, found {other:?}"),
        }
    }
}

fn build_tree(units: Vec<Unit>) -> Result<GlycanTree> {
    let mut nodes: Vec<GlycanResidue> = Vec::new();
    let mut current_parent: Option<NodeId> = None;
    let mut branch_stack: Vec<Option<NodeId>> = Vec::new();
    let mut root: Option<NodeId> = None;

    for unit in units.into_iter().rev() {
        match unit {
            Unit::BranchClose => {
                branch_stack.push(current_parent);
            }
            Unit::BranchOpen => {
                let Some(saved) = branch_stack.pop() else {
                    bail!("unbalanced '[' in glycan notation");
                };
                current_parent = saved;
            }
            Unit::Residue { entry, linkage } => {
                let id = nodes.len();
                nodes.push(GlycanResidue {
                    mono: entry.mono,
                    parent: None,
                    linkage,
                    children: Vec::new(),
                });
                match (current_parent, linkage) {
                    (None, None) => {
                        if root.is_some() {
                            bail!("glycan notation has more than one root residue");
                        }
                        root = Some(id);
                    }
                    (None, Some(_)) => {
                        bail!("the reducing-end residue must not carry a linkage");
                    }
                    (Some(parent), Some(link)) => {
                        nodes[id].parent = Some(parent);
                        nodes[parent].children.push((link, id));
                    }
                    (Some(_), None) => {
                        bail!("non-root monosaccharide is missing its linkage");
                    }
                }
                current_parent = Some(id);
            }
        }
    }

    if !branch_stack.is_empty() {
        bail!("unbalanced ']' in glycan notation");
    }
    let Some(root) = root else {
        bail!("glycan notation has no reducing-end residue");
    };

    Ok(GlycanTree {
        nodes,
        root,
        attachment: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::glycan::{Anomer, SugarKind};

    #[test]
    fn parses_a_single_residue() {
        let tree = parse("GlcNAc").unwrap();
        assert_eq!(tree.nodes.len(), 1);
        assert_eq!(tree.root, 0);
        assert_eq!(tree.nodes[0].mono.kind, SugarKind::GlcNAc);
        assert!(tree.nodes[0].linkage.is_none());
        assert!(tree.nodes[0].parent.is_none());
    }

    #[test]
    fn parses_a_linear_chain() {
        let tree = parse("Man(b1-4)GlcNAc(b1-4)GlcNAc").unwrap();
        assert_eq!(tree.nodes.len(), 3);
        let root = &tree.nodes[tree.root];
        assert_eq!(root.mono.kind, SugarKind::GlcNAc);
        assert!(root.linkage.is_none());
        assert_eq!(root.children.len(), 1);
        let (link, mid) = root.children[0];
        assert_eq!(link.anomer, Anomer::Beta);
        assert_eq!(link.child_pos, 1);
        assert_eq!(link.parent_pos, 4);
        assert_eq!(tree.nodes[mid].mono.kind, SugarKind::GlcNAc);
        let (_, top) = tree.nodes[mid].children[0];
        assert_eq!(tree.nodes[top].mono.kind, SugarKind::Man);
    }

    #[test]
    fn parses_the_n_glycan_core_pentasaccharide() {
        let tree = parse("Man(a1-3)[Man(a1-6)]Man(b1-4)GlcNAc(b1-4)GlcNAc").unwrap();
        assert_eq!(tree.nodes.len(), 5);
        let root = &tree.nodes[tree.root];
        assert_eq!(root.mono.kind, SugarKind::GlcNAc);

        let (_, second) = root.children[0];
        assert_eq!(tree.nodes[second].mono.kind, SugarKind::GlcNAc);

        let (_, branch_man) = tree.nodes[second].children[0];
        assert_eq!(tree.nodes[branch_man].mono.kind, SugarKind::Man);
        assert_eq!(tree.nodes[branch_man].children.len(), 2);

        let kinds: Vec<SugarKind> = tree.nodes[branch_man]
            .children
            .iter()
            .map(|(_, id)| tree.nodes[*id].mono.kind)
            .collect();
        assert_eq!(kinds, vec![SugarKind::Man, SugarKind::Man]);

        let mut positions: Vec<u8> = tree.nodes[branch_man]
            .children
            .iter()
            .map(|(link, _)| link.parent_pos)
            .collect();
        positions.sort_unstable();
        assert_eq!(positions, vec![3, 6]);
    }

    #[test]
    fn rejects_unknown_monosaccharide() {
        let err = parse("Bogus(b1-4)GlcNAc").unwrap_err().to_string();
        assert!(err.contains("Bogus"));
        assert!(err.contains("supported tokens"));
    }

    #[test]
    fn rejects_unbalanced_brackets() {
        assert!(parse("Man(a1-3)[Man(a1-6)Man(b1-4)GlcNAc").is_err());
        assert!(parse("Man(a1-3)]Man(b1-4)GlcNAc").is_err());
    }

    #[test]
    fn rejects_root_with_linkage() {
        assert!(parse("Man(b1-4)GlcNAc(b1-4)").is_err());
    }

    #[test]
    fn rejects_empty_input() {
        assert!(parse("").is_err());
        assert!(parse("   ").is_err());
    }

    #[test]
    fn rejects_malformed_linkage() {
        assert!(parse("Man(x1-4)GlcNAc").is_err());
        assert!(parse("Man(a14)GlcNAc").is_err());
        assert!(parse("Man(a1-4GlcNAc").is_err());
    }
}
