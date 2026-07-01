use super::*;

use crate::backend::config::{DockAreaLayout, DockLayoutConfig, TaskPanelPlacement};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrimaryView {
    EntryList,
    Tasks,
    Style,
}

impl PrimaryView {
    pub fn all() -> &'static [Self] {
        &[Self::EntryList, Self::Tasks, Self::Style]
    }

    pub fn icon(self) -> &'static str {
        match self {
            Self::EntryList => egui_phosphor::regular::LIST,
            Self::Tasks => egui_phosphor::regular::LIGHTNING,
            Self::Style => egui_phosphor::regular::PALETTE,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::EntryList => "Entry List",
            Self::Tasks => "Tasks",
            Self::Style => "Style",
        }
    }

    /// Compact label shown inside the sidebar's segmented view switcher, where
    /// each segment is only a third of the sidebar width.
    pub fn short_label(self) -> &'static str {
        match self {
            Self::EntryList => "Entries",
            Self::Tasks => "Tasks",
            Self::Style => "Style",
        }
    }
}

/// A fixed (always-available) view that can be docked in either area: the
/// console, the assistant, the task monitor, or the command output. These
/// are the movable counterparts of the per-task panels and are the only tabs
/// whose placement persists across launches (task tabs are session state).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StaticView {
    Output,
    Console,
    Sequence,
    Assistant,
    TaskMonitor,
}

impl StaticView {
    /// Every fixed view. Order is used only by the load-time completeness pass
    /// (each area renders its own `tabs` order); the historical bottom-panel tab
    /// order is preserved here for familiarity.
    pub fn all() -> &'static [Self] {
        &[
            Self::Console,
            Self::Sequence,
            Self::Assistant,
            Self::TaskMonitor,
            Self::Output,
        ]
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Console => "Console",
            Self::Sequence => "Sequence",
            Self::Assistant => "Assistant",
            Self::TaskMonitor => "Task Monitor",
            Self::Output => "Output",
        }
    }

    /// Stable token used for persistence — decoupled from the enum order so the
    /// variants can be reordered without invalidating a saved layout (mirrors
    /// [`AtomStyle::token`]).
    pub fn token(self) -> &'static str {
        match self {
            Self::Console => "console",
            Self::Sequence => "sequence",
            Self::Assistant => "assistant",
            Self::TaskMonitor => "task_monitor",
            Self::Output => "output",
        }
    }

    pub fn from_token(token: &str) -> Option<Self> {
        Some(match token {
            "console" => Self::Console,
            "sequence" => Self::Sequence,
            "assistant" => Self::Assistant,
            "task_monitor" => Self::TaskMonitor,
            "output" => Self::Output,
            _ => return None,
        })
    }

    /// The area a view defaults into when a saved layout doesn't place it. Assistant
    /// lives on the right (next to the structure, like comparable assistants);
    /// the rest live in the bottom panel.
    pub fn home_area(self) -> DockArea {
        match self {
            Self::Assistant => DockArea::Right,
            _ => DockArea::Bottom,
        }
    }
}

/// One tab in a dock area: either a fixed view or a per-task detail panel keyed
/// by its task-run id. `Copy` + `'static` so it is a valid drag-and-drop payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DockTab {
    Static(StaticView),
    Task(u64),
}

/// A dockable area. The left primary sidebar is intentionally not a dock target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DockArea {
    Bottom,
    Right,
}

impl DockArea {
    pub fn all() -> [Self; 2] {
        [Self::Bottom, Self::Right]
    }
}

/// The ordered tabs of one dock area, the active tab, and whether the user has
/// explicitly collapsed it. `active` is `None` only when `tabs` is empty.
#[derive(Debug, Clone, Default)]
pub struct DockAreaState {
    pub tabs: Vec<DockTab>,
    pub active: Option<DockTab>,
    pub collapsed: bool,
}

/// Session-only placement for a task panel shown as an in-window floating
/// window. The window's live position and size are owned by egui's window memory;
/// this model only records which task panels are currently floating.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FloatingTaskPanel {
    pub task_run_id: u64,
}

