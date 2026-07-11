use super::panel_bodies::{
    render_assistant_panel, render_console_panel, render_output_panel, render_plot_panel,
    render_sequence_panel, render_task_monitor_panel, weak_panel_hairline,
};
use super::secondary_sidebar::*;
use super::*;
use crate::backend::tasks::{TaskKind, TaskPanelKind, TaskRun, TaskStatus};
use crate::frontend::state::{DockArea, DockModel, DockTab, StaticView};
use crate::frontend::theme::Palette;

/// What is carried while a dock tab is being dragged. `DockTab` is `Copy`, so the
/// payload is cheap and trivially `Send + Sync + 'static`.
#[derive(Clone)]
struct DraggedTab {
    tab: DockTab,
}

/// Whether a dock tab is currently being dragged. Used to suppress the resize
/// dividers and window-resize handles (so a drag near an edge never starts a
/// resize) and to gate the reveal drop targets for hidden areas.
pub(super) fn drag_in_flight(ctx: &egui::Context) -> bool {
    egui::DragAndDrop::has_payload_of_type::<DraggedTab>(ctx)
}

/// Pre-rendered description of one tab, gathered before the strip is drawn so the
/// drag-source closures don't have to borrow `state`.
struct TabInfo {
    tab: DockTab,
    label: String,
    closeable: bool,
    selected: bool,
}

/// Render one dock area (the bottom panel or the right sidebar): a draggable tab
/// strip plus the active tab's body. The two areas share this single code path.
pub(super) fn render_dock_area(
    state: &mut AppState,
    ui: &mut egui::Ui,
    area: DockArea,
    actions: &mut Vec<AppAction>,
) {
    let pal = crate::frontend::theme::palette(ui);

    // Snapshot the tabs (dropping the borrow before bodies render). Stale task
    // tabs — those whose run no longer exists — are skipped here, mirroring the
    // old secondary-sidebar guard.
    let active = state.ui.layout.dock.area(area).active;
    let tab_infos: Vec<TabInfo> = state
        .ui
        .layout
        .dock
        .area(area)
        .tabs
        .clone()
        .into_iter()
        .filter_map(|tab| {
            let label = match tab {
                DockTab::Static(view) => view.label().to_string(),
                DockTab::Task(id) => state.tasks.task_run(id)?.title.clone(),
            };
            Some(TabInfo {
                tab,
                label,
                closeable: matches!(tab, DockTab::Task(_)),
                selected: active == Some(tab),
            })
        })
        .collect();

    // --- Tab strip --------------------------------------------------------
    let mut activate: Option<DockTab> = None;
    let mut close_task: Option<u64> = None;
    let mut hide = false;
    let mut slot_rects: Vec<(DockTab, Rect)> = Vec::new();

    let strip = ui.allocate_ui_with_layout(
        egui::vec2(ui.available_width(), 30.0),
        Layout::left_to_right(Align::Center),
        |ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            ui.spacing_mut().button_padding = egui::vec2(10.0, 5.0);

            for info in &tab_infos {
                let id = Id::new(("dock_tab", area, info.tab));
                let dragged = drag_source(ui, id, DraggedTab { tab: info.tab }, |ui| {
                    render_tab_inner(ui, id, info, &pal)
                });
                let (chip, x_clicked) = dragged.inner;
                slot_rects.push((info.tab, dragged.response.rect));
                // The chip senses click *and* drag, so egui's drag threshold keeps
                // a plain click a click — it activates here while a drag past the
                // threshold floats the tab (see `drag_source` / `render_tab_inner`).
                if chip.clicked() {
                    activate = Some(info.tab);
                }
                if x_clicked && let DockTab::Task(task_run_id) = info.tab {
                    close_task = Some(task_run_id);
                }
            }

            // Hide caret pinned to the trailing edge (down for the bottom panel,
            // right for the side panel), matching the historical chrome.
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let caret = match area {
                    DockArea::Bottom => egui_phosphor::regular::CARET_DOWN,
                    DockArea::Right => egui_phosphor::regular::CARET_RIGHT,
                };
                if with_core_button_style(ui, false, |ui| {
                    ui.add_sized(
                        [28.0, 28.0],
                        Button::new(
                            RichText::new(caret).color(core_button_text_color(&pal, false)),
                        ),
                    )
                })
                .on_hover_text("Hide")
                .clicked()
                {
                    hide = true;
                }
            });
        },
    );
    let strip_rect = strip.response.rect;

    // --- Drop handling (manual; no `dnd_drop_zone`, which would tint the strip
    // and shift its geometry) -------------------------------------------------
    let ctx = ui.ctx().clone();
    if drag_in_flight(&ctx) && ui.rect_contains_pointer(strip_rect) {
        let index = insertion_index(&slot_rects, &ctx);
        paint_insertion_bar(ui, &slot_rects, index, strip_rect, &pal);
        if ctx.input(|input| input.pointer.any_released())
            && let Some(payload) = egui::DragAndDrop::take_payload::<DraggedTab>(&ctx)
        {
            actions.push(AppAction::MoveDockTab {
                tab: payload.tab,
                to: area,
                index: Some(index),
            });
        }
    }

    // --- Apply collected intents ------------------------------------------
    if hide {
        state.ui.layout.dock.area_mut(area).collapsed = true;
        mark_dirty(state, ui);
    }
    if let Some(task_run_id) = close_task {
        actions.push(AppAction::CloseTaskPanel(task_run_id));
    }
    if let Some(tab) = activate {
        match tab {
            // Fixed-view activation is transient chrome: flip it directly so the
            // body switches this frame, and mark the layout dirty for a debounced
            // save (the active-per-area selection persists).
            DockTab::Static(_) => {
                state.ui.layout.dock.activate(area, tab);
                mark_dirty(state, ui);
            }
            // A task activation also seeds form state, so it routes through the
            // dispatcher (one focused task at a time).
            DockTab::Task(task_run_id) => actions.push(AppAction::ActivateTaskPanel(task_run_id)),
        }
    }

    weak_panel_hairline(ui, 22);

    // --- Body -------------------------------------------------------------
    ui.set_width(ui.available_width());
    // Re-read the active tab: a fixed-view click above switched it this frame.
    match state.ui.layout.dock.area(area).active {
        Some(DockTab::Static(StaticView::Output)) => render_output_panel(state, ui),
        Some(DockTab::Static(StaticView::Console)) => render_console_panel(state, ui, actions),
        Some(DockTab::Static(StaticView::Sequence)) => render_sequence_panel(state, ui, actions),
        Some(DockTab::Static(StaticView::Assistant)) => render_assistant_panel(state, ui, actions),
        Some(DockTab::Static(StaticView::TaskMonitor)) => {
            render_task_monitor_panel(state, ui, actions)
        }
        Some(DockTab::Static(StaticView::Plot)) => render_plot_panel(state, ui, actions),
        // Task bodies own variable-height content, so they scroll (matching the
        // historical side panel); the fixed-view bodies manage their own height
        // and are rendered directly in either area.
        Some(DockTab::Task(task_run_id)) => {
            docked_sidebar_scroll_area()
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    render_task_body(state, ui, task_run_id, actions);
                });
        }
        None => {
            ui.add_space(8.0);
            ui.label(
                RichText::new("Drag a tab here.")
                    .small()
                    .color(pal.text_tertiary),
            );
        }
    }
}

