use super::{AbsConfig, Anomer, Monosaccharide, RingForm, SugarKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MonosaccharideEntry {
    pub token: &'static str,
    pub mono: Monosaccharide,
    pub pdb_ccd: &'static str,
    pub charmm_rtp: &'static str,
    pub atoms: &'static [&'static str],
    pub anomeric_carbon: u8,
}

const HEXOPYRANOSE_ATOMS: &[&str] = &[
    "C1", "H1", "O1", "HO1", "C5", "H5", "O5", "C2", "H2", "O2", "HO2", "C3", "H3", "O3", "HO3",
    "C4", "H4", "O4", "HO4", "C6", "H61", "H62", "O6", "HO6",
];

const HEXNAC_ATOMS: &[&str] = &[
    "C1", "H1", "O1", "HO1", "C5", "H5", "O5", "C2", "H2", "N", "HN", "C", "O", "CT", "HT1", "HT2",
    "HT3", "C3", "H3", "O3", "HO3", "C4", "H4", "O4", "HO4", "C6", "H61", "H62", "O6", "HO6",
];

const FUCOSE_ATOMS: &[&str] = &[
    "C1", "H1", "O1", "HO1", "C5", "H5", "O5", "C2", "H2", "O2", "HO2", "C3", "H3", "O3", "HO3",
    "C4", "H4", "O4", "HO4", "C6", "H61", "H62", "H63",
];

const XYLOSE_ATOMS: &[&str] = &[
    "C1", "H1", "O1", "HO1", "C5", "H51", "H52", "O5", "C2", "H2", "O2", "HO2", "C3", "H3", "O3",
    "HO3", "C4", "H4", "O4", "HO4",
];

const URONATE_ATOMS: &[&str] = &[
    "C1", "H1", "O1", "HO1", "C5", "H5", "O5", "C2", "H2", "O2", "HO2", "C3", "H3", "O3", "HO3",
    "C4", "H4", "O4", "HO4", "C6", "O61", "O62",
];

const NEU5AC_ATOMS: &[&str] = &[
    "C1", "O11", "O12", "C2", "O2", "HO2", "C6", "H6", "O6", "C3", "H31", "H32", "C4", "H4", "O4",
    "HO4", "C5", "H5", "N", "HN", "C", "O", "CT", "HT1", "HT2", "HT3", "C7", "H7", "O7", "HO7",
    "C8", "H8", "O8", "HO8", "C9", "H91", "H92", "O9", "HO9",
];

const NEU5GC_ATOMS: &[&str] = &[
    "C1", "O11", "O12", "C2", "O2", "HO2", "C6", "H6", "O6", "C3", "H31", "H32", "C4", "H4", "O4",
    "HO4", "C5", "H5", "N", "HN", "C", "O", "C10", "H10", "H11", "O13", "HO13", "C7", "H7", "O7",
    "HO7", "C8", "H8", "O8", "HO8", "C9", "H91", "H92", "O9", "HO9",
];

const fn mono(
    kind: SugarKind,
    ring: RingForm,
    config: AbsConfig,
    anomer: Anomer,
) -> Monosaccharide {
    Monosaccharide {
        kind,
        ring,
        config,
        anomer,
    }
}

