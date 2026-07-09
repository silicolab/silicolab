//! Writing structures out as files: either one combined file (for the formats
//! whose records concatenate) or one file per structure. The only path from a
//! workspace structure to a structure file on disk.

use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};

use crate::domain::Structure;

use super::{
    structure_codec::serialize_structures, structure_format::StructureFormat, structure_paths,
};

const FALLBACK_FILE_STEM: &str = "structure";

/// Write every structure into `path` as a single file. Fails for a format that
/// cannot hold more than one (see [`StructureFormat::single_structure_reason`]).
pub fn write_structures_to_file(structures: &[&Structure], path: &Path) -> Result<()> {
    let format = structure_paths::format_from_path(path)
        .ok_or_else(|| anyhow::anyhow!(structure_paths::unsupported_write_message(path)))?;
    if !format.supports_write() {
        bail!("{} export is not supported", format.label());
    }

    let contents = serialize_structures(format, structures)?;
    fs::write(path, contents).with_context(|| format!("failed to write {}", path.display()))
}

/// One filename stem per name, sanitized for the filesystem and de-duplicated
/// (`ethanol`, `ethanol-2`, …). Entry names carry no uniqueness constraint, so
/// without this a batch export would silently overwrite its own output.
pub fn plan_file_stems(names: &[&str]) -> Vec<String> {
    let mut taken = HashSet::new();
    names
        .iter()
        .map(|name| {
            let stem = sanitize_file_stem(name);
            let mut candidate = stem.clone();
            let mut suffix = 2;
            while !taken.insert(candidate.to_lowercase()) {
                candidate = format!("{stem}-{suffix}");
                suffix += 1;
            }
            candidate
        })
        .collect()
}

pub fn plan_export_paths(
    names: &[&str],
    directory: &Path,
    format: StructureFormat,
) -> Vec<PathBuf> {
    plan_file_stems(names)
        .into_iter()
        .map(|stem| directory.join(format!("{stem}.{}", format.extension())))
        .collect()
}

/// Write each structure to its own path, reporting one outcome per structure. A
/// structure a format cannot represent (a PDB past the atom-serial limit) must
/// not discard the ones that wrote fine.
pub fn write_each(structures: &[&Structure], paths: &[PathBuf]) -> Vec<Result<()>> {
    structures
        .iter()
        .zip(paths)
        .map(|(structure, path)| write_structures_to_file(std::slice::from_ref(structure), path))
        .collect()
}

fn sanitize_file_stem(name: &str) -> String {
    let name = name.trim();
    // An entry named `4hhb.pdb` exported as CIF should be `4hhb.cif`, never
    // `4hhb.pdb.cif` — but only a real structure suffix may be dropped.
    let base = Path::new(name)
        .extension()
        .and_then(|extension| extension.to_str())
        .and_then(StructureFormat::from_extension)
        .and_then(|_| Path::new(name).file_stem())
        .and_then(|stem| stem.to_str())
        .unwrap_or(name);

    let cleaned = base
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    let cleaned = cleaned.trim_matches(['.', '_']).to_string();

    if cleaned.is_empty() {
        FALLBACK_FILE_STEM.to_string()
    } else {
        cleaned
    }
}

#[cfg(test)]
mod tests {
    use nalgebra::Point3;

    use super::{StructureFormat, plan_export_paths, plan_file_stems, write_structures_to_file};
    use crate::{
        domain::{Atom, Structure},
        io::formats::{mol2::parse_mol2_records, xyz::parse_xyz_records},
    };

    fn atom(x: f32) -> Atom {
        Atom {
            element: "C".to_string(),
            position: Point3::new(x, 0.0, 0.0),
            charge: 0.0,
        }
    }

    fn structure(title: &str, atoms: usize) -> Structure {
        Structure::new(
            title.to_string(),
            (0..atoms).map(|index| atom(index as f32 * 2.0)).collect(),
        )
    }

    #[test]
    fn duplicate_names_get_distinct_stems() {
        let stems = plan_file_stems(&["ethanol", "ethanol", "ETHANOL"]);

        assert_eq!(stems, vec!["ethanol", "ethanol-2", "ETHANOL-3"]);
    }

    #[test]
    fn structure_suffix_is_replaced_not_appended() {
        let paths = plan_export_paths(
            &["4hhb.pdb"],
            std::path::Path::new("out"),
            StructureFormat::Cif,
        );

        assert_eq!(paths[0].file_name().unwrap(), "4hhb.cif");
    }

    #[test]
    fn unsafe_characters_are_replaced() {
        assert_eq!(plan_file_stems(&["a/b:c"]), vec!["a_b_c"]);
        assert_eq!(plan_file_stems(&["   "]), vec!["structure"]);
    }

    #[test]
    fn combined_xyz_round_trips_every_structure() {
        let directory = std::env::temp_dir().join(format!("slx_export_{}", std::process::id()));
        std::fs::create_dir_all(&directory).expect("temp dir");
        let path = directory.join("combined.xyz");
        let (first, second) = (structure("one", 1), structure("two", 2));

        write_structures_to_file(&[&first, &second], &path).expect("write combined xyz");
        let source = std::fs::read_to_string(&path).expect("read back");
        std::fs::remove_dir_all(&directory).ok();

        let records = parse_xyz_records(&source).expect("parse records");
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].atoms.len(), 1);
        assert_eq!(records[1].atoms.len(), 2);
    }

    #[test]
    fn combined_mol2_round_trips_every_structure() {
        let directory =
            std::env::temp_dir().join(format!("slx_export_mol2_{}", std::process::id()));
        std::fs::create_dir_all(&directory).expect("temp dir");
        let path = directory.join("combined.mol2");
        let (first, second) = (structure("one", 1), structure("two", 2));

        write_structures_to_file(&[&first, &second], &path).expect("write combined mol2");
        let source = std::fs::read_to_string(&path).expect("read back");
        std::fs::remove_dir_all(&directory).ok();

        let records = parse_mol2_records(&source).expect("parse records");
        assert_eq!(records.len(), 2);
        assert_eq!(records[1].atoms.len(), 2);
    }

    #[test]
    fn combining_is_refused_for_single_structure_formats() {
        let directory = std::env::temp_dir().join(format!("slx_export_pdb_{}", std::process::id()));
        std::fs::create_dir_all(&directory).expect("temp dir");
        let path = directory.join("combined.pdb");
        let (first, second) = (structure("one", 1), structure("two", 1));

        let error = write_structures_to_file(&[&first, &second], &path)
            .expect_err("PDB must refuse a merge");
        std::fs::remove_dir_all(&directory).ok();

        assert!(error.to_string().contains("cannot hold"));
    }
}
