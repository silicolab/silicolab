use super::*;
use crate::frontend::state::{
    DetailTarget, DockArea, DockTab, LogFilter, LogQuery, OutputTarget, StaticView,
};

/// Mark the workbench layout dirty using the current egui clock; the debounced
/// flush (see [`flush_pending_layout_save`]) coalesces a burst of changes into a
/// single settings write once the user pauses.
fn mark_dirty(state: &mut AppState, ctx: &egui::Context) {
    let now = ctx.input(|input| input.time);
    state.mark_layout_dirty(now);
}

/// Move a dock tab to `to` at `index` (drag-and-drop; `None` appends). For a task
/// tab the move also focuses it (TaskManager + form state) so its panel renders
/// immediately in its new home.
pub(crate) fn move_dock_tab(
    state: &mut AppState,
    tab: DockTab,
    to: DockArea,
    index: Option<usize>,
    ctx: &egui::Context,
) {
    state.ui.layout.dock.move_tab(tab, to, index);
    if let DockTab::Task(task_run_id) = tab {
        state.tasks.activate_panel(task_run_id);
        ensure_panel_form(state, task_run_id);
    }
    mark_dirty(state, ctx);
}

/// Toggle a dock area's visibility (View / native menu). Revealing an empty area
/// restores a default view (console for the bottom panel, assistant for the right
/// sidebar) so the menu item is never a dead no-op.
pub(crate) fn toggle_dock_area(state: &mut AppState, area: DockArea, ctx: &egui::Context) {
    if state.ui.layout.dock.is_visible(area) {
        state.ui.layout.dock.area_mut(area).collapsed = true;
    } else {
        state.ui.layout.dock.area_mut(area).collapsed = false;
        if state.ui.layout.dock.area(area).tabs.is_empty() {
            let default_view = match area {
                DockArea::Bottom => StaticView::Console,
                DockArea::Right => StaticView::Assistant,
            };
            state.ui.layout.dock.reveal_static(default_view);
        }
    }
    mark_dirty(state, ctx);
}

/// Reveal the Output tab and apply `target`'s source/exact-job filter in one
/// step: restore the tab if absent, activate its dock area, select the filter,
/// and mark the target read. The dispatcher owns dock reveal *and* filter
/// selection so a single user action never leaves them out of sync.
pub(crate) fn reveal_output(state: &mut AppState, target: OutputTarget) {
    state.ui.layout.dock.reveal_static(StaticView::Output);
    let query = LogQuery::new(LogFilter::from_output_target(&target));
    if let Some(latest) = state.session_log().latest_matching_seq(&query) {
        state
            .ui
            .output
            .last_seen_by_target
            .insert(target.clone(), latest);
    }
    state.ui.output.auto_follow = true;
    state.ui.output.target = target;
}

/// Follow a status-notice link to its typed detail target.
pub(crate) fn open_detail_target(state: &mut AppState, target: DetailTarget) {
    match target {
        DetailTarget::Output(output_target) => reveal_output(state, output_target),
        DetailTarget::ActivityJob(job_id) => {
            state.ui.activity_job_target = Some(job_id);
            state.ui.layout.dock.reveal_static(StaticView::TaskMonitor);
        }
        DetailTarget::Settings => state.ui.layout.settings_open = true,
    }
}

/// Resize a dock area — the right sidebar's width or the bottom panel's height —
/// clamped against the viewport, mirroring the primary sidebar's clamp.
pub(crate) fn resize_area(state: &mut AppState, area: DockArea, delta: f32, ctx: &egui::Context) {
    match area {
        DockArea::Right => {
            let max_w = state
                .ui
                .layout
                .secondary_sidebar_max_width(ctx.viewport_rect().width());
            let width = &mut state.ui.layout.dock.right_width;
            *width = (*width + delta).clamp(SIDEBAR_MIN_WIDTH_SECONDARY, max_w);
        }
        DockArea::Bottom => {
            let max_h = (ctx.viewport_rect().height() * 0.6).max(160.0);
            let height = &mut state.ui.layout.dock.bottom_height;
            *height = (*height + delta).clamp(PANEL_MIN_HEIGHT, max_h);
        }
    }
    mark_dirty(state, ctx);
}

pub(crate) fn toggle_primary_sidebar(state: &mut AppState, ctx: &egui::Context) {
    state.ui.layout.show_primary_sidebar = !state.ui.layout.show_primary_sidebar;
    mark_dirty(state, ctx);
}

pub(crate) fn reset_area(state: &mut AppState, area: DockArea, ctx: &egui::Context) {
    match area {
        DockArea::Right => {
            let max_w = state
                .ui
                .layout
                .secondary_sidebar_max_width(ctx.viewport_rect().width());
            state.ui.layout.dock.right_width =
                SIDEBAR_DEFAULT_WIDTH_SECONDARY.clamp(SIDEBAR_MIN_WIDTH_SECONDARY, max_w);
        }
        DockArea::Bottom => {
            state.ui.layout.dock.bottom_height = PANEL_DEFAULT_HEIGHT;
        }
    }
    mark_dirty(state, ctx);
}

/// Reset the entire workbench layout to defaults and persist immediately — an
/// infrequent, explicit action, so it skips the debounce.
pub(crate) fn reset_workbench_layout(state: &mut AppState) {
    state.reset_layout_keep_view();
    persist_layout(state);
}

/// Write the current fixed-view layout to `settings.json` now, clearing any
/// pending debounced save. Errors are surfaced non-fatally.
pub(crate) fn persist_layout(state: &mut AppState) {
    state.clear_layout_save_deadline();
    state.config.dock_layout = state.ui.layout.dock.to_config();
    if let Err(error) = save_config(&state.config) {
        state.report_system_error(
            crate::frontend::state::SystemSubsystem::Settings,
            format!("Could not save layout: {error}"),
        );
    }
}

/// Flush a pending layout save once its debounce window elapses. Called every
/// frame from the app loop; a no-op when the layout is up to date. Unlike
/// project autosave this runs in any workspace, since the layout is a global
/// preference rather than project data.
pub(crate) fn flush_pending_layout_save(state: &mut AppState, ctx: &egui::Context) {
    let Some(deadline) = state.layout_save_deadline() else {
        return;
    };
    let now = ctx.input(|input| input.time);
    if now >= deadline {
        persist_layout(state);
    } else {
        ctx.request_repaint_after(std::time::Duration::from_secs_f64(deadline - now));
    }
}
