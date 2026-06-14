use eframe::egui::{
    self, Align, Button, CursorIcon, Frame, Id, Layout, Margin, Order, Rect, RichText, ScrollArea,
    Sense, Stroke, Ui, Vec2,
};
use egui::viewport::{ResizeDirection, ViewportCommand};

use crate::{
    backend::tasks::task_controllers,
    engines::registry::{EngineId, EngineLaunch},
    frontend::{
        CartoonSectionStyle,
        actions::AppAction,
        services::entry_details,
        state::{
            AppState, EngineDraft, PANEL_MIN_HEIGHT, PrimaryView, SIDEBAR_MIN_WIDTH_PRIMARY,
            SIDEBAR_MIN_WIDTH_SECONDARY, SelectionItem, Side, sidebar_max_width,
        },
    },
};

mod bottom_panel;
mod secondary_sidebar;
mod about;
mod settings_modal;
// Reachable from `state.rs` (which stores `SettingCategory` in `SettingsState`),
// so it can't stay private to this module.
pub(crate) mod settings_registry;
mod settings_representation;
mod style_panel;
mod workspace;

use bottom_panel::render_status_bar;
use secondary_sidebar::render_secondary_sidebar;
use style_panel::render_style_panel;
use workspace::render_workspace;

mod entry_item;
mod entry_list;
mod layout;
mod primary_sidebar;
mod title_bar;
mod views;
mod widgets;

pub(crate) use entry_item::*;
pub(crate) use entry_list::*;
pub(crate) use layout::*;
pub(crate) use primary_sidebar::*;
pub(crate) use title_bar::*;
pub(crate) use views::*;
pub(crate) use widgets::*;

/// Corner radius of the *borderless* main window, in logical points.
///
/// Window-chrome model (the reason for the `cfg`s throughout this file):
/// Windows/Linux run a borderless, transparent window and draw their own chrome
/// — resize handles, rounded title/status-bar corners, and a hairline border.
/// macOS uses the native window frame, which owns resize, the squircle corners,
/// the border, and the shadow, so the app-drawn chrome is skipped there.
#[cfg(not(target_os = "macos"))]
pub(crate) const WINDOW_CORNER_RADIUS: u8 = crate::frontend::theme::radius::WINDOW;

/// Horizontal inset of the title bar's content from the window edges. Shared
/// by the title-bar frame margin and the concentric radius of the core
/// buttons, so the geometry and the math cannot drift apart.
pub(crate) const TITLE_BAR_H_MARGIN: u8 = 8;