fn mark_dirty(state: &mut AppState, ui: &egui::Ui) {
    let now = ui.input(|input| input.time);
    state.mark_layout_dirty(now);
}

/// The in-window reveal handle for a collapsed-but-non-empty dock area: a thin
/// clickable strip drawn in place of the hidden area (an up caret + the panel's
/// name along the bottom edge, a left caret along the right edge). Clicking it
/// reopens the area. This is the always-present counterpart to the "Hide" caret,
/// so a collapsed Console/Assistant can be brought back without the native menu
/// bar (absent on macOS title bars) or a tab drag. The caller wraps it in the
/// thin panel that reserves the strip's space.
pub(super) fn render_dock_collapsed_handle(
    state: &AppState,
    ui: &mut egui::Ui,
    area: DockArea,
    actions: &mut Vec<AppAction>,
) {
    let pal = crate::frontend::theme::palette(ui);
    let label = collapsed_area_label(state, area);
    let caret = match area {
        DockArea::Bottom => egui_phosphor::regular::CARET_UP,
        DockArea::Right => egui_phosphor::regular::CARET_LEFT,
    };
    // The bottom strip is wide enough to name what reopens; the right strip is a
    // narrow column, so it shows just the caret with the name on hover.
    let text = match area {
        DockArea::Bottom => format!("{caret}  {label}"),
        DockArea::Right => caret.to_string(),
    };
    let size = egui::vec2(ui.available_width(), ui.available_height());
    let clicked = with_core_button_style(ui, false, |ui| {
        ui.add_sized(
            size,
            Button::new(RichText::new(text).color(core_button_text_color(&pal, false))),
        )
    })
    .on_hover_text(format!("Show {label}"))
    .clicked();
    if clicked {
        actions.push(AppAction::ToggleDockArea(area));
    }
}

