use super::*;
use crate::engines::registry::{EngineRegistry, EngineStatus, external_engine_specs};

/// Read-only perception over `AppState`: active entry, composition, open
/// entries, engine availability, and the latest status. Never mutates.
pub fn inspect(state: &AppState, query: Option<&str>) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "workspace: {}", state.workspace_label());

    let focused = query.and_then(|query| state.tasks.runs.execution(query.trim()));
    let focused_task =
        focused.and_then(|e| state.tasks.runs.task_run_id_for_job(&e.job_id.to_string()));
    for task in state
        .tasks
        .tasks
        .iter()
        .rev()
        .filter(|task| task.kind.is_qm())
        .filter(|task| focused_task.is_none_or(|id| id == task.id))
        .take(12)
    {
        if let Some(execution) = focused.or_else(|| state.tasks.runs.latest_execution(task.id)) {
            let result = execution
                .qm_result
                .as_ref()
                .map(|r| r.summary())
                .unwrap_or_else(|| "convergence and artifact status unknown".into());
            let _ = writeln!(
                out,
                "QM job {} (task {}, entry {:?}): execution {:?}; {}",
                execution.job_id,
                task.id,
                task.anchor_entry_id(),
                execution.execution_state,
                result
            );
        } else {
            let _ = writeln!(
                out,
                "QM task {}: convergence and artifact status unknown",
                task.id
            );
        }
        if let Some(dir) = &task.run_dir {
            let path = dir.join(crate::frontend::dispatcher::QM_OUTPUT_FILE);
            match std::fs::read_to_string(&path) {
                Ok(report) => {
                    let _ = writeln!(
                        out,
                        "QM report accessible at {}:\n{}",
                        path.display(),
                        clamp_result(&report)
                    );
                }
                Err(error) => {
                    let _ = writeln!(
                        out,
                        "QM report missing or inaccessible at {}: {error}",
                        path.display()
                    );
                }
            }
        } else {
            let _ = writeln!(out, "QM report location unknown");
        }
    }

    let entries = &state.entries.records;
    let _ = writeln!(out, "open entries: {}", entries.len());

    match state.entries.active_entry() {
        Some(active) => {
            let _ = writeln!(out, "active entry: #{} {}", active.id, active.name);
            let _ = writeln!(
                out,
                "structure: {}",
                crate::frontend::status_text(&active.structure, &state.ui.selection)
            );
            let formula = element_histogram(&active.structure);
            if !formula.is_empty() {
                let _ = writeln!(out, "composition: {formula}");
            }
            if !active.structure.bonds.is_empty() {
                let _ = writeln!(
                    out,
                    "geometry: {}",
                    crate::frontend::bond_geometry_summary(&active.structure)
                );
            }
            if let Some(bio) = &active.structure.biopolymer {
                let _ = writeln!(
                    out,
                    "biopolymer: {} chains, {} residues",
                    bio.chains.len(),
                    bio.residues.len()
                );
            }
            // Provenance: mark MD-run / QM-run outputs and trajectory availability.
            if active.origin.trajectory().is_some() {
                let _ = writeln!(out, "provenance: MD-run output (trajectory available)");
            } else if active.origin.is_qm_run() {
                let _ = writeln!(out, "provenance: QM-run output");
            }
        }
        None => {
            out.push_str("active entry: none (workspace is empty)\n");
        }
    }

    if entries.len() > 1 {
        // Each entry is listed with its id so the agent can `activate <#id>` a
        // non-active one; the active entry is marked so it is not re-activated.
        let active_id = state.entries.active_entry_id();
        let listed: Vec<String> = entries
            .iter()
            .take(12)
            .map(|entry| {
                let marker = if Some(entry.id) == active_id {
                    " (active)"
                } else {
                    ""
                };
                format!("#{} {}{}", entry.id, entry.name, marker)
            })
            .collect();
        let _ = writeln!(out, "entries: {}", listed.join(", "));
    }

    if let Some(playback) = &state.ui.trajectory {
        let _ = writeln!(
            out,
            "trajectory: playing frame {}/{}",
            playback.current_frame + 1,
            playback.frame_count()
        );
    }

    let registry = EngineRegistry::probe(&state.config.engine_overrides);
    let engines = external_engine_specs()
        .iter()
        .map(|spec| {
            let status = match registry.status(spec.id) {
                Some(EngineStatus::Verified { version, .. }) => {
                    format!("{version} (verified)")
                }
                Some(EngineStatus::Unverified { launch }) => {
                    format!("configured at {}, not verified", launch.display_command())
                }
                _ => "not configured".to_string(),
            };
            format!("{} {status}", spec.name)
        })
        .collect::<Vec<_>>()
        .join("; ");
    let _ = writeln!(out, "engines: {engines}");

    let running_jobs = crate::frontend::jobs::list_controlled_jobs(state)
        .into_iter()
        .filter(|job| job.status.is_running())
        .collect::<Vec<_>>();
    if running_jobs.is_empty() {
        let _ = writeln!(out, "running jobs: none");
    } else {
        let summary = running_jobs
            .iter()
            .take(5)
            .map(|job| {
                format!(
                    "{} {} ({})",
                    job.id.token(),
                    job.label,
                    job.stage.as_deref().unwrap_or(job.status.label())
                )
            })
            .collect::<Vec<_>>()
            .join("; ");
        let suffix = if running_jobs.len() > 5 {
            format!("; +{} more", running_jobs.len() - 5)
        } else {
            String::new()
        };
        let _ = writeln!(out, "running jobs: {summary}{suffix}");
    }

    let status = state
        .status_notice()
        .map(|notice| notice.text.as_str())
        .unwrap_or("(no active status)");
    let _ = writeln!(out, "status: {status}");
    out
}

/// A compact element histogram, most-common first (e.g. `C 6, H 12, O 6`).
fn element_histogram(structure: &crate::domain::Structure) -> String {
    use std::collections::BTreeMap;
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for atom in &structure.atoms {
        *counts.entry(atom.element.as_str()).or_default() += 1;
    }
    let mut pairs: Vec<(&str, usize)> = counts.into_iter().collect();
    pairs.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
    pairs
        .into_iter()
        .take(12)
        .map(|(element, count)| format!("{element} {count}"))
        .collect::<Vec<_>>()
        .join(", ")
}
