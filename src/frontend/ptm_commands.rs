//! Console bodies for the six protein post-translational-modification (PTM)
//! families, plus the shared, UI-agnostic [`apply_ptm`] seam they route through.
//! Both the `.sls` console and the GUI build a [`PtmRequest`], hand it to
//! [`apply_ptm`], and let it resolve the protein entry, infer the side-chain
//! anchor from the residue's name, call the right `compute_core` PTM workflow,
//! and register the product as a new active entry — one implementation path.

use anyhow::{Context, Result, anyhow, bail};

use crate::{
    domain::{
        ProteinAnchor, ResidueId, Structure,
        modification::{AcylKind, MethylDegree, PrenylKind, UblKind},
    },
    frontend::{
        console::{
            AcetylateArgs, LipidateArgs, MethylateArgs, PhosphorylateArgs, UbiquitinateArgs,
        },
        entry_ref::{entry_structure, parse_anchor, resolve_entry_id},
        state::AppState,
    },
    io::structure_io::default_structure_save_path,
    workflows::{
        glycan::{GlycosylationKind, glycosylate_protein},
        ptm::{
            acetylate_protein, acylate_protein, methylate_protein, phosphorylate_protein,
            prenylate_protein, ubiquitinate_protein,
        },
    },
};

/// The unified lipid selector for the `lipidate` verb: a thioester/thioether
/// acylation or prenylation collapsed into one kind so the console exposes a
/// single command over the two `compute_core` entry points.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LipidKind {
    Palmitoyl,
    Myristoyl,
    Farnesyl,
    GeranylGeranyl,
}

/// A requested protein modification, independent of how it was entered. The
/// anchor atom is left implicit: [`apply_ptm`] infers it from the target
/// residue's name so callers only supply the residue (and the family selector).
#[derive(Debug, Clone)]
pub(crate) enum PtmRequest {
    Phosphorylate {
        residue: ResidueId,
    },
    Acetylate {
        residue: Option<ResidueId>,
        n_terminal: bool,
    },
    Methylate {
        residue: ResidueId,
        degree: MethylDegree,
    },
    Lipidate {
        residue: ResidueId,
        kind: LipidKind,
    },
    Ubiquitinate {
        residue: ResidueId,
        ubl: UblKind,
        /// An open entry id supplying a UBL template in place of the bundled one.
        with_entry: Option<u64>,
    },
    Glycosylate {
        residue: ResidueId,
        /// IUPAC-condensed glycan notation to build and attach.
        iupac: String,
        /// N-linked (Asn ND2) or O-linked (Ser/Thr OG); the workflow derives the
        /// anchor atom from this.
        kind: GlycosylationKind,
    },
}

/// Resolve `protein_entry`, apply `req` to its structure, and add the product as
/// a new active entry, returning that entry id. `output_name`, when non-empty,
/// names the new entry. The single seam the console and the GUI share: it owns
/// entry resolution, anchor inference, the `compute_core` call, naming, and
/// registration so neither front-end re-implements the dispatch.
pub(crate) fn apply_ptm(
    state: &mut AppState,
    protein_entry: &str,
    req: PtmRequest,
    output_name: Option<&str>,
) -> Result<u64> {
    let protein_id = resolve_entry_id(state, protein_entry)?;
    state.ensure_entry_loaded(protein_id);
    let protein = entry_structure(state, protein_id, "protein")?;

    let modified = match req {
        PtmRequest::Phosphorylate { residue } => {
            let anchor = phosphorylation_anchor(&protein, &residue)?;
            phosphorylate_protein(&protein, residue, anchor)?
        }
        PtmRequest::Acetylate {
            residue,
            n_terminal,
        } => {
            let residue = residue.ok_or_else(|| {
                anyhow!("acetylate requires a target residue (--at <chain:resSeq>)")
            })?;
            if !n_terminal {
                require_family(&protein, &residue, "acetylate", &["LYS"], "Lys")?;
            }
            acetylate_protein(&protein, residue, n_terminal)?
        }
        PtmRequest::Methylate { residue, degree } => {
            let anchor = methylation_anchor(&protein, &residue)?;
            methylate_protein(&protein, residue, anchor, degree)?
        }
        PtmRequest::Lipidate { residue, kind } => apply_lipidate(&protein, residue, kind)?,
        PtmRequest::Ubiquitinate {
            residue,
            ubl,
            with_entry,
        } => {
            require_family(&protein, &residue, "ubiquitinate", &["LYS"], "Lys")?;
            let override_structure = match with_entry {
                Some(id) => {
                    state.ensure_entry_loaded(id);
                    Some(entry_structure(state, id, "ubl override")?)
                }
                None => None,
            };
            ubiquitinate_protein(&protein, residue, ubl, override_structure.as_ref())?
        }
        PtmRequest::Glycosylate {
            residue,
            iupac,
            kind,
        } => {
            let site = label(&residue);
            glycosylate_protein(&protein, &iupac, residue, kind)
                .with_context(|| format!("could not glycosylate at {site}"))?
        }
    };

    let save_path = default_structure_save_path(&modified, None);
    let entry_id = state.entries.add_entry(modified, None, save_path);
    if let Some(name) = output_name.map(str::trim).filter(|name| !name.is_empty()) {
        state.entries.rename_entry(entry_id, name.to_string());
    }
    state.show_entry(entry_id);
    Ok(entry_id)
}

