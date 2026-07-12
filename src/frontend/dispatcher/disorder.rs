//! Dispatcher logic for the Build Disordered System task: launch the packing
//! engine in the background, stream its progress into the result entry, and
//! finalize on completion. Mirrors the MD-run launch/poll wiring
//! (`simulation.rs` + `jobs.rs`), but the engine is the pure-Rust
//! [`crate::workflows::packing`] packer instead of an external engine.

use nalgebra::{Point3, Vector3};

use crate::domain::{Structure, UnitCell};
use crate::frontend::jobs::{
    DisorderWorkerMessage, LocalJobSlot, RunningDisorderJob, spawn_disorder_job,
};
use crate::frontend::state::{DisorderAmount, DisorderRegionKind, DisorderedSystemPrompt};
use crate::job::CancelSignal;
use crate::workflows::molecular_dynamics::solvation::splitmix64;
use crate::workflows::packing::{
    PackLimits, PackRequest, PackSpecies, Region, RegionSense, count_for_concentration_molar,
    count_for_density_g_per_cm3,
};

use super::*;

/// Run the packing engine from the current draft, creating the result entry up
/// front and streaming the in-progress structure into it.
pub(crate) fn start_pending_disorder(state: &mut AppState) {
    bind_active_panel_task(state, TaskPanelKind::DisorderedSystemPrompt);
    let Some(prompt) = state.ui.pending_disorder.clone() else {
        return;
    };
    if state.jobs.disorder_running() {
        state.status_neutral("a packing job is already running");
        return;
    }
    if prompt.components.is_empty() {
        state.status_neutral("Add at least one molecule to pack");
        return;
    }
    let request = match build_pack_request(state, &prompt) {
        Ok(request) => request,
        Err(error) => {
            state.status_neutral(error.to_string());
            return;
        }
    };

    let output_name = {
        let trimmed = prompt.output_name.trim();
        if trimmed.is_empty() {
            "Disordered system".to_string()
        } else {
            trimmed.to_string()
        }
    };

    // Create the result entry now and stream the packing into it, so the user
    // watches it fill in without the source molecule being touched.
    let mut placeholder = request.fixed.clone().unwrap_or_else(Structure::empty);
    placeholder.title = output_name;
    let save_path = crate::io::structure_io::default_structure_save_path(&placeholder, None);
    let entry_id = add_and_show_entry(state, placeholder, None, save_path);
    if let Some(task_run_id) = state.active_task_run {
        record_task_result_entry(state, task_run_id, entry_id);
    }

    state.ui.pending_disorder = None;
    let mut job = spawn_disorder_job(request);
    job.result_entry_id = entry_id;
    state.jobs.set_disorder(job);
    if let Some(task_run_id) = state.active_task_run {
        begin_local_job(
            state,
            crate::frontend::jobs::LocalJobSlot::Disorder,
            task_run_id,
        );
        mark_task_status(state, task_run_id, TaskStatus::Running);
    }
    state.status_neutral("Packing disordered system; press Esc to stop");
}

pub(crate) fn cancel_pending_disorder_request(state: &mut AppState) {
    bind_active_panel_task(state, TaskPanelKind::DisorderedSystemPrompt);
    if state.jobs.disorder_running() {
        let _ = crate::frontend::jobs::cancel_controlled_job(
            state,
            &crate::frontend::jobs::JobControlId::Local(
                crate::frontend::jobs::LocalJobSlot::Disorder,
            ),
        );
    }
    state.ui.pending_disorder = None;
    state.status_neutral("Packing canceled");
    complete_active_task(state, TaskKind::BuildDisorderedSystem, TaskStatus::Failed);
    close_active_task_panel(state);
}

/// Apply an edit to the disorder draft, if one is present (mirrors
/// [`with_md_run_prompt`]).
pub(crate) fn with_disorder_prompt(
    state: &mut AppState,
    edit: impl FnOnce(&mut DisorderedSystemPrompt),
) {
    if let Some(prompt) = state.ui.pending_disorder.as_mut() {
        edit(prompt);
    }
}

