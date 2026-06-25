use anyhow::{Context, Result, anyhow, bail};

use crate::{
    domain::ResidueId,
    frontend::{
        console::{GlycanArgs, GlycosylateArgs},
        entry_ref::{entry_structure, resolve_entry_id},
        state::AppState,
    },
    io::structure_io::default_structure_save_path,
    workflows::glycan::{GlycosylationKind, glycan_to_structure, glycosylate_protein},
};

pub fn glycan_command(state: &mut AppState, args: GlycanArgs) -> Result<String> {
    let structure = glycan_to_structure(&args.iupac, args.name.as_deref()).with_context(|| {
        format!(
            "could not build glycan `{}` — check the IUPAC-condensed notation",
            args.iupac
        )
    })?;
    let atom_count = structure.atoms.len();
    let save_path = default_structure_save_path(&structure, None);
    let entry_id = state.entries.add_entry(structure, None, save_path);
    state.show_entry(entry_id);
    let label = args.name.as_deref().unwrap_or(&args.iupac);
    Ok(format!(
        "built glycan {label} as entry #{entry_id} ({atom_count} atoms)"
    ))
}

pub fn glycosylate_command(state: &mut AppState, args: GlycosylateArgs) -> Result<String> {
    let protein_ref = args
        .protein
        .as_deref()
        .ok_or_else(|| anyhow!("glycosylate requires --protein <entry>"))?;
    let iupac = args
        .iupac
        .as_deref()
        .ok_or_else(|| anyhow!("glycosylate requires --iupac <notation>"))?;
    let at = args
        .at
        .as_deref()
        .ok_or_else(|| anyhow!("glycosylate requires --at <chain:resSeq>"))?;
    let kind = parse_glycosylation_kind(&args.kind)?;
    let anchor = parse_anchor(at)?;

    let protein_id = resolve_entry_id(state, protein_ref)?;
    state.ensure_entry_loaded(protein_id);
    let protein = entry_structure(state, protein_id, "protein")?;

    let structure = glycosylate_protein(&protein, iupac, anchor, kind)
        .with_context(|| format!("could not glycosylate `{protein_ref}` with `{iupac}` at {at}"))?;
    let atom_count = structure.atoms.len();
    let save_path = default_structure_save_path(&structure, None);
    let entry_id = state.entries.add_entry(structure, None, save_path);
    state.show_entry(entry_id);
    let label = args.name.as_deref().unwrap_or(iupac);
    Ok(format!(
        "glycosylated {protein_ref} with {label} at {at} as entry #{entry_id} ({atom_count} atoms)"
    ))
}

fn parse_glycosylation_kind(spec: &str) -> Result<GlycosylationKind> {
    match spec.to_ascii_lowercase().as_str() {
        "n" | "n-linked" | "nlinked" => Ok(GlycosylationKind::NLinked),
        "o" | "o-linked" | "olinked" => Ok(GlycosylationKind::OLinked),
        other => bail!("--kind expects `n` or `o`, got `{other}`"),
    }
}

fn parse_anchor(spec: &str) -> Result<ResidueId> {
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