fn apply_lipidate(protein: &Structure, residue: ResidueId, kind: LipidKind) -> Result<Structure> {
    match kind {
        LipidKind::Palmitoyl => {
            require_family(protein, &residue, "lipidate", &["CYS"], "Cys")?;
            acylate_protein(protein, residue, AcylKind::Palmitoyl)
        }
        LipidKind::Myristoyl => {
            require_family(protein, &residue, "lipidate", &["GLY"], "Gly (N-terminal)")?;
            acylate_protein(protein, residue, AcylKind::Myristoyl)
        }
        LipidKind::Farnesyl => {
            require_family(protein, &residue, "lipidate", &["CYS"], "Cys")?;
            prenylate_protein(protein, residue, PrenylKind::Farnesyl)
        }
        LipidKind::GeranylGeranyl => {
            require_family(protein, &residue, "lipidate", &["CYS"], "Cys")?;
            prenylate_protein(protein, residue, PrenylKind::GeranylGeranyl)
        }
    }
}

/// Map a phosphorylation target's residue name to its anchor atom.
fn phosphorylation_anchor(protein: &Structure, residue: &ResidueId) -> Result<ProteinAnchor> {
    match residue_name(protein, residue, "phosphorylate")?.as_str() {
        "SER" => Ok(ProteinAnchor::SerOg),
        "THR" => Ok(ProteinAnchor::ThrOg1),
        "TYR" => Ok(ProteinAnchor::TyrOh),
        "HIS" => Ok(ProteinAnchor::HisNe2),
        other => Err(family_mismatch(
            "phosphorylate",
            residue,
            other,
            "Ser/Thr/Tyr/His",
        )),
    }
}

/// Map a methylation target's residue name to its anchor atom.
fn methylation_anchor(protein: &Structure, residue: &ResidueId) -> Result<ProteinAnchor> {
    match residue_name(protein, residue, "methylate")?.as_str() {
        "LYS" => Ok(ProteinAnchor::LysNz),
        "ARG" => Ok(ProteinAnchor::ArgNh1),
        other => Err(family_mismatch("methylate", residue, other, "Lys/Arg")),
    }
}

/// Require `residue` to be one of `allowed` (upper-case three-letter names),
/// reporting a verb-specific mismatch otherwise.
fn require_family(
    protein: &Structure,
    residue: &ResidueId,
    verb: &str,
    allowed: &[&str],
    expected: &str,
) -> Result<()> {
    let name = residue_name(protein, residue, verb)?;
    if !allowed.contains(&name.as_str()) {
        return Err(family_mismatch(verb, residue, &name, expected));
    }
    Ok(())
}

