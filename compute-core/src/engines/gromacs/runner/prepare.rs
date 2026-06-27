//! System preparation: writing the coordinate, topology, and optional index
//! files into a GROMACS working directory before any stage runs.

use std::{fs, path::Path};

use anyhow::{Context, Result};

use crate::engines::gromacs::{input, topology::TopologySource};

use super::{FreezeSelection, PrepareSystemRequest, PreparedSystem};

/// Write the coordinate file and materialize the topology into `working_dir`.
pub fn prepare_system(request: PrepareSystemRequest) -> Result<PreparedSystem> {
    fs::create_dir_all(&request.working_dir).with_context(|| {
        format!(
            "failed to create GROMACS working directory {}",
            request.working_dir.display()
        )
    })?;

    let conf_file = request.working_dir.join("conf.gro");
    fs::write(
        &conf_file,
        input::to_gro(&request.structure, &request.structure.title)?,
    )
    .with_context(|| format!("failed to write {}", conf_file.display()))?;

    let topology_file = request
        .topology
        .materialize(&request.working_dir, "topol.top")?;

    // A file topology reused from a build directory may `#include` sibling `.itp`
    // files (e.g. the `posre.itp` position restraints pdb2gmx writes). Copy them
    // alongside so grompp resolves the includes when the run directory differs
    // from the build directory.
    if let TopologySource::File(source) = &request.topology {
        copy_topology_includes(source, &request.working_dir)?;
    }

    let index_file = match &request.freeze {
        Some(freeze) => {
            let path = request.working_dir.join("index.ndx");
            fs::write(
                &path,
                render_index_file(request.structure.atoms.len(), freeze),
            )
            .with_context(|| format!("failed to write {}", path.display()))?;
            Some(path)
        }
        None => None,
    };

    Ok(PreparedSystem {
        working_dir: request.working_dir,
        conf_file,
        topology_file,
        index_file,
        original_structure: request.structure,
    })
}

/// Copy what a file topology `#include`s — sibling `.itp` files (e.g. pdb2gmx's
/// `posre.itp`) and any staged force-field directory (`charmm36.ff/…`) — from the
/// topology's source directory into the run directory, so the relative includes
/// resolve when the two directories differ. No-op when they are the same;
/// best-effort if the source dir can't be read.
pub(crate) fn copy_topology_includes(topology_source: &Path, target_dir: &Path) -> Result<()> {
    let Some(source_dir) = topology_source.parent() else {
        return Ok(());
    };
    if source_dir == target_dir {
        return Ok(());
    }
    let Ok(entries) = fs::read_dir(source_dir) else {
        return Ok(());
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name() else {
            continue;
        };
        if path.is_dir() {
            // A staged force-field bundle is a `<name>.ff` directory included by
            // relative path; carry the whole tree over.
            if path.extension().and_then(|ext| ext.to_str()) == Some("ff") {
                copy_dir_recursive(&path, &target_dir.join(name))?;
            }
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("itp") {
            fs::copy(&path, target_dir.join(name))
                .with_context(|| format!("copying topology include {}", path.display()))?;
        }
    }
    Ok(())
}

fn copy_dir_recursive(source: &Path, target: &Path) -> Result<()> {
    fs::create_dir_all(target)
        .with_context(|| format!("creating include directory {}", target.display()))?;
    for entry in fs::read_dir(source)
        .with_context(|| format!("reading include directory {}", source.display()))?
        .flatten()
    {
        let path = entry.path();
        let Some(name) = path.file_name() else {
            continue;
        };
        let dest = target.join(name);
        if path.is_dir() {
            copy_dir_recursive(&path, &dest)?;
        } else {
            fs::copy(&path, &dest)
                .with_context(|| format!("copying topology include {}", path.display()))?;
        }
    }
    Ok(())
}

/// Render a GROMACS index file (`.ndx`) with a `System` group covering every
/// atom and the named freeze group. Indices are 1-based, wrapped to a column
/// width GROMACS parses without issue.
pub(crate) fn render_index_file(atom_count: usize, freeze: &FreezeSelection) -> String {
    fn group(out: &mut String, name: &str, indices: impl Iterator<Item = usize>) {
        out.push_str(&format!("[ {name} ]\n"));
        for (n, index) in indices.enumerate() {
            out.push_str(&format!("{index:>6}"));
            if (n + 1) % 15 == 0 {
                out.push('\n');
            }
        }
        if !out.ends_with('\n') {
            out.push('\n');
        }
    }

    let mut out = String::new();
    group(&mut out, "System", 1..=atom_count);
    group(
        &mut out,
        &freeze.group,
        freeze.atom_indices.iter().map(|i| i + 1),
    );
    out
}
