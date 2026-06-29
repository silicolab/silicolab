use std::collections::HashMap;

use super::structure::Atom;

mod sequence;
pub use sequence::{ResiduePolymerKind, residue_polymer_kind, residue_sequence_symbol};

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
    pub backbone_nitrogen: Option<usize>,
    pub backbone_carbon: Option<usize>,
    /// Optional even for a complete peptide unit: chain termini and coarse inputs
    /// may omit the carbonyl O. Its presence is what lets the DSSP H-bond
    /// assignment run.
    pub backbone_oxygen: Option<usize>,
    pub is_standard_amino_acid: bool,
}

impl ResidueRecord {
    /// Whether this residue carries a complete peptide backbone — the amide N,
    /// the α-carbon, and the carbonyl C. This is the topological prerequisite for
    /// drawing the residue as a cartoon ribbon: it depends only on which backbone
    /// atoms are present, never on the residue name, so force-field-protonated,
    /// disulfide, and otherwise renamed protein residues are recognized exactly
    /// like their canonical forms. The carbonyl O is deliberately not required —
    /// chain termini and some coarse inputs omit it.
    pub fn has_peptide_backbone(&self) -> bool {
        self.backbone_nitrogen.is_some()
            && self.alpha_carbon.is_some()
            && self.backbone_carbon.is_some()
    }

    /// Whether this residue can contribute one C-alpha control point to a protein
    /// cartoon trace. A full peptide backbone is accepted independent of residue
    /// name; a C-alpha-only trace has no topology to distinguish it from hetero atoms,
    /// so it keeps the legacy standard-amino-acid gate.
    pub fn has_cartoon_trace(&self) -> bool {
        self.alpha_carbon.is_some() && (self.has_peptide_backbone() || self.is_standard_amino_acid)
    }
}
/// Upper bound, in ångström, on the C(i)–N(i+1) peptide bond joining two
/// consecutive residues. A real peptide bond is ~1.33 Å; the generous ceiling
/// tolerates strained or coarse coordinates while still rejecting a chain break,
/// where the next residue's amide nitrogen sits a missing residue's width away.
const MAX_PEPTIDE_BOND: f32 = 2.0;

/// Upper bound, in ångström, on the Cα(i)–Cα(i+1) separation, used as a fallback
/// when carbonyl/amide atoms are absent (a terminus or a Cα-only trace). Bonded
/// α-carbons sit ~3.8 Å apart (trans) or ~2.9 Å (cis); a one-residue gap is
/// ~7.6 Å, well clear of this threshold.
const MAX_ALPHA_CARBON_STEP: f32 = 4.5;

/// Whether `current` is the immediate backbone successor of `previous`, judged
/// from coordinates alone. Prefers the defining peptide bond C(i)–N(i+1); when
/// either atom is absent it falls back to Cα–Cα proximity. It consults no residue
/// names or sequence numbers, so it survives renumbering, insertion codes, and
/// gaps — two residues are contiguous only when their backbones are actually
/// bonded in space.
///
/// Callers may pass either full peptide residues or legacy standard-residue
/// C-alpha traces; the latter take the C-alpha distance fallback.
pub fn residues_backbone_bonded(
    previous: &ResidueRecord,
    current: &ResidueRecord,
    atoms: &[Atom],
) -> bool {
    if let (Some(carbon), Some(nitrogen)) = (previous.backbone_carbon, current.backbone_nitrogen) {
        return atoms_within(atoms, carbon, nitrogen, MAX_PEPTIDE_BOND);
    }
    match (previous.alpha_carbon, current.alpha_carbon) {
        (Some(prev_ca), Some(cur_ca)) => {
            atoms_within(atoms, prev_ca, cur_ca, MAX_ALPHA_CARBON_STEP)
        }
        _ => false,
    }
}

