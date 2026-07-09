use anyhow::{Result, bail};

use super::dictionary::{self, MonosaccharideEntry};
use super::{Anomer, GlycanResidue, GlycanTree, GlycosylationKind, Linkage, NodeId};

/// Parse IUPAC-condensed glycan notation.
///
/// A residue's anomeric configuration is carried by the linkage that attaches it
/// to its parent — in `Man(a1-3)Gal` the `a` *is* the configuration of that
/// mannose's C1, not a property of the bond independent of it. The `a`/`b` token
/// prefixes (`aMan`) remain available and must agree where both are given.
///
/// The reducing end has no parent linkage, so its anomer is left
/// [`Anomer::Unknown`] unless the notation states one — either with the prefix
/// (`aGalNAc`) or with a reducing-end linkage (`GalNAc(a1-`, `GlcNAc(b1-N)`).
/// [`super::resolve_root_anomer`] settles it against the aglycon.
pub fn parse(input: &str) -> Result<GlycanTree> {
    let units = Scanner::new(input).run()?;
    build_tree(units)
}

/// A residue's linkage to whatever lies below it.
#[derive(Debug, Clone, Copy)]
enum LinkSpec {
    /// `(a1-3)` — a glycosidic bond to the parent residue's O3.
    Internal(Linkage),
    /// `(b1-`, `(b1-)`, `(b1-N)` — the open linkage of the reducing end, which
    /// states the anomer and optionally names the aglycon it condenses onto.
    Reducing {
        anomer: Anomer,
        aglycon: Option<GlycosylationKind>,
    },
}

