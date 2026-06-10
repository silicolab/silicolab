use anyhow::{Context, Result, anyhow, bail};
use nalgebra::{Point3, Vector3};

use crate::{
    domain::chemistry::normalized_symbol,
    domain::{Atom, PdbAtomAnnotation, Structure, UnitCell, build_biopolymer},
};

const GRO_TO_ANGSTROM: f32 = 10.0;

/// One parsed GRO atom line: the geometry plus the residue/atom-name metadata
/// needed to build a biopolymer (residue classification, cartoon, etc.).
struct ParsedAtom {
    atom: Atom,
    annotation: PdbAtomAnnotation,
}

pub fn parse_gro(input: &str) -> Result<Structure> {
    let mut lines = input.lines();

    let title = lines
        .next()
        .ok_or_else(|| anyhow!("missing GRO title line"))?
        .trim_end()
        .to_string();
    let atom_count = lines
        .next()
        .ok_or_else(|| anyhow!("missing GRO atom count line"))?
        .trim()
        .parse::<usize>()
        .context("invalid GRO atom count")?;
    let mut atoms = Vec::with_capacity(atom_count);
    let mut annotations = Vec::with_capacity(atom_count);

    for index in 0..atom_count {
        let line = lines
            .next()
            .ok_or_else(|| anyhow!("missing GRO atom line {}", index + 1))?;
        let parsed = parse_atom_line(line, index + 1)?;
        atoms.push(parsed.atom);
        annotations.push(parsed.annotation);
    }

    let box_line = lines
        .next()
        .ok_or_else(|| anyhow!("missing GRO box vector line"))?;
    let cell = parse_box_line(box_line)?;

    // GRO carries residue/atom names but no explicit chains or secondary
    // structure. Building a biopolymer lets a solvated system classify its
    // protein, solvent, and ions (for "select by type" and per-category render
    // defaults) and renders the protein as cartoon. `build_biopolymer` returns
    // `None` for a system with no protein, leaving element-based fallback.
    let biopolymer = build_biopolymer(&annotations, Vec::new());

    let mut structure = Structure::with_cell(title, atoms, cell);
    structure.biopolymer = biopolymer;
    Ok(structure)
}

fn parse_atom_line(line: &str, line_number: usize) -> Result<ParsedAtom> {
    if line.len() < 20 {
        bail!("GRO atom line {line_number} is shorter than 20 characters");
    }

    // GRO fixed columns: residue number 0..5, residue name 5..10, atom name
    // 10..15. The residue name is essential context for inferring elements (see
    // `element_from_names`) and for biopolymer classification.
    let residue_seq = line[0..5].trim().parse::<i32>().unwrap_or(0);
    let residue_name = line[5..10].trim();
    let atom_name = line[10..15].trim();
    let element = element_from_names(residue_name, atom_name);
    if element.is_empty() {
        bail!("GRO atom line {line_number} has an empty atom name");
    }

    let fields = parse_float_fields(&line[20..], line_number)?;
    if fields.len() < 3 {
        bail!("GRO atom line {line_number} does not contain x y z coordinates");
    }

    Ok(ParsedAtom {
        atom: Atom {
            element,
            position: Point3::new(
                fields[0] * GRO_TO_ANGSTROM,
                fields[1] * GRO_TO_ANGSTROM,
                fields[2] * GRO_TO_ANGSTROM,
            ),
            charge: 0.0,
        },
        // GRO has no chain column; all residues share one synthetic chain.
        annotation: PdbAtomAnnotation {
            atom_name: atom_name.to_string(),
            residue_name: residue_name.to_string(),
            chain_id: 'A',
            residue_seq,
            insertion_code: ' ',
        },
    })
}

