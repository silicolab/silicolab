use super::*;

/// A one-dimensional spring, integrated once per frame, used to glide the
/// view-switcher's selection pill the way macOS animates its controls: a quick,
/// responsive start easing into a soft settle. The spring's position and
/// velocity live in egui's frame-local data keyed by `id`, so re-clicking a
/// segment mid-slide simply redirects the carried momentum toward the new
/// target instead of restarting from a standstill. Returns this frame's eased
/// position and requests a repaint until the spring comes to rest.
#[derive(Clone, Copy)]
struct Spring {
    pos: f32,
    vel: f32,
}

fn animate_spring(
    ctx: &egui::Context,
    id: Id,
    target: f32,
    dt: f32,
    response: f32,
    damping: f32,
) -> f32 {
    // SwiftUI parameterisation → physical spring constants: `response` sets the
    // natural frequency, `damping` the fraction of critical damping.
    let omega = std::f32::consts::TAU / response.max(1e-4);
    let k = omega * omega;
    let c = 2.0 * damping * omega;
    let mut s = ctx
        .data_mut(|d| d.get_temp::<Spring>(id))
        .unwrap_or(Spring {
            pos: target,
            vel: 0.0,
        });
    // Semi-implicit Euler; clamp dt so a stalled frame can't make the step blow up.
    let dt = dt.clamp(0.0, 1.0 / 30.0);
    let accel = -k * (s.pos - target) - c * s.vel;
    s.vel += accel * dt;
    s.pos += s.vel * dt;
    if (s.pos - target).abs() < 5e-4 && s.vel.abs() < 5e-4 {
        s.pos = target;
        s.vel = 0.0;
    } else {
        ctx.request_repaint();
    }
    ctx.data_mut(|d| d.insert_temp(id, s));
    s.pos
}

