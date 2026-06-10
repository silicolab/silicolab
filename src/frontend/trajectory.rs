//! In-app playback of MD trajectories stored alongside a run in the task
//! directory. The trajectory file is decoded off the UI thread into a
//! [`Trajectory`]; the resulting [`TrajectoryPlayback`] holds the playback
//! cursor and a scratch [`Structure`] whose positions are swapped per frame.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};

use nalgebra::Point3;

use crate::domain::{Structure, Trajectory};
use crate::io::trajectory::read_xtc;
use crate::workflows::molecular_dynamics::{STAGE_EM, STAGE_NPT, STAGE_NVT, STAGE_PROD};

/// Default playback rate (frames per second).
pub const DEFAULT_PLAYBACK_FPS: f32 = 15.0;

/// One playable per-stage trajectory of an MD run, discovered next to the
/// entry's recorded trajectory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MdStage {
    /// Display label for the stage (e.g. "EM", "NVT", "NPT", "MD", or the file
    /// stem for a custom step).
    pub label: String,
    /// The stage trajectory's path, in the same form as the entry's stored
    /// trajectory (project-root-relative, or absolute for an external run dir).
    pub path: PathBuf,
}

/// Canonical run order, so stage chips read EM → NVT → NPT → MD regardless of
/// how the directory listing happens to sort.
fn stage_order(stem: &str) -> (u8, String) {
    let rank = match stem {
        STAGE_EM => 0,
        STAGE_NVT => 1,
        STAGE_NPT => 2,
        STAGE_PROD => 3,
        _ => 4,
    };
    (rank, stem.to_string())
}

/// A friendly label for a stage file stem; known stages get an uppercased name,
/// custom steps keep their stem.
fn stage_label(stem: &str) -> String {
    match stem {
        STAGE_EM | STAGE_NVT | STAGE_NPT => stem.to_ascii_uppercase(),
        STAGE_PROD => "MD".to_string(),
        other => other.to_string(),
    }
}

/// Enumerate the per-stage trajectories that live next to `primary` (the entry's
/// recorded trajectory), ordered by the canonical run sequence. Paths are
/// returned in the same form as `primary` (relative paths stay relative, so they
/// round-trip through the entry origin); `project_root` only resolves where to
/// scan on disk. Returns an empty list if the directory cannot be read.
pub fn md_stage_trajectories(primary: &Path, project_root: &Path) -> Vec<MdStage> {
    let Some(run_dir_rel) = primary.parent() else {
        return Vec::new();
    };
    let scan_dir = if run_dir_rel.is_absolute() {
        run_dir_rel.to_path_buf()
    } else {
        project_root.join(run_dir_rel)
    };
    let Ok(entries) = std::fs::read_dir(&scan_dir) else {
        return Vec::new();
    };

    let mut stages: Vec<(u8, String, MdStage)> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("xtc") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        let Some(file_name) = path.file_name() else {
            continue;
        };
        let (rank, tiebreak) = stage_order(stem);
        stages.push((
            rank,
            tiebreak,
            MdStage {
                label: stage_label(stem),
                path: run_dir_rel.join(file_name),
            },
        ));
    }
    stages.sort_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)));
    stages.into_iter().map(|(_, _, stage)| stage).collect()
}

/// A decoded trajectory bound to an entry, plus the UI playback state. Rendering
/// only happens while the bound entry is active (see the workspace renderer).
pub struct TrajectoryPlayback {
    /// Entry this trajectory belongs to.
    pub entry_id: u64,
    /// Which stage trajectory is loaded (the path it was decoded from, in the
    /// entry's stored form), so the UI can highlight the active stage.
    pub source: PathBuf,
    pub trajectory: Trajectory,
    /// The entry's topology with the current frame's coordinates applied; this
    /// is what the viewport renders during playback.
    pub scratch: Structure,
    pub current_frame: usize,
    pub playing: bool,
    pub fps: f32,
    /// egui time (seconds) at which `current_frame` was last advanced.
    pub last_advance_secs: f64,
    /// Camera framing computed once from the base structure and held fixed, so
    /// the view does not drift/zoom as the system diffuses between frames.
    pub view_center: Point3<f32>,
    pub view_radius: f32,
}

impl TrajectoryPlayback {
    pub fn frame_count(&self) -> usize {
        self.trajectory.frame_count()
    }

    /// Apply `current_frame`'s coordinates to the scratch structure.
    pub fn sync_scratch(&mut self) {
        self.trajectory
            .apply_frame(self.current_frame, &mut self.scratch.atoms);
    }

    /// Jump to `frame` (clamped) and refresh the scratch structure.
    pub fn set_frame(&mut self, frame: usize) {
        let last = self.frame_count().saturating_sub(1);
        self.current_frame = frame.min(last);
        self.sync_scratch();
    }

    /// Advance one frame (wrapping) and refresh the scratch structure.
    pub fn advance_frame(&mut self) {
        let count = self.frame_count();
        if count == 0 {
            return;
        }
        self.current_frame = (self.current_frame + 1) % count;
        self.sync_scratch();
    }
}

/// An in-flight background decode of an entry's trajectory file.
pub struct RunningTrajectoryLoad {
    pub entry_id: u64,
    /// The stage trajectory being decoded (in the entry's stored form), carried
    /// through to the resulting playback so the UI can track the active stage.
    pub source: PathBuf,
    /// Delivers the decoded trajectory, or an error message, once decoding ends.
    pub receiver: Receiver<Result<Trajectory, String>>,
    /// The entry's base structure (topology), captured at spawn time; used to
    /// build the playback scratch and fixed view once the trajectory arrives.
    pub base_structure: Structure,
    /// Whether the unit cell is shown, for the fixed-view computation.
    pub include_cell: bool,
}

/// Spawn a background thread that decodes `path` into a [`Trajectory`]. The
/// caller stores the returned handle and polls its `receiver` on the UI thread.
pub fn spawn_trajectory_load(
    entry_id: u64,
    source: PathBuf,
    path: PathBuf,
    base_structure: Structure,
    include_cell: bool,
) -> RunningTrajectoryLoad {
    let (sender, receiver) = channel();
    std::thread::spawn(move || {
        let result = read_xtc(&path).map_err(|error| format!("{error:#}"));
        let _ = sender.send(result);
    });
    RunningTrajectoryLoad {
        entry_id,
        source,
        receiver,
        base_structure,
        include_cell,
    }
}

#[cfg(test)]
mod tests {
    use super::md_stage_trajectories;
    use std::path::PathBuf;

    #[test]
    fn stage_trajectories_are_listed_in_run_order() {
        let root = std::env::temp_dir().join("silicolab_md_stage_order");
        let run = root.join("runs").join("run-1");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&run).unwrap();
        // Created in scrambled order; the `.log` file must be ignored.
        for name in ["md.xtc", "em.xtc", "npt.xtc", "nvt.xtc", "md.log"] {
            std::fs::write(run.join(name), b"x").unwrap();
        }

        let primary = PathBuf::from("runs").join("run-1").join("md.xtc");
        let stages = md_stage_trajectories(&primary, &root);

        let labels: Vec<&str> = stages.iter().map(|stage| stage.label.as_str()).collect();
        assert_eq!(labels, vec!["EM", "NVT", "NPT", "MD"]);
        // Paths keep `primary`'s relative form so they round-trip via the entry.
        assert_eq!(stages[3].path, primary);
        assert!(stages.iter().all(|stage| stage.path.is_relative()));

        let _ = std::fs::remove_dir_all(&root);
    }
}
