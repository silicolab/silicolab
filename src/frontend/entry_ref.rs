//! Resolving a console `<entry>` argument — `active`, a numeric id (`#2` or `2`),
//! or an entry name — to an entry id and its structure. Shared by the `dock`,
//! `score`, and `qm` command groups so the reference grammar stays identical.

use anyhow::{Result, anyhow, bail};

use crate::domain::{ResidueId, Structure};
use crate::frontend::state::AppState;

/// Resolve `active`, `#id`/`id`, or an entry name to an entry id.
pub(crate) fn resolve_entry_id(state: &AppState, reference: &str) -> Result<u64> {
    if reference.eq_ignore_ascii_case("active") {
        return state
            .entries
            .active_entry_id()
            .ok_or_else(|| anyhow!("no active entry to use for `{reference}`"));
    }
    if let Some(id) = reference
        .strip_prefix('#')
        .unwrap_or(reference)
        .parse::<u64>()
        .ok()
        .filter(|id| state.entries.entry(*id).is_some())
    {
        return Ok(id);
    }
    state
        .entries
        .records
        .iter()
        .find(|record| record.name.eq_ignore_ascii_case(reference))
        .map(|record| record.id)
        .ok_or_else(|| {
            anyhow!(
                "no open entry matches `{reference}` (use active, an id like #2, or an entry name)"
            )
        })
}

/// Clone a non-empty structure out of an open entry, naming the `role` in errors.
pub(crate) fn entry_structure(state: &AppState, entry_id: u64, role: &str) -> Result<Structure> {
    let entry = state
        .entries
        .entry(entry_id)
        .ok_or_else(|| anyhow!("{role} entry #{entry_id} not found"))?;
    if entry.structure.atoms.is_empty() {
        bail!("{role} entry #{entry_id} has no atoms");
    }
    Ok(entry.structure.clone())
}

/// Parse a `chain:resSeq` anchor reference (e.g. `A:297`, `B:52A`) into a
/// [`ResidueId`]. Shared by the glycosylation and PTM `--at` arguments so the
/// residue grammar stays identical across the modification commands.
pub(crate) fn parse_anchor(spec: &str) -> Result<ResidueId> {
    let (chain_part, rest) = spec
        .split_once(':')
        .ok_or_else(|| anyhow!("--at expects `chain:resSeq` (e.g. A:297), got `{spec}`"))?;
    let chain_id = {
        let mut chars = chain_part.trim().chars();
        let chain = chars
            .next()
            .ok_or_else(|| anyhow!("--at chain id is empty in `{spec}`"))?;
        if chars.next().is_some() {
            bail!("--at chain id must be a single character in `{spec}`");
        }
        chain
    };
    let rest = rest.trim();
    let (digits, insertion_code) = match rest.find(|ch: char| !ch.is_ascii_digit() && ch != '-') {
        Some(split) => {
            let (num, code) = rest.split_at(split);
            let code = code.chars().next().unwrap_or(' ');
            (num, code)
        }
        None => (rest, ' '),
    };
    let sequence_number = digits
        .parse::<i32>()
        .map_err(|_| anyhow!("--at residue number is invalid in `{spec}`"))?;
    Ok(ResidueId::new(chain_id, sequence_number, insertion_code))
}
