use super::*;

pub(crate) fn render_primary_sidebar(
    state: &mut AppState,
    ui: &mut egui::Ui,
    actions: &mut Vec<AppAction>,
) {
    let pal = crate::frontend::theme::palette(ui);
    // Pin scroll bars to the panel's edge, macOS source-list style: pull them
    // out through the sidebar's 10px right margin (bar right edge lands 2px
    // shy of the divider hairline), so the bar and the divider don't read as
    // two parallel bars with a dead gap between them. Inherited by every
    // ScrollArea in the sidebar's views.
    ui.spacing_mut().scroll.bar_outer_margin = -8.0;
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
        Frame::default()
            .fill(pal.neutral_overlay(if dark { 26 } else { 14 }))
            .corner_radius(egui::CornerRadius::same(
                crate::frontend::theme::radius::CONTROL + SEGMENT_INSET,
            ))
            .inner_margin(Margin::same(SEGMENT_INSET as i8))
            .show(ui, |ui| {
                let seg_w = (ui.available_width() / PrimaryView::all().len() as f32).floor();
                ui.spacing_mut().item_spacing.x = 0.0;
                ui.horizontal(|ui| {
                    for view in PrimaryView::all() {
                        let selected = state.ui.layout.active_primary_view == *view;
                        let (rect, response) =
                            ui.allocate_exact_size(egui::vec2(seg_w, 28.0), Sense::click());
                        let mut job = egui::text::LayoutJob::default();
                        job.append(
                            view.icon(),
                            0.0,
                            egui::TextFormat {
                                font_id: egui::FontId::proportional(14.0),
                                color: segment_icon_color(*view, &pal, selected),
                                valign: Align::Center,
                                ..Default::default()
                            },
                        );
                        if selected {
                            job.append(
                                view.short_label(),
                                6.0,
                                egui::TextFormat {
                                    font_id: egui::FontId::proportional(12.5),
                                    color: pal.text_strong,
                                    valign: Align::Center,
                                    ..Default::default()
                                },
                            );
                        }
                        let painter = ui.painter();
                        let galley = painter.layout_job(job);
                        let segment_radius =
                            egui::CornerRadius::same(crate::frontend::theme::radius::CONTROL);
                        if selected {
                            painter.rect(
                                rect,
                                segment_radius,
                                card_fill,
                                Stroke::new(1.0, pal.hairline),
                                egui::StrokeKind::Inside,
                            );
                        } else if response.hovered() {
                            painter.rect_filled(rect, segment_radius, pal.neutral_overlay(14));
                        }
                        let pos = rect.center() - galley.size() / 2.0;
                        painter.galley(pos, galley, pal.text_primary);
                        if !selected {
                            response.clone().on_hover_text(view.label());
                        }
                        if response.clicked() {
                            state.ui.layout.active_primary_view = *view;
                        }
                    }
                });
            });
    }
    ui.add_space(8.0);

    match state.ui.layout.active_primary_view {
        PrimaryView::EntryList => render_entry_list(state, ui, actions),
        PrimaryView::Tasks => render_tasks_view(state, ui, actions),
        PrimaryView::Style => render_style_panel(state, ui, actions),
    }
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
