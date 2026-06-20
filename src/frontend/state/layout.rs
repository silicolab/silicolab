use super::*;

/// An item in the sidebar list that can be selected: either an entry or a group header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelectionItem {
    Entry(u64),
    Group(String),
}

#[derive(Debug, Clone, Default)]
pub struct EntryListState {
    pub search_query: String,
    pub search_open: bool,
    pub selected_entry_ids: std::collections::BTreeSet<u64>,
    pub selected_group_ids: std::collections::BTreeSet<String>,
    pub selection_anchor: Option<SelectionItem>,
    pub collapsed_group_ids: std::collections::BTreeSet<String>,
    pub renaming_entry_id: Option<u64>,
    pub rename_buffer: String,
    pub creating_group: bool,
    pub new_group_name: String,
    pub renaming_group_id: Option<String>,
    pub rename_group_buffer: String,
    /// Set once focus is handed to the group rename editor, so it is requested
    /// only on the first frame of a rename.
    pub rename_group_focus_requested: bool,
}

#[derive(Debug, Clone)]
pub struct LayoutState {
    pub active_primary_view: PrimaryView,
    pub show_primary_sidebar: bool,
    /// Whether the Settings modal dialog is open. Transient window chrome (it is
    /// never persisted), so — like the sidebar-visibility flags above — it is
    /// flipped directly by the UI entry points rather than through an AppAction:
    /// no persisted state changes when it toggles, so there is nothing for the
    /// dispatcher to mediate.
    pub settings_open: bool,
    /// Whether the custom About window is open. Transient chrome like
    /// `settings_open`: flipped directly by the menu entry points, never
    /// persisted, so the dispatcher does not mediate it.
    pub about_open: bool,
    /// Whether the system-monitor detail popover is open. Transient chrome like
    /// `settings_open`: flipped directly by clicking the compact monitor widget.
    pub monitor_popover_open: bool,
    /// Screen rect of the compact monitor widget (sidebar footer, or status-bar
    /// fallback when the sidebar is hidden), captured each frame so the popover
    /// can anchor itself just above it.
    pub monitor_anchor: Option<eframe::egui::Rect>,
    /// Measured height of the sidebar monitor footer (the CPU/Memory/GPU bar
    /// stack), captured each frame and reserved on the next so every GPU row fits
    /// — the row count varies by machine. Seeded with a one-GPU estimate.
    pub monitor_footer_height: f32,
    pub primary_sidebar_width: f32,
    /// The dockable bottom panel + right sidebar: which views/panels live where,
    /// their order, the active tab per area, visibility, and the two area sizes.
    /// Replaces the former fixed `active_panel_tab` / `show_secondary_sidebar` /
    /// `show_panel` / `secondary_sidebar_width` / `panel_height` fields. Its fixed
    /// views are persisted across launches; see [`DockModel`].
    pub dock: DockModel,
}

pub const SIDEBAR_MIN_WIDTH_PRIMARY: f32 = 220.0;
pub const SIDEBAR_MIN_WIDTH_SECONDARY: f32 = 240.0;
pub const SIDEBAR_DEFAULT_WIDTH_PRIMARY: f32 = 240.0;
pub const SIDEBAR_DEFAULT_WIDTH_SECONDARY: f32 = 320.0;
pub const PANEL_MIN_HEIGHT: f32 = 120.0;
pub const PANEL_DEFAULT_HEIGHT: f32 = 180.0;

/// Width the central workspace (the 3D viewport and its content column) is always
/// guaranteed to keep. Instead of capping a sidebar at a fixed pixel width, the
/// window is treated as a split between the sidebars and the workspace: a sidebar
/// may be dragged as wide as the user likes until the workspace would shrink past
/// this floor. On a wide display this lets a sidebar grow far beyond any fixed
/// cap; on a small window the viewport is still protected.
pub const WORKSPACE_MIN_WIDTH: f32 = 360.0;

/// Width of the slim reveal strip that stands in for the right sidebar while it is
/// collapsed — the horizontal space it still occupies when reserving room for the
/// opposite sidebar's growth.
pub const SECONDARY_HANDLE_WIDTH: f32 = 30.0;

/// Maximum width one sidebar may take, given the window width and the horizontal
/// space the *opposite* sidebar currently occupies (`0` when it is hidden). The
/// sidebar may expand until the central workspace would fall below
/// [`WORKSPACE_MIN_WIDTH`]; the result is floored at the sidebar's own minimum so
/// `clamp(min, max)` stays valid (std `clamp` requires `min <= max`) even in a
/// window too narrow to honor the reservation. Subtracting the opposite sidebar's
/// footprint means the two sidebars can never jointly squeeze the workspace below
/// its minimum. Shared by the UI rendering pass and the resize dispatcher; callers
/// usually reach it through [`LayoutState::primary_sidebar_max_width`] /
/// [`LayoutState::secondary_sidebar_max_width`].
pub fn sidebar_max_width(viewport_width: f32, opposite_occupied: f32, own_min: f32) -> f32 {
    (viewport_width - opposite_occupied - WORKSPACE_MIN_WIDTH).max(own_min)
}

impl Default for LayoutState {
    fn default() -> Self {
        Self {
            active_primary_view: PrimaryView::EntryList,
            show_primary_sidebar: true,
            settings_open: false,
            about_open: false,
            monitor_popover_open: false,
            monitor_anchor: None,
            monitor_footer_height: 70.0,
            primary_sidebar_width: SIDEBAR_DEFAULT_WIDTH_PRIMARY,
            dock: DockModel::default(),
        }
    }
}

impl LayoutState {
    /// Horizontal space the primary (left) sidebar lays claim to right now: its
    /// configured width when shown, nothing when hidden. This is the opposite-side
    /// footprint that bounds how wide the secondary sidebar may grow.
    fn primary_occupied_width(&self) -> f32 {
        if self.show_primary_sidebar {
            self.primary_sidebar_width
        } else {
            0.0
        }
    }

    /// Horizontal space the secondary (right) sidebar lays claim to right now: its
    /// configured width when docked, the reveal strip when collapsed, nothing when
    /// empty. This is the opposite-side footprint that bounds how wide the primary
    /// sidebar may grow.
    fn secondary_occupied_width(&self) -> f32 {
        if self.dock.is_visible(DockArea::Right) {
            self.dock.right_width
        } else if self.dock.is_collapsed(DockArea::Right) {
            SECONDARY_HANDLE_WIDTH
        } else {
            0.0
        }
    }

    /// Largest width the primary sidebar may take in a window this wide, leaving
    /// room for the secondary sidebar's footprint and the workspace minimum.
    pub fn primary_sidebar_max_width(&self, viewport_width: f32) -> f32 {
        sidebar_max_width(
            viewport_width,
            self.secondary_occupied_width(),
            SIDEBAR_MIN_WIDTH_PRIMARY,
        )
    }

    /// Largest width the secondary sidebar may take in a window this wide, leaving
    /// room for the primary sidebar's footprint and the workspace minimum.
    pub fn secondary_sidebar_max_width(&self, viewport_width: f32) -> f32 {
        sidebar_max_width(
            viewport_width,
            self.primary_occupied_width(),
            SIDEBAR_MIN_WIDTH_SECONDARY,
        )
    }
}
