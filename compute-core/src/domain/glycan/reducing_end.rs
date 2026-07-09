//! The reducing end's anomeric configuration.
//!
//! Every other residue takes its anomer from the linkage that attaches it to its
//! parent. The reducing end has no parent, so in standard IUPAC-condensed
//! notation nothing states its configuration — yet the protein junction it forms
//! is stereospecific. Rather than demand the caller spell it out, derive it: for
//! a given aglycon and reducing sugar the configuration is fixed by biology.

use anyhow::{Result, bail};

use super::dictionary;
use super::{Anomer, GlycanTree, GlycosylationKind, SugarKind};

/// The configuration a reducing sugar takes when it condenses onto a protein.
/// `None` where no single configuration is canonical, in which case the notation
/// has to state one.
pub fn canonical_anomer(kind: GlycosylationKind, sugar: SugarKind) -> Option<Anomer> {
    use GlycosylationKind::{NLinked, OLinked};
    use SugarKind::{Fuc, Gal, GalNAc, Glc, GlcNAc, Man, Xyl};

    Some(match (kind, sugar) {
        (NLinked, GlcNAc) => Anomer::Beta,
        (OLinked, GalNAc) => Anomer::Alpha, // mucin type
        (OLinked, GlcNAc) => Anomer::Beta,  // nucleocytoplasmic O-GlcNAc
        (OLinked, Man) => Anomer::Alpha,
        (OLinked, Fuc) => Anomer::Alpha,
        (OLinked, Glc) => Anomer::Beta,
        (OLinked, Gal) => Anomer::Beta, // collagen, on hydroxylysine
        (OLinked, Xyl) => Anomer::Beta, // proteoglycan linker
        _ => return None,
    })
}