/// Placement and sizing of the two dock areas. The single source of truth for
/// which view/panel is shown where; the renderer and the dispatcher both drive
/// it through the helpers below (none of which touch `egui::Context`). The fixed
/// views are mirrored to [`DockLayoutConfig`] for persistence; task tabs are
/// session-only and are rebuilt by [`DockModel::add_task`].
#[derive(Debug, Clone)]
pub struct DockModel {
    pub bottom: DockAreaState,
    pub right: DockAreaState,
    pub floating_tasks: Vec<FloatingTaskPanel>,
    pub right_width: f32,
    pub bottom_height: f32,
}

impl Default for DockModel {
    fn default() -> Self {
        Self {
            // Bottom shows console / monitor / output (console active), matching
            // the historical bottom-panel tabs — visible at rest.
            bottom: DockAreaState {
                tabs: vec![
                    DockTab::Static(StaticView::Console),
                    DockTab::Static(StaticView::Sequence),
                    DockTab::Static(StaticView::TaskMonitor),
                    DockTab::Static(StaticView::Output),
                ],
                active: Some(DockTab::Static(StaticView::Console)),
                collapsed: false,
            },
            // Assistant's home is the right sidebar and it is shown at rest, so a
            // first run opens straight into the assistant.
            right: DockAreaState {
                tabs: vec![DockTab::Static(StaticView::Assistant)],
                active: Some(DockTab::Static(StaticView::Assistant)),
                collapsed: false,
            },
            floating_tasks: Vec::new(),
            right_width: SIDEBAR_DEFAULT_WIDTH_SECONDARY,
            bottom_height: PANEL_DEFAULT_HEIGHT,
        }
    }
}

impl DockModel {
    pub fn area(&self, area: DockArea) -> &DockAreaState {
        match area {
            DockArea::Bottom => &self.bottom,
            DockArea::Right => &self.right,
        }
    }

    pub fn area_mut(&mut self, area: DockArea) -> &mut DockAreaState {
        match area {
            DockArea::Bottom => &mut self.bottom,
            DockArea::Right => &mut self.right,
        }
    }

    /// An area is shown when it holds at least one tab and the user has not
    /// explicitly collapsed it — the single visibility rule (auto-hide is
    /// structural; `collapsed` is the explicit override).
    pub fn is_visible(&self, area: DockArea) -> bool {
        let state = self.area(area);
        !state.tabs.is_empty() && !state.collapsed
    }

    /// An area the user has explicitly collapsed while it still holds tabs — the
    /// "I hid it and now it's gone" state. Distinct from a merely *empty* area
    /// (also hidden, but with nothing to reopen): only this state earns the
    /// in-window reveal handle.
    pub fn is_collapsed(&self, area: DockArea) -> bool {
        let state = self.area(area);
        !state.tabs.is_empty() && state.collapsed
    }

    pub fn area_of(&self, tab: DockTab) -> Option<DockArea> {
        DockArea::all()
            .into_iter()
            .find(|&area| self.area(area).tabs.contains(&tab))
    }

    /// Remove `tab` from whichever area holds it, repointing that area's active
    /// tab to the last remaining one when the removed tab was active (mirrors
    /// `TaskManager::close_panel`). Returns the area it left.
    pub fn remove_tab(&mut self, tab: DockTab) -> Option<DockArea> {
        if let DockTab::Task(task_run_id) = tab {
            self.floating_tasks
                .retain(|panel| panel.task_run_id != task_run_id);
        }
        let area = self.area_of(tab)?;
        let state = self.area_mut(area);
        state.tabs.retain(|candidate| *candidate != tab);
        if state.active == Some(tab) {
            state.active = state.tabs.last().copied();
        }
        Some(area)
    }

    /// Insert `tab` into `area` at `at` (appended when `None`), after removing any
    /// existing copy from either area so a tab lives in exactly one place. Makes
    /// it active and reveals the area.
    pub fn insert_tab(&mut self, area: DockArea, tab: DockTab, at: Option<usize>) {
        self.remove_tab(tab);
        let state = self.area_mut(area);
        let index = at.unwrap_or(state.tabs.len()).min(state.tabs.len());
        state.tabs.insert(index, tab);
        state.active = Some(tab);
        state.collapsed = false;
    }