/// Add a molecule row, seeded with the given entry (or the active entry).
pub(crate) fn add_disorder_component(state: &mut AppState, entry: Option<u64>) {
    let seed = entry.or_else(|| state.entries.active_entry_id());
    with_disorder_prompt(state, |prompt| {
        let mut draft = crate::frontend::state::DisorderComponentDraft::default();
        if let Some(entry_id) = seed {
            draft.entry_id = entry_id;
        }
        prompt.components.push(draft);
    });
}

/// Pick a fresh RNG seed for the packing (UI convenience; the packing itself is
/// deterministic for whatever seed is chosen).
pub(crate) fn randomize_disorder_seed(state: &mut AppState) {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(1);
    with_disorder_prompt(state, |prompt| {
        prompt.seed = splitmix64(nonce.wrapping_add(prompt.seed)).max(1);
    });
}

/// Build the engine request from the draft, resolving each molecule entry and
/// converting density/concentration targets into copy counts.
fn build_pack_request(
    state: &mut AppState,
    prompt: &DisorderedSystemPrompt,
) -> anyhow::Result<PackRequest> {
    let region = build_region(prompt);
    // A box has no exterior to fill, so a stale "outside" flag (left over from a
    // sphere/cylinder) is treated as inside rather than failing the build.
    let sense = if prompt.sense_outside && prompt.region_kind != DisorderRegionKind::Box {
        RegionSense::Outside
    } else {
        RegionSense::Inside
    };

    let mut species = Vec::with_capacity(prompt.components.len());
    for component in &prompt.components {
        state.ensure_entry_loaded(component.entry_id);
        let molecule = state
            .entries
            .entry(component.entry_id)
            .map(|entry| entry.structure.clone())
            .ok_or_else(|| anyhow!("a selected molecule is no longer in the workspace"))?;
        if molecule.atoms.is_empty() {
            bail!("a selected molecule has no atoms to pack");
        }
        let count = match prompt.amount_mode {
            DisorderAmount::Count => component.count as usize,
            DisorderAmount::DensityGCm3 => {
                count_for_density_g_per_cm3(&molecule, component.amount_value, &region)
            }
            DisorderAmount::ConcentrationMolar => {
                count_for_concentration_molar(&molecule, component.amount_value, &region)
            }
        };
        species.push(PackSpecies { molecule, count });
    }
    if species.iter().all(|s| s.count == 0) {
        bail!("nothing to pack: every molecule resolves to zero copies");
    }

    let fixed = prompt.obstacle_entry_id.and_then(|entry_id| {
        state.ensure_entry_loaded(entry_id);
        state
            .entries
            .entry(entry_id)
            .map(|entry| entry.structure.clone())
    });

    let is_box = prompt.region_kind == DisorderRegionKind::Box;
    let output_cell = (prompt.set_cell_from_region && is_box).then(|| {
        UnitCell::from_parameters(
            prompt.box_lengths[0],
            prompt.box_lengths[1],
            prompt.box_lengths[2],
            90.0,
            90.0,
            90.0,
        )
    });
    let periodic = prompt.periodic && is_box;

    Ok(PackRequest {
        species,
        region,
        sense,
        tolerance: prompt.tolerance_angstrom,
        periodic,
        seed: prompt.seed,
        fixed,
        output_cell,
        limits: PackLimits {
            max_restarts: prompt.max_restarts as usize,
            max_steps: prompt.max_steps as usize,
            ..PackLimits::default()
        },
    })
}

/// Build the geometric region, placed in the positive octant so the result sits
/// at the origin like a built MD box.
fn build_region(prompt: &DisorderedSystemPrompt) -> Region {
    match prompt.region_kind {
        DisorderRegionKind::Box => {
            let [x, y, z] = prompt.box_lengths;
            Region::Box {
                min: Point3::origin(),
                max: Point3::new(x, y, z),
            }
        }
        DisorderRegionKind::Sphere => {
            let r = prompt.sphere_radius;
            Region::Sphere {
                center: Point3::new(r, r, r),
                radius: r,
            }
        }
        DisorderRegionKind::Cylinder => {
            let r = prompt.cyl_radius;
            let length = prompt.cyl_length;
            Region::Cylinder {
                center: Point3::new(r, r, length * 0.5),
                axis: Vector3::new(0.0, 0.0, 1.0),
                radius: r,
                length,
            }
        }
    }
}

