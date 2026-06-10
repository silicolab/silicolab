#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StructureFormat {
    Xyz,
    Cif,
    Mol2,
    Psf,
    Gro,
    Pdb,
}

pub const READABLE_FORMATS: [StructureFormat; 6] = [
    StructureFormat::Xyz,
    StructureFormat::Cif,
    StructureFormat::Mol2,
    StructureFormat::Psf,
    StructureFormat::Gro,
    StructureFormat::Pdb,
];

pub const WRITABLE_FORMATS: [StructureFormat; 4] = [
    StructureFormat::Xyz,
    StructureFormat::Cif,
    StructureFormat::Mol2,
    StructureFormat::Pdb,
];

impl StructureFormat {
    pub fn label(self) -> &'static str {
        match self {
            Self::Xyz => "XYZ",
            Self::Cif => "CIF",
            Self::Mol2 => "MOL2",
            Self::Psf => "SLF",
            Self::Gro => "GRO",
            Self::Pdb => "PDB",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Self::Xyz => "xyz",
            Self::Cif => "cif",
            Self::Mol2 => "mol2",
            Self::Psf => "slf",
            Self::Gro => "gro",
            Self::Pdb => "pdb",
        }
    }

    pub fn supports_write(self) -> bool {
        WRITABLE_FORMATS.contains(&self)
    }

    pub fn from_extension(extension: &str) -> Option<Self> {
        if extension.eq_ignore_ascii_case("xyz") {
            Some(Self::Xyz)
        } else if extension.eq_ignore_ascii_case("cif") {
            Some(Self::Cif)
        } else if extension.eq_ignore_ascii_case("mol2") {
            Some(Self::Mol2)
        } else if extension.eq_ignore_ascii_case("slf") {
            Some(Self::Psf)
        } else if extension.eq_ignore_ascii_case("gro") {
            Some(Self::Gro)
        } else if extension.eq_ignore_ascii_case("pdb") {
            Some(Self::Pdb)
        } else {
            None
        }
    }
}
