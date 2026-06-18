use anyhow::{Result, bail};

use crate::domain::Structure;

use super::structure_format::StructureFormat;

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

fn codec_for(format: StructureFormat) -> &'static dyn StructureCodec {
    match format {
        StructureFormat::Xyz => &XyzCodec,
        StructureFormat::Cif => &CifCodec,
        StructureFormat::Mol2 => &Mol2Codec,
        StructureFormat::Psf => &PsfCodec,
        StructureFormat::Gro => &GroCodec,
        StructureFormat::Pdb => &PdbCodec,
        StructureFormat::Pdbqt => &PdbqtCodec,
    }
}

struct XyzCodec;
struct CifCodec;
struct Mol2Codec;
struct PsfCodec;
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

impl StructureCodec for PsfCodec {
    fn parse(&self, source: &str) -> Result<Structure> {
        crate::io::formats::psf::parse_psf(source)
    }

    fn serialize(&self, _structure: &Structure) -> Result<String> {
        bail!("PSF export is not supported");
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