fn parse_box_line(line: &str) -> Result<UnitCell> {
    let fields = line
        .split_whitespace()
        .map(|value| {
            value
                .parse::<f32>()
                .with_context(|| format!("invalid GRO box value {value}"))
        })
        .collect::<Result<Vec<_>>>()?;

    let vectors = match fields.as_slice() {
        [ax, by, cz] => [
            Vector3::new(ax * GRO_TO_ANGSTROM, 0.0, 0.0),
            Vector3::new(0.0, by * GRO_TO_ANGSTROM, 0.0),
            Vector3::new(0.0, 0.0, cz * GRO_TO_ANGSTROM),
        ],
        [v1x, v2y, v3z, v1y, v1z, v2x, v2z, v3x, v3y] => [
            Vector3::new(
                v1x * GRO_TO_ANGSTROM,
                v1y * GRO_TO_ANGSTROM,
                v1z * GRO_TO_ANGSTROM,
            ),
            Vector3::new(
                v2x * GRO_TO_ANGSTROM,
                v2y * GRO_TO_ANGSTROM,
                v2z * GRO_TO_ANGSTROM,
            ),
            Vector3::new(
                v3x * GRO_TO_ANGSTROM,
                v3y * GRO_TO_ANGSTROM,
                v3z * GRO_TO_ANGSTROM,
            ),
        ],
        _ => bail!("GRO box line must contain 3 or 9 floating-point values"),
    };

    Ok(UnitCell::from_vectors(vectors))
}

fn parse_float_fields(fields: &str, line_number: usize) -> Result<Vec<f32>> {
    let width = infer_field_width(fields).unwrap_or(8);
    let trimmed = fields.trim_end_matches(['\r', ' ']);

    if trimmed.len() < width * 3 {
        bail!("GRO atom line {line_number} is too short for fixed-width coordinates");
    }

    let field_count = trimmed.len() / width;
    if field_count != 3 && field_count != 6 {
        bail!("GRO atom line {line_number} must contain 3 coordinates and optional 3 velocities");
    }

    let mut values = Vec::with_capacity(field_count);
    for field_index in 0..field_count {
        let start = field_index * width;
        let end = start + width;
        let value = trimmed[start..end].trim().parse::<f32>().with_context(|| {
            format!(
                "invalid floating-point field {} on GRO atom line {}",
                field_index + 1,
                line_number
            )
        })?;
        values.push(value);
    }

    Ok(values)
}

fn infer_field_width(fields: &str) -> Option<usize> {
    let decimal_points = fields
        .match_indices('.')
        .map(|(index, _)| index)
        .collect::<Vec<_>>();

    decimal_points
        .windows(2)
        .next()
        .map(|window| window[1].saturating_sub(window[0]))
        .filter(|width| *width > 0)
}

/// Infer an atom's element from its residue and atom name. GRO files carry no
/// element column, only names, so the residue name disambiguates the otherwise
/// fatal collisions between biomolecular atom names and two-letter element
/// symbols.
///
/// Three cases, in order:
/// 1. **Monatomic ions** (`NA`, `CL`, `CA`/`MG`/`ZN`/… as a one-atom residue):
///    the residue *is* the ion, so map it directly to that element.
/// 2. **Biopolymer residues** (amino acids, nucleotides, water): atom names are
///    organic, where the leading letter is the element — `CA` is the α-carbon,
///    `NE` an ε-nitrogen, `SD` a δ-sulfur. Trusting a two-letter match here is
///    exactly the bug that turned α-carbons into calcium and produced bogus
///    bonds, so we take the first letter.
/// 3. **Everything else** (materials, ligands, unknown residues): fall back to
///    the general heuristic that prefers a real two-letter element symbol.
fn element_from_names(residue_name: &str, atom_name: &str) -> String {
    let core: String = atom_name
        .chars()
        .take_while(|ch| ch.is_ascii_alphabetic())
        .collect();
    if core.is_empty() {
        return String::new();
    }
    let core = core.to_ascii_uppercase();
    let residue = residue_name.to_ascii_uppercase();

    if let Some(element) = monatomic_ion_element(&residue) {
        return element.to_string();
    }
    if is_organic_residue(&residue) {
        return normalized_symbol(&core[..1]);
    }
    element_from_atom_name(&core)
}

/// The element of a monatomic-ion residue, covering the common AMBER (`NA`,
/// `CL`, `CA`, …) and CHARMM (`SOD`, `CLA`, `POT`, …) ion residue names.
fn monatomic_ion_element(residue: &str) -> Option<&'static str> {
    Some(match residue {
        "NA" | "NA+" | "SOD" => "Na",
        "CL" | "CL-" | "CLA" => "Cl",
        "K" | "K+" | "POT" => "K",
        "CA" | "CA2+" | "CAL" => "Ca",
        "MG" | "MG2+" => "Mg",
        "ZN" | "ZN2" | "ZN2+" => "Zn",
        "FE" | "FE2" | "FE3" => "Fe",
        "MN" => "Mn",
        "CU" | "CU1" | "CU2" => "Cu",
        "CO" => "Co",
        "NI" => "Ni",
        "LI" | "LIT" => "Li",
        "RB" | "RUB" => "Rb",
        "CS" | "CES" => "Cs",
        "BR" | "BR-" => "Br",
        "I" | "I-" | "IOD" => "I",
        "BA" | "BAR" => "Ba",
        _ => return None,
    })
}