/// Settle the reducing end's anomer, which [`super::parse`] leaves
/// [`Anomer::Unknown`] unless the notation stated one.
///
/// With no aglycon the sugar is free and takes the dictionary's default. With
/// one, the configuration is derived from `(aglycon, sugar)`; a notation that
/// states a configuration contradicting that pairing is an error rather than a
/// silent override, since it would build a stereochemistry that does not occur.
///
/// `override_anomer` suppresses that derivation outright, for the unusual
/// linkages the table does not know. It still may not contradict a configuration
/// the notation itself states — two explicit answers that disagree are a mistake,
/// not a precedence question — nor relax which sugar an aglycon accepts.
pub fn resolve_root_anomer(
    tree: &mut GlycanTree,
    kind: Option<GlycosylationKind>,
    override_anomer: Option<Anomer>,
) -> Result<()> {
    let root = &tree.nodes[tree.root];
    let sugar = root.mono.kind;
    let stated = root.mono.anomer;

    if let (Some(named), Some(requested)) = (tree.aglycon, kind)
        && named != requested
    {
        bail!(
            "the notation's reducing-end linkage says {}, but {} glycosylation was requested",
            named.name(),
            requested.name()
        );
    }

    if let Some(forced) = override_anomer {
        if forced == Anomer::Unknown {
            bail!("the reducing-end anomer override must be alpha or beta");
        }
        if stated != Anomer::Unknown && stated != forced {
            bail!(
                "the notation's reducing end is {}, but {} was requested",
                stated.name(),
                forced.name()
            );
        }
    }

    let Some(kind) = kind else {
        let resolved = match (override_anomer, stated) {
            (Some(forced), _) => forced,
            (None, Anomer::Unknown) => dictionary::default_anomer(sugar)
                .ok_or_else(|| anyhow::anyhow!("no default anomer for the reducing sugar"))?,
            (None, stated) => stated,
        };
        tree.nodes[tree.root].mono.anomer = resolved;
        return Ok(());
    };

    if kind == GlycosylationKind::NLinked && sugar != SugarKind::GlcNAc {
        bail!("N-linked glycosylation requires a GlcNAc reducing end");
    }
    // Now that the aglycon is settled, record it so the canonical rendering names it.
    tree.aglycon = Some(kind);

    // An explicit override answers the question the table exists to answer.
    if let Some(forced) = override_anomer {
        tree.nodes[tree.root].mono.anomer = forced;
        return Ok(());
    }

    let resolved = match (stated, canonical_anomer(kind, sugar)) {
        (Anomer::Unknown, Some(canonical)) => canonical,
        (Anomer::Unknown, None) => bail!(
            "no canonical anomeric configuration for a {} {sugar:?} reducing end; \
             state one with a reducing-end linkage such as `{token}(a1-`, the \
             `a`/`b` token prefix, or the reducing-end anomer override",
            kind.name(),
            token = dictionary::base_token(sugar).unwrap_or("Sugar"),
        ),
        (stated, Some(canonical)) if stated != canonical => bail!(
            "{} glycosylation of {sugar:?} is {}-configured, but the notation states {}",
            kind.name(),
            canonical.name(),
            stated.name(),
        ),
        (stated, _) => stated,
    };

    tree.nodes[tree.root].mono.anomer = resolved;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::glycan::parse;

    fn resolved(notation: &str, kind: Option<GlycosylationKind>) -> Result<Anomer> {
        forced(notation, kind, None)
    }

    fn forced(
        notation: &str,
        kind: Option<GlycosylationKind>,
        override_anomer: Option<Anomer>,
    ) -> Result<Anomer> {
        let mut tree = parse(notation)?;
        resolve_root_anomer(&mut tree, kind, override_anomer)?;
        Ok(tree.nodes[tree.root].mono.anomer)
    }

    #[test]
    fn n_linked_reducing_glcnac_is_beta() {
        let anomer = resolved("GlcNAc", Some(GlycosylationKind::NLinked)).unwrap();
        assert_eq!(anomer, Anomer::Beta);
    }

    #[test]
    fn o_linked_galnac_is_alpha_without_being_asked() {
        let anomer = resolved("GalNAc", Some(GlycosylationKind::OLinked)).unwrap();
        assert_eq!(anomer, Anomer::Alpha, "mucin-type O-GalNAc is alpha");
    }

    #[test]
    fn o_linked_glcnac_stays_beta() {
        let anomer = resolved("GlcNAc", Some(GlycosylationKind::OLinked)).unwrap();
        assert_eq!(anomer, Anomer::Beta);
    }

    #[test]
    fn a_free_glycan_takes_the_dictionary_default() {
        assert_eq!(resolved("GalNAc", None).unwrap(), Anomer::Beta);
        assert_eq!(resolved("Fuc", None).unwrap(), Anomer::Alpha);
    }

    #[test]
    fn an_explicit_reducing_anomer_survives_when_it_agrees() {
        assert_eq!(
            resolved("aGalNAc", Some(GlycosylationKind::OLinked)).unwrap(),
            Anomer::Alpha
        );
        assert_eq!(
            resolved("GlcNAc(b1-", Some(GlycosylationKind::NLinked)).unwrap(),
            Anomer::Beta
        );
    }

    #[test]
    fn a_contradicting_reducing_anomer_is_rejected() {
        let err = resolved("aGlcNAc", Some(GlycosylationKind::NLinked)).unwrap_err();
        let message = err.to_string();
        assert!(message.contains("beta"), "{message}");
        assert!(message.contains("alpha"), "{message}");
    }

    #[test]
    fn n_linked_demands_a_glcnac_reducing_end() {
        let err = resolved("Man", Some(GlycosylationKind::NLinked)).unwrap_err();
        assert!(err.to_string().contains("GlcNAc"));
    }

    #[test]
    fn an_uncanonical_o_linked_sugar_must_state_its_anomer() {
        let err = resolved("Neu5Ac", Some(GlycosylationKind::OLinked)).unwrap_err();
        assert!(err.to_string().contains("state one"), "{err}");
        assert_eq!(
            resolved("Neu5Ac(a2-", Some(GlycosylationKind::OLinked)).unwrap(),
            Anomer::Alpha,
            "stating it explicitly is accepted"
        );
    }

    /// The override answers the question the table exists to answer, so the table
    /// is not consulted — an alpha N-linked GlcNAc is buildable when asked for.
    #[test]
    fn an_override_suppresses_the_derivation() {
        let n = Some(GlycosylationKind::NLinked);
        assert_eq!(resolved("GlcNAc", n).unwrap(), Anomer::Beta);
        assert_eq!(
            forced("GlcNAc", n, Some(Anomer::Alpha)).unwrap(),
            Anomer::Alpha,
            "the override wins over the canonical beta"
        );
    }

    /// A sugar the table has no opinion on is buildable via the override alone.
    #[test]
    fn an_override_covers_what_the_table_does_not_know() {
        let o = Some(GlycosylationKind::OLinked);
        assert!(resolved("Neu5Ac", o).is_err());
        assert_eq!(
            forced("Neu5Ac", o, Some(Anomer::Alpha)).unwrap(),
            Anomer::Alpha
        );
    }

    #[test]
    fn an_override_applies_to_a_free_glycan_too() {
        assert_eq!(resolved("GalNAc", None).unwrap(), Anomer::Beta);
        assert_eq!(
            forced("GalNAc", None, Some(Anomer::Alpha)).unwrap(),
            Anomer::Alpha
        );
    }

    /// Two explicit answers that disagree are a mistake, not a precedence question.
    #[test]
    fn an_override_may_not_contradict_the_notation() {
        let err = forced("aGalNAc", None, Some(Anomer::Beta)).unwrap_err();
        assert!(err.to_string().contains("is alpha"), "{err}");

        let err = forced("GlcNAc(b1-", None, Some(Anomer::Alpha)).unwrap_err();
        assert!(err.to_string().contains("beta"), "{err}");

        // Agreeing is fine.
        assert_eq!(
            forced("aGalNAc", None, Some(Anomer::Alpha)).unwrap(),
            Anomer::Alpha
        );
    }

    /// The override settles stereochemistry, not which sugar an aglycon accepts.
    #[test]
    fn an_override_does_not_relax_the_n_linked_glcnac_rule() {
        let err = forced("Man", Some(GlycosylationKind::NLinked), Some(Anomer::Beta)).unwrap_err();
        assert!(err.to_string().contains("GlcNAc"), "{err}");
    }

    #[test]
    fn an_unknown_override_is_rejected() {
        let err = forced("GlcNAc", None, Some(Anomer::Unknown)).unwrap_err();
        assert!(err.to_string().contains("alpha or beta"), "{err}");
    }

    /// Resolving against an aglycon records it, so the canonical rendering names
    /// the junction the structure was actually built for.
    #[test]
    fn resolving_records_the_aglycon_for_rendering() {
        let mut tree = parse("GalNAc").unwrap();
        assert_eq!(tree.aglycon, None);
        resolve_root_anomer(&mut tree, Some(GlycosylationKind::OLinked), None).unwrap();
        assert_eq!(tree.aglycon, Some(GlycosylationKind::OLinked));
        assert_eq!(crate::domain::glycan::to_iupac(&tree), "GalNAc(a1-O)");

        let mut free = parse("GalNAc").unwrap();
        resolve_root_anomer(&mut free, None, None).unwrap();
        assert_eq!(free.aglycon, None);
        assert_eq!(crate::domain::glycan::to_iupac(&free), "GalNAc(b1-)");
    }

    #[test]
    fn a_named_aglycon_must_match_the_requested_kind() {
        let err = resolved("GlcNAc(b1-N)", Some(GlycosylationKind::OLinked)).unwrap_err();
        assert!(err.to_string().contains("N-linked"), "{err}");
        assert!(
            resolved("GlcNAc(b1-N)", Some(GlycosylationKind::NLinked)).is_ok(),
            "the marker agrees"
        );
    }
}
