use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ResidueId {
    pub chain_id: char,
    pub sequence_number: i32,
    pub insertion_code: char,
}

impl ResidueId {
    pub fn new(chain_id: char, sequence_number: i32, insertion_code: char) -> Self {
        Self {
            chain_id,
            sequence_number,
            insertion_code,
        }
    }

    pub fn ordering_key(&self) -> (i32, u8) {
        (self.sequence_number, self.insertion_code as u8)
    }
}

#[derive(Debug, Clone)]
pub struct ResidueRecord {
    pub id: ResidueId,
    pub residue_name: String,
    pub atom_indices: Vec<usize>,
    pub alpha_carbon: Option<usize>,
    pub is_standard_amino_acid: bool,
}

#[derive(Debug, Clone)]
pub struct ChainRecord {
    pub id: char,
    pub residue_indices: Vec<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecondaryStructureKind {
    Helix,
    Sheet,
}

#[derive(Debug, Clone)]
pub struct SecondaryStructureSpan {
    pub kind: SecondaryStructureKind,
    pub start: ResidueId,
    pub end: ResidueId,
}

#[derive(Debug, Clone)]
pub struct Biopolymer {
    pub residues: Vec<ResidueRecord>,
    pub chains: Vec<ChainRecord>,
    pub secondary_structures: Vec<SecondaryStructureSpan>,
    pub residue_for_atom: Vec<Option<usize>>,
    /// PDB atom name (e.g. "CA", "CB", "HB1") for each atom, indexed by global
    /// atom index, parallel to [`Self::residue_for_atom`]. Preserved from the
    /// source file so a structure can be matched against force-field residue
    /// templates (RTP), which key on (residue_name, atom_name). `None` for atoms
    /// with no recorded name (e.g. hydrogens added after load).
    pub atom_name_for_atom: Vec<Option<String>>,
}

impl Biopolymer {
    pub fn is_compatible_with_atom_count(&self, atom_count: usize) -> bool {
        self.residue_for_atom.len() == atom_count
    }

    /// The PDB atom name recorded for an atom, if any.
    pub fn atom_name(&self, atom_index: usize) -> Option<&str> {
        self.atom_name_for_atom
            .get(atom_index)
            .and_then(|name| name.as_deref())
    }
}

#[derive(Debug, Clone)]
pub struct PdbAtomAnnotation {
    pub atom_name: String,
    pub residue_name: String,
    pub chain_id: char,
    pub residue_seq: i32,
    pub insertion_code: char,
}

impl PdbAtomAnnotation {
    pub fn residue_id(&self) -> ResidueId {
        ResidueId::new(self.chain_id, self.residue_seq, self.insertion_code)
    }
}

pub fn build_biopolymer(
    atom_annotations: &[PdbAtomAnnotation],
    secondary_structures: Vec<SecondaryStructureSpan>,
) -> Option<Biopolymer> {
    let mut residues = Vec::new();
    let mut chains = Vec::new();
    let mut residue_for_atom = vec![None; atom_annotations.len()];
    let atom_name_for_atom: Vec<Option<String>> = atom_annotations
        .iter()
        .map(|annotation| Some(annotation.atom_name.clone()))
        .collect();
    let mut residue_index_by_id = HashMap::new();
    let mut chain_index_by_id = HashMap::new();

    for (atom_index, annotation) in atom_annotations.iter().enumerate() {
        let residue_id = annotation.residue_id();
        let residue_index = if let Some(&existing_index) = residue_index_by_id.get(&residue_id) {
            existing_index
        } else {
            let index = residues.len();
            residues.push(ResidueRecord {
                id: residue_id.clone(),
                residue_name: annotation.residue_name.clone(),
                atom_indices: Vec::new(),
                alpha_carbon: None,
                is_standard_amino_acid: is_standard_amino_acid(&annotation.residue_name),
            });
            residue_index_by_id.insert(residue_id.clone(), index);

            let chain_index =
                if let Some(&existing_index) = chain_index_by_id.get(&annotation.chain_id) {
                    existing_index
                } else {
                    let index = chains.len();
                    chains.push(ChainRecord {
                        id: annotation.chain_id,
                        residue_indices: Vec::new(),
                    });
                    chain_index_by_id.insert(annotation.chain_id, index);
                    index
                };
            chains[chain_index].residue_indices.push(index);
            index
        };

        let residue = &mut residues[residue_index];
        residue.atom_indices.push(atom_index);
        if annotation.atom_name == "CA" {
            residue.alpha_carbon = Some(atom_index);
        }
        residue_for_atom[atom_index] = Some(residue_index);
    }

    let has_protein_residues = residues
        .iter()
        .any(|residue| residue.is_standard_amino_acid);
    if !has_protein_residues && secondary_structures.is_empty() {
        return None;
    }

    Some(Biopolymer {
        residues,
        chains,
        secondary_structures,
        residue_for_atom,
        atom_name_for_atom,
    })
}

/// Broad chemical class of an atom, used for "quick select" of all atoms of a
/// kind (protein, solvent, …) and for choosing sensible default styles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum AtomCategory {
    Protein,
    NucleicAcid,
    /// A small molecule / hetero group that is neither polymer, solvent, nor a
    /// monatomic ion.
    Ligand,
    /// Explicit solvent (water).
    Solvent,
    /// A free monatomic ion (counter-ion, salt, structural metal).
    Ion,
    /// Anything that could not be classified (e.g. a bare structure with no
    /// residue metadata).
    Other,
}

