use super::*;

use std::fmt::Write as _;

use crate::engines::docking::{
    DockingConfig, DockingInput, DockingKind, DockingOutcome, DockingRequest,
};
use crate::frontend::jobs::{DockingWorkerMessage, spawn_docking_job};

/// File name of the saved multi-pose PDBQT inside a docking task's run directory.
pub(crate) const DOCK_POSES_FILE: &str = "poses.pdbqt";

/// Launch the molecular docking run described by the panel form.
pub(crate) fn start_pending_docking(state: &mut AppState) {
    bind_active_panel_task(state, TaskPanelKind::DockingPrompt);
    let Some(prompt) = state.ui.pending_docking.clone() else {
        return;
    };
    if state.jobs.docking_running() {
        state.set_message("a docking run is already in progress; press Esc to stop".to_string());
        return;
    }
    let Some(receptor_id) = prompt.receptor_entry else {
        state.set_message("choose a receptor entry before docking".to_string());
        return;
    };
    let Some(ligand_id) = prompt.ligand_entry else {
        state.set_message("choose a ligand entry before docking".to_string());
        return;
    };
    if receptor_id == ligand_id {
        state.set_message("the receptor and ligand must be different entries".to_string());
        return;
    }
    if prompt.box_size.iter().any(|&size| size <= 0.0) {
        state.set_message("the search box must have a positive size on every axis".to_string());
        return;
    }
    let Some(receptor) = entry_structure(state, receptor_id) else {
        state.set_message("the receptor entry has no structure".to_string());
        return;
    };
    let Some(ligand) = entry_structure(state, ligand_id) else {
        state.set_message("the ligand entry has no structure".to_string());
        return;
    };
    // Create the run directory up front so the poses artifact has a home and the
    // run records its source entry.
    if let Err(error) = ensure_active_task_run_dir(state, TaskKind::RunDocking, None) {
        state.set_message(format!("could not create docking run directory: {error}"));
        return;
    }

    let request = DockingRequest {
        receptor: DockingInput::Structure(Box::new(receptor)),
        ligand: DockingInput::Structure(Box::new(ligand)),
        box_center: prompt.box_center.map(f64::from),
        box_size: prompt.box_size.map(f64::from),
        config: DockingConfig {
            exhaustiveness: prompt.exhaustiveness.max(1) as usize,
            num_modes: prompt.num_modes.max(1) as usize,
            seed: prompt.seed,
        },
        kind: if prompt.score_only {
            DockingKind::ScoreOnly
        } else {
            DockingKind::Dock
        },
    };

    state.ui.pending_docking = None;
    match resolve_remote_host(state, &prompt.prefs.target) {
        // A configured remote target: deploy + submit detached, tracked via the
        // job registry and the opt-in refresh. Docking is single-threaded, so no
        // core count rides along.
        Some(host) => {
            let resources = prompt.prefs.job_resources();
            start_remote_engine(
                state,
                host,
                crate::wire::Engine::Docking(request),
                resources,
            )
        }
        None => {
            let job = spawn_docking_job(request);
            state.jobs.set_docking(job);
            if let Some(task_run_id) = state.active_task_run {
                begin_local_job(
                    state,
                    crate::frontend::jobs::LocalJobSlot::Docking,
                    task_run_id,
                );
                state.tasks.mark_status(task_run_id, TaskStatus::Running);
            }
            state.set_message("docking running; press Esc to stop".to_string());
        }
    }
}

fn entry_structure(state: &mut AppState, entry_id: u64) -> Option<Structure> {
    state.ensure_entry_loaded(entry_id);
    state
        .entries
        .entry(entry_id)
        .map(|entry| entry.structure.clone())
        .filter(|structure| !structure.atoms.is_empty())
}

pub(crate) fn cancel_pending_docking_request(state: &mut AppState) {
    bind_active_panel_task(state, TaskPanelKind::DockingPrompt);
    if state.jobs.docking_running() {
        let _ = crate::frontend::jobs::cancel_controlled_job(
            state,
            &crate::frontend::jobs::JobControlId::Local(
                crate::frontend::jobs::LocalJobSlot::Docking,
            ),
        );
    }
    state.ui.pending_docking = None;
    state.set_message("docking canceled".to_string());
    complete_active_task(state, TaskKind::RunDocking, TaskStatus::Failed);
    close_active_task_panel(state);
}