const TABLE: &[MonosaccharideEntry] = &[
    MonosaccharideEntry {
        token: "Glc",
        mono: mono(
            SugarKind::Glc,
            RingForm::Pyranose,
            AbsConfig::D,
            Anomer::Beta,
        ),
        pdb_ccd: "BGC",
        charmm_rtp: "BGLC",
        atoms: HEXOPYRANOSE_ATOMS,
        anomeric_carbon: 1,
    },
    MonosaccharideEntry {
        token: "aGlc",
        mono: mono(
            SugarKind::Glc,
            RingForm::Pyranose,
            AbsConfig::D,
            Anomer::Alpha,
        ),
        pdb_ccd: "GLC",
        charmm_rtp: "AGLC",
        atoms: HEXOPYRANOSE_ATOMS,
        anomeric_carbon: 1,
    },
    MonosaccharideEntry {
        token: "Gal",
        mono: mono(
            SugarKind::Gal,
            RingForm::Pyranose,
            AbsConfig::D,
            Anomer::Beta,
        ),
        pdb_ccd: "GAL",
        charmm_rtp: "BGAL",
        atoms: HEXOPYRANOSE_ATOMS,
        anomeric_carbon: 1,
    },
    MonosaccharideEntry {
        token: "aGal",
        mono: mono(
            SugarKind::Gal,
            RingForm::Pyranose,
            AbsConfig::D,
            Anomer::Alpha,
        ),
        pdb_ccd: "GLA",
        charmm_rtp: "AGAL",
        atoms: HEXOPYRANOSE_ATOMS,
        anomeric_carbon: 1,
    },
    MonosaccharideEntry {
        token: "Man",
        mono: mono(
            SugarKind::Man,
            RingForm::Pyranose,
            AbsConfig::D,
            Anomer::Beta,
        ),
        pdb_ccd: "BMA",
        charmm_rtp: "BMAN",
        atoms: HEXOPYRANOSE_ATOMS,
        anomeric_carbon: 1,
    },
    MonosaccharideEntry {
        token: "aMan",
        mono: mono(
            SugarKind::Man,
            RingForm::Pyranose,
            AbsConfig::D,
            Anomer::Alpha,
        ),
        pdb_ccd: "MAN",
        charmm_rtp: "AMAN",
        atoms: HEXOPYRANOSE_ATOMS,
        anomeric_carbon: 1,
    },
    MonosaccharideEntry {
        token: "Fuc",
        mono: mono(
            SugarKind::Fuc,
            RingForm::Pyranose,
            AbsConfig::L,
            Anomer::Alpha,
        ),
        pdb_ccd: "FUC",
        charmm_rtp: "AFUC",
        atoms: FUCOSE_ATOMS,
        anomeric_carbon: 1,
    },
    MonosaccharideEntry {
        token: "bFuc",
        mono: mono(
            SugarKind::Fuc,
            RingForm::Pyranose,
            AbsConfig::L,
            Anomer::Beta,
        ),
        pdb_ccd: "FUL",
        charmm_rtp: "BFUC",
        atoms: FUCOSE_ATOMS,
        anomeric_carbon: 1,
    },
    MonosaccharideEntry {
        token: "Xyl",
        mono: mono(
            SugarKind::Xyl,
            RingForm::Pyranose,
            AbsConfig::D,
            Anomer::Beta,
        ),
        pdb_ccd: "XYP",
        charmm_rtp: "BXYL",
        atoms: XYLOSE_ATOMS,
        anomeric_carbon: 1,
    },
    MonosaccharideEntry {
        token: "aXyl",
        mono: mono(
            SugarKind::Xyl,
            RingForm::Pyranose,
            AbsConfig::D,
            Anomer::Alpha,
        ),
        pdb_ccd: "XYS",
        charmm_rtp: "AXYL",
        atoms: XYLOSE_ATOMS,
        anomeric_carbon: 1,
    },
    MonosaccharideEntry {
        token: "GlcNAc",
        mono: mono(
            SugarKind::GlcNAc,
            RingForm::Pyranose,
            AbsConfig::D,
            Anomer::Beta,
        ),
        pdb_ccd: "NAG",
        charmm_rtp: "BGLCNA",
        atoms: HEXNAC_ATOMS,
        anomeric_carbon: 1,
    },
    MonosaccharideEntry {
        token: "aGlcNAc",
        mono: mono(
            SugarKind::GlcNAc,
            RingForm::Pyranose,
            AbsConfig::D,
            Anomer::Alpha,
        ),
        pdb_ccd: "NDG",
        charmm_rtp: "AGLCNA",
        atoms: HEXNAC_ATOMS,
        anomeric_carbon: 1,
    },
    MonosaccharideEntry {
        token: "GalNAc",
        mono: mono(
            SugarKind::GalNAc,
            RingForm::Pyranose,
            AbsConfig::D,
            Anomer::Beta,
        ),
        pdb_ccd: "NGA",
        charmm_rtp: "BGALNA",
        atoms: HEXNAC_ATOMS,
        anomeric_carbon: 1,
    },
    MonosaccharideEntry {
        token: "aGalNAc",
        mono: mono(
            SugarKind::GalNAc,
            RingForm::Pyranose,
            AbsConfig::D,
            Anomer::Alpha,
        ),
        pdb_ccd: "A2G",
        charmm_rtp: "AGALNA",
        atoms: HEXNAC_ATOMS,
        anomeric_carbon: 1,
    },
    MonosaccharideEntry {
        token: "ManNAc",
        mono: mono(
            SugarKind::ManNAc,
            RingForm::Pyranose,
            AbsConfig::D,
            Anomer::Beta,
        ),
        pdb_ccd: "BM3",
        charmm_rtp: "BMANNA",
        atoms: HEXNAC_ATOMS,
        anomeric_carbon: 1,
    },
    MonosaccharideEntry {
        token: "Neu5Ac",
        mono: mono(
            SugarKind::Neu5Ac,
            RingForm::Pyranose,
            AbsConfig::D,
            Anomer::Alpha,
        ),
        pdb_ccd: "SIA",
        charmm_rtp: "ANE5AC",
        atoms: NEU5AC_ATOMS,
        anomeric_carbon: 2,
    },
    MonosaccharideEntry {
        token: "bNeu5Ac",
        mono: mono(
            SugarKind::Neu5Ac,
            RingForm::Pyranose,
            AbsConfig::D,
            Anomer::Beta,
        ),
        pdb_ccd: "SLB",
        charmm_rtp: "BNE5AC",
        atoms: NEU5AC_ATOMS,
        anomeric_carbon: 2,
    },
    MonosaccharideEntry {
        token: "Neu5Gc",
        mono: mono(
            SugarKind::Neu5Gc,
            RingForm::Pyranose,
            AbsConfig::D,
            Anomer::Alpha,
        ),
        pdb_ccd: "NGC",
        charmm_rtp: "ANE5GC",
        atoms: NEU5GC_ATOMS,
        anomeric_carbon: 2,
    },
    MonosaccharideEntry {
        token: "GlcA",
        mono: mono(
            SugarKind::GlcA,
            RingForm::Pyranose,
            AbsConfig::D,
            Anomer::Beta,
        ),
        pdb_ccd: "BDP",
        charmm_rtp: "BGLCA",
        atoms: URONATE_ATOMS,
        anomeric_carbon: 1,
    },
    MonosaccharideEntry {
        token: "aGlcA",
        mono: mono(
            SugarKind::GlcA,
            RingForm::Pyranose,
            AbsConfig::D,
            Anomer::Alpha,
        ),
        pdb_ccd: "GCU",
        charmm_rtp: "AGLCA",
        atoms: URONATE_ATOMS,
        anomeric_carbon: 1,
    },
    MonosaccharideEntry {
        token: "IdoA",
        mono: mono(
            SugarKind::IdoA,
            RingForm::Pyranose,
            AbsConfig::L,
            Anomer::Alpha,
        ),
        pdb_ccd: "IDR",
        charmm_rtp: "AIDOA",
        atoms: URONATE_ATOMS,
        anomeric_carbon: 1,
    },
    MonosaccharideEntry {
        token: "GalA",
        mono: mono(
            SugarKind::GalA,
            RingForm::Pyranose,
            AbsConfig::D,
            Anomer::Alpha,
        ),
        pdb_ccd: "ADA",
        charmm_rtp: "AGALA",
        atoms: URONATE_ATOMS,
        anomeric_carbon: 1,
    },
];

