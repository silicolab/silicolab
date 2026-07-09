use anyhow::{Result, bail};

use super::{ExportArgs, ExportFormatArg, ExportScopeArg, ScriptContext};
use crate::frontend::{
    dispatcher::{
        directory_export_summary, entry_ids_for_scope, selection_entry_ids, structure_count,
        writable_format_of, write_ids_to_directory, write_ids_to_file,
    },
    state::{AppState, ExportScope},
};
use crate::io::structure_format::StructureFormat;

pub(crate) fn export_command(
    state: &mut AppState,
    context: &mut ScriptContext,
    args: ExportArgs,
) -> Result<String> {
    let scope = match args.scope {
        ExportScopeArg::Active => ExportScope::Active,
        ExportScopeArg::Selected => ExportScope::Selected,
        ExportScopeArg::All => ExportScope::All,
    };
    let selected = selection_entry_ids(state);
    let ids = entry_ids_for_scope(state, scope, &selected);
    if ids.is_empty() {
        bail!("nothing to export for scope `{:?}`", args.scope);
    }

    let path = context.resolve_path(&args.path);
    // A structure extension means "write this one file"; anything else names a
    // folder to fill, so a script never has to guess which the console chose.
    if writable_format_of(&path).is_some() {
        write_ids_to_file(state, &ids, &path)?;
        return Ok(format!(
            "exported {} to {}",
            structure_count(ids.len()),
            path.display()
        ));
    }

    std::fs::create_dir_all(&path)?;
    let (paths, results) = write_ids_to_directory(state, &ids, &path, format_of(args.format));
    let summary = directory_export_summary(&path, &paths, &results);
    if results.iter().all(|result| result.is_err()) {
        bail!("{summary}");
    }
    Ok(summary)
}

fn format_of(format: ExportFormatArg) -> StructureFormat {
    match format {
        ExportFormatArg::Xyz => StructureFormat::Xyz,
        ExportFormatArg::Cif => StructureFormat::Cif,
        ExportFormatArg::Mol2 => StructureFormat::Mol2,
        ExportFormatArg::Pdb => StructureFormat::Pdb,
        ExportFormatArg::Pdbqt => StructureFormat::Pdbqt,
    }
}
