use super::*;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Result, anyhow, bail};

/// Run a quantum-chemistry calculation.
///
/// `report` receives coarse stage strings (`"running scf"`, …). `cancel` is
/// **best-effort**: it is honored before the calculation starts, but hartree's
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
        ts,
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
        ts.as_ref(),
    )?;

    if cancel.load(Ordering::Relaxed) {
        bail!("cancelled before the calculation started");
    }
    report(match kind {
        QmKind::SinglePoint => "running scf",
        QmKind::Optimize => "optimizing geometry",
        QmKind::Frequencies => "running scf and hessian",
        QmKind::TransitionState => "searching for transition state",
    });

    let result = resolved
        .job
        .run()
        .map_err(|e| anyhow!("hartree calculation failed: {e}"))?;

    report("collecting results");

    let energy_hartree = result.best_energy();
    let converged = result.converged();

    // A TS search surfaces its best saddle even when it did not converge, so the
    // geometry survives for inspection / restart.
    let optimized_structure = match kind {
        QmKind::Optimize => result.optimized_geometry.as_ref().map(|opt| {
            let mut relaxed = structure_with_positions(&structure, &opt.positions);
            if let Ok(relaxed) = &mut relaxed {
                relaxed.title = format!(
                    "{} ({}/{} opt)",
                    structure.title,
                    method.label(),
                    resolved.basis
                );
            }
            relaxed
        }),
        QmKind::TransitionState => result.transition_state.as_ref().map(|ts| {
            let mut saddle = structure_with_positions(&structure, &ts.positions);
            if let Ok(saddle) = &mut saddle {
                saddle.title = format!(
                    "{} ({}/{} TS)",
                    structure.title,
                    method.label(),
                    resolved.basis
                );
            }
            saddle
        }),
        QmKind::SinglePoint | QmKind::Frequencies => None,
    }
    .transpose()?;

    let summary = format_summary(&method, &resolved, kind, &structure, &result);

    // The summary formatter deliberately trims the per-iteration tables; these
    // vectors carry them whole for the chart pipeline.
    let scf_trace: Vec<f64> = result.scf.history.iter().map(|step| step.energy).collect();
    let opt_trace: Vec<f64> = match kind {
        QmKind::Optimize => result
            .optimized_geometry
            .as_ref()
            .map(|opt| opt.history.iter().map(|step| step.energy).collect())
            .unwrap_or_default(),
        QmKind::TransitionState => result
            .transition_state
            .as_ref()
            .map(|ts| ts.history.iter().map(|step| step.energy).collect())
            .unwrap_or_default(),
        QmKind::SinglePoint | QmKind::Frequencies => Vec::new(),
    };
    let frequencies: Vec<f64> = match kind {
        QmKind::Frequencies => result
            .frequencies
            .as_ref()
            .map(|data| data.frequencies.frequencies_cm1.clone())
            .unwrap_or_default(),
        _ => Vec::new(),
    };

    Ok(QmOutcome {
        energy_hartree,
        converged,
        optimized_structure,
        summary,
        scf_trace,
        opt_trace,
        frequencies,
    })
}