pub(crate) fn poll_docking_job(state: &mut AppState, ctx: &egui::Context) {
    let Some(running) = state.jobs.take_docking() else {
        return;
    };
    let job_id = state
        .jobs
        .local_execution(crate::frontend::jobs::LocalJobSlot::Docking);
    let task_run_id = job_id.and_then(|id| state.tasks.runs.task_run_id_for_job(&id.to_string()));
    let fingerprint_before = state.entries_fingerprint();

    if ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
        running
            .cancel
            .store(true, std::sync::atomic::Ordering::Relaxed);
        // The Vina search is one opaque call; cancel only takes effect once the
        // current search returns (its result is then discarded).
        state.set_message("docking stopping (the current search runs to completion)".to_string());
    }

    let mut finished = false;
    let mut changed = false;
    while let Ok(message) = running.receiver.try_recv() {
        match message {
            DockingWorkerMessage::Progress { stage } => {
                state.set_message(format!("Docking: {stage}; press Esc to stop"));
            }
            DockingWorkerMessage::Finished(outcome) => {
                let outcome = *outcome;
                for line in outcome.summary.lines() {
                    state.output_log.push(line.to_string());
                }
                let poses_path = save_dock_poses(state, &outcome);
                let already =
                    job_id.is_some_and(|id| outcome_already_materialized(state, &id.to_string()));
                if !already {
                    let pose_ids = add_dock_pose_entries(state, &outcome, task_run_id, poses_path);
                    changed = !pose_ids.is_empty();
                    if let Some(job_id) = &job_id {
                        record_dock_materialization(state, &job_id.to_string(), &pose_ids);
                    }
                }
                let best = outcome
                    .poses
                    .first()
                    .map(|pose| pose.affinity)
                    .unwrap_or(0.0);
                state.set_message(format!(
                    "Docking complete: {} pose(s), best {:+.2} kcal/mol",
                    outcome.poses.len(),
                    best
                ));
                complete_local_job(state, job_id, TaskStatus::Completed);
                state
                    .jobs
                    .take_local_execution(crate::frontend::jobs::LocalJobSlot::Docking);
                finished = true;
            }
            DockingWorkerMessage::Failed(error) => {
                state.set_message(format!("docking failed: {error}"));
                complete_local_job(state, job_id, TaskStatus::Failed);
                state
                    .jobs
                    .take_local_execution(crate::frontend::jobs::LocalJobSlot::Docking);
                finished = true;
            }
        }
    }

    if !finished {
        state.jobs.set_docking(running);
        request_next_optimization_poll(ctx);
    } else {
        if changed && state.entries_fingerprint() != fingerprint_before {
            let now = ctx.input(|input| input.time);
            state.request_autosave(now, AUTOSAVE_DEBOUNCE_SECS);
        }
        ctx.request_repaint();
    }
}

/// Apply a retrieved remote docking outcome: log the summary, save the poses
/// beside the run, add a pose entry per pose (only when the job belongs to the
/// current workspace — see [`outcome_belongs_to_current_workspace`]), and mark the
/// task complete. The detached analogue of the local `poll_docking_job` finish
/// path; mirrors `apply_remote_qm_outcome`.
pub(crate) fn apply_remote_docking_outcome(
    state: &mut AppState,
    row: &crate::backend::storage::jobs::RemoteJob,
    outcome: DockingOutcome,
) {
    for line in outcome.summary.lines() {
        state.output_log.push(line.to_string());
    }
    let run_dir = PathBuf::from(&row.local_run_dir);
    let _ = std::fs::create_dir_all(&run_dir);
    let poses_path = if outcome.poses.is_empty() {
        None
    } else {
        let path = run_dir.join(DOCK_POSES_FILE);
        std::fs::write(&path, dock_poses_pdbqt(&outcome))
            .ok()
            .map(|()| path)
    };

    let belongs_here = outcome_belongs_to_current_workspace(state, row);
    let already = outcome_already_materialized(state, &row.job_id);
    let task_id = state.tasks.runs.task_run_id_for_job(&row.job_id);

    if belongs_here && !already {
        let pose_ids = add_dock_pose_entries(state, &outcome, task_id, poses_path);
        record_dock_materialization(state, &row.job_id, &pose_ids);
    }
    if let Some(task_id) = task_id {
        mark_task_status(state, task_id, TaskStatus::Completed);
    }
    let best = outcome
        .poses
        .first()
        .map(|pose| pose.affinity)
        .unwrap_or(0.0);
    state.set_message(format!(
        "Remote docking complete: {} pose(s), best {:+.2} kcal/mol",
        outcome.poses.len(),
        best
    ));
}

