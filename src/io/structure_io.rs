use std::{fs, path::Path};

use anyhow::{Context, Result, bail};

use crate::domain::Structure;

use super::{
    structure_codec::{parse_structure, serialize_structure},
    structure_paths,
};

pub use super::{
    structure_format::StructureFormat,
    structure_paths::{
        default_structure_save_path, path_with_format_extension, preferred_save_format,
        readable_extensions, suggested_save_stem, writable_formats,
    },
    structure_text::{to_cif, to_pdb, to_xyz},
};

pub fn load_structure(path: &Path) -> Result<Structure> {
    let source =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    let format = structure_paths::format_from_path(path)
        .ok_or_else(|| anyhow::anyhow!(structure_paths::unsupported_read_message(path)))?;

    parse_structure(format, &source)
        .with_context(|| format!("failed to parse {} input", format.label()))
}

/// The parsed contents of a structure file. Most formats yield a single
/// structure; a multi-model deposition (an NMR PDB ensemble) yields one per
/// `MODEL`. `title`/`identifier` carry deposition metadata when the format
/// provides it (e.g. a PDB `TITLE` and `HEADER` id) and are `None` otherwise.
///
/// This type is deliberately free of workspace concepts (groups, entry names):
/// how these structures are surfaced as entries is a frontend policy decision
/// (see `frontend::structure_import`).
pub struct ParsedStructures {
    pub title: Option<String>,
    pub identifier: Option<String>,
    pub structures: Vec<Structure>,
}

/// Parse a structure file into one or more structures plus any deposition
/// metadata, dispatching on the file format. PDB files are parsed model-aware
/// so NMR ensembles preserve every conformer instead of merging them.
pub fn load_structures(path: &Path) -> Result<ParsedStructures> {
    let source =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    let format = structure_paths::format_from_path(path)
        .ok_or_else(|| anyhow::anyhow!(structure_paths::unsupported_read_message(path)))?;

    if format == StructureFormat::Pdb {
        let document = crate::io::formats::pdb::parse_pdb_document(&source)
            .with_context(|| format!("failed to parse {} input", format.label()))?;
        // Fall back to the file stem so a header-less PDB still carries a short
        // identifier (used downstream for naming).
        let identifier = document
            .pdb_id
            .filter(|id| !id.trim().is_empty())
            .or_else(|| {
                Some(structure_paths::suggested_save_stem(Some(path)).to_ascii_uppercase())
            });
        return Ok(ParsedStructures {
            title: Some(document.title),
            identifier,
            structures: document.models,
        });
    }

    let structure = parse_structure(format, &source)
        .with_context(|| format!("failed to parse {} input", format.label()))?;
    Ok(ParsedStructures {
        title: None,
        identifier: None,
        structures: vec![structure],
    })
}

pub fn save_structure(structure: &Structure, path: &Path) -> Result<()> {
    let format = structure_paths::format_from_path(path)
        .ok_or_else(|| anyhow::anyhow!(structure_paths::unsupported_write_message(path)))?;
    if !format.supports_write() {
        bail!("{} export is not supported", format.label());
    }

    let contents = serialize_structure(format, structure)?;

    fs::write(path, contents).with_context(|| format!("failed to write {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::load_structures;

    const MULTI_MODEL_PDB: &str = "\
HEADER    ANTIMICROBIAL PROTEIN                   24-JUN-18   6A5J
TITLE     SOLUTION NMR STRUCTURE OF SMALL PEPTIDE
NUMMDL    2
MODEL        1
ATOM      1  N   GLY A   1       0.000   0.000   0.000  1.00  0.00           N
ATOM      2  CA  GLY A   1       1.450   0.000   0.000  1.00  0.00           C
ENDMDL
MODEL        2
ATOM      1  N   GLY A   1       0.100   0.000   0.000  1.00  0.00           N
ATOM      2  CA  GLY A   1       1.550   0.000   0.000  1.00  0.00           C
ENDMDL
END
";

    const SINGLE_MODEL_PDB: &str = "\
HEADER    HYDROLASE                               01-JAN-00   1ABC
TITLE     CRYSTAL STRUCTURE OF A HYDROLASE
ATOM      1  N   GLY A   1       0.000   0.000   0.000  1.00  0.00           N
ATOM      2  CA  GLY A   1       1.450   0.000   0.000  1.00  0.00           C
END
";

    fn write_temp_pdb(name: &str, contents: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "silicolab_import_{name}_{}.pdb",
            std::process::id()
        ));
        std::fs::write(&path, contents).expect("write temp pdb");
        path
    }

    #[test]
    fn parses_every_nmr_model_with_metadata() {
        let path = write_temp_pdb("6a5j", MULTI_MODEL_PDB);
        let parsed = load_structures(&path).expect("load multi-model pdb");
        std::fs::remove_file(&path).ok();

        assert_eq!(parsed.identifier.as_deref(), Some("6A5J"));
        assert_eq!(
            parsed.title.as_deref(),
            Some("SOLUTION NMR STRUCTURE OF SMALL PEPTIDE")
        );
        // Each model is a distinct structure, not a merged blob.
        assert_eq!(parsed.structures.len(), 2);
        assert_eq!(parsed.structures[0].atoms.len(), 2);
        assert_eq!(parsed.structures[1].atoms.len(), 2);
    }

    #[test]
    fn parses_single_model_pdb_with_metadata() {
        let path = write_temp_pdb("1abc", SINGLE_MODEL_PDB);
        let parsed = load_structures(&path).expect("load single-model pdb");
        std::fs::remove_file(&path).ok();

        assert_eq!(parsed.identifier.as_deref(), Some("1ABC"));
        assert_eq!(
            parsed.title.as_deref(),
            Some("CRYSTAL STRUCTURE OF A HYDROLASE")
        );
        assert_eq!(parsed.structures.len(), 1);
    }
}
