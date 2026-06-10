use super::*;

pub(crate) const WINDOW_RESIZE_HANDLE_THICKNESS: f32 = 6.0;

pub(crate) const WINDOW_RESIZE_CORNER_SIZE: f32 = 18.0;

pub(crate) fn render_window_resize_handles(ctx: &egui::Context) {
    // Runtime guard rather than a cfg'd call site: this keeps the helper types
    // below referenced on macOS (which has no app-drawn handles) so they don't
    // trip dead-code warnings.
    if cfg!(target_os = "macos") {
        return;
    }

    let maximized = ctx.input(|input| input.viewport().maximized.unwrap_or(false));
    if maximized {
        return;
    }

    let viewport_rect = ctx.viewport_rect();
    let handle = WINDOW_RESIZE_HANDLE_THICKNESS;
    let corner = WINDOW_RESIZE_CORNER_SIZE;

    for spec in [
        ResizeHandleSpec::new(
            "north_west",
            Rect::from_min_size(viewport_rect.min, egui::vec2(corner, corner)),
            ResizeDirection::NorthWest,
            CursorIcon::ResizeNorthWest,
        ),
        ResizeHandleSpec::new(
            "north_east",
            Rect::from_min_size(
                egui::pos2(viewport_rect.right() - corner, viewport_rect.top()),
                egui::vec2(corner, corner),
            ),
            ResizeDirection::NorthEast,
            CursorIcon::ResizeNorthEast,
        ),
        ResizeHandleSpec::new(
            "south_west",
            Rect::from_min_size(
                egui::pos2(viewport_rect.left(), viewport_rect.bottom() - corner),
                egui::vec2(corner, corner),
            ),
            ResizeDirection::SouthWest,
            CursorIcon::ResizeSouthWest,
        ),
        ResizeHandleSpec::new(
            "south_east",
            Rect::from_min_size(
                egui::pos2(
                    viewport_rect.right() - corner,
                    viewport_rect.bottom() - corner,
                ),
                egui::vec2(corner, corner),
            ),
            ResizeDirection::SouthEast,
            CursorIcon::ResizeSouthEast,
        ),
        ResizeHandleSpec::new(
            "north",
            Rect::from_min_max(
                egui::pos2(viewport_rect.left() + corner, viewport_rect.top()),
                egui::pos2(viewport_rect.right() - corner, viewport_rect.top() + handle),
            ),
            ResizeDirection::North,
            CursorIcon::ResizeNorth,
        ),
        ResizeHandleSpec::new(
            "south",
            Rect::from_min_max(
                egui::pos2(
                    viewport_rect.left() + corner,
                    viewport_rect.bottom() - handle,
                ),
                egui::pos2(viewport_rect.right() - corner, viewport_rect.bottom()),
            ),
            ResizeDirection::South,
            CursorIcon::ResizeSouth,
        ),
        ResizeHandleSpec::new(
            "west",
            Rect::from_min_max(
                egui::pos2(viewport_rect.left(), viewport_rect.top() + corner),
                egui::pos2(
                    viewport_rect.left() + handle,
                    viewport_rect.bottom() - corner,
                ),
            ),
            ResizeDirection::West,
            CursorIcon::ResizeWest,
        ),
        ResizeHandleSpec::new(
            "east",
            Rect::from_min_max(
                egui::pos2(viewport_rect.right() - handle, viewport_rect.top() + corner),
                egui::pos2(viewport_rect.right(), viewport_rect.bottom() - corner),
            ),
            ResizeDirection::East,
            CursorIcon::ResizeEast,
        ),
    ] {
        render_resize_handle(ctx, spec);
    }
}

#[derive(Clone, Copy)]
pub(crate) struct ResizeHandleSpec {
    id: &'static str,
    rect: Rect,
    direction: ResizeDirection,
    cursor_icon: CursorIcon,
}

impl ResizeHandleSpec {
    const fn new(
        id: &'static str,
        rect: Rect,
        direction: ResizeDirection,
        cursor_icon: CursorIcon,
    ) -> Self {
        Self {
            id,
            rect,
            direction,
            cursor_icon,
        }
    }
}

pub(crate) fn render_resize_handle(ctx: &egui::Context, spec: ResizeHandleSpec) {
    egui::Area::new(Id::new(spec.id))
        .order(Order::Foreground)
        .fixed_pos(spec.rect.min)
        .interactable(true)
        .show(ctx, |ui| {
            let (_, response) = ui.allocate_exact_size(spec.rect.size(), Sense::click_and_drag());
            if response.hovered() || response.dragged() {
                ui.ctx().set_cursor_icon(spec.cursor_icon);
            }
            if response.drag_started() {
                ui.ctx()
                    .send_viewport_cmd(ViewportCommand::BeginResize(spec.direction));
            }
        });
}

/// Whether a resize divider runs vertically (between side-by-side panels, drags
/// horizontally) or horizontally (between stacked panels, drags vertically).
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum DividerKind {
    Vertical,
    Horizontal,
}

/// Proximity-reveal tuning for the resize dividers (sidebars + bottom panel).
/// How near the pointer must come (in points) before the bar begins to fade in.
pub(crate) const DIVIDER_PROXIMITY_RADIUS: f32 = 24.0;

/// Half-width of the slim interactive grab strip centered on the divider line.
pub(crate) const DIVIDER_GRAB_HALF_WIDTH: f32 = 4.0;

/// Fade in/out duration for the indicator bar.
pub(crate) const DIVIDER_FADE_SECONDS: f32 = 0.15;

/// Alpha of the always-visible at-rest separator hairline (subtle light gray, so
/// the sidebar keeps a clear edge and doesn't read as detached from the content).
pub(crate) const DIVIDER_REST_ALPHA: u8 = 180;