impl AtomCategory {
    /// Every category, in canonical order.
    pub fn all() -> &'static [Self] {
        &[
            Self::Protein,
            Self::NucleicAcid,
            Self::Ligand,
            Self::Solvent,
            Self::Ion,
            Self::Other,
        ]
    }

    /// The categories offered as one-click selections, in menu order.
    pub fn selectable() -> &'static [Self] {
        &[
            Self::Protein,
            Self::NucleicAcid,
            Self::Ligand,
            Self::Solvent,
            Self::Ion,
        ]
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Protein => "Protein",
            Self::NucleicAcid => "Nucleic acid",
            Self::Ligand => "Ligand",
            Self::Solvent => "Solvent",
            Self::Ion => "Ion",
            Self::Other => "Other",
        }
    }

    /// Stable string token for persistence.
    pub fn token(self) -> &'static str {
        match self {
            Self::Protein => "protein",
            Self::NucleicAcid => "nucleic",
            Self::Ligand => "ligand",
            Self::Solvent => "solvent",
            Self::Ion => "ion",
            Self::Other => "other",
        }
    }

    pub fn from_token(token: &str) -> Option<Self> {
        Some(match token {
            "protein" => Self::Protein,
            "nucleic" => Self::NucleicAcid,
            "ligand" => Self::Ligand,
            "solvent" => Self::Solvent,
            "ion" => Self::Ion,
            "other" => Self::Other,
            _ => return None,
        })
    }
}

/// Standard PDB residue names for water / explicit solvent.
pub fn is_water_residue(residue_name: &str) -> bool {
    matches!(residue_name, "HOH" | "WAT" | "H2O" | "SOL" | "TIP3" | "TIP")
}

