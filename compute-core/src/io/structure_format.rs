#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StructureFormat {
    Xyz,
    Cif,
    Mol2,
    Slf,
    Gro,
    Pdb,
    Pdbqt,
}

pub const READABLE_FORMATS: [StructureFormat; 7] = [
    StructureFormat::Xyz,
    StructureFormat::Cif,
    StructureFormat::Mol2,
    StructureFormat::Slf,
    StructureFormat::Gro,
    StructureFormat::Pdb,
    StructureFormat::Pdbqt,
];

pub const WRITABLE_FORMATS: [StructureFormat; 5] = [
    StructureFormat::Xyz,
    StructureFormat::Cif,
    StructureFormat::Mol2,
    StructureFormat::Pdb,
    StructureFormat::Pdbqt,
];

/// Whether one file of this format can hold several structures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MultiStructureFile {
    /// Records concatenate: writing several structures back to back produces a
    /// file that parses back into the same several structures.
    Concatenated,
    /// One structure per file.
    Single,
}

impl StructureFormat {
    pub fn label(self) -> &'static str {
        match self {
            Self::Xyz => "XYZ",
            Self::Cif => "CIF",
            Self::Mol2 => "MOL2",
            Self::Slf => "SLF",
            Self::Gro => "GRO",
            Self::Pdb => "PDB",
            Self::Pdbqt => "PDBQT",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Self::Xyz => "xyz",
            Self::Cif => "cif",
            Self::Mol2 => "mol2",
            Self::Slf => "slf",
            Self::Gro => "gro",
            Self::Pdb => "pdb",
            Self::Pdbqt => "pdbqt",
        }
    }

    pub fn supports_write(self) -> bool {
        WRITABLE_FORMATS.contains(&self)
    }

    pub fn multi_structure_file(self) -> MultiStructureFile {
        match self {
            Self::Xyz | Self::Mol2 => MultiStructureFile::Concatenated,
            Self::Cif | Self::Slf | Self::Gro | Self::Pdb | Self::Pdbqt => {
                MultiStructureFile::Single
            }
        }
    }

    /// Why several structures cannot share one file of this format, for the UI
    /// to show next to the disabled "combine" choice. `None` when they can.
    pub fn single_structure_reason(self) -> Option<&'static str> {
        match self {
            Self::Xyz | Self::Mol2 => None,
            // A PDB MODEL record means "another conformer of this system", so
            // merging unrelated structures produces a file that opens fine and
            // means something else. Refusing beats writing a plausible lie.
            Self::Pdb | Self::Pdbqt => {
                Some("MODEL records describe conformers of one system, not unrelated structures.")
            }
            Self::Cif => {
                Some("Most readers, including SilicoLab, only parse the first mmCIF data block.")
            }
            Self::Slf | Self::Gro => Some("This format cannot be written."),
        }
    }

    pub fn from_extension(extension: &str) -> Option<Self> {
        if extension.eq_ignore_ascii_case("xyz") {
            Some(Self::Xyz)
        } else if extension.eq_ignore_ascii_case("cif") {
            Some(Self::Cif)
        } else if extension.eq_ignore_ascii_case("mol2") {
            Some(Self::Mol2)
        } else if extension.eq_ignore_ascii_case("slf") {
            Some(Self::Slf)
        } else if extension.eq_ignore_ascii_case("gro") {
            Some(Self::Gro)
        } else if extension.eq_ignore_ascii_case("pdb") {
            Some(Self::Pdb)
        } else if extension.eq_ignore_ascii_case("pdbqt") {
            Some(Self::Pdbqt)
        } else {
            None
        }
    }
}