/// Whether a residue is a biopolymer/water residue whose atom names follow the
/// organic convention (leading letter == element). Standard amino acids plus the
/// common AMBER/CHARMM protonation and terminus variants, capping groups,
/// nucleotides, and water models.
fn is_organic_residue(residue: &str) -> bool {
    if crate::domain::biopolymer::is_standard_amino_acid(residue) {
        return true;
    }
    // Strip a leading N/C terminus marker AMBER prepends (e.g. `NALA`, `CARG`).
    if residue.len() == 4
        && (residue.starts_with('N') || residue.starts_with('C'))
        && crate::domain::biopolymer::is_standard_amino_acid(&residue[1..])
    {
        return true;
    }
    matches!(
        residue,
        // Histidine / cysteine / acidic / basic protonation variants.
        "HID" | "HIE" | "HIP" | "HSD" | "HSE" | "HSP"
            | "CYX" | "CYM" | "CYS2"
            | "ASH" | "GLH" | "LYN" | "ARN" | "TYM"
            // Capping groups.
            | "ACE" | "NME" | "NMA" | "NHE" | "NH2" | "FOR"
            // Water models.
            | "SOL" | "HOH" | "WAT" | "TIP" | "TIP3" | "TIP4" | "TIP5"
            | "SPC" | "SPCE" | "T3P" | "T4P"
            // Nucleotides (DNA/RNA), incl. 5'/3'-terminal variants.
            | "DA" | "DC" | "DG" | "DT" | "DU"
            | "DA5" | "DC5" | "DG5" | "DT5" | "DA3" | "DC3" | "DG3" | "DT3"
            | "RA" | "RC" | "RG" | "RU"
            | "A" | "C" | "G" | "U" | "T"
    )
}

/// General element guess from an atom-name stem (already uppercased, letters
/// only): prefer a real two-letter element symbol, else the first letter.
fn element_from_atom_name(core: &str) -> String {
    if core.len() >= 2 {
        let two_letter = normalized_symbol(&core[..2]);
        if is_known_two_letter_symbol(&two_letter) {
            return two_letter;
        }
    }
    normalized_symbol(&core[..1])
}

fn is_known_two_letter_symbol(symbol: &str) -> bool {
    matches!(
        symbol,
        "Ag" | "Al"
            | "Ar"
            | "Au"
            | "Br"
            | "Ca"
            | "Cl"
            | "Co"
            | "Cu"
            | "Fe"
            | "He"
            | "Hg"
            | "Kr"
            | "Li"
            | "Mg"
            | "Mn"
            | "Na"
            | "Ne"
            | "Ni"
            | "Pd"
            | "Pb"
            | "Pt"
            | "Si"
            | "Sn"
            | "Xe"
            | "Zn"
    )
}

#[cfg(test)]
mod tests {
    use super::{element_from_names, parse_gro};

    #[test]
    fn protein_atom_names_resolve_to_organic_elements_not_two_letter_metals() {
        // The classic collisions: these are carbons/nitrogens/sulfur in an amino
        // acid, NOT calcium / sodium / cadmium / etc.
        assert_eq!(element_from_names("ALA", "CA"), "C"); // α-carbon, not Ca
        assert_eq!(element_from_names("LYS", "NZ"), "N");
        assert_eq!(element_from_names("ARG", "NE"), "N"); // not Ne
        assert_eq!(element_from_names("MET", "SD"), "S");
        assert_eq!(element_from_names("ASP", "CB"), "C");
        assert_eq!(element_from_names("HIS", "CD2"), "C"); // not Cd
        // Protonation / terminus variants still count as organic.
        assert_eq!(element_from_names("HIE", "CA"), "C");
        assert_eq!(element_from_names("NALA", "CA"), "C");
    }

