use super::*;
use std::path::PathBuf;

use crate::{
    backend::project::{ProjectSession, WorkspaceSession},
    domain::Structure,
};

#[test]
fn remote_gpu_live_apply_tracks_latest_and_history() {
    use crate::engines::remote::hardware::RemoteGpuStat;
    let mut live = RemoteGpuLive::default();
    live.apply(vec![RemoteGpuStat {
        index: 0,
        name: "GPU A".into(),
        util_pct: Some(10.0),
        vram_used_mib: Some(100),
        vram_total_mib: Some(8192),
        temp_c: Some(40),
        power_w: Some(15.0),
    }]);
    live.apply(vec![RemoteGpuStat {
        index: 0,
        name: "GPU A".into(),
        util_pct: Some(80.0),
        vram_used_mib: Some(200),
        vram_total_mib: Some(8192),
        temp_c: Some(55),
        power_w: Some(120.0),
    }]);
    assert_eq!(live.gpus.len(), 1);
    assert_eq!(live.gpus[0].latest.util_pct, Some(80.0));
    assert_eq!(live.gpus[0].util_history.len(), 2);
    assert_eq!(live.gpus[0].util_history.back().copied(), Some(Some(80.0)));
    assert!(live.last_error.is_none());
}

#[test]
fn scratch_content_requires_leave_confirmation() {
    let mut state = AppState::scratch(Default::default(), Vec::new());
    assert!(!state.needs_leave_confirmation());

    state
        .entries
        .add_entry(Structure::empty(), None, PathBuf::from("scratch.cif"));

    assert!(state.scratch_has_unsaved_content());
    assert!(state.needs_leave_confirmation());
}

#[test]
fn project_saved_fingerprint_tracks_leave_confirmation() {
    let project =
        ProjectSession::from_root(PathBuf::from("target/test-leave-project"), "test".into());
    let mut state = AppState::new(
        Structure::empty(),
        None,
        WorkspaceSession::Project(project),
        Default::default(),
        Vec::new(),
        None,
    );

    assert!(!state.has_project_changes_to_save());
    assert!(!state.needs_leave_confirmation());

    state
        .entries
        .add_entry(Structure::empty(), None, PathBuf::from("entry.cif"));
    assert!(state.has_project_changes_to_save());
    assert!(state.needs_leave_confirmation());

    state.mark_project_saved();
    assert!(!state.has_project_changes_to_save());
    assert!(!state.needs_leave_confirmation());

    state.mark_project_save_failed("disk full");
    assert!(state.has_project_changes_to_save());
    assert!(state.needs_leave_confirmation());
}
