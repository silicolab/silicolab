use super::*;
use crate::frontend::state::ChartState;

#[test]
fn reset_chart_caches_clears_chart_and_both_memos() {
    let mut state = AppState::scratch(Default::default(), Vec::new());
    state.ui.chart = Some(ChartState::new("project-a run".to_string()));
    state.ui.chart_availability.insert(1, true);
    state.ui.chart_availability.insert(2, false);
    state.ui.task_chart_thumbnails.insert(1, None);

    reset_chart_caches(&mut state);

    assert!(state.ui.chart.is_none());
    assert!(state.ui.chart_availability.is_empty());
    assert!(state.ui.task_chart_thumbnails.is_empty());
}

/// A completed task's `result_entry_id` reaches disk only through the atomic
/// entries+ledger save; the every-frame narrow flush must not write that row
/// ahead of it, or a crash in the gap leaves the task pointing at an entry
/// that was never persisted. The flush therefore defers while a full save is
/// pending — signalled either by a scheduled autosave (an unsaved entry) or a
/// dirty ledger (an unsaved materialization) — and only then persists.
#[test]
fn narrow_task_flush_defers_to_a_pending_full_save() {
    let root = std::env::temp_dir().join(format!("sl-flush-defer-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let session = crate::backend::project::create_project(&root, "flush").unwrap();
    let mut state = AppState::scratch(Default::default(), Vec::new());
    state.workspace = crate::backend::project::WorkspaceSession::Project(session);

    let controller = *crate::backend::tasks::task_controller_by_id("qm-optimize").unwrap();
    let task = state.tasks.create_task_run(controller);
    state.tasks.set_result_entry_id(task, Some(42));
    state.tasks.mark_status(task, TaskStatus::Completed);
    assert!(state.tasks.has_dirty_task_runs());

    // A scheduled autosave (an unsaved entry) defers the flush.
    state.request_autosave(0.0, 0.5);
    flush_dirty_task_runs(&mut state);
    assert!(
        state.tasks.has_dirty_task_runs(),
        "must defer while an autosave is pending",
    );

    // So does a dirty ledger (an unsaved materialization), even with no autosave.
    state.clear_autosave_deadline();
    state
        .materializations
        .record(crate::backend::materialization::Materialization::single(
            "job-1".to_string(),
            0,
            42,
            "result",
        ));
    flush_dirty_task_runs(&mut state);
    assert!(
        state.tasks.has_dirty_task_runs(),
        "must defer while the ledger is dirty",
    );

    // Once nothing is pending, the entry is already durable, so the flush writes.
    state.materializations.mark_saved();
    flush_dirty_task_runs(&mut state);
    assert!(
        !state.tasks.has_dirty_task_runs(),
        "must persist once no full save is pending",
    );

    std::fs::remove_dir_all(&root).ok();
}

/// A scratch workspace has no database to persist the ledger into, so
/// `persist_project` must resolve its dirty flag on the early-return path.
/// Left set, the flag would re-arm the autosave every frame and pin
/// `full_save_pending` for the whole session, permanently deferring the
/// narrow flushes.
#[test]
fn scratch_persist_resolves_a_dirty_ledger() {
    let mut state = AppState::scratch(Default::default(), Vec::new());
    state
        .materializations
        .record(crate::backend::materialization::Materialization::report(
            "job-1".to_string(),
            0,
        ));
    assert!(state.materializations.is_dirty());
    assert!(full_save_pending(&state));

    persist_project(&mut state, false).unwrap();

    assert!(
        !state.materializations.is_dirty(),
        "scratch persist must resolve the ledger's dirty flag",
    );
    assert!(!full_save_pending(&state));
}