    #[test]
    fn ion_residues_resolve_to_their_metal_or_halide() {
        assert_eq!(element_from_names("NA", "NA"), "Na");
        assert_eq!(element_from_names("CL", "CL"), "Cl");
        assert_eq!(element_from_names("CA", "CA"), "Ca"); // calcium ion residue
        assert_eq!(element_from_names("CLA", "CLA"), "Cl"); // CHARMM chloride
    }

    #[test]
    fn unknown_residues_keep_two_letter_element_heuristic() {
        // Materials / ligands with no biomolecular residue context.
        assert_eq!(element_from_names("MOL", "FE"), "Fe");
        assert_eq!(element_from_names("CELL", "C1"), "C");
        assert_eq!(element_from_names("XYZ", "ZN1"), "Zn");
    }

    #[test]
    fn water_atoms_resolve_correctly() {
        assert_eq!(element_from_names("SOL", "OW"), "O");
        assert_eq!(element_from_names("SOL", "HW1"), "H");
    }

    #[test]
    fn parses_water_box_and_converts_nm_to_angstrom() {
        let structure = parse_gro(
            "\
MD of 2 waters, t= 0.0
    6
    1WATER  OW1    1   0.126   1.624   1.679  0.1227 -0.0580  0.0434
    1WATER  HW2    2   0.190   1.661   1.747  0.8085  0.3191 -0.7791
    1WATER  HW3    3   0.177   1.568   1.613 -0.9045 -2.6469  1.3180
    2WATER  OW1    4   1.275   0.053   0.622  0.2519  0.3140 -0.1734
    2WATER  HW2    5   1.337   0.002   0.680 -1.0641 -1.1349  0.0257
    2WATER  HW3    6   1.326   0.120   0.568  1.9427 -0.8216 -0.0244
   1.82060   1.82060   1.82060
",
        )
        .expect("valid gro");

        let cell = structure.cell.as_ref().expect("periodic cell");

        assert_eq!(structure.title, "MD of 2 waters, t= 0.0");
        assert_eq!(structure.atoms.len(), 6);
        assert_eq!(structure.atoms[0].element, "O");
        assert!((structure.atoms[0].position.x - 1.26).abs() < 0.0001);
        assert!((cell.a - 18.206).abs() < 0.0001);
        assert_eq!(structure.bonds.len(), 4);
    }

    #[test]
    fn builds_biopolymer_so_solvated_system_classifies_and_ions_stay_unbonded() {
        use crate::domain::AtomCategory;

        // A minimal solvated protein: one alanine residue, one water, one Na+
        // ion placed only ~2 Å from the water oxygen.
        let structure = parse_gro(
            "\
solvated protein
    4
    1ALA     CA    1   0.000   0.000   0.000
    2SOL     OW    2   1.000   0.000   0.000
    2SOL    HW1    3   1.097   0.000   0.000
    3NA      NA    4   1.200   0.000   0.000
   5.00000   5.00000   5.00000
",
        )
        .expect("valid gro");

        // The biopolymer is built from residue metadata.
        assert!(structure.biopolymer.is_some());
        assert_eq!(structure.atom_category(0), AtomCategory::Protein);
        assert_eq!(structure.atom_category(1), AtomCategory::Solvent);
        assert_eq!(structure.atom_category(3), AtomCategory::Ion);

        // The Na+ ion (index 3) is 2 Å from the water O (index 1) — inside the
        // covalent cutoff — yet must have no bonds.
        assert!(
            structure
                .bonds
                .iter()
                .all(|bond| bond.a != 3 && bond.b != 3)
        );
    }

    #[test]
    fn parses_triclinic_box_vectors() {
        let structure = parse_gro(
            "\
triclinic
    1
    1CELL    C1    1   0.100   0.200   0.300
   1.00000   1.50000   2.00000   0.10000   0.20000   0.30000   0.40000   0.50000   0.60000
",
        )
        .expect("valid gro");

        let vectors = structure.cell.expect("periodic cell").vectors;

        assert!((vectors[0].x - 10.0).abs() < 0.0001);
        assert!((vectors[0].y - 1.0).abs() < 0.0001);
        assert!((vectors[1].x - 3.0).abs() < 0.0001);
        assert!((vectors[1].z - 4.0).abs() < 0.0001);
        assert!((vectors[2].x - 5.0).abs() < 0.0001);
        assert!((vectors[2].y - 6.0).abs() < 0.0001);
        assert!((vectors[2].z - 20.0).abs() < 0.0001);
    }
}