pub fn lookup(token: &str) -> Option<MonosaccharideEntry> {
    TABLE.iter().copied().find(|entry| entry.token == token)
}

/// The entry realising an exact stereochemistry. `None` when the anomer is
/// unspecified, or when that anomer of the sugar has no dictionary entry
/// (α-ManNAc, β-Neu5Gc, …) — those carry no PDB CCD code or CHARMM residue.
pub fn entry_for(mono: Monosaccharide) -> Option<MonosaccharideEntry> {
    TABLE.iter().copied().find(|entry| entry.mono == mono)
}

/// Whether a token names its own anomer. Only the `a`/`b`-prefixed spellings do;
/// the bare ones (`Man`, `Fuc`, …) merely carry the dictionary's default, which
/// a linkage is free to override.
pub fn token_states_anomer(token: &str) -> bool {
    token.starts_with(['a', 'b'])
}

fn base_entry(kind: SugarKind) -> Option<MonosaccharideEntry> {
    TABLE
        .iter()
        .copied()
        .find(|entry| entry.mono.kind == kind && !token_states_anomer(entry.token))
}

/// The unprefixed spelling of a sugar, used to render canonical notation where
/// the anomer travels in the linkage rather than the token.
pub fn base_token(kind: SugarKind) -> Option<&'static str> {
    base_entry(kind).map(|entry| entry.token)
}

/// The anomer a bare token implies — the configuration a free sugar of this kind
/// takes when nothing states otherwise.
pub fn default_anomer(kind: SugarKind) -> Option<Anomer> {
    base_entry(kind).map(|entry| entry.mono.anomer)
}

/// The anomeric carbon of a sugar, independent of its configuration — C2 for the
/// sialic acids, C1 elsewhere.
pub fn anomeric_carbon(kind: SugarKind) -> Option<u8> {
    base_entry(kind).map(|entry| entry.anomeric_carbon)
}

