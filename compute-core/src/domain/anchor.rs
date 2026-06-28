//! Protein attachment sites a fragment can bond to. Glycosylation uses the
//! Asn/Ser/Thr side-chain atoms; the remaining variants are the side-chain and
//! terminal atoms targeted by post-translational modifications.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProteinAnchor {
    AsnNd2,
    SerOg,
    ThrOg1,
    TyrOh,
    HisNd1,
    HisNe2,
    LysNz,
    CysSg,
    ArgNh1,
    ArgNh2,
    // Backbone termini; no builder targets them.
    #[allow(dead_code)]
    NTerminus,
    #[allow(dead_code)]
    CTerminus,
}

impl ProteinAnchor {
    /// Three-letter residue name carrying the anchor, or `None` for the chain
    /// termini, which are not tied to a specific residue type.
    pub fn residue_name(self) -> Option<&'static str> {
        Some(match self {
            ProteinAnchor::AsnNd2 => "ASN",
            ProteinAnchor::SerOg => "SER",
            ProteinAnchor::ThrOg1 => "THR",
            ProteinAnchor::TyrOh => "TYR",
            ProteinAnchor::HisNd1 | ProteinAnchor::HisNe2 => "HIS",
            ProteinAnchor::LysNz => "LYS",
            ProteinAnchor::CysSg => "CYS",
            ProteinAnchor::ArgNh1 | ProteinAnchor::ArgNh2 => "ARG",
            ProteinAnchor::NTerminus | ProteinAnchor::CTerminus => return None,
        })
    }

    pub fn atom_name(self) -> &'static str {
        match self {
            ProteinAnchor::AsnNd2 => "ND2",
            ProteinAnchor::SerOg => "OG",
            ProteinAnchor::ThrOg1 => "OG1",
            ProteinAnchor::TyrOh => "OH",
            ProteinAnchor::HisNd1 => "ND1",
            ProteinAnchor::HisNe2 => "NE2",
            ProteinAnchor::LysNz => "NZ",
            ProteinAnchor::CysSg => "SG",
            ProteinAnchor::ArgNh1 => "NH1",
            ProteinAnchor::ArgNh2 => "NH2",
            ProteinAnchor::NTerminus => "N",
            ProteinAnchor::CTerminus => "C",
        }
    }

    /// Resolve a side-chain anchor from a residue/atom name pair. Termini are
    /// not resolvable here: their atom names (`N`/`C`) are shared by every
    /// residue, so they require chain-position context the caller must supply.
    pub fn from_residue_atom(residue_name: &str, atom_name: &str) -> Option<Self> {
        match (residue_name.trim(), atom_name) {
            ("ASN", "ND2") => Some(ProteinAnchor::AsnNd2),
            ("SER", "OG") => Some(ProteinAnchor::SerOg),
            ("THR", "OG1") => Some(ProteinAnchor::ThrOg1),
            ("TYR", "OH") => Some(ProteinAnchor::TyrOh),
            ("HIS", "ND1") => Some(ProteinAnchor::HisNd1),
            ("HIS", "NE2") => Some(ProteinAnchor::HisNe2),
            ("LYS", "NZ") => Some(ProteinAnchor::LysNz),
            ("CYS", "SG") => Some(ProteinAnchor::CysSg),
            ("ARG", "NH1") => Some(ProteinAnchor::ArgNh1),
            ("ARG", "NH2") => Some(ProteinAnchor::ArgNh2),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glycan_anchors_round_trip() {
        for anchor in [
            ProteinAnchor::AsnNd2,
            ProteinAnchor::SerOg,
            ProteinAnchor::ThrOg1,
        ] {
            let residue = anchor.residue_name().unwrap();
            assert_eq!(
                ProteinAnchor::from_residue_atom(residue, anchor.atom_name()),
                Some(anchor)
            );
        }
    }

    #[test]
    fn ptm_side_chain_anchors_resolve() {
        assert_eq!(
            ProteinAnchor::from_residue_atom("TYR", "OH"),
            Some(ProteinAnchor::TyrOh)
        );
        assert_eq!(
            ProteinAnchor::from_residue_atom("CYS", "SG"),
            Some(ProteinAnchor::CysSg)
        );
        assert_eq!(
            ProteinAnchor::from_residue_atom("ARG", "NH2"),
            Some(ProteinAnchor::ArgNh2)
        );
    }

    #[test]
    fn termini_have_no_residue_name() {
        assert_eq!(ProteinAnchor::NTerminus.residue_name(), None);
        assert_eq!(ProteinAnchor::CTerminus.residue_name(), None);
    }
}
