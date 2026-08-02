use super::*;

use std::path::PathBuf;

use anyhow::{Context, Result, anyhow, bail};

use crate::{
    backend::entries::EntryOrigin,
    frontend::state::AppState,
    io::{online_structures, pdb_fetch},
};

pub(crate) fn open_command(
    state: &mut AppState,
    context: &ScriptContext,
    path: &str,
) -> Result<String> {
    let path = context.resolve_path(path);
    open_structure_path(state, path.clone())?;
    Ok(format!("opened {}", path.display()))
}

/// `activate <#id|name>` — make an already-open entry the active one so the
/// next render/md/qm command targets it. `open`/`fetch`/`sketch` only ever
/// *create* a new active entry; this is the way to switch back to an existing
/// one without re-importing it. Entry ids are shown by `inspect` and echoed by
/// `sketch`/`open` (e.g. "entry #2").
pub(crate) fn activate_command(state: &mut AppState, reference: &str) -> Result<String> {
    let entry_id = resolve_entry_reference(state, reference)?;
    state.show_entry(entry_id);
    let name = state
        .entries
        .entry(entry_id)
        .map(|entry| entry.name.clone())
        .unwrap_or_default();
    Ok(format!("activated entry #{entry_id} ({name})"))
}

/// Resolve a user-facing entry reference to an entry id. A `#`-prefixed or bare
/// integer is treated as an entry id; anything else is matched against entry
/// names. Name matches must be unambiguous — duplicates (e.g. two `O=O` entries)
/// report the candidate ids so the caller can disambiguate by id.
fn resolve_entry_reference(state: &AppState, reference: &str) -> Result<u64> {
    if state.entries.records.is_empty() {
        bail!("no entries are open");
    }
    let trimmed = reference.trim();
    let id_token = trimmed.strip_prefix('#').unwrap_or(trimmed);
    if let Ok(id) = id_token.parse::<u64>() {
        if state.entries.entry(id).is_some() {
            return Ok(id);
        }
        bail!("no open entry with id #{id}; run `inspect` to list open entries");
    }

    let matches: Vec<u64> = state
        .entries
        .records
        .iter()
        .filter(|entry| entry.name == trimmed)
        .map(|entry| entry.id)
        .collect();
    match matches.as_slice() {
        [] => bail!("no open entry named `{trimmed}`; run `inspect` to list open entries"),
        [id] => Ok(*id),
        many => {
            let ids = many
                .iter()
                .map(|id| format!("#{id}"))
                .collect::<Vec<_>>()
                .join(", ");
            bail!(
                "`{trimmed}` matches {} entries ({ids}); activate by id, e.g. `activate #{}`",
                many.len(),
                many[0]
            )
        }
    }
}

pub(crate) fn fetch_command(
    state: &mut AppState,
    id: &str,
    db: Option<&str>,
    dir: Option<PathBuf>,
    revision: Option<&str>,
) -> Result<String> {
    if let Some(cod_id) = id.strip_prefix("cod:") {
        let mut candidate = online_structures::lookup_cod_candidate(cod_id, revision)?;
        if let Some(revision) = revision {
            candidate.revision = Some(revision.to_string());
        }
        let target_dir = dir.unwrap_or_else(|| state.structures_dir());
        let fetched = match db {
            Some(base_url) => {
                online_structures::fetch_cod_with_base_url(id, &candidate, &target_dir, base_url)?
            }
            None => online_structures::fetch_cod(id, &candidate, &target_dir)?,
        };
        let atom_count = fetched.structure.atoms.len();
        let save_path = crate::io::structure_io::default_structure_save_path(
            &fetched.structure,
            Some(&fetched.path),
        );
        let entry_id =
            state
                .entries
                .add_entry(fetched.structure, Some(fetched.path.clone()), save_path);
        state.entries.set_entry_origin(
            entry_id,
            EntryOrigin::Online {
                source: Box::new(fetched.source),
            },
        );
        state.show_entry(entry_id);
        let verb = if fetched.downloaded {
            "fetched"
        } else {
            "loaded cached"
        };
        return Ok(format!(
            "{verb} COD {cod_id} as entry #{entry_id} ({atom_count} atoms); inspect the crystal before starting a calculation"
        ));
    }
    if revision.is_some() {
        bail!("--revision is only valid with a cod:<id>");
    }
    let base_url = db.unwrap_or(pdb_fetch::RCSB_DEFAULT_BASE_URL);
    let target_dir = dir.unwrap_or_else(|| state.structures_dir());
    let fetched = pdb_fetch::fetch_pdb(id, base_url, &target_dir)?;
    open_structure_path(state, fetched.path.clone())?;
    let verb = if fetched.downloaded {
        "fetched"
    } else {
        "loaded cached"
    };
    Ok(format!("{verb} {}", fetched.path.display()))
}