fn atoms_within(atoms: &[Atom], a: usize, b: usize, max: f32) -> bool {
    match (atoms.get(a), atoms.get(b)) {
        (Some(a), Some(b)) => (a.position - b.position).norm_squared() <= max * max,
        _ => false,
    }
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
                backbone_nitrogen: None,
                backbone_carbon: None,
                backbone_oxygen: None,
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
        // The atom names "N"/"CA"/"C"/"O" are invariant across force-field
        // protonation, disulfide, and modified-residue renaming, unlike the
        // residue name — keying backbone identity on them lets renderability be
        // decided from topology alone.
        match annotation.atom_name.as_str() {
            "CA" => residue.alpha_carbon = Some(atom_index),
            "N" => residue.backbone_nitrogen = Some(atom_index),
            "C" => residue.backbone_carbon = Some(atom_index),
            "O" => residue.backbone_oxygen = Some(atom_index),
            _ => {}
        }
        residue_for_atom[atom_index] = Some(residue_index);
    }

    let has_overlay_residues = residues.iter().any(|residue| {
        residue.is_standard_amino_acid
            || residue.has_peptide_backbone()
            || is_nucleic_acid_residue(&residue.residue_name)
            || is_carbohydrate_residue(&residue.residue_name)
    });
    if !has_overlay_residues && secondary_structures.is_empty() {
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
    Carbohydrate,
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
            Self::Carbohydrate,
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
            Self::Carbohydrate,
            Self::Ligand,
            Self::Solvent,
            Self::Ion,
        ]
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Protein => "Protein",
            Self::NucleicAcid => "Nucleic acid",
            Self::Carbohydrate => "Carbohydrate",
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
            Self::Carbohydrate => "carbohydrate",
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
            "carbohydrate" => Self::Carbohydrate,
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
            | "T"
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
        // Backbone atoms are derived, not persisted: this must match how
        // `build_biopolymer` and the payload reload derive them, or renderability
        // would differ in memory versus after a save/load round trip.
        let backbone_atom = |target: &str| {
            appended_residue
                .atoms
                .iter()
                .find_map(|(atom_index, name)| (name.as_str() == target).then_some(*atom_index))
        };
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
            alpha_carbon: backbone_atom("CA"),
            backbone_nitrogen: backbone_atom("N"),
            backbone_carbon: backbone_atom("C"),
            backbone_oxygen: backbone_atom("O"),
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

