use super::*;

use std::path::PathBuf;

use anyhow::{Context, Result, anyhow, bail};

use crate::{frontend::state::AppState, io::pdb_fetch};

pub(crate) fn open_command(
    state: &mut AppState,
    context: &ScriptContext,
    args: &[String],
) -> Result<String> {
    let path = args
        .first()
        .ok_or_else(|| anyhow!("usage: open <structure-path>"))?;
    let path = context.resolve_path(path);
    open_structure_path(state, path.clone())?;
    Ok(format!("opened {}", path.display()))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedFetchCommand {
    pub(crate) id: String,
    pub(crate) base_url: String,
    pub(crate) dir: Option<PathBuf>,
}

pub(crate) fn fetch_command(state: &mut AppState, args: &[String]) -> Result<String> {
    let parsed = parse_fetch_command_args(args)?;
    let target_dir = parsed.dir.unwrap_or_else(|| state.structures_dir());
    let fetched = pdb_fetch::fetch_pdb(&parsed.id, &parsed.base_url, &target_dir)?;
    open_structure_path(state, fetched.path.clone())?;
    let verb = if fetched.downloaded {
        "fetched"
    } else {
        "loaded cached"
    };
    Ok(format!("{verb} {}", fetched.path.display()))
}

pub(crate) fn parse_fetch_command_args(args: &[String]) -> Result<ParsedFetchCommand> {
    let mut id: Option<String> = None;
    let mut base_url = pdb_fetch::RCSB_DEFAULT_BASE_URL.to_string();
    let mut dir = None;

    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--db" => {
                base_url = args.get(index + 1).cloned().ok_or_else(|| {
                    anyhow!("usage: fetch <pdb-id> [--db <base-url>] [--dir <directory>]")
                })?;
                index += 2;
            }
            "--dir" => {
                dir = Some(args.get(index + 1).map(PathBuf::from).ok_or_else(|| {
                    anyhow!("usage: fetch <pdb-id> [--db <base-url>] [--dir <directory>]")
                })?);
                index += 2;
            }
            flag if flag.starts_with("--") => bail!("unknown flag `{flag}` for fetch"),
            value => {
                if id.is_some() {
                    bail!("unexpected extra argument `{value}`; fetch takes a single PDB id");
                }
                id = Some(value.to_string());
                index += 1;
            }
        }
    }

    Ok(ParsedFetchCommand {
        id: id.ok_or_else(|| {
            anyhow!("usage: fetch <pdb-id> [--db <base-url>] [--dir <directory>]")
        })?,
        base_url,
        dir,
    })
}

/// Load a structure file at `path` into a new active entry, resetting the
/// viewport. Shared by the `open` and `fetch` commands.
fn open_structure_path(state: &mut AppState, path: PathBuf) -> Result<()> {
    let document = crate::frontend::structure_import::load_document(&path)?;
    state.save_viewport_for_active_entry();
    let entry_id =
        crate::frontend::structure_import::import_document(&mut state.entries, document, path)
            .ok_or_else(|| anyhow!("structure file did not contain any models"))?;
    state.entries.activate_entry(entry_id);
    state.history.set_active_entry(Some(entry_id));
    state.ui.entry_list.selected_entry_ids.clear();
    state.ui.entry_list.selected_entry_ids.insert(entry_id);
    state.ui.selection.clear();
    state.ui.camera = crate::frontend::ViewCamera::default();
    state.ui.viewport_cache.clear();
    state.load_viewport_for_active_entry();
    Ok(())
}

/// `sketch <SMILES>` — parse a SMILES string, generate a 3D structure, and add
/// it as a new active entry. The scriptable counterpart of the GUI sketcher's
/// Build action; available in both the console and headless `.sls` scripts.
pub(crate) fn sketch_command(state: &mut AppState, args: &[String]) -> Result<String> {
    let smiles = args
        .first()
        .ok_or_else(|| anyhow!("usage: sketch <SMILES>"))?;
    let structure = crate::workflows::sketch_to_structure::smiles_to_structure(smiles, None)
        .with_context(|| format!("could not sketch `{smiles}`"))?;
    let atom_count = structure.atoms.len();
    let save_path = crate::io::structure_io::default_structure_save_path(&structure, None);
    let entry_id = state.entries.add_entry(structure, None, save_path);
    state.show_entry(entry_id);
    Ok(format!(
        "sketched {smiles} as entry #{entry_id} ({atom_count} atoms)"
    ))
}

pub(crate) fn source_command(
    state: &mut AppState,
    context: &mut ScriptContext,
    args: &[String],
) -> Result<String> {
    let path = args
        .first()
        .ok_or_else(|| anyhow!("usage: source <script.sls>"))?;
    run_script_path_with_context(state, context, path)?;
    Ok(String::new())
}