/// The upper-cased residue name for `residue`, or a "not found" error tagged with
/// the verb.
fn residue_name(protein: &Structure, residue: &ResidueId, verb: &str) -> Result<String> {
    protein
        .biopolymer
        .as_ref()
        .and_then(|bio| bio.residues.iter().find(|record| &record.id == residue))
        .map(|record| record.residue_name.trim().to_ascii_uppercase())
        .ok_or_else(|| {
            anyhow!(
                "{verb}: residue {} not found in the protein",
                label(residue)
            )
        })
}

fn family_mismatch(verb: &str, residue: &ResidueId, name: &str, expected: &str) -> anyhow::Error {
    anyhow!(
        "{verb}: residue {} is {name}, expected {expected}",
        label(residue)
    )
}

fn label(residue: &ResidueId) -> String {
    if residue.insertion_code == ' ' {
        format!("{}:{}", residue.chain_id, residue.sequence_number)
    } else {
        format!(
            "{}:{}{}",
            residue.chain_id, residue.sequence_number, residue.insertion_code
        )
    }
}

pub(crate) fn atom_count(state: &AppState, entry_id: u64) -> usize {
    state
        .entries
        .entry(entry_id)
        .map(|entry| entry.structure.atoms.len())
        .unwrap_or_default()
}

pub(crate) fn phosphorylate_command(
    state: &mut AppState,
    args: PhosphorylateArgs,
) -> Result<String> {
    let protein_ref = args
        .protein
        .as_deref()
        .ok_or_else(|| anyhow!("phosphorylate requires --protein <entry>"))?
        .to_string();
    let at = args
        .at
        .as_deref()
        .ok_or_else(|| anyhow!("phosphorylate requires --at <chain:resSeq>"))?
        .to_string();
    let residue = parse_anchor(&at)?;
    let entry_id = apply_ptm(
        state,
        &protein_ref,
        PtmRequest::Phosphorylate { residue },
        args.name.as_deref(),
    )?;
    Ok(format!(
        "phosphorylated {protein_ref} at {at} as entry #{entry_id} ({} atoms)",
        atom_count(state, entry_id)
    ))
}

pub(crate) fn acetylate_command(state: &mut AppState, args: AcetylateArgs) -> Result<String> {
    let protein_ref = args
        .protein
        .as_deref()
        .ok_or_else(|| anyhow!("acetylate requires --protein <entry>"))?
        .to_string();
    let at = args
        .at
        .as_deref()
        .ok_or_else(|| anyhow!("acetylate requires --at <chain:resSeq>"))?
        .to_string();
    let residue = parse_anchor(&at)?;
    let n_terminal = args.n_terminal;
    let entry_id = apply_ptm(
        state,
        &protein_ref,
        PtmRequest::Acetylate {
            residue: Some(residue),
            n_terminal,
        },
        args.name.as_deref(),
    )?;
    let site = if n_terminal { "N-terminus" } else { "Lys NZ" };
    Ok(format!(
        "acetylated {protein_ref} at {at} ({site}) as entry #{entry_id} ({} atoms)",
        atom_count(state, entry_id)
    ))
}

pub(crate) fn methylate_command(state: &mut AppState, args: MethylateArgs) -> Result<String> {
    let protein_ref = args
        .protein
        .as_deref()
        .ok_or_else(|| anyhow!("methylate requires --protein <entry>"))?
        .to_string();
    let at = args
        .at
        .as_deref()
        .ok_or_else(|| anyhow!("methylate requires --at <chain:resSeq>"))?
        .to_string();
    let degree = parse_methyl_degree(&args.degree)?;
    let residue = parse_anchor(&at)?;
    let entry_id = apply_ptm(
        state,
        &protein_ref,
        PtmRequest::Methylate { residue, degree },
        args.name.as_deref(),
    )?;
    Ok(format!(
        "{}methylated {protein_ref} at {at} as entry #{entry_id} ({} atoms)",
        methyl_prefix(degree),
        atom_count(state, entry_id)
    ))
}

