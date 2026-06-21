use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

use crate::domain::Structure;

use super::structure_format::{READABLE_FORMATS, StructureFormat, WRITABLE_FORMATS};

const READABLE_EXTENSIONS: [&str; 7] = ["xyz", "cif", "mol2", "slf", "gro", "pdb", "pdbqt"];

pub fn readable_extensions() -> &'static [&'static str] {
    &READABLE_EXTENSIONS
}

pub fn writable_formats() -> &'static [StructureFormat] {
    &WRITABLE_FORMATS
}

pub fn preferred_save_format(
    structure: &Structure,
    preferred_path: Option<&Path>,
) -> StructureFormat {
    preferred_path
        .and_then(format_from_path)
        .filter(|format| format.supports_write())
        .unwrap_or_else(|| {
            if structure.cell.is_some() {
                StructureFormat::Cif
            } else {
                StructureFormat::Xyz
            }
        })
}

pub fn default_structure_save_path(structure: &Structure, source_path: Option<&Path>) -> PathBuf {
    let format = preferred_save_format(structure, source_path);
    let stem = default_file_stem(source_path);

    if let Some(source_path) = source_path
        && format_from_path(source_path) == Some(format)
    {
        return source_path.to_path_buf();
    }

    PathBuf::from(format!("{stem}.{}", format.extension()))
}

pub fn suggested_save_stem(path: Option<&Path>) -> String {
    default_file_stem(path)
}

pub fn path_with_format_extension(path: &Path, format: StructureFormat) -> PathBuf {
    let expected_extension = format.extension();
    let current_extension = path.extension().and_then(|extension| extension.to_str());

    if current_extension
        .map(|extension| extension.eq_ignore_ascii_case(expected_extension))
        .unwrap_or(false)
    {
        return path.to_path_buf();
    }

    let mut output = path.to_path_buf();
    let mut file_name = output
        .file_name()
        .map(OsString::from)
        .unwrap_or_else(|| OsString::from(default_file_stem(Some(path))));

    if current_extension.is_none() {
        file_name.push(".");
        file_name.push(expected_extension);
        output.set_file_name(file_name);
        return output;
    }

    output.set_extension(expected_extension);
    output
}

pub(crate) fn format_from_path(path: &Path) -> Option<StructureFormat> {
    path.extension()
        .and_then(|extension| extension.to_str())
        .and_then(StructureFormat::from_extension)
}

pub(crate) fn unsupported_read_message(path: &Path) -> String {
    let supported = READABLE_FORMATS
        .iter()
        .map(|format| format.extension())
        .collect::<Vec<_>>()
        .join(", .");

    format!(
        "unsupported structure format for {}; expected one of: .{}",
        path.display(),
        supported
    )
}

pub(crate) fn unsupported_write_message(path: &Path) -> String {
    let supported = writable_formats()
        .iter()
        .map(|format| format.extension())
        .collect::<Vec<_>>()
        .join(", .");

    format!(
        "save path must end in one of the supported extensions: .{} (got {})",
        supported,
        path.display()
    )
}

fn default_file_stem(path: Option<&Path>) -> String {
    path.and_then(|path| path.file_stem())
        .and_then(|stem| stem.to_str())
        .filter(|stem| !stem.is_empty())
        .unwrap_or("edited")
        .to_string()
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use nalgebra::Point3;

    use super::{
        StructureFormat, default_structure_save_path, path_with_format_extension,
        preferred_save_format,
    };
    use crate::domain::{Atom, Structure, UnitCell};

    #[test]
    fn keeps_writable_source_extension_as_default_structure_save_path() {
        let structure = Structure::with_bonds("structure", Vec::new(), Vec::new());
        let save_path =
            default_structure_save_path(&structure, Some(Path::new("tmp/structure.pdb")));

        assert_eq!(save_path, PathBuf::from("tmp/structure.pdb"));
    }

    #[test]
    fn replaces_non_writable_source_extension_in_default_structure_save_path() {
        let structure = Structure::with_cell(
            "cell",
            vec![Atom {
                element: "C".to_string(),
                position: Point3::new(0.0, 0.0, 0.0),
                charge: 0.0,
            }],
            UnitCell::from_parameters(10.0, 10.0, 10.0, 90.0, 90.0, 90.0),
        );
        let save_path = default_structure_save_path(&structure, Some(Path::new("tmp/cell.gro")));

        assert_eq!(save_path, PathBuf::from("cell.cif"));
    }

    #[test]
    fn path_with_format_extension_replaces_mismatched_suffix() {
        let output = path_with_format_extension(Path::new("tmp/example.xyz"), StructureFormat::Pdb);

        assert_eq!(output, PathBuf::from("tmp/example.pdb"));
    }

    #[test]
    fn preferred_save_format_uses_source_format_when_writable() {
        let structure = Structure::with_bonds("structure", Vec::new(), Vec::new());

        assert_eq!(
            preferred_save_format(&structure, Some(Path::new("tmp/structure.mol2"))),
            StructureFormat::Mol2
        );
    }
}
