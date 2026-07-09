use anyhow::{Result, bail};

use crate::domain::Structure;

use super::structure_format::{MultiStructureFile, StructureFormat};

pub trait StructureCodec {
    fn parse(&self, source: &str) -> Result<Structure>;
    fn serialize(&self, structure: &Structure) -> Result<String>;
}

pub fn parse_structure(format: StructureFormat, source: &str) -> Result<Structure> {
    codec_for(format).parse(source)
}

pub fn serialize_structure(format: StructureFormat, structure: &Structure) -> Result<String> {
    codec_for(format).serialize(structure)
}

/// Serialize several structures into the contents of one file. Only formats
/// whose records concatenate accept more than one; the rest must be written one
/// structure per file (see [`StructureFormat::single_structure_reason`]).
pub fn serialize_structures(format: StructureFormat, structures: &[&Structure]) -> Result<String> {
    match structures {
        [] => bail!("no structures to serialize"),
        [only] => serialize_structure(format, only),
        many => {
            if format.multi_structure_file() != MultiStructureFile::Concatenated {
                bail!(
                    "{} cannot hold {} structures in one file",
                    format.label(),
                    many.len()
                );
            }
            let mut output = String::new();
            for structure in many {
                output.push_str(&serialize_structure(format, structure)?);
            }
            Ok(output)
        }
    }
}

fn codec_for(format: StructureFormat) -> &'static dyn StructureCodec {
    match format {
        StructureFormat::Xyz => &XyzCodec,
        StructureFormat::Cif => &CifCodec,
        StructureFormat::Mol2 => &Mol2Codec,
        StructureFormat::Slf => &SlfCodec,
        StructureFormat::Gro => &GroCodec,
        StructureFormat::Pdb => &PdbCodec,
        StructureFormat::Pdbqt => &PdbqtCodec,
    }
}

struct XyzCodec;
struct CifCodec;
struct Mol2Codec;
struct SlfCodec;
struct GroCodec;
struct PdbCodec;
struct PdbqtCodec;

impl StructureCodec for XyzCodec {
    fn parse(&self, source: &str) -> Result<Structure> {
        crate::io::formats::xyz::parse_xyz(source)
    }

    fn serialize(&self, structure: &Structure) -> Result<String> {
        Ok(crate::io::structure_text::to_xyz(structure))
    }
}

impl StructureCodec for CifCodec {
    fn parse(&self, source: &str) -> Result<Structure> {
        crate::io::formats::cif::parse_cif(source)
    }

    fn serialize(&self, structure: &Structure) -> Result<String> {
        crate::io::formats::cif::to_cif(structure)
    }
}

impl StructureCodec for Mol2Codec {
    fn parse(&self, source: &str) -> Result<Structure> {
        crate::io::formats::mol2::parse_mol2(source)
    }

    fn serialize(&self, structure: &Structure) -> Result<String> {
        Ok(crate::io::formats::mol2::to_mol2(structure))
    }
}

impl StructureCodec for SlfCodec {
    fn parse(&self, source: &str) -> Result<Structure> {
        crate::io::formats::slf::parse_slf(source)
    }

    fn serialize(&self, _structure: &Structure) -> Result<String> {
        // Writing SLF means writing a building block, which needs the reticular
        // metadata only the block editor holds (`formats::slf::to_slf`).
        bail!("SLF export is not supported");
    }
}

impl StructureCodec for GroCodec {
    fn parse(&self, source: &str) -> Result<Structure> {
        crate::io::formats::gro::parse_gro(source)
    }

    fn serialize(&self, _structure: &Structure) -> Result<String> {
        bail!("GRO export is not supported");
    }
}

impl StructureCodec for PdbCodec {
    fn parse(&self, source: &str) -> Result<Structure> {
        crate::io::formats::pdb::parse_pdb(source)
    }

    fn serialize(&self, structure: &Structure) -> Result<String> {
        crate::io::formats::pdb::to_pdb(structure)
    }
}

impl StructureCodec for PdbqtCodec {
    fn parse(&self, source: &str) -> Result<Structure> {
        crate::io::formats::pdbqt::parse_pdbqt(source)
    }

    fn serialize(&self, structure: &Structure) -> Result<String> {
        crate::io::formats::pdbqt::to_pdbqt(structure)
    }
}