    /// Move `tab` to `area` at the given index, handling a same-area reorder (the
    /// target index is recomputed after the source removal shifts it).
    pub fn move_tab(&mut self, tab: DockTab, to: DockArea, at: Option<usize>) {
        let adjusted = match (self.area_of(tab), at) {
            (Some(from), Some(index)) if from == to => {
                let old = self.area(to).tabs.iter().position(|t| *t == tab);
                match old {
                    Some(old) if old < index => Some(index - 1),
                    _ => Some(index),
                }
            }
            _ => at,
        };
        self.insert_tab(to, tab, adjusted);
    }

    /// Make `tab` the active tab of `area` and reveal the area (no-op if the tab
    /// isn't in that area).
    pub fn activate(&mut self, area: DockArea, tab: DockTab) {
        let state = self.area_mut(area);
        if state.tabs.contains(&tab) {
            state.active = Some(tab);
            state.collapsed = false;
        }
    }

    /// Ensure the fixed `view` exists (appending it to its home area if absent),
    /// then activate and reveal its area. Backs the "show this view" buttons.
    pub fn reveal_static(&mut self, view: StaticView) {
        let tab = DockTab::Static(view);
        let area = self.area_of(tab).unwrap_or_else(|| {
            let home = view.home_area();
            self.area_mut(home).tabs.push(tab);
            home
        });
        self.activate(area, tab);
    }

    /// Add a task panel to the requested default host, unless it is already open.
    /// Existing docked panels stay docked; existing floating panels are brought
    /// to the front by rendering them last.
    pub fn add_task(&mut self, task_run_id: u64, placement: TaskPanelPlacement) {
        let tab = DockTab::Task(task_run_id);
        if let Some(area) = self.area_of(tab) {
            self.activate(area, tab);
            return;
        }
        if let Some(index) = self
            .floating_tasks
            .iter()
            .position(|panel| panel.task_run_id == task_run_id)
        {
            let panel = self.floating_tasks.remove(index);
            self.floating_tasks.push(panel);
            return;
        }
        match placement {
            TaskPanelPlacement::Floating => {
                self.floating_tasks.push(FloatingTaskPanel { task_run_id });
            }
            TaskPanelPlacement::RightSidebar => self.insert_tab(DockArea::Right, tab, None),
            TaskPanelPlacement::BottomPanel => self.insert_tab(DockArea::Bottom, tab, None),
        }
    }

    pub fn remove_task(&mut self, task_run_id: u64) {
        self.remove_tab(DockTab::Task(task_run_id));
    }

    /// Drop every task tab (keeping the fixed views), e.g. on project close.
    pub fn clear_task_tabs(&mut self) {
        self.floating_tasks.clear();
        for area in DockArea::all() {
            let state = self.area_mut(area);
            state.tabs.retain(|tab| matches!(tab, DockTab::Static(_)));
            if state
                .active
                .is_some_and(|active| !state.tabs.contains(&active))
            {
                state.active = state.tabs.last().copied();
            }
        }
    }

    /// Serialize the fixed-view placement (task tabs are excluded structurally).
    pub fn to_config(&self) -> DockLayoutConfig {
        DockLayoutConfig {
            bottom: area_to_config(&self.bottom),
            right: area_to_config(&self.right),
            right_width: self.right_width,
            bottom_height: self.bottom_height,
        }
    }

    /// Rebuild from a saved layout, then run a completeness pass so every fixed
    /// view appears in exactly one area (no view can ever be unreachable, even
    /// from a hand-edited or older `settings.json`). No task tabs are restored.
    pub fn from_config(config: &DockLayoutConfig) -> Self {
        let mut model = Self {
            bottom: area_from_config(&config.bottom),
            right: area_from_config(&config.right),
            floating_tasks: Vec::new(),
            right_width: config.right_width,
            bottom_height: config.bottom_height,
        };
        model.ensure_all_static_views();
        model
    }

