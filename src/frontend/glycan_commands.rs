use anyhow::{Context, Result, anyhow, bail};

use crate::{
    domain::glycan::Anomer,
    frontend::{
        console::{GlycanArgs, GlycosylateArgs},
        entry_ref::parse_anchor,
        ptm_commands::{PtmRequest, apply_ptm, atom_count},
        state::AppState,
    },
    io::structure_io::default_structure_save_path,
    workflows::glycan::{GlycosylationKind, canonical_notation, glycan_to_structure},
};

pub fn glycan_command(state: &mut AppState, args: GlycanArgs) -> Result<String> {
    let structure = glycan_to_structure(&args.iupac, args.name.as_deref()).with_context(|| {
        format!(
            "could not build glycan `{}` — check the IUPAC-condensed notation",
            args.iupac
        )
    })?;
    let canonical = canonical_notation(&args.iupac, None, None)?;
    let atom_count = structure.atoms.len();
    let save_path = default_structure_save_path(&structure, None);
    let entry_id = state.entries.add_entry(structure, None, save_path);
    state.show_entry(entry_id);
    let label = args.name.as_deref().unwrap_or(&args.iupac);
    Ok(format!(
        "built glycan {label} as entry #{entry_id} ({atom_count} atoms); built as {canonical}"
    ))
}

pub fn glycosylate_command(state: &mut AppState, args: GlycosylateArgs) -> Result<String> {
    let protein_ref = args
        .protein
        .as_deref()
        .ok_or_else(|| anyhow!("glycosylate requires --protein <entry>"))?
        .to_string();
    let iupac = args
        .iupac
        .as_deref()
        .ok_or_else(|| anyhow!("glycosylate requires --iupac <notation>"))?
        .to_string();
    let at = args
        .at
        .as_deref()
        .ok_or_else(|| anyhow!("glycosylate requires --at <chain:resSeq>"))?
        .to_string();
    let kind = args
        .kind
        .as_deref()
        .map(parse_glycosylation_kind)
        .transpose()?;
    let root_anomer = args.anomer.as_deref().map(parse_anomer).transpose()?;
    let residue = parse_anchor(&at)?;

    let outcome = apply_ptm(
        state,
        &protein_ref,
        PtmRequest::Glycosylate {
            residue,
            iupac: iupac.clone(),
            kind,
            root_anomer,
        },
        args.name.as_deref(),
    )
    .with_context(|| format!("could not glycosylate `{protein_ref}` with `{iupac}` at {at}"))?;
    let entry_id = outcome.entry_id;
    let label = args.name.as_deref().unwrap_or(&iupac);
    let built = outcome.detail.unwrap_or_default();
    Ok(format!(
        "glycosylated {protein_ref} with {label} at {at} as entry #{entry_id} ({} atoms); \
         built as {built}",
        atom_count(state, entry_id)
    ))
}

fn parse_glycosylation_kind(spec: &str) -> Result<GlycosylationKind> {
    match spec.to_ascii_lowercase().as_str() {
        "n" | "n-linked" | "nlinked" => Ok(GlycosylationKind::NLinked),
        "o" | "o-linked" | "olinked" => Ok(GlycosylationKind::OLinked),
        other => bail!("--kind expects `n` or `o`, got `{other}`"),
    }
}

fn parse_anomer(spec: &str) -> Result<Anomer> {
    match spec.to_ascii_lowercase().as_str() {
        "a" | "alpha" => Ok(Anomer::Alpha),
        "b" | "beta" => Ok(Anomer::Beta),
        other => bail!("--anomer expects `a` or `b`, got `{other}`"),
    }
}