pub(crate) fn find_command(
    _state: &mut AppState,
    text: &str,
    kind: online_structures::QueryKind,
    include_disorder: bool,
    limit: usize,
) -> Result<String> {
    let mut query = online_structures::StructureQuery::new(text, kind);
    query.include_disorder = include_disorder;
    query.limit = limit;
    let result = online_structures::search_structures(&query)?;
    Ok(format_structure_search_result(&result))
}

pub(crate) fn format_structure_search_result(
    result: &online_structures::StructureSearchResult,
) -> String {
    let mut lines = Vec::new();
    if result.crystals.is_empty() {
        lines.push("No compatible experimental COD crystals found.".to_string());
    } else {
        lines.push(format!(
            "COD candidates for `{}` (use `fetch cod:<id>` only after choosing one):",
            result.query
        ));
        for candidate in &result.crystals {
            let temperature = candidate
                .temperature_k
                .map(|value| format!("{value:.0} K"))
                .unwrap_or_else(|| "T unknown".to_string());
            let space_group = candidate.space_group.as_deref().unwrap_or("SG unknown");
            let r_factor = candidate
                .r_factor
                .map(|value| format!("R={value:.4}"))
                .unwrap_or_else(|| "R unknown".to_string());
            let warning = if candidate.warnings.is_empty() {
                String::new()
            } else {
                format!("; {}", candidate.warnings.join("; "))
            };
            lines.push(format!(
                "- cod:{} | {} | {} | {} | {} | {}{}",
                candidate.cod_id,
                candidate.name,
                candidate.formula,
                temperature,
                space_group,
                r_factor,
                warning
            ));
        }
    }
    if let Some(compound) = result.resolved.as_ref() {
        lines.push(format!(
            "Non-periodic fallback (PubChem CID {}): {} | {} | SMILES `{}`. This is not an experimental crystal; ask before building it.",
            compound.cid, compound.title, compound.formula, compound.smiles
        ));
    }
    lines.extend(
        result
            .warnings
            .iter()
            .map(|warning| format!("Warning: {warning}")),
    );
    lines.join("\n")
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
pub(crate) fn sketch_command(
    state: &mut AppState,
    smiles: &str,
    name: Option<&str>,
) -> Result<String> {
    let structure = crate::workflows::sketch_to_structure::smiles_to_structure(smiles, name)
        .with_context(|| {
            format!(
                "could not sketch `{smiles}` — check the SMILES; diatomics need explicit \
                 atoms (H₂ is `[H][H]`, O₂ is `O=O`, N₂ is `N#N`)"
            )
        })?;
    let atom_count = structure.atoms.len();
    let save_path = crate::io::structure_io::default_structure_save_path(&structure, None);
    let entry_id = state.entries.add_entry(structure, None, save_path);
    state.show_entry(entry_id);
    let label = name.unwrap_or(smiles);
    Ok(format!(
        "sketched {label} as entry #{entry_id} ({atom_count} atoms)"
    ))
}