/// Name of the panel a collapsed area would reopen to — its active tab (or the
/// first tab), so the reveal handle reads "Assistant" / "Console" rather than a
/// generic label. Falls back to "Panel" for the (unreachable here) empty case.
fn collapsed_area_label(state: &AppState, area: DockArea) -> String {
    let area_state = state.ui.layout.dock.area(area);
    let tab = area_state
        .active
        .or_else(|| area_state.tabs.first().copied());
    match tab {
        Some(DockTab::Static(view)) => view.label().to_string(),
        Some(DockTab::Task(id)) => state
            .tasks
            .task_run(id)
            .map(|task| task.title.clone())
            .unwrap_or_else(|| "Panel".to_string()),
        None => "Panel".to_string(),
    }
}

/// Render task panels that are not currently docked in the bottom panel or the
/// right sidebar. They stay session-only, like docked task tabs, but default to
/// their own movable window so opening a builder does not consume the right
/// sidebar unless the user drags it there.
pub(super) fn render_floating_task_windows(
    state: &mut AppState,
    ctx: &egui::Context,
    actions: &mut Vec<AppAction>,
) {
    let floating = state.ui.layout.dock.floating_tasks.clone();
    let mut stale_tasks = Vec::new();

    for (index, panel) in floating.into_iter().enumerate() {
        let task_run_id = panel.task_run_id;
        let Some(task) = state.tasks.task_run(task_run_id).cloned() else {
            stale_tasks.push(task_run_id);
            continue;
        };

        let mut open = true;
        egui::Window::new(task.title.clone())
            .id(Id::new(("floating_task_window", task_run_id)))
            .default_pos(default_floating_task_pos(ctx, index))
            .default_size(egui::vec2(380.0, 560.0))
            .min_width(320.0)
            .min_height(260.0)
            .resizable(true)
            .collapsible(false)
            .order(Order::Foreground)
            .open(&mut open)
            .show(ctx, |ui| {
                render_floating_task_handle(ui, &task, actions);
                weak_panel_hairline(ui, 10);
                docked_sidebar_scroll_area()
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        render_task_body(state, ui, task_run_id, actions);
                    });
            });

        if !open {
            actions.push(AppAction::CloseTaskPanel(task_run_id));
        }
    }

    for task_run_id in stale_tasks {
        state.ui.layout.dock.remove_task(task_run_id);
    }
}

fn default_floating_task_pos(ctx: &egui::Context, index: usize) -> egui::Pos2 {
    let rect = ctx.content_rect();
    let offset = (index.min(5) as f32) * 26.0;
    rect.center() - egui::vec2(190.0 - offset, 280.0 - offset)
}

fn render_floating_task_handle(ui: &mut egui::Ui, task: &TaskRun, actions: &mut Vec<AppAction>) {
    let pal = crate::frontend::theme::palette(ui);
    ui.horizontal(|ui| {
        let tab = DockTab::Task(task.id);
        let id = Id::new(("floating_task_handle", task.id));
        let info = TabInfo {
            tab,
            label: task.title.clone(),
            closeable: false,
            selected: true,
        };
        let dragged = drag_source(ui, id, DraggedTab { tab }, |ui| {
            render_tab_inner(ui, id, &info, &pal)
        });
        let (chip, _) = dragged.inner;
        if chip.clicked() {
            actions.push(AppAction::ActivateTaskPanel(task.id));
        }
        chip.on_hover_text("Drag to dock this task panel");
    });
}

