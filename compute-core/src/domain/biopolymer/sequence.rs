use super::{ResidueRecord, is_nucleic_acid_residue, is_standard_amino_acid};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ResiduePolymerKind {
    Protein,
    NucleicAcid,
}

pub fn residue_polymer_kind(
    residue_name: &str,
    residue: &ResidueRecord,
) -> Option<ResiduePolymerKind> {
    if is_standard_amino_acid(residue_name) || residue.has_peptide_backbone() {
        return Some(ResiduePolymerKind::Protein);
    }

    if is_nucleic_acid_residue(residue_name) {
        return Some(ResiduePolymerKind::NucleicAcid);
    }

    None
}

pub fn residue_sequence_symbol(residue_name: &str, kind: ResiduePolymerKind) -> char {
    match kind {
        ResiduePolymerKind::Protein => amino_acid_sequence_symbol(residue_name).unwrap_or('X'),
        ResiduePolymerKind::NucleicAcid => {
            nucleic_acid_sequence_symbol(residue_name).unwrap_or('N')
        }
    }
}

fn amino_acid_sequence_symbol(residue_name: &str) -> Option<char> {
    Some(match normalized_residue_name(residue_name).as_str() {
        "ALA" => 'A',
        "ARG" => 'R',
        "ASN" => 'N',
        "ASP" => 'D',
        "CYS" => 'C',
        "GLN" => 'Q',
        "GLU" => 'E',
        "GLY" => 'G',
        "HIS" => 'H',
        "ILE" => 'I',
        "LEU" => 'L',
        "LYS" => 'K',
        "MET" => 'M',
        "PHE" => 'F',
        "PRO" => 'P',
        "SER" => 'S',
        "THR" => 'T',
        "TRP" => 'W',
        "TYR" => 'Y',
        "VAL" => 'V',
        _ => return None,
    })
}

fn nucleic_acid_sequence_symbol(residue_name: &str) -> Option<char> {
    Some(match normalized_residue_name(residue_name).as_str() {
        "A" | "DA" | "RA" | "ADE" => 'A',
        "C" | "DC" | "RC" | "CYT" => 'C',
        "G" | "DG" | "RG" | "GUA" => 'G',
        "T" | "DT" | "THY" => 'T',
        "U" | "DU" | "RU" | "URA" => 'U',
        "I" | "DI" => 'I',
        _ => return None,
    })
}

fn normalized_residue_name(residue_name: &str) -> String {
    residue_name.trim().to_ascii_uppercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::biopolymer::ResidueId;

    fn residue(
        alpha_carbon: Option<usize>,
        backbone_nitrogen: Option<usize>,
        backbone_carbon: Option<usize>,
    ) -> ResidueRecord {
        ResidueRecord {
            id: ResidueId::new('A', 1, ' '),
            residue_name: "ALA".to_string(),
            atom_indices: Vec::new(),
            alpha_carbon,
            backbone_nitrogen,
            backbone_carbon,
            backbone_oxygen: None,
            is_standard_amino_acid: true,
        }
    }

    #[test]
    fn residue_sequence_symbol_maps_standard_amino_acids() {
        for (residue_name, symbol) in [
            ("ALA", 'A'),
            ("ARG", 'R'),
            ("ASN", 'N'),
            ("ASP", 'D'),
            ("CYS", 'C'),
            ("GLN", 'Q'),
            ("GLU", 'E'),
            ("GLY", 'G'),
            ("HIS", 'H'),
            ("ILE", 'I'),
            ("LEU", 'L'),
            ("LYS", 'K'),
            ("MET", 'M'),
            ("PHE", 'F'),
            ("PRO", 'P'),
            ("SER", 'S'),
            ("THR", 'T'),
            ("TRP", 'W'),
            ("TYR", 'Y'),
            ("VAL", 'V'),
        ] {
            assert_eq!(
                residue_sequence_symbol(residue_name, ResiduePolymerKind::Protein),
                symbol,
                "{residue_name}"
            );
        }
        assert_eq!(
            residue_sequence_symbol("MSE", ResiduePolymerKind::Protein),
            'X'
        );
    }

    #[test]
    fn residue_sequence_symbol_maps_common_nucleic_acids() {
        for (residue_name, symbol) in [
            ("DA", 'A'),
            ("DC", 'C'),
            ("DG", 'G'),
            ("DT", 'T'),
            ("A", 'A'),
            ("C", 'C'),
            ("G", 'G'),
            ("U", 'U'),
            ("RA", 'A'),
            ("RC", 'C'),
            ("RG", 'G'),
            ("RU", 'U'),
            ("ADE", 'A'),
            ("CYT", 'C'),
            ("GUA", 'G'),
            ("THY", 'T'),
            ("URA", 'U'),
            ("DI", 'I'),
        ] {
            assert_eq!(
                residue_sequence_symbol(residue_name, ResiduePolymerKind::NucleicAcid),
                symbol,
                "{residue_name}"
            );
        }
        assert_eq!(
            residue_sequence_symbol("PSU", ResiduePolymerKind::NucleicAcid),
            'N'
        );
    }

    #[test]
    fn residue_polymer_kind_classifies_sequence_supported_residues() {
        let standard = residue(None, None, None);
        assert_eq!(
            residue_polymer_kind("ALA", &standard),
            Some(ResiduePolymerKind::Protein)
        );

        let non_standard_backbone = ResidueRecord {
            residue_name: "MSE".to_string(),
            is_standard_amino_acid: false,
            ..residue(Some(0), Some(1), Some(2))
        };
        assert_eq!(
            residue_polymer_kind("MSE", &non_standard_backbone),
            Some(ResiduePolymerKind::Protein)
        );
        assert_eq!(
            residue_sequence_symbol("MSE", ResiduePolymerKind::Protein),
            'X'
        );

        let nucleic = ResidueRecord {
            residue_name: "DG".to_string(),
            is_standard_amino_acid: false,
            ..residue(None, None, None)
        };
        assert_eq!(
            residue_polymer_kind("DG", &nucleic),
            Some(ResiduePolymerKind::NucleicAcid)
        );

        let water = ResidueRecord {
            residue_name: "HOH".to_string(),
            is_standard_amino_acid: false,
            ..residue(None, None, None)
        };
        assert_eq!(residue_polymer_kind("HOH", &water), None);
    }
}