pub(crate) fn render_primary_sidebar(
    state: &mut AppState,
    ui: &mut egui::Ui,
    actions: &mut Vec<AppAction>,
) {
    // Selection-pill spring (SwiftUI-style: `response` ≈ settle time, `damping`
    // ≈ fraction of critical). ~0.34s / 0.82 reads as a snappy macOS control
    // glide — quick off the mark, soft into place, a barely-there overshoot.
    const PILL_SPRING_RESPONSE: f32 = 0.34;
    const PILL_SPRING_DAMPING: f32 = 0.82;
    // Panel body fade is a short fixed ease; kept under ~0.2s so it never drags.
    const CONTENT_FADE_SECONDS: f32 = 0.18;

    let pal = crate::frontend::theme::palette(ui);
    // Header strip (Claude Desktop-style): just the native traffic lights and
    // the sidebar-hide toggle — the view switcher lives in the segmented
    // control below, not crammed beside the lights. The sidebar panel has no
    // top margin, so the strip starts at the window's top edge; on macOS the
    // traffic lights overlay its left end. No hairline: the sidebar reads as
    // one continuous surface, with the toolbar's separator confined to the
    // content area.
    let strip_rect = Rect::from_min_size(
        ui.max_rect().min,
        egui::vec2(ui.max_rect().width(), SIDEBAR_HEADER_HEIGHT),
    );
    let icons_rect = ui
        .allocate_ui_with_layout(
            strip_rect.size(),
            Layout::left_to_right(Align::Center),
            |ui| {
                // Clear the native traffic lights; the sidebar's 10px left
                // margin already supplies part of the inset.
                #[cfg(target_os = "macos")]
                ui.add_space(MACOS_TRAFFIC_LIGHTS_WIDTH - 10.0);
                if with_core_button_style(ui, false, |ui| {
                    ui.add_sized(
                        SIDEBAR_HEADER_BUTTON_SIZE,
                        Button::new(
                            RichText::new(egui_phosphor::regular::SIDEBAR_SIMPLE)
                                .size(SIDEBAR_HEADER_ICON_SIZE)
                                .color(core_button_text_color(&pal, false)),
                        ),
                    )
                })
                .on_hover_text("Hide sidebar")
                .clicked()
                {
                    state.ui.layout.show_primary_sidebar = false;
                }
                let search_selected =
                    state.ui.entry_list.search_open || sidebar_search_active(state);
                if with_core_button_style(ui, search_selected, |ui| {
                    ui.add_sized(
                        SIDEBAR_HEADER_BUTTON_SIZE,
                        Button::new(
                            RichText::new(egui_phosphor::regular::MAGNIFYING_GLASS)
                                .size(SIDEBAR_HEADER_ICON_SIZE)
                                .color(core_button_text_color(&pal, search_selected)),
                        ),
                    )
                })
                .on_hover_text(sidebar_search_placeholder(state))
                .clicked()
                {
                    state.ui.entry_list.search_open = !state.ui.entry_list.search_open;
                }
                ui.min_rect()
            },
        )
        .inner;

    if state.ui.entry_list.search_open {
        render_sidebar_search_popover(state, ui, strip_rect, icons_rect);
    }

    // The strip's blank areas drag the window, like the title bar (on macOS the
    // leading zone sits under the traffic lights, whose native buttons consume
    // their own clicks — the gaps still drag, matching native sidebars). The
    // zones exclude the toggle button so it keeps its clicks.
    let leading = Rect::from_min_max(
        strip_rect.min,
        egui::pos2(
            icons_rect
                .left()
                .clamp(strip_rect.left(), strip_rect.right()),
            strip_rect.bottom(),
        ),
    );
    let trailing = Rect::from_min_max(
        egui::pos2(
            icons_rect
                .right()
                .clamp(strip_rect.left(), strip_rect.right()),
            strip_rect.top(),
        ),
        strip_rect.max,
    );
    for (id, rect) in [
        ("sidebar_header_drag_lead", leading),
        ("sidebar_header_drag_trail", trailing),
    ] {
        if rect.width() <= 0.0 {
            continue;
        }
        let response = ui.interact(rect, Id::new(id), Sense::click_and_drag());
        if response.drag_started() {
            ui.ctx().send_viewport_cmd(ViewportCommand::StartDrag);
        }
        if response.double_clicked() && !cfg!(target_os = "macos") {
            let maximized = ui.input(|i| i.viewport().maximized.unwrap_or(false));
            ui.ctx()
                .send_viewport_cmd(ViewportCommand::Maximized(!maximized));
        }
    }
    ui.add_space(6.0);

    // Segmented view switcher, Claude Desktop-style: a rounded low-contrast
    // track holding one equal-width segment per primary view. The active
    // segment reads as a raised card (white in light mode) carrying its icon
    // and a compact label; inactive segments are muted icon-only.
    {
        let dark = ui.visuals().dark_mode;
        let card_fill = if dark {
            pal.item_fill_active
        } else {
            egui::Color32::WHITE
        };
        // Concentric corners: the track's radius is the segment card's radius
        // plus the 3px inner margin, so both curves share a center.
        const SEGMENT_INSET: u8 = 3;
        const SEGMENT_TRACK_HEIGHT: f32 = 34.0;
        // The primary sidebar frame uses asymmetric content margins (10px left,
        // 2px right) so wide content aligns with the resize divider. Keep this
        // chrome left-aligned in the content box and trim 8px from the right,
        // yielding equal 10px outer margins relative to the sidebar.
        const SEGMENT_TRACK_MARGIN_COMPENSATION: f32 = 8.0;
        let available_width = ui.available_width();
        let track_width = (available_width - SEGMENT_TRACK_MARGIN_COMPENSATION).max(0.0);
        let (slot_rect, _) = ui.allocate_exact_size(
            egui::vec2(available_width, SEGMENT_TRACK_HEIGHT),
            Sense::hover(),
        );
        let track_rect =
            Rect::from_min_size(slot_rect.min, egui::vec2(track_width, SEGMENT_TRACK_HEIGHT));
        let track_radius =
            egui::CornerRadius::same(crate::frontend::theme::radius::CONTROL + SEGMENT_INSET);
        let segment_radius = egui::CornerRadius::same(crate::frontend::theme::radius::CONTROL);
        let painter = ui.painter();
        painter.rect_filled(
            track_rect,
            track_radius,
            pal.neutral_overlay(if dark { 26 } else { 14 }),
        );

        let inner_rect = track_rect.shrink(f32::from(SEGMENT_INSET));
        let views = PrimaryView::all();
        let segment_count = views.len() as f32;
        let slot_w = inner_rect.width() / segment_count;

        // Sliding selection pill: a single slot-wide card springs to the active
        // segment rather than snapping or sliding at constant speed. The spring
        // eases out like a macOS control and absorbs mid-slide re-clicks via its
        // carried velocity.
        let active_index = views
            .iter()
            .position(|v| *v == state.ui.layout.active_primary_view)
            .unwrap_or(0);
        let dt = ui.input(|i| i.stable_dt);
        let pill_pos = animate_spring(
            ui.ctx(),
            Id::new("primary_view_pill"),
            active_index as f32,
            dt,
            PILL_SPRING_RESPONSE,
            PILL_SPRING_DAMPING,
        );
        let card_rect = Rect::from_min_size(
            egui::pos2(inner_rect.left() + pill_pos * slot_w, inner_rect.top()),
            egui::vec2(slot_w, inner_rect.height()),
        );
        painter.rect(
            card_rect,
            segment_radius,
            card_fill,
            Stroke::new(1.0, pal.hairline),
            egui::StrokeKind::Inside,
        );

        const ICON_LABEL_GAP: f32 = 6.0;
        for (index, view) in views.iter().enumerate() {
            let left = egui::lerp(
                inner_rect.left()..=inner_rect.right(),
                index as f32 / segment_count,
            );
            let right = egui::lerp(
                inner_rect.left()..=inner_rect.right(),
                (index + 1) as f32 / segment_count,
            );
            let rect = Rect::from_min_max(
                egui::pos2(left, inner_rect.top()),
                egui::pos2(right, inner_rect.bottom()),
            );
            let response = ui.interact(
                rect,
                Id::new(("primary_view_segment", view.label())),
                Sense::click(),
            );
            let selected = state.ui.layout.active_primary_view == *view;

            // A segment's label and icon tint track how close the sliding pill
            // is to resting on it: full when centered, fading as it departs.
            // This cross-fades the leaving and arriving labels during the slide.
            let label_t = (1.0 - (pill_pos - index as f32).abs()).clamp(0.0, 1.0);

            // Hover wash only on idle segments the pill is not over, so it never
            // double-draws under the card.
            if !selected && label_t < 0.5 && response.hovered() {
                painter.rect_filled(rect, segment_radius, pal.neutral_overlay(14));
            }

            let icon_galley = painter.layout_no_wrap(
                view.icon().to_owned(),
                egui::FontId::proportional(14.0),
                lerp_color(pal.text_muted, pal.text_strong, label_t),
            );
            let label_galley = painter.layout_no_wrap(
                view.short_label().to_owned(),
                egui::FontId::proportional(12.5),
                pal.text_strong.gamma_multiply(label_t),
            );
            let icon_w = icon_galley.size().x;
            let label_w = label_galley.size().x;
            // Keep the icon centered when no label shows and recenter the
            // icon+label pair as the label fades in, so idle segments reserve no
            // label gap and the icon never sits visibly off-center.
            let content_w = egui::lerp(icon_w..=(icon_w + ICON_LABEL_GAP + label_w), label_t);
            let start_x = rect.center().x - content_w / 2.0;
            painter.galley(
                egui::pos2(start_x, rect.center().y - icon_galley.size().y / 2.0),
                icon_galley,
                egui::Color32::PLACEHOLDER,
            );
            if label_t > 0.01 {
                painter.galley(
                    egui::pos2(
                        start_x + icon_w + ICON_LABEL_GAP,
                        rect.center().y - label_galley.size().y / 2.0,
                    ),
                    label_galley,
                    egui::Color32::PLACEHOLDER,
                );
            }

            if !selected {
                response.clone().on_hover_text(view.label());
            }
            if response.clicked() {
                state.ui.layout.active_primary_view = *view;
            }
        }
    }
    ui.add_space(8.0);

    // Panel content transition: the incoming view fades in with a small upward
    // settle. Each frame the inactive views are driven back toward 0 so that
    // returning to a previously shown view always replays the fade instead of
    // appearing instantly at full opacity (egui retains the last animated value
    // per id). Cross-dissolving the outgoing panel is intentionally avoided —
    // stacking two interactive, scrollable panels in immediate mode is fragile —
    // so only the arriving panel animates.
    let active = state.ui.layout.active_primary_view;
    let appear = ui.ctx().animate_bool_with_time_and_easing(
        Id::new(("primary_view_content", active.label())),
        true,
        CONTENT_FADE_SECONDS,
        egui::emath::easing::cubic_out,
    );
    for view in PrimaryView::all() {
        if *view != active {
            ui.ctx().animate_bool_with_time_and_easing(
                Id::new(("primary_view_content", view.label())),
                false,
                CONTENT_FADE_SECONDS,
                egui::emath::easing::cubic_out,
            );
        }
    }
    // Reserve a fixed footer for the system monitor when it is enabled, so the
    // active view's scroll area stops above it instead of pushing it off-screen
    // (render_pinned clips this panel to a fixed rect). The view renders into the
    // region above the footer; the compact monitor renders into the strip below.
    let show_monitor = state.config.show_utilization_bars;
    // Reserve the footer height measured last frame (seeded with a one-GPU
    // estimate). Measuring instead of guessing keeps every GPU row visible
    // regardless of how many cards the machine has.
    let footer_h = if show_monitor {
        state.ui.layout.monitor_footer_height
    } else {
        0.0
    };
    let full = ui.available_rect_before_wrap();
    let content_rect = Rect::from_min_max(
        full.min,
        egui::pos2(full.max.x, (full.max.y - footer_h).max(full.min.y)),
    );

    let mut content_ui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(content_rect)
            .layout(Layout::top_down(Align::Min)),
    );
    content_ui.set_clip_rect(content_rect.intersect(ui.clip_rect()));
    content_ui.scope(|ui| {
        ui.set_opacity(appear);
        if appear < 1.0 {
            ui.add_space((1.0 - appear) * 6.0);
        }
        match active {
            PrimaryView::EntryList => render_entry_list(state, ui, actions),
            PrimaryView::Tasks => render_tasks_view(state, ui, actions),
            PrimaryView::Style => render_style_panel(state, ui, actions),
        }
    });

    if show_monitor {
        let footer_rect =
            Rect::from_min_max(egui::pos2(full.min.x, full.max.y - footer_h), full.max);
        let mut footer_ui = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(footer_rect)
                .layout(Layout::top_down(Align::Min)),
        );
        render_sidebar_monitor_footer(state, &mut footer_ui, actions);
        // Cache the actual rendered height for next frame's reservation; repaint
        // once when it changes so a newly-correct height is applied without a
        // visible clip. The threshold avoids a perpetual repaint loop.
        let measured = footer_ui.min_rect().height();
        if (measured - state.ui.layout.monitor_footer_height).abs() > 0.5 {
            state.ui.layout.monitor_footer_height = measured;
            ui.ctx().request_repaint();
        }
    }
}