/// The floating-preview half of a dock-tab drag, standing in for the painting
/// that [`egui::Ui::dnd_drag_source`] does. The *sensing* half lives on the chip
/// itself (see [`render_tab_inner`]), which senses `click_and_drag` so egui's
/// native 6px / long-press threshold (`is_decidedly_dragging`) decides click vs
/// drag — a plain click never becomes a drag. (egui's own helper instead layers
/// a pure `Sense::drag()` widget on top, which both skips that threshold *and*,
/// per `hit_test`, makes egui ignore the click-widgets beneath it — so the tab
/// wouldn't activate and its close glyph was dead.) Once the chip is decidedly
/// dragging, `is_being_dragged` is true and we repaint it on a layer that follows
/// the pointer, with the drag payload set for the strip's drop handling.
fn drag_source<R>(
    ui: &mut egui::Ui,
    id: Id,
    payload: DraggedTab,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::InnerResponse<R> {
    if ui.ctx().is_being_dragged(id) {
        egui::DragAndDrop::set_payload(ui.ctx(), payload);

        // Paint the body to a floating layer that tracks the pointer.
        let layer_id = egui::LayerId::new(Order::Tooltip, id);
        let egui::InnerResponse { inner, response } =
            ui.scope_builder(egui::UiBuilder::new().layer_id(layer_id), add_contents);
        if let Some(pointer) = ui.ctx().pointer_interact_pos() {
            let delta = pointer - response.rect.center();
            ui.ctx().transform_layer_shapes(
                layer_id,
                egui::emath::TSTransform::from_translation(delta),
            );
        }
        egui::InnerResponse::new(inner, response)
    } else {
        ui.scope(add_contents)
    }
}

/// Render one tab as a single rounded chip: the (elided) title and, for a task
/// tab, a close affordance *inside* the chip on its right edge. Returns the
/// chip's response (for click-to-activate / drag) and whether the close glyph was
/// clicked. Drawn by hand rather than as two `Button`s so the close glyph sits
/// within the chip with its own hit target, and so a long title elides instead
/// of shoving the close glyph past the (narrow) sidebar's clip edge.
fn render_tab_inner(
    ui: &mut egui::Ui,
    id: Id,
    info: &TabInfo,
    pal: &Palette,
) -> (egui::Response, bool) {
    const H_PAD: f32 = 10.0; // chip side padding (matches egui's button_padding.x)
    const V_PAD: f32 = 5.0;
    const GAP: f32 = 6.0; // title → close glyph
    const CLOSE: f32 = 14.0; // close hit box (square)
    const MAX_TITLE_W: f32 = 180.0; // elide longer titles so the close stays in view

    let selected = info.selected;

    // Single-line, ellipsized title. Its colour is left as `PLACEHOLDER` and
    // resolved at paint time so hover can lift it.
    let mut job = egui::text::LayoutJob::single_section(
        info.label.clone(),
        egui::text::TextFormat {
            font_id: egui::TextStyle::Button.resolve(ui.style()),
            color: egui::Color32::PLACEHOLDER,
            ..Default::default()
        },
    );
    job.wrap = egui::text::TextWrapping::truncate_at_width(MAX_TITLE_W);
    let galley = ui.painter().layout_job(job);

    let close_w = if info.closeable { GAP + CLOSE } else { 0.0 };
    let chip_size = egui::vec2(
        galley.size().x + close_w + 2.0 * H_PAD,
        galley.size().y.max(CLOSE) + 2.0 * V_PAD,
    );

    // Reserve the slot, then sense click *and* drag on the chip under the stable
    // `id` (so `drag_source` can track the drag). Sensing both lets egui's drag
    // threshold tell a click from a drag: a short press activates, a press that
    // travels past the threshold drags.
    let (rect, _) = ui.allocate_exact_size(chip_size, egui::Sense::hover());
    let response = ui.interact(rect, id, egui::Sense::click_and_drag());

    // The close glyph is interacted *after* the chip so it owns clicks over its
    // own rect (egui reports the close click and the chip drag both); reading it
    // now also lets the chip keep its hover wash while the pointer is over the
    // glyph (which otherwise occludes the chip's `hovered`).
    let close = info.closeable.then(|| {
        let close_rect = egui::Rect::from_center_size(
            egui::pos2(rect.right() - H_PAD - CLOSE / 2.0, rect.center().y),
            egui::vec2(CLOSE, CLOSE),
        );
        let response = ui
            .interact(
                close_rect,
                Id::new(("dock_tab_close", info.tab)),
                egui::Sense::click(),
            )
            .on_hover_text("Close tab");
        (close_rect, response)
    });

    let hovered = response.hovered() || close.as_ref().is_some_and(|(_, r)| r.hovered());

    // Chip background, mirroring standard tab-button states (transparent at rest,
    // a soft wash on hover, a blue tint when active).
    let fill = match (selected, hovered) {
        (true, true) => pal.blue_overlay(74),
        (true, false) => pal.blue_overlay(58),
        (false, true) => pal.neutral_overlay(18),
        (false, false) => egui::Color32::TRANSPARENT,
    };
    if fill != egui::Color32::TRANSPARENT {
        ui.painter().rect_filled(
            rect,
            egui::CornerRadius::same(crate::frontend::theme::radius::CHIP),
            fill,
        );
    }

    let title_pos = egui::pos2(rect.left() + H_PAD, rect.center().y - galley.size().y / 2.0);
    ui.painter().galley(
        title_pos,
        galley,
        core_button_text_color(pal, selected || hovered),
    );

    let mut x_clicked = false;
    if let Some((close_rect, close)) = close {
        let close_hovered = close.hovered();
        if close_hovered {
            ui.painter().rect_filled(
                close_rect,
                egui::CornerRadius::same(crate::frontend::theme::radius::CHIP),
                pal.neutral_overlay(30),
            );
        }
        ui.painter().text(
            close_rect.center(),
            egui::Align2::CENTER_CENTER,
            egui_phosphor::regular::X,
            egui::FontId::proportional(12.0),
            core_button_text_color(pal, selected || hovered || close_hovered),
        );
        x_clicked = close.clicked();
    }

    (response, x_clicked)
}

/// Index at which a dropped tab should be inserted, from the pointer's x against
/// each tab's in-place slot rect (the floating drag preview is a separate layer
/// and does not affect these rects).
fn insertion_index(slot_rects: &[(DockTab, Rect)], ctx: &egui::Context) -> usize {
    let Some(pointer) =
        ctx.input(|input| input.pointer.interact_pos().or(input.pointer.hover_pos()))
    else {
        return slot_rects.len();
    };
    slot_rects
        .iter()
        .position(|(_, rect)| pointer.x < rect.center().x)
        .unwrap_or(slot_rects.len())
}

/// Paint the 2px insertion indicator at the computed drop position.
fn paint_insertion_bar(
    ui: &egui::Ui,
    slot_rects: &[(DockTab, Rect)],
    index: usize,
    strip_rect: Rect,
    pal: &Palette,
) {
    let x = if let Some((_, rect)) = slot_rects.get(index) {
        rect.left() - 3.0
    } else if let Some((_, rect)) = slot_rects.last() {
        rect.right() + 3.0
    } else {
        strip_rect.left() + 2.0
    };
    ui.painter().vline(
        x,
        (strip_rect.top() + 4.0)..=(strip_rect.bottom() - 4.0),
        Stroke::new(2.0_f32, pal.accent),
    );
}

/// The body of a task-detail tab: header plus the per-kind panel. Only the
/// globally focused task (the one whose form state is loaded) shows its panel; a
/// task tab focused in another area offers to bring focus here, since the
/// per-task forms share a single set of pending-form slots.
fn render_task_body(
    state: &mut AppState,
    ui: &mut egui::Ui,
    task_run_id: u64,
    actions: &mut Vec<AppAction>,
) {
    let pal = crate::frontend::theme::palette(ui);
    let Some(task) = state.tasks.task_run(task_run_id).cloned() else {
        return;
    };

    ui.label(RichText::new(task.title).strong());
    ui.label(
        RichText::new(format!(
            "{} / {} / {}",
            task.theme, task.method, task.application
        ))
        .small()
        .color(pal.text_tertiary),
    );
    ui.separator();

    // Completed QM runs embed a small chart preview with a click-through to
    // the Plot panel. Single-point/frequencies runs create no entry, so this
    // is their chart affordance.
    if task.status == TaskStatus::Completed
        && matches!(
            task.kind,
            TaskKind::RunQmEnergy
                | TaskKind::RunQmOptimize
                | TaskKind::RunQmFrequencies
                | TaskKind::RunQmTransitionState
        )
        && let Some(spec) = crate::frontend::dispatcher::task_chart_thumbnail(state, task_run_id)
    {
        super::plot_view::render_chart(
            ui,
            &spec,
            ("task-chart-thumb", task_run_id),
            110.0,
            false,
            false,
        );
        if ui.button("Open in Plot panel").clicked() {
            actions.push(AppAction::OpenChart(
                crate::frontend::actions::ChartTarget::TaskRun(task_run_id),
            ));
        }
        ui.separator();
    }

    if state.tasks.active_panel != Some(task_run_id) {
        ui.label(
            RichText::new("This task panel is focused in another area.")
                .small()
                .color(pal.text_tertiary),
        );
        if ui.button("Show here").clicked() {
            actions.push(AppAction::ActivateTaskPanel(task_run_id));
        }
        return;
    }

    match task.panel {
        TaskPanelKind::ReticularBuilder => render_framework_task_panel(state, ui, actions),
        TaskPanelKind::NanosheetBuilder => render_nanosheet_task_panel(state, ui, actions),
        TaskPanelKind::BuildingBlockEditor => render_building_block_task_panel(state, ui, actions),
        TaskPanelKind::OptimizationPrompt => render_optimization_task_panel(state, ui, actions),
        TaskPanelKind::QmPrompt => render_qm_task_panel(state, ui, actions),
        TaskPanelKind::SupercellPrompt => render_supercell_task_panel(state, ui, actions),
        TaskPanelKind::ProteinPrepPrompt => render_protein_prep_task_panel(state, ui, actions),
        TaskPanelKind::MdSystemPrompt => render_md_system_task_panel(state, ui, actions),
        TaskPanelKind::DisorderedSystemPrompt => render_disorder_task_panel(state, ui, actions),
        TaskPanelKind::MdRunPrompt => render_md_run_task_panel(state, ui, actions),
        TaskPanelKind::DockingPrompt => render_docking_task_panel(state, ui, actions),
        TaskPanelKind::PtmPrompt => render_ptm_task_panel(state, ui, actions),
        TaskPanelKind::None => {
            ui.label("This task runs directly and does not need a panel.");
            if ui
                .button(format!("{}  Close", egui_phosphor::regular::X))
                .clicked()
            {
                actions.push(AppAction::CloseTaskPanel(task_run_id));
            }
        }
    }
}

/// Drop targets that reveal a hidden/empty dock area: drawn only while a tab is
/// being dragged, inset inside the workspace so they never collide with the
/// window-resize handles (suppressed during a drag) or the window edges. Dropping
/// here moves the tab into that area, which clears its collapsed flag and reveals
/// it next frame. Called once from the workbench after the central panel.
pub(super) fn render_dock_reveal_targets(
    ctx: &egui::Context,
    workspace_rect: Rect,
    dock: &DockModel,
    pal: &Palette,
    actions: &mut Vec<AppAction>,
) {
    if !drag_in_flight(ctx) {
        return;
    }
    const STRIP: f32 = 40.0;
    const INSET: f32 = 12.0;
    let pointer = ctx.input(|input| input.pointer.interact_pos().or(input.pointer.hover_pos()));

    for area in DockArea::all() {
        if dock.is_visible(area) {
            continue;
        }
        let rect = match area {
            DockArea::Right => Rect::from_min_max(
                egui::pos2(
                    workspace_rect.right() - STRIP - INSET,
                    workspace_rect.top() + INSET,
                ),
                egui::pos2(
                    workspace_rect.right() - INSET,
                    workspace_rect.bottom() - INSET,
                ),
            ),
            DockArea::Bottom => Rect::from_min_max(
                egui::pos2(
                    workspace_rect.left() + INSET,
                    workspace_rect.bottom() - STRIP - INSET,
                ),
                egui::pos2(
                    workspace_rect.right() - INSET,
                    workspace_rect.bottom() - INSET,
                ),
            ),
        };
        if rect.width() <= 0.0 || rect.height() <= 0.0 {
            continue;
        }
        let hovering = pointer.is_some_and(|p| rect.contains(p));

        egui::Area::new(Id::new(("dock_reveal_target", area)))
            .order(Order::Tooltip)
            .fixed_pos(rect.min)
            .interactable(false)
            .show(ctx, |ui| {
                let fill = if hovering {
                    pal.selection_fill
                } else {
                    pal.selection_fill.gamma_multiply(0.45)
                };
                ui.painter().rect(
                    rect,
                    egui::CornerRadius::same(crate::frontend::theme::radius::CARD),
                    fill,
                    Stroke::new(1.0_f32, pal.accent),
                    egui::StrokeKind::Inside,
                );
            });

        if hovering
            && ctx.input(|input| input.pointer.any_released())
            && let Some(payload) = egui::DragAndDrop::take_payload::<DraggedTab>(ctx)
        {
            actions.push(AppAction::MoveDockTab {
                tab: payload.tab,
                to: area,
                index: None,
            });
        }
    }
}
