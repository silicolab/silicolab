use super::*;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Result, anyhow, bail};

/// Run a quantum-chemistry calculation.
///
/// `report` receives coarse stage strings (`"running scf"`, …). `cancel` is
/// **best-effort**: it is honored before the calculation starts, but chemx's
/// `Job::run` is a single opaque call with no preemption hook, so an in-flight
/// SCF cannot be interrupted — the worker runs to completion and the caller
/// discards the result.
pub fn run_qm(
    request: QmRequest,
    cancel: Arc<AtomicBool>,
    mut report: impl FnMut(&str),
) -> Result<QmOutcome> {
    let QmRequest {
        structure,
        method,
        basis,
        charge,
        multiplicity,
        kind,
        options,
    } = request;

    if structure.atoms.is_empty() {
        bail!("the structure has no atoms to compute");
    }
    if cancel.load(Ordering::Relaxed) {
        bail!("cancelled before the calculation started");
    }

    report("preparing molecule");
    let resolved = build_job(
        &structure,
        &method,
        &basis,
        charge,
        multiplicity,
        kind,
        &options,
    )?;

    if cancel.load(Ordering::Relaxed) {
        bail!("cancelled before the calculation started");
    }
    report(match kind {
        QmKind::SinglePoint => "running scf",
        QmKind::Optimize => "optimizing geometry",
        QmKind::Frequencies => "running scf and hessian",
    });

    let result = resolved
        .job
        .run()
        .map_err(|e| anyhow!("chemx calculation failed: {e}"))?;

    report("collecting results");

    let energy_hartree = result.best_energy();
    let converged = result.converged();

    let optimized_structure = match (kind, &result.optimized_geometry) {
        (QmKind::Optimize, Some(opt)) => {
            let mut relaxed = structure_with_positions(&structure, &opt.positions)?;
            // Distinguish the relaxed copy from the original in the entry list.
            relaxed.title = format!(
                "{} ({}/{} opt)",
                structure.title,
                method.label(),
                resolved.basis
            );
            Some(relaxed)
        }
        _ => None,
    };

    let summary = format_summary(&method, &resolved, kind, &structure, &result);

    Ok(QmOutcome {
        energy_hartree,
        converged,
        optimized_structure,
        summary,
    })
}
