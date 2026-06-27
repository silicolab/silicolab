use super::*;

mod compute;
mod poll;
#[cfg(test)]
mod tests;

pub(crate) use compute::*;
pub(crate) use poll::*;

/// Begin (or resume) playback of one of an entry's MD-run trajectories. The
/// trajectory files live in the run directory and are decoded in the background;
/// this only kicks off the load (or resumes if that exact stage is already
/// loaded). `requested` selects a specific stage's trajectory (in the entry's
/// stored form); `None` plays the entry's default (production) trajectory.
pub(crate) fn load_trajectory(
    state: &mut AppState,
    entry_id: u64,
    requested: Option<PathBuf>,
    ctx: &egui::Context,
) {
    // Resolve which stage trajectory to play: the explicit request, else the
    // entry's recorded default.
    state.ensure_entry_loaded(entry_id);
    let Some(entry) = state.entries.entry(entry_id) else {
        return;
    };
    let relative =
        match requested.or_else(|| entry.origin.trajectory().map(|path| path.to_path_buf())) {
            Some(relative) => relative,
            None => {
                state.set_message("This entry has no trajectory to play");
                return;
            }
        };
    let base_structure = entry.structure.clone();

    // Already playing exactly this stage: just ensure it is running.
    if state
        .ui
        .trajectory
        .as_ref()
        .is_some_and(|p| p.entry_id == entry_id && p.source == relative)
    {
        if let Some(playback) = state.ui.trajectory.as_mut() {
            playback.playing = true;
            playback.last_advance_secs = ctx.input(|input| input.time);
        }
        ctx.request_repaint();
        return;
    }
    // Already decoding exactly this stage.
    if state
        .jobs
        .trajectory_load
        .as_ref()
        .is_some_and(|l| l.entry_id == entry_id && l.source == relative)
    {
        return;
    }

    let Some(project) = state.workspace.project() else {
        state.set_message("Trajectory playback requires an open project");
        return;
    };
    let absolute = project.root.join(&relative);
    if !absolute.exists() {
        state.set_message(format!(
            "Trajectory file is missing: {}",
            absolute.display()
        ));
        return;
    }

    let include_cell = state.ui.viewport.show_cell;
    // Drop any stale playback bound to a different entry or stage.
    state.ui.trajectory = None;
    state.jobs.trajectory_load = Some(spawn_trajectory_load(
        entry_id,
        relative,
        absolute,
        base_structure,
        include_cell,
    ));
    state.set_message("Loading trajectory…");
    ctx.request_repaint_after(engine_poll_frame());
}

pub(crate) fn toggle_trajectory_play(state: &mut AppState, ctx: &egui::Context) {
    if let Some(playback) = state.ui.trajectory.as_mut() {
        playback.playing = !playback.playing;
        playback.last_advance_secs = ctx.input(|input| input.time);
        ctx.request_repaint();
    }
}

pub(crate) fn set_trajectory_frame(state: &mut AppState, frame: usize) {
    if let Some(playback) = state.ui.trajectory.as_mut() {
        playback.set_frame(frame);
        // Scrubbing pauses playback so the chosen frame stays put.
        playback.playing = false;
    }
}

pub(crate) fn stop_trajectory(state: &mut AppState) {
    state.ui.trajectory = None;
    state.jobs.trajectory_load = None;
}

/// The provenance for an MD-run output entry: an [`EntryOrigin::MdRun`] carrying
/// the run's trajectory (when it wrote one) stored relative to the project root
/// so it survives the project being moved — absolute when the run directory
/// lives outside the project, and `None` when the run produced no trajectory.
///
/// The `MdRun` origin (not the trajectory) is what drives the "MD" badge, so a
/// run is marked even when it wrote no playable trajectory (e.g. a relax-only
/// run); playback is offered separately, only when `trajectory` is present.
pub(crate) fn md_run_origin(
    trajectory: Option<PathBuf>,
    project_root: Option<&Path>,
) -> EntryOrigin {
    let trajectory = trajectory.map(|path| match project_root {
        Some(root) => path
            .strip_prefix(root)
            .map(Path::to_path_buf)
            .unwrap_or(path),
        None => path,
    });
    EntryOrigin::MdRun { trajectory }
}

/// Mark an entry as the output of an MD run (provenance badge + playback gating).
pub(crate) fn set_md_run_origin(state: &mut AppState, entry_id: u64, trajectory: Option<PathBuf>) {
    let project_root = state
        .workspace
        .project()
        .map(|project| project.root.clone());
    let origin = md_run_origin(trajectory, project_root.as_deref());
    state.entries.set_entry_origin(entry_id, origin);
}

/// File name of the saved QM output report inside a QM task's run directory.
pub(crate) const QM_OUTPUT_FILE: &str = "output.txt";

/// Mark an entry as the output of a QM run. Like [`set_md_run_origin`], the
/// report path is stored relative to the project root so it survives the
/// project being moved; the badge tracks the origin, not the path, so the
/// entry is marked even when saving the report failed.
pub(crate) fn set_qm_run_origin(state: &mut AppState, entry_id: u64, output: Option<PathBuf>) {
    let project_root = state
        .workspace
        .project()
        .map(|project| project.root.clone());
    let output = output.map(|path| match project_root.as_deref() {
        Some(root) => path
            .strip_prefix(root)
            .map(Path::to_path_buf)
            .unwrap_or(path),
        None => path,
    });
    state
        .entries
        .set_entry_origin(entry_id, EntryOrigin::QmRun { output });
}

/// Open the saved QM output report of `entry_id` in the shared text viewer.
/// The report is read from disk on every open (it is small), so the viewer
/// never holds a stale copy and nothing extra is persisted in the project
/// database.
pub(crate) fn show_qm_output(state: &mut AppState, entry_id: u64) {
    let Some(entry) = state.entries.entry(entry_id) else {
        return;
    };
    let entry_name = entry.name.clone();
    let Some(relative) = entry.origin.qm_output().map(Path::to_path_buf) else {
        state.set_message("This entry has no saved QM output".to_string());
        return;
    };
    // Stored relative to the project root (absolute when the run directory
    // lives outside a project); `join` keeps an already-absolute path as-is.
    let absolute = match state.workspace.project() {
        Some(project) => project.root.join(&relative),
        None => relative,
    };
    match std::fs::read_to_string(&absolute) {
        Ok(text) => {
            state.ui.text_viewer = Some(crate::frontend::state::TextViewer {
                title: format!("QM Output — {entry_name}"),
                text,
            });
        }
        Err(error) => state.set_message(format!(
            "Could not read QM output {}: {error}",
            absolute.display()
        )),
    }
}
