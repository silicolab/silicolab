use std::fmt::Write as _;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Result, anyhow, bail};

use crate::io::formats::pdbqt::{parse_pdbqt, prepare_ligand_pdbqt, prepare_receptor_pdbqt};

use super::types::*;

/// Run a molecular docking calculation against the bundled Vina engine.
///
/// `report` receives coarse stage strings. `cancel` is **best-effort**: it is
/// honored before the search begins, but `docking::api::dock` is a single opaque
/// blocking call (a Monte-Carlo search with no preemption hook), so an in-flight
/// search runs to completion and the caller discards the result on cancel.
pub fn run_docking(
    request: DockingRequest,
    cancel: Arc<AtomicBool>,
    mut report: impl FnMut(&str),
) -> Result<DockingOutcome> {
    let DockingRequest {
        receptor,
        ligand,
        box_center,
        box_size,
        config,
        kind,
    } = request;

    if box_size.iter().any(|&s| s <= 0.0) {
        bail!("the search box must have a positive size on every axis");
    }
    if cancel.load(Ordering::Relaxed) {
        bail!("cancelled before docking started");
    }

    let mut notes = Vec::new();

    report("preparing receptor");
    let receptor_pdbqt = prepare_input(receptor, InputRole::Receptor, &mut notes)?;
    report("preparing ligand");
    let ligand_pdbqt = prepare_input(ligand, InputRole::Ligand, &mut notes)?;

    if cancel.load(Ordering::Relaxed) {
        bail!("cancelled before docking started");
    }

    let poses = match kind {
        DockingKind::Dock => {
            report(&format!(
                "searching (exhaustiveness {})",
                config.exhaustiveness
            ));
            let cfg = docking::api::DockConfig {
                exhaustiveness: config.exhaustiveness.max(1),
                num_modes: config.num_modes.max(1),
                seed: config.seed,
                ..Default::default()
            };
            let raw =
                docking::api::dock(&receptor_pdbqt, &ligand_pdbqt, box_center, box_size, &cfg)
                    .map_err(|e| anyhow!("docking search failed: {e}"))?;
            report("collecting poses");
            raw.into_iter()
                .enumerate()
                .map(|(rank, pose)| {
                    pose_to_docked(
                        rank,
                        pose.pdbqt,
                        pose.affinity,
                        pose.intermolecular,
                        pose.internal,
                        pose.torsional,
                    )
                })
                .collect::<Result<Vec<_>>>()?
        }
        DockingKind::ScoreOnly => {
            report("scoring input pose");
            let breakdown =
                docking::api::score_only(&receptor_pdbqt, &ligand_pdbqt, box_center, box_size)
                    .map_err(|e| anyhow!("scoring failed: {e}"))?;
            // The scored pose is the ligand's input conformation.
            vec![pose_to_docked(
                0,
                ligand_pdbqt.clone(),
                breakdown.estimated_free_energy,
                breakdown.intermolecular,
                breakdown.internal,
                breakdown.torsional,
            )?]
        }
    };

    if poses.is_empty() {
        bail!("docking returned no poses");
    }

    let summary = format_summary(kind, &poses, &notes);
    Ok(DockingOutcome {
        poses,
        notes,
        summary,
    })
}

#[derive(Clone, Copy)]
enum InputRole {
    Receptor,
    Ligand,
}

/// Convert a receptor/ligand input to PDBQT text, recording preparation caveats.
fn prepare_input(input: DockingInput, role: InputRole, notes: &mut Vec<String>) -> Result<String> {
    match input {
        DockingInput::Pdbqt(text) => {
            if text.trim().is_empty() {
                bail!("the supplied PDBQT input is empty");
            }
            Ok(text)
        }
        DockingInput::Structure(structure) => {
            let prepared = match role {
                InputRole::Receptor => prepare_receptor_pdbqt(&structure)?,
                InputRole::Ligand => prepare_ligand_pdbqt(&structure)?,
            };
            let what = match role {
                InputRole::Receptor => "receptor",
                InputRole::Ligand => "ligand",
            };
            notes.push(format!(
                "{what} was prepared heuristically by silicolab (approximate atom typing); \
                 supply an already-prepared .pdbqt for production-quality results"
            ));
            notes.extend(prepared.notes);
            Ok(prepared.text)
        }
    }
}

/// Parse a pose's PDBQT into a structure and assemble a [`DockedPose`].
fn pose_to_docked(
    rank: usize,
    pdbqt: String,
    affinity: f64,
    intermolecular: f64,
    internal: f64,
    torsional: f64,
) -> Result<DockedPose> {
    let mut structure = parse_pdbqt(&pdbqt)?;
    structure.title = format!("pose {} ({:+.2} kcal/mol)", rank + 1, affinity);
    Ok(DockedPose {
        affinity,
        intermolecular,
        internal,
        torsional,
        structure,
        pdbqt,
    })
}

fn format_summary(kind: DockingKind, poses: &[DockedPose], notes: &[String]) -> String {
    let mut out = String::new();
    match kind {
        DockingKind::Dock => {
            let _ = writeln!(out, "Docking complete: {} pose(s).", poses.len());
            let _ = writeln!(out, "\n  rank  affinity (kcal/mol)   inter    intra");
            for (rank, pose) in poses.iter().enumerate() {
                let _ = writeln!(
                    out,
                    "  {:>4}  {:>17.2}   {:>6.2}   {:>6.2}",
                    rank + 1,
                    pose.affinity,
                    pose.intermolecular,
                    pose.internal,
                );
            }
        }
        DockingKind::ScoreOnly => {
            if let Some(pose) = poses.first() {
                let _ = writeln!(out, "Score only:");
                let _ = writeln!(
                    out,
                    "  estimated free energy: {:.2} kcal/mol",
                    pose.affinity
                );
                let _ = writeln!(
                    out,
                    "  intermolecular:        {:.2} kcal/mol",
                    pose.intermolecular
                );
                let _ = writeln!(
                    out,
                    "  internal:              {:.2} kcal/mol",
                    pose.internal
                );
                let _ = writeln!(
                    out,
                    "  torsional:             {:.2} kcal/mol",
                    pose.torsional
                );
            }
        }
    }
    for note in notes {
        let _ = writeln!(out, "\nnote: {note}");
    }
    out
}