/// Render the system-monitor footer: a hairline divider above the full-width
/// stack of compact utilization bars. Only called when the monitor is enabled.
fn render_sidebar_monitor_footer(
    state: &mut AppState,
    ui: &mut egui::Ui,
    actions: &mut Vec<AppAction>,
) {
    let pal = crate::frontend::theme::palette(ui);
    let top = ui.max_rect().left_top();
    let right = ui.max_rect().right_top().x;
    ui.painter()
        .hline(top.x..=right, top.y, Stroke::new(1.0, pal.hairline));
    ui.add_space(9.0);
    // Inset the right edge so the gauges sit with equal ~10px margins inside the
    // sidebar (the panel's own inner margins are 10 left / 2 right): this keeps
    // the right-aligned values off the border and centers the cluster — and with
    // it the detail popover, which tracks this cluster's width.
    egui::Frame::default()
        .inner_margin(egui::Margin {
            left: 0,
            right: 8,
            top: 0,
            bottom: 0,
        })
        .show(ui, |ui| {
            panel_bodies::render_compact_monitor(state, ui, actions);
        });
}

pub(crate) fn render_sidebar_search_popover(
    state: &mut AppState,
    ui: &mut Ui,
    strip_rect: Rect,
    icons_rect: Rect,
) {
    let ctx = ui.ctx().clone();
    let pal = crate::frontend::theme::palette(ui);
    let left = icons_rect.left().max(strip_rect.left() + 8.0);
    let width = (strip_rect.right() - left - 10.0).clamp(150.0, 260.0);
    let pos = egui::pos2(left, strip_rect.bottom() + 4.0);

    {
        let (placeholder, query) = sidebar_search_query_mut(state);
        egui::Area::new(Id::new("sidebar_search_popover"))
            .order(Order::Foreground)
            .fixed_pos(pos)
            .show(&ctx, |ui| {
                Frame::popup(ui.style())
                    .fill(pal.input_fill)
                    .stroke(Stroke::new(1.0, pal.hairline))
                    .corner_radius(egui::CornerRadius::same(
                        crate::frontend::theme::radius::CARD,
                    ))
                    .inner_margin(Margin::symmetric(8, 6))
                    .show(ui, |ui| {
                        ui.set_width(width);
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(egui_phosphor::regular::MAGNIFYING_GLASS)
                                    .size(16.0)
                                    .color(pal.text_tertiary),
                            );
                            let clear_width = if query.is_empty() { 0.0 } else { 28.0 };
                            let field_width = (ui.available_width() - clear_width).max(80.0);
                            let response = ui.add_sized(
                                [field_width, 24.0],
                                egui::TextEdit::singleline(query)
                                    .hint_text(placeholder)
                                    .desired_width(f32::INFINITY),
                            );
                            response.request_focus();

                            if !query.is_empty()
                                && with_core_button_style(ui, false, |ui| {
                                    ui.add_sized(
                                        [24.0, 24.0],
                                        Button::new(
                                            RichText::new(egui_phosphor::regular::X)
                                                .size(13.0)
                                                .color(core_button_text_color(&pal, false)),
                                        )
                                        .frame(false),
                                    )
                                })
                                .on_hover_text("Clear search")
                                .clicked()
                            {
                                query.clear();
                            }
                        });
                    });
            });
    }

    if ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
        || ctx.input(|input| input.key_pressed(egui::Key::Enter))
    {
        state.ui.entry_list.search_open = false;
    }
}

pub(crate) fn sidebar_search_query(state: &AppState) -> &str {
    match state.ui.layout.active_primary_view {
        PrimaryView::EntryList => &state.ui.entry_list.search_query,
        PrimaryView::Tasks => &state.tasks.task_list.search_query,
        PrimaryView::Style => &state.ui.style.search_query,
    }
}

pub(crate) fn sidebar_search_query_mut(state: &mut AppState) -> (&'static str, &mut String) {
    match state.ui.layout.active_primary_view {
        PrimaryView::EntryList => ("Search entries", &mut state.ui.entry_list.search_query),
        PrimaryView::Tasks => ("Search tasks", &mut state.tasks.task_list.search_query),
        PrimaryView::Style => ("Search style", &mut state.ui.style.search_query),
    }
}

pub(crate) fn sidebar_search_placeholder(state: &AppState) -> &'static str {
    match state.ui.layout.active_primary_view {
        PrimaryView::EntryList => "Search entries",
        PrimaryView::Tasks => "Search tasks",
        PrimaryView::Style => "Search style",
    }
}

pub(crate) fn sidebar_search_active(state: &AppState) -> bool {
    !sidebar_search_query(state).is_empty()
}