pub fn show_workbench(state: &mut AppState, ui: &mut egui::Ui, actions: &mut Vec<AppAction>) {
    let ctx = ui.ctx().clone();
    let pal = crate::frontend::theme::palette(ui);
    // When Liquid Glass is revealed, the perimeter chrome (title/status bars and
    // sidebars) is painted semi-transparent at the user's chosen tint intensity
    // so the window's vibrancy material shows through; the central panel stays
    // opaque (see below). `None` means opaque chrome.
    let glass = state.ui.glass_alpha;

    render_window_resize_handles(&ctx);

    // Ctrl+, (Cmd+, on macOS) toggles the Settings modal — the platform
    // convention for Preferences. `consume_key` so the keystroke doesn't also
    // reach a focused text field. Transient chrome, flipped directly (see
    // `LayoutState::settings_open`).
    if ctx.input_mut(|input| input.consume_key(egui::Modifiers::COMMAND, egui::Key::Comma)) {
        state.ui.layout.settings_open = !state.ui.layout.settings_open;
    }

    // Frame-consistent snapshot of the sidebar visibility: the toggle button,
    // View menu, and settings checkbox can all flip `show_primary_sidebar`
    // mid-frame, and the corner radii, panel order, and the title bar's
    // traffic-light spacer must agree within a single frame.
    let sidebar_visible = state.ui.layout.show_primary_sidebar;

    // Rounded window corners (non-macOS, where the app draws its own chrome).
    // With the full-height sidebar visible it owns the left corners (nw + sw);
    // the title/status bars keep only their right ones. With the sidebar hidden
    // the top and bottom bars span the full width and reclaim all four.
    #[cfg(target_os = "macos")]
    let (sidebar_corners, top_corners, bottom_corners) = (
        egui::CornerRadius::ZERO,
        egui::CornerRadius::ZERO,
        egui::CornerRadius::ZERO,
    );
    // DWM squares the corners when maximized; match it so the corner cutouts
    // don't expose the Acrylic backdrop.
    #[cfg(not(target_os = "macos"))]
    let window_corner_radius = if ctx.input(|input| input.viewport().maximized.unwrap_or(false)) {
        0
    } else {
        WINDOW_CORNER_RADIUS
    };
    #[cfg(not(target_os = "macos"))]
    let (sidebar_corners, top_corners, bottom_corners) = if sidebar_visible {
        (
            egui::CornerRadius {
                nw: window_corner_radius,
                ne: 0,
                sw: window_corner_radius,
                se: 0,
            },
            egui::CornerRadius {
                nw: 0,
                ne: window_corner_radius,
                sw: 0,
                se: 0,
            },
            egui::CornerRadius {
                nw: 0,
                ne: 0,
                sw: 0,
                se: window_corner_radius,
            },
        )
    } else {
        (
            egui::CornerRadius::ZERO,
            egui::CornerRadius {
                nw: window_corner_radius,
                ne: window_corner_radius,
                sw: 0,
                se: 0,
            },
            egui::CornerRadius {
                nw: 0,
                ne: 0,
                sw: window_corner_radius,
                se: window_corner_radius,
            },
        )
    };

    // Sidebars are fixed-width panels driven by our own proximity-revealed
    // resize dividers (wired after the central panel; see `render_resize_divider`).
    // egui's native resize is off (`resizable(false)`) so it never paints the
    // harsh full-height hover line, and `show_separator_line(false)` hands the
    // at-rest hairline to our overlay too. `exact_size` also dodges egui's
    // resizable-panel growth bug (the same reason the bottom panel uses it).
    // Transient per-frame panel widths used to place the resize divider, the
    // central column, and the bottom panel flush with each sidebar's edge. The
    // sidebar content is pinned to the exact width by `render_pinned` (see its
    // doc comment) so a wide widget can't push the panel's content rect — and thus
    // egui's placement of the central column and the bottom panel nested in it —
    // out past the requested edge. With overflow pinned away the rendered width is
    // always the requested `width`, so we key everything off `width` directly.
    // Seed these with the stored width so that if a panel is toggled on mid-frame
    // (e.g. the bottom panel's "Open Tasks" button, which runs after this block) the
    // divider falls back to a sane position rather than the activity-bar edge.
    let mut primary_rendered_w = state.ui.layout.primary_sidebar_width;
    let mut secondary_rendered_w = state.ui.layout.secondary_sidebar_width;

    // The primary sidebar is added FIRST so it spans the full window height —
    // the macOS 27 edge-to-edge sidebar: it reaches the window's top and bottom
    // edges, the traffic lights overlay its header strip, and the title/status
    // bars span only from its right edge to the window's right edge. (No
    // separate vertical activity rail: the primary-view switcher lives in the
    // sidebar's header strip — see `render_primary_sidebar` — and the title bar
    // carries the show/hide toggle.)
    if sidebar_visible {
        let max_w = sidebar_max_width(ctx.viewport_rect().width());
        // Clamp to the displayable range for this frame; the stored value is
        // intentionally NOT written back so the user's desired width is
        // preserved when the window is later widened again.
        let width = state
            .ui
            .layout
            .primary_sidebar_width
            .clamp(SIDEBAR_MIN_WIDTH_PRIMARY, max_w);
        egui::Panel::left("primary_sidebar")
            .resizable(false)
            .exact_size(width)
            .show_separator_line(false)
            .frame(
                Frame::default()
                    .fill(crate::frontend::theme::chrome_fill(pal.sidebar, glass))
                    .corner_radius(sidebar_corners)
                    // No top margin: the 32px header strip (aligned with the
                    // title bar band) manages its own padding.
                    .inner_margin(Margin {
                        left: 10,
                        right: 2,
                        top: 0,
                        bottom: 10,
                    }),
            )
            .show_inside(ui, |ui| {
                render_pinned(ui, |ui| render_primary_sidebar(state, ui, actions));
            });
        primary_rendered_w = width;
    }

    egui::Panel::top("title_bar")
        .exact_size(32.0)
        .show_separator_line(false)
        .frame(
            Frame::default()
                .fill(crate::frontend::theme::chrome_fill(pal.title_bar, glass))
                .corner_radius(top_corners)
                .inner_margin(Margin::symmetric(TITLE_BAR_H_MARGIN as i8, 3)),
        )
        .show_inside(ui, |ui| render_title_bar(state, ui, actions));

    egui::Panel::bottom("status_bar")
        .exact_size(24.0)
        .frame(
            Frame::default()
                .fill(crate::frontend::theme::chrome_fill(pal.status_bar, glass))
                .corner_radius(bottom_corners)
                .inner_margin(Margin::symmetric(10, 3)),
        )
        .show_inside(ui, |ui| render_status_bar(state, ui));

    if state.ui.layout.show_secondary_sidebar {
        let max_w = sidebar_max_width(ctx.viewport_rect().width());
        let width = state
            .ui
            .layout
            .secondary_sidebar_width
            .clamp(SIDEBAR_MIN_WIDTH_SECONDARY, max_w);
        egui::Panel::right("secondary_sidebar")
            .resizable(false)
            .exact_size(width)
            .show_separator_line(false)
            .frame(
                Frame::default()
                    .fill(crate::frontend::theme::chrome_fill(pal.sidebar, glass))
                    .inner_margin(Margin {
                        left: 10,
                        right: 2,
                        top: 10,
                        bottom: 10,
                    }),
            )
            .show_inside(ui, |ui| {
                render_pinned(ui, |ui| render_secondary_sidebar(state, ui, actions));
            });
        secondary_rendered_w = width;
    }

    egui::CentralPanel::default()
        .frame(
            Frame::default()
                .fill(pal.central)
                .inner_margin(Margin::same(0)),
        )
        .show_inside(ui, |ui| render_workspace(state, ui, actions));

    // The bottom panel is fixed-size (see `render_workspace`) to avoid egui's
    // resizable-panel growth bug, so it gets a custom resize handle — a subtle
    // centered pill on hover that drives `panel_height`. Its horizontal divider
    // shares no edge with a scroll bar, so there's no grab conflict. (Sidebars
    // use egui's native resize above.)
    if state.ui.layout.show_panel {
        let viewport_rect = ctx.viewport_rect();
        let content_bottom = viewport_rect.bottom() - 24.0; // above the status bar
        // Use the panels' *rendered* widths (see the note above the sidebar panels)
        // so the bottom-panel divider stays flush with the central column.
        let workspace_left = viewport_rect.left()
            + if state.ui.layout.show_primary_sidebar {
                primary_rendered_w
            } else {
                0.0
            };
        let workspace_right = viewport_rect.right()
            - if state.ui.layout.show_secondary_sidebar {
                secondary_rendered_w
            } else {
                0.0
            };
        let y = content_bottom - state.ui.layout.panel_height;
        let max_panel_height = (viewport_rect.height() * 0.6).max(160.0);
        // Inset the grab strip past the sidebar dividers (which now run full
        // height at workspace_left / workspace_right) so the bottom corners
        // aren't an ambiguous two-axis drag target.
        match render_resize_divider(
            &ctx,
            "bottom_panel_resize",
            DividerKind::Horizontal,
            Rect::from_min_max(
                egui::pos2(workspace_left + DIVIDER_GRAB_HALF_WIDTH, y - 4.0),
                egui::pos2(workspace_right - DIVIDER_GRAB_HALF_WIDTH, y + 4.0),
            ),
            y,
            DividerConfig {
                sign: -1.0,
                min: PANEL_MIN_HEIGHT,
                max: max_panel_height,
            },
            &pal,
        ) {
            DividerEffect::Delta(d) => actions.push(AppAction::ResizePanel(d)),
            DividerEffect::Reset => actions.push(AppAction::ResetPanel),
            DividerEffect::None => {}
        }
    }

    // Sidebar resize dividers — proximity-revealed, matching the bottom panel.
    // Drawn over the central panel so the soft bar floats on the shared edge;
    // the panels themselves are fixed-width (see above).
    {
        let vp = ctx.viewport_rect();
        let content_top = vp.top() + 32.0; // below the title bar
        let content_bottom = vp.bottom() - 24.0; // above the status bar
        // The sidebar spans the full content height — the bottom panel is nested
        // inside the central column (to the right of the sidebar), not under it —
        // so its resize divider runs the whole way down to the status bar rather
        // than stopping at the bottom panel's top edge.
        let divider_bottom = content_bottom;
        let max_w = sidebar_max_width(vp.width());
        // Keyed off the same frame-start snapshot as the panel itself: the title
        // bar's toggle (rendered in between) can flip the live flag mid-frame,
        // and the divider must match the sidebar actually drawn this frame.
        if sidebar_visible {
            // Draw the divider at the panel's rendered edge (see the note above the
            // sidebar panels); drag emits AppAction::ResizeSidebar which the
            // dispatcher applies to the stored `primary_sidebar_width`.
            // The primary sidebar is edge-to-edge (full window height), so its
            // divider runs from the window's top edge to its bottom edge rather
            // than stopping at the title/status bars.
            let line_x = vp.left() + primary_rendered_w;
            match render_resize_divider(
                &ctx,
                "primary_sidebar_resize",
                DividerKind::Vertical,
                Rect::from_min_max(
                    egui::pos2(line_x - DIVIDER_GRAB_HALF_WIDTH, vp.top()),
                    egui::pos2(line_x + DIVIDER_GRAB_HALF_WIDTH, vp.bottom()),
                ),
                line_x,
                DividerConfig {
                    sign: 1.0,
                    min: SIDEBAR_MIN_WIDTH_PRIMARY,
                    max: max_w,
                },
                &pal,
            ) {
                DividerEffect::Delta(d) => actions.push(AppAction::ResizeSidebar(Side::Primary, d)),
                DividerEffect::Reset => actions.push(AppAction::ResetSidebar(Side::Primary)),
                DividerEffect::None => {}
            }
        }
        if state.ui.layout.show_secondary_sidebar {
            let line_x = vp.right() - secondary_rendered_w;
            match render_resize_divider(
                &ctx,
                "secondary_sidebar_resize",
                DividerKind::Vertical,
                Rect::from_min_max(
                    egui::pos2(line_x - DIVIDER_GRAB_HALF_WIDTH, content_top),
                    egui::pos2(line_x + DIVIDER_GRAB_HALF_WIDTH, divider_bottom),
                ),
                line_x,
                DividerConfig {
                    sign: -1.0,
                    min: SIDEBAR_MIN_WIDTH_SECONDARY,
                    max: max_w,
                },
                &pal,
            ) {
                DividerEffect::Delta(d) => {
                    actions.push(AppAction::ResizeSidebar(Side::Secondary, d))
                }
                DividerEffect::Reset => actions.push(AppAction::ResetSidebar(Side::Secondary)),
                DividerEffect::None => {}
            }
        }
    }

    // Hairline border hugging the rounded window. Painted last so it sits atop
    // the panel fills; `StrokeKind::Inside` keeps the full 1px within the window
    // so it isn't clipped at the physical edge.
    #[cfg(not(target_os = "macos"))]
    ui.painter().rect_stroke(
        ctx.viewport_rect(),
        egui::CornerRadius::same(window_corner_radius),
        ctx.global_style().visuals.window_stroke,
        egui::StrokeKind::Inside,
    );

    render_structure_editor_window(state, actions, &ctx);
    crate::frontend::sketcher::render_sketcher_window(state, actions, &ctx);
    render_pdb_fetch_window(state, actions, &ctx);
    render_text_viewer_window(state, &ctx);
    settings_modal::show(state, &ctx, actions);
    about::show(state, &ctx, actions);
}