pub fn supported_tokens() -> Vec<&'static str> {
    TABLE.iter().map(|entry| entry.token).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_tokens_resolve_to_expected_ccd_codes() {
        assert_eq!(lookup("GlcNAc").unwrap().pdb_ccd, "NAG");
        assert_eq!(lookup("Man").unwrap().pdb_ccd, "BMA");
        assert_eq!(lookup("aMan").unwrap().pdb_ccd, "MAN");
        assert_eq!(lookup("Gal").unwrap().pdb_ccd, "GAL");
        assert_eq!(lookup("Fuc").unwrap().pdb_ccd, "FUC");
        assert_eq!(lookup("Neu5Ac").unwrap().pdb_ccd, "SIA");
        assert_eq!(lookup("Glc").unwrap().pdb_ccd, "BGC");
        assert_eq!(lookup("aGlc").unwrap().pdb_ccd, "GLC");
    }

    #[test]
    fn neu5ac_anomeric_carbon_is_c2() {
        assert_eq!(lookup("Neu5Ac").unwrap().anomeric_carbon, 2);
        assert_eq!(lookup("bNeu5Ac").unwrap().anomeric_carbon, 2);
    }

    #[test]
    fn hexopyranose_roster_carries_full_carbon_ladder() {
        let entry = lookup("aGlc").unwrap();
        for name in ["C1", "C2", "C3", "C4", "C5", "C6", "O5"] {
            assert!(entry.atoms.contains(&name), "missing {name}");
        }
        assert!(entry.atoms.contains(&"O1"));
        assert!(entry.atoms.contains(&"HO1"));
    }

    #[test]
    fn hexnac_roster_carries_acetamido_group() {
        let entry = lookup("GlcNAc").unwrap();
        for name in ["N", "HN", "C", "O", "CT", "HT1", "HT2", "HT3"] {
            assert!(entry.atoms.contains(&name), "missing {name}");
        }
    }

    #[test]
    fn uronate_roster_uses_charmm_carboxylate_names() {
        let entry = lookup("GlcA").unwrap();
        assert!(entry.atoms.contains(&"O61"));
        assert!(entry.atoms.contains(&"O62"));
        assert!(!entry.atoms.contains(&"O6A"));
        assert!(!entry.atoms.contains(&"O6B"));
    }

    #[test]
    fn sialic_roster_uses_charmm_names() {
        let entry = lookup("Neu5Ac").unwrap();
        for name in [
            "O11", "O12", "N", "HN", "C", "O", "CT", "HT1", "HT2", "HT3", "O6",
        ] {
            assert!(entry.atoms.contains(&name), "Neu5Ac missing {name}");
        }
        assert!(!entry.atoms.contains(&"O1A"));
        assert!(!entry.atoms.contains(&"N5"));
        let gc = lookup("Neu5Gc").unwrap();
        for name in ["C10", "H10", "H11", "O13", "HO13"] {
            assert!(gc.atoms.contains(&name), "Neu5Gc missing {name}");
        }
        assert!(!gc.atoms.contains(&"CT"));
    }

    #[test]
    fn unknown_token_returns_none() {
        assert!(lookup("Bogus").is_none());
    }

    #[test]
    fn only_prefixed_tokens_state_their_anomer() {
        for token in ["Man", "Fuc", "GlcNAc", "Neu5Ac", "IdoA", "GalA"] {
            assert!(!token_states_anomer(token), "{token}");
        }
        for token in ["aMan", "bFuc", "aGlcNAc", "bNeu5Ac"] {
            assert!(token_states_anomer(token), "{token}");
        }
    }

    #[test]
    fn base_token_and_default_anomer_come_from_the_unprefixed_spelling() {
        assert_eq!(base_token(SugarKind::Man), Some("Man"));
        assert_eq!(base_token(SugarKind::Fuc), Some("Fuc"));
        assert_eq!(default_anomer(SugarKind::Man), Some(Anomer::Beta));
        assert_eq!(default_anomer(SugarKind::Fuc), Some(Anomer::Alpha));
        assert_eq!(anomeric_carbon(SugarKind::Neu5Ac), Some(2));
        assert_eq!(anomeric_carbon(SugarKind::Man), Some(1));
    }

    #[test]
    fn entry_for_resolves_an_exact_stereochemistry() {
        let alpha_man = mono(
            SugarKind::Man,
            RingForm::Pyranose,
            AbsConfig::D,
            Anomer::Alpha,
        );
        assert_eq!(entry_for(alpha_man).unwrap().pdb_ccd, "MAN");

        let alpha_mannac = mono(
            SugarKind::ManNAc,
            RingForm::Pyranose,
            AbsConfig::D,
            Anomer::Alpha,
        );
        assert!(
            entry_for(alpha_mannac).is_none(),
            "the dictionary carries no alpha-ManNAc residue"
        );
    }

    /// Every sugar in the table must have an unprefixed spelling, or canonical
    /// notation could not be rendered for it.
    #[test]
    fn every_sugar_has_a_base_token() {
        for entry in TABLE {
            assert!(
                base_token(entry.mono.kind).is_some(),
                "{:?} has no unprefixed token",
                entry.mono.kind
            );
        }
    }

    #[test]
    fn supported_tokens_lists_core_residues() {
        let tokens = supported_tokens();
        for token in [
            "Glc", "Gal", "Man", "Fuc", "Xyl", "GlcNAc", "Neu5Ac", "GlcA", "IdoA",
        ] {
            assert!(tokens.contains(&token), "missing {token}");
        }
    }
}