/// Format every pose as one multi-`MODEL` PDBQT document — the saved run artifact,
/// shared by the local and remote result paths.
fn dock_poses_pdbqt(outcome: &DockingOutcome) -> String {
    let mut text = String::new();
    for (index, pose) in outcome.poses.iter().enumerate() {
        let _ = writeln!(text, "MODEL {}", index + 1);
        text.push_str(&pose.pdbqt);
        if !pose.pdbqt.ends_with('\n') {
            text.push('\n');
        }
        text.push_str("ENDMDL\n");
    }
    text
}

/// Persist all poses as one multi-`MODEL` PDBQT in the task's run directory, the
/// way the QM run saves its report. Failures are logged but never abort result
/// handling. Returns the written path.
fn save_dock_poses(state: &mut AppState, outcome: &DockingOutcome) -> Option<PathBuf> {
    let run_dir = match ensure_active_task_run_dir(state, TaskKind::RunDocking, None) {
        Ok(run_dir) => run_dir,
        Err(error) => {
            state
                .output_log
                .push(format!("failed to create docking run directory: {error}"));
            return None;
        }
    };
    let path = run_dir.join(DOCK_POSES_FILE);
    match std::fs::write(&path, dock_poses_pdbqt(outcome)) {
        Ok(()) => {
            state
                .output_log
                .push(format!("docking poses saved to {}", path.display()));
            Some(path)
        }
        Err(error) => {
            state
                .output_log
                .push(format!("failed to save docking poses: {error}"));
            None
        }
    }
}

/// Create one entry per pose under a "Docking poses" group, activating the best
/// (first) pose. Returns the created pose entry ids in rank order (best first), so
/// the caller can record them in the materialization ledger; empty when the outcome
/// has no poses.
fn add_dock_pose_entries(
    state: &mut AppState,
    outcome: &DockingOutcome,
    task_run_id: Option<u64>,
    poses_path: Option<PathBuf>,
) -> Vec<u64> {
    if outcome.poses.is_empty() {
        return Vec::new();
    }
    let group_id = state
        .entries
        .create_group("Docking poses")
        .unwrap_or_default();

    let mut entry_ids = Vec::with_capacity(outcome.poses.len());
    for pose in &outcome.poses {
        let structure = pose.structure.clone();
        let name = structure.title.clone();
        let save_path = structure_io::default_structure_save_path(&structure, None);
        let entry_id = state.entries.add_entry_to_group(
            structure,
            None,
            save_path,
            group_id.clone(),
            Some(name),
            false,
        );
        set_dock_run_origin(state, entry_id, poses_path.clone());
        entry_ids.push(entry_id);
    }

    if let Some(&best_id) = entry_ids.first() {
        if let Some(task_run_id) = task_run_id {
            record_task_result_entry(state, task_run_id, best_id);
        }
        show_existing_entry(state, best_id);
    }
    entry_ids
}

/// Record a docking outcome's poses in the materialization ledger under `job_id`,
/// so a refresh/compensation never re-creates the poses. An empty pose list records
/// a report (proof the outcome was applied).
fn record_dock_materialization(state: &mut AppState, job_id: &str, pose_ids: &[u64]) {
    if pose_ids.is_empty() {
        record_materialization(state, job_id, "report", None, &[]);
    } else {
        record_materialization(state, job_id, "pose", pose_ids.first().copied(), pose_ids);
    }
}

