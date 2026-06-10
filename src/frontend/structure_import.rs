//! Maps a parsed structure file onto workspace entries and groups.
//!
//! The `io` layer parses a file into [`ParsedStructures`] (one or more
//! structures plus deposition metadata) and knows nothing about groups or entry
//! names. This module owns that workspace policy: a deposition (a PDB,
//! identified by its accession id) becomes a group named after its title, so a
//! crystal/cryo-EM single structure and an NMR ensemble look the same in the
//! entry list.

use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::{
    backend::entries::EntryStore,
    domain::Structure,
    io::structure_io::{self, ParsedStructures},
};

/// One structure to surface as a workspace entry.
pub struct ImportedEntry {
    /// Explicit entry name, or `None` to derive it from the structure title.
    pub name: Option<String>,
    pub structure: Structure,
    pub save_path: PathBuf,
}

/// How a loaded file should appear in the workspace.
pub struct ImportedDocument {
    /// When set, every entry is placed in a group with this name.
    pub group_name: Option<String>,
    pub entries: Vec<ImportedEntry>,
}

/// Load a structure file and decide how it should appear in the workspace.
pub fn load_document(path: &Path) -> Result<ImportedDocument> {
    Ok(build_document(structure_io::load_structures(path)?, path))
}

/// Apply the workspace grouping policy to parsed structures.
///
/// A file carrying a deposition identifier (a PDB) becomes a group named after
/// its title, with entries named by the id — `<id>` for a single structure and
/// `<id> (model N)` for each conformer of a multi-model ensemble. This keeps the
/// entry list consistent across crystal/cryo-EM and NMR depositions. Files
/// without an identifier (XYZ, CIF, ...) import as a single ungrouped entry that
/// keeps its title-derived name.
fn build_document(parsed: ParsedStructures, path: &Path) -> ImportedDocument {
    let Some(identifier) = parsed.identifier else {
        let structure = parsed
            .structures
            .into_iter()
            .next()
            .expect("at least one structure");
        let save_path = structure_io::default_structure_save_path(&structure, Some(path));
        return ImportedDocument {
            group_name: None,
            entries: vec![ImportedEntry {
                name: None,
                structure,
                save_path,
            }],
        };
    };

    // The group is named after the deposition title; fall back to the id when
    // the title is missing or just the generic parser placeholder.
    let title = parsed.title.unwrap_or_default();
    let group_name = if title.trim().is_empty() || title == "PDB structure" {
        identifier.clone()
    } else {
        title
    };

    // Single-structure files keep saving back to the source path; multi-model
    // files get one export file per conformer so they don't overwrite one
    // another.
    let multi_model = parsed.structures.len() > 1;
    let stem = structure_io::suggested_save_stem(Some(path));
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("pdb");
    let dir = path.parent();

    let entries = parsed
        .structures
        .into_iter()
        .enumerate()
        .map(|(index, structure)| {
            if multi_model {
                let model_number = index + 1;
                let save_name = format!("{stem}_model_{model_number}.{extension}");
                let save_path = match dir {
                    Some(dir) => dir.join(save_name),
                    None => PathBuf::from(save_name),
                };
                ImportedEntry {
                    name: Some(format!("{identifier} (model {model_number})")),
                    structure,
                    save_path,
                }
            } else {
                let save_path = structure_io::default_structure_save_path(&structure, Some(path));
                ImportedEntry {
                    name: Some(identifier.clone()),
                    structure,
                    save_path,
                }
            }
        })
        .collect();

    ImportedDocument {
        group_name: Some(group_name),
        entries,
    }
}