pub fn is_carbohydrate_residue(residue_name: &str) -> bool {
    matches!(
        residue_name.trim(),
        "NAG"
            | "NDG"
            | "NGA"
            | "A2G"
            | "BM3"
            | "MAN"
            | "BMA"
            | "GAL"
            | "GLA"
            | "GLC"
            | "BGC"
            | "FUC"
            | "FUL"
            | "XYS"
            | "XYP"
            | "SIA"
            | "SLB"
            | "NGC"
            | "BDP"
            | "GCU"
            | "IDR"
            | "ADA"
    )
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

#[cfg(test)]
mod tests {
    use super::*;
    use nalgebra::Point3;

    fn atom_at(x: f32, y: f32, z: f32) -> Atom {
        Atom {
            element: "C".to_string(),
            position: Point3::new(x, y, z),
            charge: 0.0,
        }
    }

    #[test]
    fn carbohydrate_residue_table_matches_dictionary_ccd_codes() {
        for code in [
            "NAG", "MAN", "BMA", "GAL", "FUC", "SIA", "GLC", "BGC", "NDG", "A2G", "XYS",
        ] {
            assert!(is_carbohydrate_residue(code), "{code} not recognized");
        }
        assert!(!is_carbohydrate_residue("ALA"));
        assert!(!is_carbohydrate_residue("HOH"));
    }

    #[test]
    fn carbohydrate_atom_category_round_trips_through_token() {
        assert_eq!(
            AtomCategory::from_token(AtomCategory::Carbohydrate.token()),
            Some(AtomCategory::Carbohydrate)
        );
        assert!(AtomCategory::all().contains(&AtomCategory::Carbohydrate));
        assert!(AtomCategory::selectable().contains(&AtomCategory::Carbohydrate));
    }

    /// A residue record carrying only the backbone atom indices under test.
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
    fn nucleic_acid_only_annotations_build_biopolymer() {
        let annotations = [
            ("P", "DA", 1),
            ("C1'", "DA", 1),
            ("N9", "DA", 1),
            ("P", "DC", 2),
            ("C1'", "DC", 2),
            ("N1", "DC", 2),
        ]
        .into_iter()
        .map(|(atom_name, residue_name, residue_seq)| PdbAtomAnnotation {
            atom_name: atom_name.to_string(),
            residue_name: residue_name.to_string(),
            chain_id: 'A',
            residue_seq,
            insertion_code: ' ',
        })
        .collect::<Vec<_>>();

        let biopolymer = build_biopolymer(&annotations, Vec::new()).expect("biopolymer");
        assert_eq!(biopolymer.residues.len(), 2);
        assert!(biopolymer.residues.iter().all(|residue| {
            residue_polymer_kind(&residue.residue_name, residue)
                == Some(ResiduePolymerKind::NucleicAcid)
        }));
    }

    #[test]
    fn has_peptide_backbone_requires_n_ca_c() {
        assert!(residue(Some(0), Some(1), Some(2)).has_peptide_backbone());
        assert!(
            !residue(Some(0), None, Some(2)).has_peptide_backbone(),
            "missing N"
        );
        assert!(
            !residue(Some(0), Some(1), None).has_peptide_backbone(),
            "missing C"
        );
        // A calcium ion has a stray "CA" atom but no N/C — correctly not backbone.
        assert!(
            !residue(Some(0), None, None).has_peptide_backbone(),
            "Cα only"
        );
    }

    #[test]
    fn peptide_bond_decides_contiguity_when_carbonyl_and_amide_present() {
        // prev carbonyl C at 0; cur amide N at 1.33 Å (bonded) vs 5.0 Å (broken).
        let atoms = vec![
            atom_at(0.0, 0.0, 0.0),
            atom_at(1.33, 0.0, 0.0),
            atom_at(5.0, 0.0, 0.0),
        ];
        let prev = residue(None, None, Some(0));
        let cur_bonded = residue(None, Some(1), None);
        let cur_broken = residue(None, Some(2), None);
        assert!(residues_backbone_bonded(&prev, &cur_bonded, &atoms));
        assert!(!residues_backbone_bonded(&prev, &cur_broken, &atoms));
    }

    #[test]
    fn falls_back_to_alpha_carbon_distance_when_carbonyl_or_amide_absent() {
        // No carbonyl/amide recorded: decide by Cα–Cα. 3.8 Å bonded, 7.6 Å gap.
        let atoms = vec![
            atom_at(0.0, 0.0, 0.0),
            atom_at(3.8, 0.0, 0.0),
            atom_at(7.6, 0.0, 0.0),
        ];
        let prev = residue(Some(0), None, None);
        let cur_bonded = residue(Some(1), None, None);
        let cur_gap = residue(Some(2), None, None);
        assert!(residues_backbone_bonded(&prev, &cur_bonded, &atoms));
        assert!(!residues_backbone_bonded(&prev, &cur_gap, &atoms));
    }

    #[test]
    fn peptide_bond_is_authoritative_over_alpha_carbon_proximity() {
        // Both C and N present but far apart ⇒ a break, even though the Cα–Cα
        // distance alone would pass the fallback threshold. The bond wins.
        let atoms = vec![
            atom_at(0.0, 0.0, 0.0), // prev Cα
            atom_at(0.5, 0.0, 0.0), // prev carbonyl C — near
            atom_at(3.8, 0.0, 0.0), // cur Cα — within 4.5 Å of prev Cα
            atom_at(9.0, 0.0, 0.0), // cur amide N — far from prev C
        ];
        let prev = residue(Some(0), None, Some(1));
        let cur = residue(Some(2), Some(3), None);
        assert!(
            !residues_backbone_bonded(&prev, &cur, &atoms),
            "a long C–N distance is a break even when Cα–Cα is close"
        );
    }

    #[test]
    fn missing_alpha_carbons_are_not_bonded() {
        let atoms = vec![atom_at(0.0, 0.0, 0.0)];
        assert!(!residues_backbone_bonded(
            &residue(None, None, None),
            &residue(None, None, None),
            &atoms,
        ));
    }
    #[test]
    fn full_backbone_non_standard_residues_build_biopolymer_without_secondary_records() {
        let annotations = [
            ("N", "HID", 1),
            ("CA", "HID", 1),
            ("C", "HID", 1),
            ("N", "CYX", 2),
            ("CA", "CYX", 2),
            ("C", "CYX", 2),
        ]
        .into_iter()
        .map(|(atom_name, residue_name, residue_seq)| PdbAtomAnnotation {
            atom_name: atom_name.to_string(),
            residue_name: residue_name.to_string(),
            chain_id: 'A',
            residue_seq,
            insertion_code: ' ',
        })
        .collect::<Vec<_>>();

        let biopolymer = build_biopolymer(&annotations, Vec::new()).expect("biopolymer");
        assert!(
            biopolymer
                .residues
                .iter()
                .all(|residue| residue.has_peptide_backbone())
        );
        assert!(
            biopolymer
                .residues
                .iter()
                .all(|residue| !residue.is_standard_amino_acid)
        );
    }
}