/// Standard PDB/GROMACS residue names for nucleic-acid residues (DNA and RNA),
/// including legacy three-letter forms.
pub fn is_nucleic_acid_residue(residue_name: &str) -> bool {
    matches!(
        residue_name,
        "DA" | "DC"
            | "DG"
            | "DT"
            | "DI"
            | "DU"
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

/// Common PDB/lipid-builder residue names for membrane lipids and sterols
/// (CHARMM-GUI / Slipids / AMBER Lipid conventions). Used to recognize a membrane
/// system; not exhaustive — an unrecognized lipid is reported as "not detected"
/// rather than misclassified, which is why detection is paired with a manual
/// override.
pub fn is_lipid_residue(residue_name: &str) -> bool {
    matches!(
        residue_name.trim(),
        // Phosphatidylcholine / -ethanolamine / -glycerol / -serine / -inositol
        "POPC" | "POPE" | "POPG" | "POPS" | "POPI" | "POPA"
            | "DOPC" | "DOPE" | "DOPG" | "DOPS"
            | "DPPC" | "DPPE" | "DPPG"
            | "DMPC" | "DMPE" | "DMPG"
            | "DLPC" | "DSPC" | "DSPE"
            | "PLPC" | "SOPC" | "OPPC"
            // Sterols
            | "CHOL" | "CHL1" | "ERG"
            // Sphingo / other
            | "PSM" | "SSM" | "DPSM" | "CER"
    )
}

/// A non-polymer residue (water molecule or ion) to splice into a biopolymer's
/// per-atom coverage so that a solvated system stays self-consistent
/// (`residue_for_atom.len() == atoms.len()`).
#[derive(Debug, Clone)]
pub struct AppendedResidue {
    pub residue_name: String,
    pub chain_id: char,
    pub sequence_number: i32,
    /// `(global_atom_index, atom_name)` for each atom in the residue.
    pub atoms: Vec<(usize, String)>,
}

/// Extend a biopolymer (or build one from scratch) so its per-atom coverage
/// spans `total_atom_count`, splicing in `appended` solvent/ion residues. The
/// `base` biopolymer is assumed to cover atoms `0..solute_atom_count`.
///
/// Returns `None` only when there is nothing to describe (no base biopolymer and
/// no appended residues).
pub fn extend_biopolymer_coverage(
    base: Option<&Biopolymer>,
    solute_atom_count: usize,
    total_atom_count: usize,
    appended: &[AppendedResidue],
) -> Option<Biopolymer> {
    if base.is_none() && appended.is_empty() {
        return None;
    }

    let mut residues = base.map(|b| b.residues.clone()).unwrap_or_default();
    let mut chains = base.map(|b| b.chains.clone()).unwrap_or_default();
    let secondary_structures = base
        .map(|b| b.secondary_structures.clone())
        .unwrap_or_default();

    let mut residue_for_atom = base
        .map(|b| b.residue_for_atom.clone())
        .unwrap_or_else(|| vec![None; solute_atom_count]);
    let mut atom_name_for_atom = base
        .map(|b| b.atom_name_for_atom.clone())
        .unwrap_or_else(|| vec![None; solute_atom_count]);
    residue_for_atom.resize(total_atom_count, None);
    atom_name_for_atom.resize(total_atom_count, None);

    let mut chain_index_by_id: HashMap<char, usize> = chains
        .iter()
        .enumerate()
        .map(|(index, chain)| (chain.id, index))
        .collect();

    for appended_residue in appended {
        let residue_index = residues.len();
        residues.push(ResidueRecord {
            id: ResidueId::new(
                appended_residue.chain_id,
                appended_residue.sequence_number,
                ' ',
            ),
            residue_name: appended_residue.residue_name.clone(),
            atom_indices: appended_residue
                .atoms
                .iter()
                .map(|(atom_index, _)| *atom_index)
                .collect(),
            alpha_carbon: None,
            is_standard_amino_acid: false,
        });

        let chain_index = *chain_index_by_id
            .entry(appended_residue.chain_id)
            .or_insert_with(|| {
                let index = chains.len();
                chains.push(ChainRecord {
                    id: appended_residue.chain_id,
                    residue_indices: Vec::new(),
                });
                index
            });
        chains[chain_index].residue_indices.push(residue_index);

        for (atom_index, atom_name) in &appended_residue.atoms {
            if let Some(slot) = residue_for_atom.get_mut(*atom_index) {
                *slot = Some(residue_index);
            }
            if let Some(slot) = atom_name_for_atom.get_mut(*atom_index) {
                *slot = Some(atom_name.clone());
            }
        }
    }

    Some(Biopolymer {
        residues,
        chains,
        secondary_structures,
        residue_for_atom,
        atom_name_for_atom,
    })
}

pub fn is_standard_amino_acid(residue_name: &str) -> bool {
    matches!(
        residue_name,
        "ALA"
            | "ARG"
            | "ASN"
            | "ASP"
            | "CYS"
            | "GLN"
            | "GLU"
            | "GLY"
            | "HIS"
            | "ILE"
            | "LEU"
            | "LYS"
            | "MET"
            | "PHE"
            | "PRO"
            | "SER"
            | "THR"
            | "TRP"
            | "TYR"
            | "VAL"
    )
}
