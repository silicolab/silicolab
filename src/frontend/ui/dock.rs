use super::panel_bodies::{
    render_chat_panel, render_console_panel, render_output_panel, render_task_monitor_panel,
    weak_panel_hairline,
};
use super::secondary_sidebar::*;
use super::*;
use crate::backend::tasks::TaskPanelKind;
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
                let dragged = ui.dnd_drag_source(id, DraggedTab { tab: info.tab }, |ui| {
                    render_tab_inner(ui, info, &pal)
                });
                let (button, x_clicked) = dragged.inner;
                slot_rects.push((info.tab, dragged.response.rect));
                // The click resolves on the inner button (the drag source only
                // senses drags), so click-to-activate and drag coexist.
                if button.clicked() {
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
        Some(DockTab::Static(StaticView::Chat)) => render_chat_panel(state, ui, actions),
        Some(DockTab::Static(StaticView::TaskMonitor)) => {
            render_task_monitor_panel(state, ui, actions)
        }
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

/// Render one tab's button (and, for a task tab, a trailing close affordance).
/// Returns the button's response (for click-to-activate) and whether the close
/// affordance was clicked.
fn render_tab_inner(ui: &mut egui::Ui, info: &TabInfo, pal: &Palette) -> (egui::Response, bool) {
    ui.spacing_mut().item_spacing.x = 4.0;
    let button = ui
        .scope(|ui| {
            configure_panel_tab_button_visuals(ui, info.selected);
            ui.add(
                Button::new(
                    RichText::new(&info.label).color(core_button_text_color(pal, info.selected)),
                )
                .selected(info.selected),
            )
        })
        .inner;
    let mut x_clicked = false;
    if info.closeable {
        x_clicked = ui
            .add(
                Button::new(
                    RichText::new(egui_phosphor::regular::X)
                        .size(12.0)
                        .color(core_button_text_color(pal, info.selected)),
                )
                .frame(false),
            )
            .on_hover_text("Close tab")
            .clicked();
    }
    (button, x_clicked)
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
        Stroke::new(2.0, pal.accent),
    );
}

/// Visuals for a dock tab button: transparent at rest, a soft wash on hover, and
/// a tinted highlight when selected. Moved here from the bottom panel so both
/// areas share one styling.
pub(super) fn configure_panel_tab_button_visuals(ui: &mut Ui, selected: bool) {
    let pal = crate::frontend::theme::palette(ui);
    let inactive_fill = egui::Color32::TRANSPARENT;
    let hovered_fill = pal.neutral_overlay(18);
    let selected_fill = pal.blue_overlay(58);
    let selected_hover_fill = pal.blue_overlay(74);
    let text_color = core_button_text_color(&pal, selected);
    let selected_text = core_button_text_color(&pal, true);
    let visuals = &mut ui.style_mut().visuals.widgets;

    visuals.inactive.weak_bg_fill = inactive_fill;
    visuals.inactive.bg_fill = inactive_fill;
    visuals.inactive.bg_stroke = Stroke::NONE;
    visuals.inactive.fg_stroke.color = text_color;

    visuals.hovered.weak_bg_fill = hovered_fill;
    visuals.hovered.bg_fill = hovered_fill;
    visuals.hovered.bg_stroke = Stroke::NONE;
    visuals.hovered.fg_stroke.color = selected_text;

    visuals.active.weak_bg_fill = selected_hover_fill;
    visuals.active.bg_fill = selected_hover_fill;
    visuals.active.bg_stroke = Stroke::NONE;
    visuals.active.fg_stroke.color = selected_text;

    visuals.open.weak_bg_fill = selected_fill;
    visuals.open.bg_fill = selected_fill;
    visuals.open.bg_stroke = Stroke::NONE;
    visuals.open.fg_stroke.color = selected_text;
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
                    Stroke::new(1.0, pal.accent),
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