/// Insert every entry of an imported document into `entries`. When the document
/// carries a group name (e.g. a deposition), a group is created and all entries
/// are placed in it; only the first entry opens a tab and becomes active.
/// Returns the id of the entry that should be activated, or `None` when the
/// document is empty.
pub fn import_document(
    entries: &mut EntryStore,
    document: ImportedDocument,
    source_path: PathBuf,
) -> Option<u64> {
    let group_id = match &document.group_name {
        Some(name) => entries.create_group(name).unwrap_or_default(),
        None => String::new(),
    };

    let mut active = None;
    for (index, entry) in document.entries.into_iter().enumerate() {
        let activate = index == 0;
        let entry_id = entries.add_entry_to_group(
            entry.structure,
            Some(source_path.clone()),
            entry.save_path,
            group_id.clone(),
            entry.name,
            activate,
        );
        if activate {
            active = Some(entry_id);
        }
    }
    active
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{ParsedStructures, build_document, import_document};
    use crate::{backend::entries::EntryStore, domain::Structure};

    fn structure(title: &str) -> Structure {
        Structure::with_bonds(title.to_string(), Vec::new(), Vec::new())
    }

    #[test]
    fn nmr_ensemble_becomes_a_titled_group_of_per_model_entries() {
        let parsed = ParsedStructures {
            title: Some("SOLUTION NMR STRUCTURE OF SMALL PEPTIDE".to_string()),
            identifier: Some("6A5J".to_string()),
            structures: vec![structure("m1"), structure("m2"), structure("m3")],
        };
        let document = build_document(parsed, Path::new("/data/6A5J.pdb"));

        assert_eq!(
            document.group_name.as_deref(),
            Some("SOLUTION NMR STRUCTURE OF SMALL PEPTIDE")
        );
        assert_eq!(document.entries.len(), 3);
        assert_eq!(document.entries[0].name.as_deref(), Some("6A5J (model 1)"));
        assert_eq!(document.entries[2].name.as_deref(), Some("6A5J (model 3)"));
        // Conformers save to distinct files so they don't overwrite one another.
        assert_ne!(document.entries[0].save_path, document.entries[1].save_path);
    }

    #[test]
    fn single_deposition_becomes_a_group_with_one_id_named_entry() {
        let parsed = ParsedStructures {
            title: Some("CRYSTAL STRUCTURE OF A HYDROLASE".to_string()),
            identifier: Some("1ABC".to_string()),
            structures: vec![structure("only")],
        };
        let document = build_document(parsed, Path::new("/data/1ABC.pdb"));

        // A crystal/cryo-EM structure groups identically to an NMR ensemble.
        assert_eq!(
            document.group_name.as_deref(),
            Some("CRYSTAL STRUCTURE OF A HYDROLASE")
        );
        assert_eq!(document.entries.len(), 1);
        assert_eq!(document.entries[0].name.as_deref(), Some("1ABC"));
    }

    #[test]
    fn plain_structure_without_identifier_is_ungrouped() {
        let parsed = ParsedStructures {
            title: None,
            identifier: None,
            structures: vec![structure("molecule")],
        };
        let document = build_document(parsed, Path::new("/data/molecule.xyz"));

        assert!(document.group_name.is_none());
        assert_eq!(document.entries.len(), 1);
        // Keeps its title-derived name (no explicit override).
        assert!(document.entries[0].name.is_none());
    }

    #[test]
    fn import_document_creates_group_and_activates_only_the_first_entry() {
        let parsed = ParsedStructures {
            title: Some("ENSEMBLE".to_string()),
            identifier: Some("9ZZZ".to_string()),
            structures: vec![structure("m1"), structure("m2")],
        };
        let document = build_document(parsed, Path::new("/data/9ZZZ.pdb"));

        let mut entries = EntryStore::new_empty();
        let active = import_document(&mut entries, document, "/data/9ZZZ.pdb".into());

        assert_eq!(entries.records.len(), 2);
        assert_eq!(entries.groups.len(), 1);
        // Every entry lands in the one group; only the first opens a tab.
        assert!(
            entries
                .records
                .iter()
                .all(|e| e.group_id == entries.groups[0].id)
        );
        assert_eq!(entries.tabs.len(), 1);
        assert_eq!(active, Some(entries.tabs[0].entry_id));
    }
}