pub(crate) fn lipidate_command(state: &mut AppState, args: LipidateArgs) -> Result<String> {
    let protein_ref = args
        .protein
        .as_deref()
        .ok_or_else(|| anyhow!("lipidate requires --protein <entry>"))?
        .to_string();
    let at = args
        .at
        .as_deref()
        .ok_or_else(|| anyhow!("lipidate requires --at <chain:resSeq>"))?
        .to_string();
    let kind = parse_lipid_kind(&args.kind)?;
    let residue = parse_anchor(&at)?;
    let entry_id = apply_ptm(
        state,
        &protein_ref,
        PtmRequest::Lipidate { residue, kind },
        args.name.as_deref(),
    )?;
    Ok(format!(
        "lipidated {protein_ref} with {} at {at} as entry #{entry_id} ({} atoms)",
        lipid_label(kind),
        atom_count(state, entry_id)
    ))
}

pub(crate) fn ubiquitinate_command(state: &mut AppState, args: UbiquitinateArgs) -> Result<String> {
    let protein_ref = args
        .protein
        .as_deref()
        .ok_or_else(|| anyhow!("ubiquitinate requires --protein <entry>"))?
        .to_string();
    let at = args
        .at
        .as_deref()
        .ok_or_else(|| anyhow!("ubiquitinate requires --at <chain:resSeq>"))?
        .to_string();
    let ubl = parse_ubl_kind(&args.ubl)?;
    let with_entry = match args.with.as_deref() {
        Some(reference) => Some(resolve_entry_id(state, reference)?),
        None => None,
    };
    let residue = parse_anchor(&at)?;
    let entry_id = apply_ptm(
        state,
        &protein_ref,
        PtmRequest::Ubiquitinate {
            residue,
            ubl,
            with_entry,
        },
        args.name.as_deref(),
    )?;
    Ok(format!(
        "conjugated {} to {protein_ref} at {at} as entry #{entry_id} ({} atoms)",
        ubl_label(ubl),
        atom_count(state, entry_id)
    ))
}

fn parse_methyl_degree(spec: &str) -> Result<MethylDegree> {
    match spec.to_ascii_lowercase().as_str() {
        "mono" | "1" => Ok(MethylDegree::Mono),
        "di" | "2" => Ok(MethylDegree::Di),
        "tri" | "3" => Ok(MethylDegree::Tri),
        other => bail!("--degree expects `mono`, `di`, or `tri`, got `{other}`"),
    }
}

fn parse_lipid_kind(spec: &str) -> Result<LipidKind> {
    match spec.to_ascii_lowercase().as_str() {
        "palmitoyl" => Ok(LipidKind::Palmitoyl),
        "myristoyl" => Ok(LipidKind::Myristoyl),
        "farnesyl" => Ok(LipidKind::Farnesyl),
        "geranylgeranyl" | "geranyl-geranyl" => Ok(LipidKind::GeranylGeranyl),
        other => bail!(
            "--kind expects `palmitoyl`, `myristoyl`, `farnesyl`, or `geranylgeranyl`, got `{other}`"
        ),
    }
}

fn parse_ubl_kind(spec: &str) -> Result<UblKind> {
    match spec.to_ascii_lowercase().as_str() {
        "ubiquitin" | "ub" => Ok(UblKind::Ubiquitin),
        "sumo" | "sumo1" => Ok(UblKind::Sumo),
        "nedd8" => Ok(UblKind::Nedd8),
        other => bail!("--ubl expects `ubiquitin`, `sumo`, or `nedd8`, got `{other}`"),
    }
}

pub(crate) fn methyl_prefix(degree: MethylDegree) -> &'static str {
    match degree {
        MethylDegree::Mono => "mono",
        MethylDegree::Di => "di",
        MethylDegree::Tri => "tri",
    }
}

pub(crate) fn lipid_label(kind: LipidKind) -> &'static str {
    match kind {
        LipidKind::Palmitoyl => "palmitoyl",
        LipidKind::Myristoyl => "myristoyl",
        LipidKind::Farnesyl => "farnesyl",
        LipidKind::GeranylGeranyl => "geranylgeranyl",
    }
}

pub(crate) fn ubl_label(kind: UblKind) -> &'static str {
    match kind {
        UblKind::Ubiquitin => "ubiquitin",
        UblKind::Sumo => "sumo",
        UblKind::Nedd8 => "nedd8",
    }
}
