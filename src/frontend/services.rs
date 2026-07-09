use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

use crate::{
    domain::Structure,
    frontend::{BuildingBlockEditor, bond_geometry_summary},
    io::structure_io,
    workflows::nanosheet::{NanosheetSpec, build_nanosheet},
    workflows::reticular::{ReticularBuildSpec, build_framework},
};

pub struct BuiltFramework {
    pub structure: Structure,
    pub save_path: PathBuf,
    pub analysis: String,
}

pub struct StructureService;

impl StructureService {
    pub fn open_dialog() -> Option<PathBuf> {
        rfd::FileDialog::new()
            .add_filter("Structure", structure_io::readable_extensions())
            .pick_file()
    }
}

pub struct ReticularService;

impl ReticularService {
    pub fn preview(spec: &ReticularBuildSpec) -> Result<BuiltFramework> {
        Self::build(spec)
    }

    pub fn build(spec: &ReticularBuildSpec) -> Result<BuiltFramework> {
        let structure = build_framework(spec)?;
        let analysis = bond_geometry_summary(&structure);
        let save_path = PathBuf::from(format!("{}.cif", spec.name));
        Ok(BuiltFramework {
            structure,
            save_path,
            analysis,
        })
    }
}

pub struct NanosheetService;

impl NanosheetService {
    pub fn preview(spec: &NanosheetSpec) -> Result<BuiltFramework> {
        Self::build(spec)
    }

    pub fn build(spec: &NanosheetSpec) -> Result<BuiltFramework> {
        let structure = build_nanosheet(spec)?;
        let analysis = bond_geometry_summary(&structure);
        let save_path = PathBuf::from(format!("{}.cif", spec.name));
        Ok(BuiltFramework {
            structure,
            save_path,
            analysis,
        })
    }
}

pub struct BuildingBlockService;

impl BuildingBlockService {
    pub fn save(editor: &BuildingBlockEditor, structure: &Structure) -> Result<(PathBuf, String)> {
        editor.save(structure)
    }
}

pub fn entry_details(structure: &Structure, source_path: Option<&Path>) -> Vec<String> {
    let mut lines = vec![
        format!("Atoms: {}", structure.atoms.len()),
        format!("Bonds: {}", structure.bonds.len()),
        format!("Formula: {}", empirical_formula(structure)),
    ];
    if let Some(path) = source_path {
        lines.push(format!("Source: {}", path.display()));
    }
    lines
}

fn empirical_formula(structure: &Structure) -> String {
    if structure.atoms.is_empty() {
        return "Empty".to_string();
    }

    let mut counts = std::collections::BTreeMap::<String, usize>::new();
    for atom in &structure.atoms {
        *counts.entry(atom.element.clone()).or_default() += 1;
    }

    let mut symbols = counts.keys().cloned().collect::<Vec<_>>();
    symbols.sort_by(|a, b| hill_order(a).cmp(&hill_order(b)).then_with(|| a.cmp(b)));

    symbols
        .into_iter()
        .map(|symbol| {
            let count = counts[&symbol];
            if count == 1 {
                symbol
            } else {
                format!("{symbol}{count}")
            }
        })
        .collect::<Vec<_>>()
        .join("")
}

fn hill_order(symbol: &str) -> (u8, &str) {
    match symbol {
        "C" => (0, symbol),
        "H" => (1, symbol),
        _ => (2, symbol),
    }
}

pub fn require_periodic_structure(structure: &Structure, message: &str) -> Result<()> {
    if structure.cell.is_none() {
        bail!("{message}");
    }
    Ok(())
}
