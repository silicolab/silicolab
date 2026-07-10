//! Dispatcher glue for the GROMACS relay: submit a prepared remote `gmx` job
//! detached, and apply a retrieved relay outcome back to the open project. The
//! remote analogue of the local pipeline/build finish paths in `frontend::jobs`;
//! mirrors `dispatcher::docking`'s remote handling.

use super::*;

use crate::workflows::gromacs::{GromacsJob, GromacsOutcome, WireTopology};

/// Submit a prepared GROMACS job to a remote host as a detached relay: tag the
/// task with the engine label, then deploy + stage + launch via the shared
/// [`start_remote_engine`]. `gmx` is resolved and the whole pipeline runs on the
/// node, so no client launch or core count rides along.
pub(crate) fn relay_gromacs_job(
    state: &mut AppState,
    host: crate::backend::config::RemoteHost,
    engine_label: &str,
    job: GromacsJob,
    resources: crate::backend::config::JobResources,
) {
    if let Some(task_run_id) = state.active_task_run {
        state
            .tasks
            .set_engine_label(task_run_id, Some(engine_label.to_string()));
        sync_task_manifest(state, task_run_id);
    }
    start_remote_engine(state, host, crate::wire::Engine::Gromacs(job), resources);
}

/// Apply a retrieved remote GROMACS outcome: log the summary, and — only when the
/// job belongs to the current workspace — write the run's artifacts beside it and
/// add the produced structure as an entry (a run becomes an MD-run entry, a build
/// an ordinary one), then mark the task complete. The detached analogue of the
/// local pipeline/build finish paths; mirrors `apply_remote_docking_outcome`.
///
/// "Belongs to the current workspace" ([`outcome_belongs_to_current_workspace`])
/// means the job's origin matches what is open now: its own project, or a scratch
/// session for a job submitted with no project open. The materialization is gated
/// this way because a build's `topol.top`/system context are discoverable by a
/// later run ONLY through its result entry (`latest_completed_run_for_result`), so
/// writing them while a *different* project is open would orphan files no run could
/// find. A build retrieved while a different project is open is therefore left
/// alone (matching QM/docking) — open its project and refresh to reuse it.
pub(crate) fn apply_remote_gromacs_outcome(
    state: &mut AppState,
    row: &crate::backend::storage::jobs::RemoteJob,
    outcome: GromacsOutcome,
) {
    for line in outcome.summary.lines() {
        state.output_log.push(line.to_string());
    }

    let belongs_here = outcome_belongs_to_current_workspace(state, row);
    let task_id = state
        .tasks
        .task_run_by_uuid(&row.run_uuid)
        .map(|task| task.id);

    if belongs_here {
        let run_dir = PathBuf::from(&row.local_run_dir);
        let _ = std::fs::create_dir_all(&run_dir);
        // A build carries the topology + system context a later run reuses; a run
        // carries the trajectory. Persist whichever the outcome holds.
        if let Some(topology) = &outcome.topology {
            write_wire_topology(&run_dir, topology);
        }
        if let Some(context) = &outcome.system_context {
            let _ =
                context.save(&run_dir.join(crate::frontend::md_support::MD_SYSTEM_CONTEXT_FILE));
        }
        if let Some(material) = &outcome.material {
            let meta = crate::frontend::md_support::FrameworkRunMetadata {
                periodic_molecules: material.hints.periodic_molecules,
                freeze_group: material.hints.freeze_group.clone(),
                framework_atom_count: material.framework_atom_count,
            };
            let _ = meta.save(&run_dir.join(crate::frontend::md_support::MD_FRAMEWORK_FILE));
        }
        let trajectory_path = outcome.trajectory.as_ref().and_then(|trajectory| {
            let path = run_dir.join(&trajectory.file_name);
            std::fs::write(&path, &trajectory.bytes).ok().map(|()| path)
        });
        // A build records a system context; a run does not — that distinguishes the
        // entry provenance (an MD-run badge + playback vs an ordinary built system).
        let is_run = outcome.system_context.is_none();
        let structure = outcome.structure;
        let save_path = structure_io::default_structure_save_path(&structure, None);
        let entry_id = add_and_show_entry(state, structure, None, save_path);
        if let Some(task_id) = task_id {
            record_task_result_entry(state, task_id, entry_id);
        }
        if is_run {
            set_md_run_origin(state, entry_id, trajectory_path);
        }
    }
    if let Some(task_id) = task_id {
        mark_task_status(state, task_id, TaskStatus::Completed);
    }
    let headline = outcome
        .summary
        .lines()
        .next()
        .unwrap_or("GROMACS run complete");
    state.set_message(format!("Remote {headline}"));
}

/// Write a relayed topology (`topol.top` plus its `.itp` includes) into the run
/// directory so a later run reuses it via `TopologySource::File`.
fn write_wire_topology(run_dir: &Path, topology: &WireTopology) {
    let _ = std::fs::write(
        run_dir.join(crate::frontend::md_support::MD_GROMACS_TOPOLOGY_FILE),
        &topology.top,
    );
    for (name, contents) in &topology.includes {
        let _ = std::fs::write(run_dir.join(name), contents);
    }
}