/// Alpha of the bar when revealed on approach.
pub(crate) const DIVIDER_ACTIVE_ALPHA: u8 = 220;

/// Width of the fully-revealed bar (it thins to 1 px at rest).
pub(crate) const DIVIDER_BAR_WIDTH: f32 = 2.0;

/// Parameters for a proximity-revealed resize divider, grouped to keep the call
/// sites readable. `sign` is `1.0` for left/top panels (drag away from center
/// increases size) and `-1.0` for right/bottom panels. `min`/`max` are used to
/// pre-clip drag deltas to the panel's full range; the dispatcher applies the
/// authoritative clamp on the stored value.
pub(crate) struct DividerConfig {
    pub(crate) sign: f32,
    pub(crate) min: f32,
    pub(crate) max: f32,
}

/// What a resize divider interaction produced this frame.
pub(crate) enum DividerEffect {
    /// No interaction.
    None,
    /// Drag: a signed delta in the "grows the panel" direction (`sign * screen_delta`),
    /// pre-clipped to the panel's full range. The dispatcher clamps the stored value.
    Delta(f32),
    /// Double-click: reset to the panel's default (known to the caller/dispatcher).
    Reset,
}

/// Interactive resize handle for a panel divider, Claude-style: a faint hairline
/// at rest that fades into a soft, theme-inverting indicator bar as the pointer
/// nears it (within `DIVIDER_PROXIMITY_RADIUS`) — hinting that the edge is
/// draggable without the harsh full-height line egui's native resize paints.
///
/// `hit_rect` is the slim `Sense::click_and_drag` strip (`±DIVIDER_GRAB_HALF_WIDTH`
/// around the line, spanning its full length) — narrow so it never steals clicks
/// from panel content or overlaps the scroll bar. `divider` is the on-screen
/// position of the line (x for a vertical divider, y for a horizontal one) where
/// the bar is painted. Returns a `DividerEffect` that the caller maps to an
/// `AppAction`; the stored panel dimension is only mutated by the dispatcher.
pub(crate) fn render_resize_divider(
    ctx: &egui::Context,
    id: &str,
    kind: DividerKind,
    hit_rect: Rect,
    divider: f32,
    config: DividerConfig,
    pal: &crate::frontend::theme::Palette,
) -> DividerEffect {
    // Proximity is a wider band than the grab strip: the bar reveals as the
    // pointer approaches, but only the slim strip senses drags/clicks.
    let proximity = ctx
        .input(|i| i.pointer.hover_pos())
        .is_some_and(|p| match kind {
            DividerKind::Vertical => {
                (p.x - divider).abs() <= DIVIDER_PROXIMITY_RADIUS
                    && p.y >= hit_rect.top()
                    && p.y <= hit_rect.bottom()
            }
            DividerKind::Horizontal => {
                (p.y - divider).abs() <= DIVIDER_PROXIMITY_RADIUS
                    && p.x >= hit_rect.left()
                    && p.x <= hit_rect.right()
            }
        });
    let mut effect = DividerEffect::None;
    // Middle, not Foreground: above the panels the divider separates, but below
    // floating windows (Settings), so the line never draws across a dialog.
    egui::Area::new(Id::new(id))
        .order(Order::Middle)
        .fixed_pos(hit_rect.min)
        .interactable(true)
        .show(ctx, |ui| {
            let (_, response) = ui.allocate_exact_size(hit_rect.size(), Sense::click_and_drag());
            if response.hovered() || response.dragged() {
                ui.ctx().set_cursor_icon(match kind {
                    DividerKind::Vertical => CursorIcon::ResizeHorizontal,
                    DividerKind::Horizontal => CursorIcon::ResizeVertical,
                });
            }
            if response.double_clicked() {
                effect = DividerEffect::Reset;
            } else if response.dragged() {
                let raw = match kind {
                    DividerKind::Vertical => response.drag_delta().x,
                    DividerKind::Horizontal => response.drag_delta().y,
                };
                // Pre-clip the delta to the panel's full range so a fast drag
                // never produces an overshoot larger than the entire extent;
                // the dispatcher applies the authoritative clamp on the stored value.
                let range = config.max - config.min;
                effect = DividerEffect::Delta((config.sign * raw).clamp(-range, range));
            }
            // Fade the bar in on approach / drag and out when the pointer leaves;
            // `animate_bool_with_time` self-requests repaints while in flight.
            let reveal = ui.ctx().animate_bool_with_time(
                Id::new((id, "reveal")),
                proximity || response.dragged(),
                DIVIDER_FADE_SECONDS,
            );
            let mut alpha = egui::lerp(
                DIVIDER_REST_ALPHA as f32..=DIVIDER_ACTIVE_ALPHA as f32,
                reveal,
            );
            if response.dragged() {
                alpha = (alpha + 20.0).min(245.0);
            }
            let thickness = egui::lerp(1.0..=DIVIDER_BAR_WIDTH, reveal);
            // Light-gray `hairline` tone (not the darker neutral tint) kept faint:
            // a soft pale-gray line on light, a soft lighter-than-bg line on dark.
            let [hr, hg, hb, _] = pal.hairline.to_array();
            let color = egui::Color32::from_rgba_unmultiplied(hr, hg, hb, alpha.round() as u8);
            let bar = match kind {
                DividerKind::Vertical => Rect::from_center_size(
                    egui::pos2(divider, hit_rect.center().y),
                    egui::vec2(thickness, hit_rect.height()),
                ),
                DividerKind::Horizontal => Rect::from_center_size(
                    egui::pos2(hit_rect.center().x, divider),
                    egui::vec2(hit_rect.width(), thickness),
                ),
            };
            ui.painter()
                .rect_filled(bar, egui::CornerRadius::same(1), color);
        });
    effect
}