#[derive(Debug, Clone)]
enum Unit {
    Residue {
        entry: MonosaccharideEntry,
        link: Option<LinkSpec>,
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
                    let link = if self.chars.get(self.pos) == Some(&'(') {
                        Some(self.read_linkage(&entry)?)
                    } else {
                        None
                    };
                    self.units.push(Unit::Residue { entry, link });
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

    fn read_linkage(&mut self, entry: &MonosaccharideEntry) -> Result<LinkSpec> {
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

        // Past the '-', a parent position means an ordinary glycosidic bond;
        // anything else closes the reducing end.
        let aglycon = match self.chars.get(self.pos) {
            None => None,
            Some(')') => None,
            Some('N') => Some(GlycosylationKind::NLinked),
            Some('O') => Some(GlycosylationKind::OLinked),
            Some(_) => {
                let parent_pos = self.read_position()?;
                if self.chars.get(self.pos) != Some(&')') {
                    bail!("expected ')' to close linkage");
                }
                self.pos += 1;
                return Ok(LinkSpec::Internal(Linkage {
                    anomer,
                    child_pos,
                    parent_pos,
                }));
            }
        };
        if aglycon.is_some() {
            self.pos += 1;
        }
        // The open form `GlcNAc(b1-` may simply end the input; otherwise close it.
        if self.pos < self.chars.len() {
            if self.chars.get(self.pos) != Some(&')') {
                bail!("expected ')' to close the reducing-end linkage");
            }
            self.pos += 1;
        }
        if self.pos != self.chars.len() {
            bail!("a reducing-end linkage must end the notation");
        }

        if child_pos != 0 && child_pos != entry.anomeric_carbon {
            bail!(
                "{} is anomeric at C{}, but the reducing-end linkage names C{child_pos}",
                entry.token,
                entry.anomeric_carbon
            );
        }
        Ok(LinkSpec::Reducing { anomer, aglycon })
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

/// The configuration a residue ends up with, given what its token and its linkage
/// each say. The linkage wins; a prefixed token that disagrees is a contradiction
/// rather than something to silently resolve one way or the other.
fn reconcile_anomer(entry: &MonosaccharideEntry, stated: Anomer) -> Result<Anomer> {
    if !dictionary::token_states_anomer(entry.token) {
        return Ok(stated);
    }
    let by_token = entry.mono.anomer;
    match stated {
        Anomer::Unknown => Ok(by_token),
        stated if stated == by_token => Ok(stated),
        stated => bail!(
            "`{}` is {}-configured, but its linkage states {}",
            entry.token,
            by_token.name(),
            stated.name()
        ),
    }
}

fn build_tree(units: Vec<Unit>) -> Result<GlycanTree> {
    let mut nodes: Vec<GlycanResidue> = Vec::new();
    let mut current_parent: Option<NodeId> = None;
    let mut branch_stack: Vec<Option<NodeId>> = Vec::new();
    let mut root: Option<NodeId> = None;
    let mut aglycon: Option<GlycosylationKind> = None;

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
            Unit::Residue { entry, link } => {
                let id = nodes.len();
                let mut mono = entry.mono;
                let mut linkage = None;

                match (current_parent, link) {
                    (None, None) => {
                        if root.is_some() {
                            bail!("glycan notation has more than one root residue");
                        }
                        // Left unspecified for `resolve_root_anomer` unless the
                        // token itself declares one.
                        mono.anomer = if dictionary::token_states_anomer(entry.token) {
                            entry.mono.anomer
                        } else {
                            Anomer::Unknown
                        };
                        root = Some(id);
                    }
                    (
                        None,
                        Some(LinkSpec::Reducing {
                            anomer,
                            aglycon: named,
                        }),
                    ) => {
                        if root.is_some() {
                            bail!("glycan notation has more than one root residue");
                        }
                        mono.anomer = reconcile_anomer(&entry, anomer)?;
                        aglycon = named;
                        root = Some(id);
                    }
                    (None, Some(LinkSpec::Internal(_))) => {
                        bail!(
                            "the reducing-end residue must not carry a linkage to a parent; \
                             use the open form, e.g. `{}(b1-`",
                            entry.token
                        );
                    }
                    (Some(parent), Some(LinkSpec::Internal(link))) => {
                        mono.anomer = reconcile_anomer(&entry, link.anomer)?;
                        linkage = Some(link);
                        nodes[parent].children.push((link, id));
                    }
                    (Some(_), Some(LinkSpec::Reducing { .. })) => {
                        bail!("only the reducing-end residue may carry an open linkage");
                    }
                    (Some(_), None) => {
                        bail!("non-root monosaccharide is missing its linkage");
                    }
                }

                nodes.push(GlycanResidue {
                    mono,
                    parent: current_parent,
                    linkage,
                    children: Vec::new(),
                });
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
        aglycon,
    })
}

/// Render a tree back to canonical IUPAC-condensed notation.
///
/// The anomer always travels in the linkage — bare tokens throughout, and an
/// explicit reducing-end linkage — so the string states every configuration the
/// structure was actually built with. Round-trips through [`parse`].
pub fn to_iupac(tree: &GlycanTree) -> String {
    let mut out = String::new();
    write_subtree(tree, tree.root, &mut out);

    let root = &tree.nodes[tree.root];
    let anomeric_carbon = dictionary::anomeric_carbon(root.mono.kind).unwrap_or(1);
    let aglycon = match tree.aglycon {
        Some(GlycosylationKind::NLinked) => "N",
        Some(GlycosylationKind::OLinked) => "O",
        None => "",
    };
    out.push_str(&format!(
        "({}{anomeric_carbon}-{aglycon})",
        root.mono.anomer.symbol()
    ));
    out
}

/// Append `node`'s branches followed by `node` itself and the linkage that binds
/// it to its parent. Children were collected in reverse of their written order,
/// so the last one is the unbracketed main chain and the rest are branches.
fn write_subtree(tree: &GlycanTree, node: NodeId, out: &mut String) {
    let children = &tree.nodes[node].children;
    if let Some(&(_, main)) = children.last() {
        write_subtree(tree, main, out);
    }
    for &(_, branch) in children[..children.len().saturating_sub(1)].iter().rev() {
        out.push('[');
        write_subtree(tree, branch, out);
        out.push(']');
    }

    let residue = &tree.nodes[node];
    let token = dictionary::base_token(residue.mono.kind).unwrap_or("?");
    out.push_str(token);
    if let Some(link) = residue.linkage {
        out.push_str(&format!(
            "({}{}-{})",
            residue.mono.anomer.symbol(),
            position_symbol(link.child_pos),
            position_symbol(link.parent_pos)
        ));
    }
}

/// `read_position` folds an unknown `?` position onto zero; restore it.
fn position_symbol(position: u8) -> String {
    if position == 0 {
        "?".to_string()
    } else {
        position.to_string()
    }
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
        assert_eq!(
            tree.nodes[0].mono.anomer,
            Anomer::Unknown,
            "a bare reducing-end token states no configuration"
        );
    }

    /// The linkage, not the token, fixes a residue's anomeric configuration.
    #[test]
    fn the_linkage_sets_the_child_anomer() {
        let alpha = parse("Man(a1-3)Gal").unwrap();
        let beta = parse("Man(b1-3)Gal").unwrap();
        let child = |tree: &GlycanTree| tree.nodes[tree.nodes[tree.root].children[0].1].mono.anomer;
        assert_eq!(child(&alpha), Anomer::Alpha);
        assert_eq!(child(&beta), Anomer::Beta);
    }

    /// The bare token `Man` defaults to beta, but `Man(a1-3)` is alpha-mannose;
    /// the prefixed spelling is an equivalent way of saying the same thing.
    #[test]
    fn a_prefixed_token_agreeing_with_its_linkage_is_accepted() {
        let bare = parse("Man(a1-3)Gal").unwrap();
        let prefixed = parse("aMan(a1-3)Gal").unwrap();
        assert_eq!(bare.nodes, prefixed.nodes);
    }

    #[test]
    fn a_prefixed_token_contradicting_its_linkage_is_rejected() {
        let err = parse("aMan(b1-3)Gal").unwrap_err().to_string();
        assert!(err.contains("aMan"), "{err}");
        assert!(err.contains("alpha") && err.contains("beta"), "{err}");
    }

    #[test]
    fn an_unknown_linkage_anomer_falls_back_to_the_token() {
        let tree = parse("aMan(?1-3)Gal").unwrap();
        let child = tree.nodes[tree.nodes[tree.root].children[0].1].mono.anomer;
        assert_eq!(child, Anomer::Alpha);

        let tree = parse("Man(?1-3)Gal").unwrap();
        let child = tree.nodes[tree.nodes[tree.root].children[0].1].mono.anomer;
        assert_eq!(
            child,
            Anomer::Unknown,
            "a bare token cannot rescue an unspecified linkage"
        );
    }

    #[test]
    fn parses_the_reducing_end_open_linkage() {
        for notation in ["GalNAc(a1-", "GalNAc(a1-)", "GalNAc(a1-O)"] {
            let tree = parse(notation).unwrap_or_else(|e| panic!("{notation}: {e}"));
            assert_eq!(
                tree.nodes[tree.root].mono.anomer,
                Anomer::Alpha,
                "{notation}"
            );
        }
        assert_eq!(
            parse("GlcNAc(b1-N)").unwrap().aglycon,
            Some(GlycosylationKind::NLinked)
        );
        assert_eq!(
            parse("GalNAc(a1-O)").unwrap().aglycon,
            Some(GlycosylationKind::OLinked)
        );
        assert_eq!(parse("GalNAc(a1-").unwrap().aglycon, None);
    }

    #[test]
    fn rejects_a_misplaced_or_mis_numbered_reducing_linkage() {
        // Anomeric carbon must be the sugar's own.
        assert!(parse("GlcNAc(b2-").is_err());
        assert!(parse("Neu5Ac(a2-").is_ok());
        // Only the reducing end may carry an open linkage.
        assert!(parse("Man(b1-)GlcNAc").is_err());
        // ...and it must end the notation.
        assert!(parse("GlcNAc(b1-N)Gal").is_err());
    }

    #[test]
    fn canonical_notation_round_trips() {
        let mut tree = parse("aMan(a1-3)[aMan(a1-6)]Man(b1-4)GlcNAc(b1-4)GlcNAc(b1-N)").unwrap();
        let rendered = to_iupac(&tree);
        assert_eq!(
            rendered,
            "Man(a1-3)[Man(a1-6)]Man(b1-4)GlcNAc(b1-4)GlcNAc(b1-N)"
        );
        assert_eq!(parse(&rendered).unwrap().nodes, tree.nodes);

        // A free glycan renders the reducing anomer with an empty aglycon slot.
        tree.aglycon = None;
        assert_eq!(
            to_iupac(&tree),
            "Man(a1-3)[Man(a1-6)]Man(b1-4)GlcNAc(b1-4)GlcNAc(b1-)"
        );
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

        // Both arms are alpha-mannose; only the core mannose is beta.
        assert_eq!(tree.nodes[branch_man].mono.anomer, Anomer::Beta);
        for (_, arm) in &tree.nodes[branch_man].children {
            assert_eq!(tree.nodes[*arm].mono.anomer, Anomer::Alpha);
        }
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