    fn ensure_all_static_views(&mut self) {
        for &view in StaticView::all() {
            let tab = DockTab::Static(view);
            let holders: Vec<DockArea> = DockArea::all()
                .into_iter()
                .filter(|&area| self.area(area).tabs.contains(&tab))
                .collect();
            match holders.as_slice() {
                [] => {
                    // Missing entirely — restore it to its home area.
                    let state = self.area_mut(view.home_area());
                    state.tabs.push(tab);
                    if state.active.is_none() {
                        state.active = Some(tab);
                    }
                }
                [_only] => {}
                _ => {
                    // Present in more than one area — keep the first, drop the rest.
                    for &area in &holders[1..] {
                        let state = self.area_mut(area);
                        state.tabs.retain(|candidate| *candidate != tab);
                        if state.active == Some(tab) {
                            state.active = state.tabs.last().copied();
                        }
                    }
                }
            }
        }
    }
}

/// Serialize one area's fixed views (in order); a task tab can't be restored, so
/// an active task falls back to the area's first fixed view.
fn area_to_config(state: &DockAreaState) -> DockAreaLayout {
    let tabs: Vec<String> = state
        .tabs
        .iter()
        .filter_map(|tab| match tab {
            DockTab::Static(view) => Some(view.token().to_string()),
            DockTab::Task(_) => None,
        })
        .collect();
    let active = match state.active {
        Some(DockTab::Static(view)) => Some(view.token().to_string()),
        _ => tabs.first().cloned(),
    };
    DockAreaLayout {
        tabs,
        active,
        collapsed: state.collapsed,
    }
}

/// Rebuild one area from a saved layout: parse known tokens (skip unknown ones),
/// de-duplicate, and reconcile the active tab to a present view.
fn area_from_config(layout: &DockAreaLayout) -> DockAreaState {
    let mut tabs: Vec<DockTab> = Vec::new();
    for token in &layout.tabs {
        if let Some(view) = StaticView::from_token(token) {
            let tab = DockTab::Static(view);
            if !tabs.contains(&tab) {
                tabs.push(tab);
            }
        }
    }
    let active = layout
        .active
        .as_deref()
        .and_then(StaticView::from_token)
        .map(DockTab::Static)
        .filter(|tab| tabs.contains(tab))
        .or_else(|| tabs.first().copied());
    DockAreaState {
        tabs,
        active,
        collapsed: layout.collapsed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::config::{DockAreaLayout, DockLayoutConfig};

    #[test]
    fn from_config_restores_missing_sequence() {
        let config = DockLayoutConfig {
            bottom: DockAreaLayout {
                tabs: vec!["console".into()],
                active: Some("console".into()),
                collapsed: false,
            },
            right: DockAreaLayout {
                tabs: vec!["assistant".into()],
                active: Some("assistant".into()),
                collapsed: false,
            },
            right_width: 320.0,
            bottom_height: 240.0,
        };
        let model = DockModel::from_config(&config);
        assert!(
            model
                .bottom
                .tabs
                .contains(&DockTab::Static(StaticView::Sequence))
        );
    }
    #[test]
    fn from_config_restores_missing_task_monitor() {
        let config = DockLayoutConfig {
            bottom: DockAreaLayout {
                tabs: vec!["console".into()],
                active: Some("console".into()),
                collapsed: false,
            },
            right: DockAreaLayout {
                tabs: vec!["assistant".into()],
                active: Some("assistant".into()),
                collapsed: false,
            },
            right_width: 320.0,
            bottom_height: 240.0,
        };
        let model = DockModel::from_config(&config);
        assert!(
            model
                .bottom
                .tabs
                .contains(&DockTab::Static(StaticView::TaskMonitor))
        );
    }

    #[test]
    fn dock_model_default_matches_layout_config_default() {
        let model = DockModel::default().to_config();
        let config = DockLayoutConfig::default();

        assert_eq!(model.bottom.tabs, config.bottom.tabs);
        assert_eq!(model.right.tabs, config.right.tabs);
        assert_eq!(model.bottom.active, config.bottom.active);
        assert_eq!(model.right.active, config.right.active);
        assert_eq!(model.bottom.collapsed, config.bottom.collapsed);
        assert_eq!(model.right.collapsed, config.right.collapsed);
        assert_eq!(model.right_width, config.right_width);
        assert_eq!(model.bottom_height, config.bottom_height);
    }
}
