//! Engine-neutral catalog of the biomolecular force fields the MD System Builder
//! offers, plus a content-aware default pick.
//!
//! Each [`ForceFieldEntry`] pairs a `token` (the engine-facing selector value an
//! adapter passes to its topology generator) with a short `title` for the UI. The
//! full catalog is always shown — a force field and water model still have to be
//! a valid combination, but that is the user's call, so the builder never hides
//! options. To make the common case effortless, [`recommended_force_field`]
//! inspects the structure and picks the force field that best fits proteins,
//! nucleic acids, crystals/materials, or small molecules.

use crate::domain::Structure;

/// One selectable force field: the engine selector value and its UI title.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ForceFieldEntry {
    /// Engine-facing identifier (e.g. the value passed to a topology generator).
    pub token: &'static str,
    /// Short label shown in menus (no description or citation).
    pub title: &'static str,
}

/// Every force field the builder offers, in menu order.
pub const FORCE_FIELDS: &[ForceFieldEntry] = &[
    ForceFieldEntry {
        token: "amber03",
        title: "AMBER03",
    },
    ForceFieldEntry {
        token: "amber14sb",
        title: "AMBER14SB",
    },
    ForceFieldEntry {
        token: "amber19sb",
        title: "AMBER19SB",
    },
    ForceFieldEntry {
        token: "amber94",
        title: "AMBER94",
    },
    ForceFieldEntry {
        token: "amber96",
        title: "AMBER96",
    },
    ForceFieldEntry {
        token: "amber99",
        title: "AMBER99",
    },
    ForceFieldEntry {
        token: "amber99sb",
        title: "AMBER99SB",
    },
    ForceFieldEntry {
        token: "amber99sb-ildn",
        title: "AMBER99SB-ILDN",
    },
    ForceFieldEntry {
        token: "amberGS",
        title: "AMBERGS",
    },
    ForceFieldEntry {
        token: "charmm27",
        title: "CHARMM27",
    },
    ForceFieldEntry {
        token: "gromos43a1",
        title: "GROMOS96 43a1",
    },
    ForceFieldEntry {
        token: "gromos43a2",
        title: "GROMOS96 43a2",
    },
    ForceFieldEntry {
        token: "gromos45a3",
        title: "GROMOS96 45a3",
    },
    ForceFieldEntry {
        token: "gromos53a5",
        title: "GROMOS96 53a5",
    },
    ForceFieldEntry {
        token: "gromos53a6",
        title: "GROMOS96 53a6",
    },
    ForceFieldEntry {
        token: "gromos54a7",
        title: "GROMOS96 54a7",
    },
    ForceFieldEntry {
        token: "oplsaa",
        title: "OPLS-AA/L",
    },
];

/// Default force-field token when the structure's content does not suggest a
/// better one (see [`recommended_force_field`]).
pub const DEFAULT_FORCE_FIELD: &str = "amber99sb-ildn";

/// The UI title for a force-field token, falling back to the token itself for an
/// unknown value (e.g. one set from a script).
pub fn force_field_title(token: &str) -> &str {
    FORCE_FIELDS
        .iter()
        .find(|e| e.token == token)
        .map(|e| e.title)
        .unwrap_or(token)
}

/// Broad structural class of a system, used to choose a default force field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemContent {
    /// Contains standard amino-acid residues.
    Protein,
    /// Contains nucleotide residues (DNA/RNA).
    NucleicAcid,
    /// Periodic with no recognized biopolymer — a crystal or bulk material.
    Crystal,
    /// Everything else: an isolated small molecule.
    SmallMolecule,
}

/// Classify a structure for the purpose of defaulting a force field. Protein
/// wins over nucleic acid when both are present (the common protein–DNA case is
/// still parameterized by an AMBER protein+nucleic field).
pub fn classify(structure: &Structure) -> SystemContent {
    if let Some(bio) = &structure.biopolymer {
        let has_protein = bio.residues.iter().any(|r| r.is_standard_amino_acid);
        if has_protein {
            return SystemContent::Protein;
        }
        if bio.residues.iter().any(|r| is_nucleotide(&r.residue_name)) {
            return SystemContent::NucleicAcid;
        }
    }
    if structure.cell.is_some() {
        SystemContent::Crystal
    } else {
        SystemContent::SmallMolecule
    }
}

/// The best default force-field token for a structure. The AMBER99SB-ILDN field
/// parameterizes both protein and nucleic systems; OPLS-AA/L is the broadest
/// all-atom organic field for crystals/materials and isolated small molecules.
pub fn recommended_force_field(structure: &Structure) -> &'static str {
    match classify(structure) {
        SystemContent::Protein | SystemContent::NucleicAcid => "amber99sb-ildn",
        SystemContent::Crystal | SystemContent::SmallMolecule => "oplsaa",
    }
}

/// Whether a residue name denotes a DNA or RNA nucleotide (common PDB spellings,
/// including 5'/3'-terminal and protonation variants).
fn is_nucleotide(residue_name: &str) -> bool {
    matches!(
        residue_name.trim(),
        "DA" | "DC"
            | "DG"
            | "DT"
            | "DU"
            | "DI"
            | "A"
            | "C"
            | "G"
            | "U"
            | "I"
            | "RA"
            | "RC"
            | "RG"
            | "RU"
            | "ADE"
            | "CYT"
            | "GUA"
            | "THY"
            | "URA"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::Structure;

    #[test]
    fn catalog_has_all_seventeen_with_unique_tokens() {
        assert_eq!(FORCE_FIELDS.len(), 17);
        let mut tokens: Vec<&str> = FORCE_FIELDS.iter().map(|e| e.token).collect();
        tokens.sort_unstable();
        let unique = tokens.len();
        tokens.dedup();
        assert_eq!(tokens.len(), unique, "force-field tokens must be unique");
    }

    #[test]
    fn title_lookup_falls_back_to_token() {
        assert_eq!(force_field_title("oplsaa"), "OPLS-AA/L");
        assert_eq!(force_field_title("amber99sb-ildn"), "AMBER99SB-ILDN");
        assert_eq!(force_field_title("custom-xyz"), "custom-xyz");
    }

    #[test]
    fn empty_small_molecule_defaults_to_opls() {
        let s = Structure::new("ethane", vec![]);
        assert_eq!(classify(&s), SystemContent::SmallMolecule);
        assert_eq!(recommended_force_field(&s), "oplsaa");
    }

    #[test]
    fn celled_nonbiopolymer_is_crystal_and_defaults_to_opls() {
        use crate::domain::UnitCell;
        let cell = UnitCell::from_parameters(5.0, 5.0, 5.0, 90.0, 90.0, 90.0);
        let s = Structure::with_cell("quartz", vec![], cell);
        assert_eq!(classify(&s), SystemContent::Crystal);
        assert_eq!(recommended_force_field(&s), "oplsaa");
    }
}