/// Drain the background packing job: stream intermediate structures into the
/// result entry, and on completion create the entry result + mark the task done
/// (mirrors `poll_optimization_job` streaming with `poll_engine_job`'s
/// create-entry completion).
pub(crate) fn poll_disorder_job(state: &mut AppState, ctx: &egui::Context) {
    let Some(running) = state.jobs.take_disorder() else {
        return;
    };
    if let Some(running) = drive(state, ctx, running) {
        state.jobs.set_disorder(running);
    }
}

impl JobRuntime for RunningDisorderJob {
    fn slot(&self) -> LocalJobSlot {
        LocalJobSlot::Disorder
    }

    fn request_cancel(&mut self, state: &mut AppState) -> CancelSignal {
        self.cancel
            .store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some(job_id) = state.jobs.local_execution(self.slot()) {
            state.job_notice(job_id, "Packing stopping; finishing the current pass");
        }
        CancelSignal::Accepted
    }

    fn poll(&mut self, state: &mut AppState, cx: &JobContext) -> JobPoll {
        // Disorder streams into the entry created up front (`result_entry_id`),
        // overwriting it in place; it adds no new entry and records no ledger row.
        let entry_id = self.result_entry_id;
        loop {
            match self.receiver.try_recv() {
                Ok(DisorderWorkerMessage::Progress { structure, report }) => {
                    // Placement counts are structured progress Activity reads, not a
                    // log firehose; the Esc hint is posted once, on the first update.
                    let first = self.latest_report.is_none();
                    write_disorder_structure(state, entry_id, structure);
                    self.latest_report = Some(report);
                    if first {
                        state.status_neutral("Packing disordered system… press Esc to stop");
                    }
                }
                Ok(DisorderWorkerMessage::Finished { structure, report }) => {
                    write_disorder_structure(state, entry_id, structure);
                    // The packed result may carry a simulation cell the placeholder
                    // lacked; refresh the export path so Save serializes to a
                    // cell-aware format (CIF) instead of the placeholder's XYZ.
                    if let Some(entry) = state.entries.entry_mut(entry_id) {
                        entry.save_path = crate::io::structure_io::default_structure_save_path(
                            &entry.structure,
                            None,
                        );
                    }
                    let message = disorder_finished_message(&report);
                    match cx.job_id {
                        Some(job_id) => state.job_succeeded(job_id, message),
                        None => state.status_success(message),
                    }
                    self.latest_report = Some(report);
                    return JobPoll::Terminal(TaskStatus::Completed);
                }
                Ok(DisorderWorkerMessage::Failed(error)) => {
                    let message = format!("Packing failed: {error}");
                    match cx.job_id {
                        Some(job_id) => state.job_failed(job_id, message),
                        None => state.status_error(message),
                    }
                    return JobPoll::Terminal(TaskStatus::Failed);
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => return JobPoll::Running,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => return JobPoll::ChannelLost,
            }
        }
    }
}

/// Overwrite the result entry's structure with a streamed/final packing, marking
/// the viewport dirty when it is the active entry. Bumps the entry revision
/// unconditionally so the entries fingerprint changes — and the result is
/// autosaved — even when the user has switched away from the result entry.
fn write_disorder_structure(state: &mut AppState, entry_id: u64, structure: Structure) {
    if let Some(entry) = state.entries.entry_mut(entry_id) {
        entry.structure = structure;
        entry.loaded = true;
        entry.revision = entry.revision.wrapping_add(1);
    }
    if state.entries.active_entry_id() == Some(entry_id) {
        state.mark_structure_changed();
    }
}

fn disorder_finished_message(report: &crate::workflows::packing::PackReport) -> String {
    let placed = report.total_placed();
    let requested = report.total_requested();
    if report.converged {
        format!("Packed {placed} molecules into a disordered system")
    } else if report.timed_out {
        format!(
            "Packing timed out: placed {placed}/{requested} (worst overlap {:.2} Å) — \
             enlarge the region or lower the density",
            report.max_overlap
        )
    } else {
        format!(
            "Packed {placed}/{requested}; {} still overlap (worst {:.2} Å) — \
             enlarge the region or lower the density",
            requested.saturating_sub(placed),
            report.max_overlap
        )
    }
}
