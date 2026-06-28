use anyhow::{Context, Result, anyhow, bail};

use crate::{
    frontend::{
        console::{GlycanArgs, GlycosylateArgs},
        entry_ref::parse_anchor,
        ptm_commands::{PtmRequest, apply_ptm, atom_count},
        state::AppState,
    },
    io::structure_io::default_structure_save_path,
    workflows::glycan::{GlycosylationKind, glycan_to_structure},
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
    let kind = parse_glycosylation_kind(&args.kind)?;
    let residue = parse_anchor(&at)?;

    let entry_id = apply_ptm(
        state,
        &protein_ref,
        PtmRequest::Glycosylate {
            residue,
            iupac: iupac.clone(),
            kind,
        },
        args.name.as_deref(),
    )
    .with_context(|| format!("could not glycosylate `{protein_ref}` with `{iupac}` at {at}"))?;
    let label = args.name.as_deref().unwrap_or(&iupac);
    Ok(format!(
        "glycosylated {protein_ref} with {label} at {at} as entry #{entry_id} ({} atoms)",
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