/// Activate an already-added entry, preserving the active task run so the caller
/// can still record the result entry and complete the run (mirrors the task-run
/// preservation in [`add_and_show_entry`]).
fn show_existing_entry(state: &mut AppState, entry_id: u64) {
    let active_task_run = state.active_task_run;
    state.save_viewport_for_active_entry();
    state.entries.activate_entry(entry_id);
    state.ui.entry_list.selected_entry_ids.clear();
    state.ui.entry_list.selected_entry_ids.insert(entry_id);
    load_active_entry(state);
    state.active_task_run = active_task_run;
}

/// Mark an entry as a docking pose. Like [`set_qm_run_origin`], the poses path is
/// stored relative to the project root so it survives the project being moved.
fn set_dock_run_origin(state: &mut AppState, entry_id: u64, poses: Option<PathBuf>) {
    let project_root = state
        .workspace
        .project()
        .map(|project| project.root.clone());
    let poses = poses.map(|path| match project_root.as_deref() {
        Some(root) => path
            .strip_prefix(root)
            .map(Path::to_path_buf)
            .unwrap_or(path),
        None => path,
    });
    state
        .entries
        .set_entry_origin(entry_id, EntryOrigin::DockRun { poses });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::storage::jobs as registry;
    use crate::engines::docking::DockedPose;

    fn pose(title: &str, affinity: f64) -> DockedPose {
        DockedPose {
            affinity,
            intermolecular: 0.0,
            internal: 0.0,
            torsional: 0.0,
            structure: crate::domain::Structure::new(title, Vec::new()),
            pdbqt: "REMARK pose\n".to_string(),
        }
    }

    fn dock_outcome(count: usize) -> DockingOutcome {
        DockingOutcome {
            poses: (0..count)
                .map(|rank| pose(&format!("pose-{rank}"), -8.0 + rank as f64))
                .collect(),
            notes: Vec::new(),
            summary: "docking".to_string(),
        }
    }

    fn remote_row(job_id: &str, run_dir: &std::path::Path) -> registry::RemoteJob {
        registry::RemoteJob {
            job_id: job_id.to_string(),
            host_id: "h".to_string(),
            host_label: "H".to_string(),
            remote_dir: "~/.silicolab/runs/x".to_string(),
            scheduler: "direct".to_string(),
            launch_handle: "1".to_string(),
            cluster: None,
            engine_id: "vina".to_string(),
            job_kind: "dock-ligand".to_string(),
            project_id: None,
            project_root_hint: None,
            local_run_dir: run_dir.to_string_lossy().to_string(),
            status: registry::RemoteJobStatus::Done,
            submitted_at_ms: 0,
            last_polled_at_ms: None,
            exit_code: None,
            scheduler_state: None,
            reason: None,
            console_offset: 0,
            unknown_since_ms: None,
        }
    }

    #[test]
    fn remote_docking_outcome_is_idempotent_across_repeated_apply() {
        // import cardinality (N): docking's N poses become N entries once, and
        // re-applying the same terminal outcome (a refresh/cancel race, or an
        // open-project compensation) never creates a second set.
        let run_dir = std::path::PathBuf::from("target/test-dock-idempotency");
        let _ = std::fs::remove_dir_all(&run_dir);
        std::fs::create_dir_all(&run_dir).unwrap();
        let mut state = AppState::scratch(Default::default(), Vec::new());
        let row = remote_row("job-dock", &run_dir);

        apply_remote_docking_outcome(&mut state, &row, dock_outcome(3));
        let after_first = state.entries.records.len();
        assert_eq!(after_first, 3, "three poses become three entries");
        let record = state
            .materializations
            .get("job-dock")
            .expect("the poses are recorded in the ledger");
        assert_eq!(record.entries.len(), 3);
        assert_eq!(
            record.primary_entry_id,
            record.entries.first().map(|entry| entry.entry_id),
            "the best pose is the primary result"
        );

        apply_remote_docking_outcome(&mut state, &row, dock_outcome(3));
        assert_eq!(
            state.entries.records.len(),
            after_first,
            "re-applying the outcome creates no duplicate poses"
        );
        assert_eq!(state.materializations.len(), 1);
    }
}
