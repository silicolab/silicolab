//! The standalone Settings dialog.
//!
//! Settings is a floating window that opens centered but can be dragged
//! anywhere — never a full-area view — so the precious 3D workspace is never
//! covered. The body is generated from the schema-driven [`settings_registry`]:
//! a two-pane layout with a category rail on the left and the selected
//! category's groups on the right, plus a search box that flattens matches
//! across every category (VSCode-style).
//!
//! Like the other transient chrome flags, `settings_open` is flipped directly
//! by the entry points and here on dismissal — no persisted state changes when
//! the dialog opens or closes, so there is nothing for the dispatcher to
//! mediate. Every *value* change inside still flows through `AppAction`.

use eframe::egui::{self, Align, Color32, Layout, RichText, Sense, UiBuilder};

use super::settings_registry;
use crate::frontend::{actions::AppAction, state::AppState};

/// Default dialog size — comfortably wider than the old sidebar so the two-pane
/// layout breathes and the blur slider has room.
const MODAL_WIDTH: f32 = 720.0;
const PANE_HEIGHT: f32 = 440.0;
const RAIL_WIDTH: f32 = 165.0;
const FRAME_MARGIN: i8 = 12;
const FRAME_RIGHT_MARGIN: i8 = 6;

/// Render the Settings modal when open. A no-op while closed.
pub fn show(state: &mut AppState, ctx: &egui::Context, actions: &mut Vec<AppAction>) {
    if !state.ui.layout.settings_open {
        return;
    }

    render_backdrop(state, ctx);

    let mut close = false;

    // A floating Window rather than egui's Modal: Modal pins the dialog to the
    // screen center, which made it impossible to drag aside to see the
    // workspace underneath. The window opens centered (pivot + default_pos) and
    // is draggable by its header/background; Esc still dismisses it.
    egui::Window::new("settings_modal")
        .id(egui::Id::new("settings_modal"))
        .title_bar(false)
        .resizable(false)
        .collapsible(false)
        .order(egui::Order::Foreground)
        .frame(settings_window_frame(ctx))
        .pivot(egui::Align2::CENTER_CENTER)
        .default_pos(ctx.content_rect().center())
        .show(ctx, |ui| {
            ui.set_width(MODAL_WIDTH);
            let pal = crate::frontend::theme::palette(ui);

            // Header: title + close button.
            ui.horizontal(|ui| {
                ui.heading("Settings");
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui
                        .button(RichText::new(egui_phosphor::regular::X))
                        .on_hover_text("Close (Esc)")
                        .clicked()
                    {
                        close = true;
                    }
                });
            });

            // Search box spanning the dialog. A non-empty query ignores the selected
            // category and shows a flat, cross-category result list below.
            ui.add(
                egui::TextEdit::singleline(&mut state.ui.settings.search_query)
                    .hint_text("Search settings")
                    .desired_width(f32::INFINITY),
            );
            ui.separator();

            let search = state.ui.settings.search_query.to_lowercase();

            ui.allocate_ui_with_layout(
                egui::vec2(ui.available_width(), PANE_HEIGHT),
                Layout::left_to_right(Align::Min),
                |ui| {
                    render_rail(state, ui, &search, &pal);
                    ui.separator();
                    ui.scope(|ui| {
                        ui.spacing_mut().scroll.bar_outer_margin = 0.0;
                        egui::ScrollArea::vertical()
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                // The scroll content inherits this pane's left-to-right
                                // layout, which would lay the groups out side by side
                                // (pushing later ones past the window edge). Force the
                                // normal top-down flow.
                                ui.vertical(|ui| {
                                    if search.is_empty() {
                                        settings_registry::render_category(
                                            state,
                                            ui,
                                            actions,
                                            state.ui.settings.selected_category,
                                        );
                                    } else {
                                        settings_registry::render_search_results(
                                            state, ui, actions, &search,
                                        );
                                    }
                                });
                            });
                    });
                },
            );
        });

    if close || ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
        state.ui.layout.settings_open = false;
    }
}

fn settings_window_frame(ctx: &egui::Context) -> egui::Frame {
    let style = ctx.global_style();
    let dark = style.visuals.dark_mode;
    let shadow_color = if dark {
        Color32::from_black_alpha(105)
    } else {
        Color32::from_black_alpha(55)
    };
    let stroke_color = if dark {
        Color32::from_white_alpha(28)
    } else {
        Color32::from_black_alpha(22)
    };

    egui::Frame::window(&style)
        .inner_margin(egui::Margin {
            left: FRAME_MARGIN,
            right: FRAME_RIGHT_MARGIN,
            top: FRAME_MARGIN,
            bottom: FRAME_MARGIN,
        })
        .outer_margin(egui::Margin::same(18))
        .corner_radius(egui::CornerRadius::same(
            crate::frontend::theme::radius::MODAL,
        ))
        .stroke(egui::Stroke::new(1.0, stroke_color))
        .shadow(egui::Shadow {
            offset: [0, 8],
            blur: 24,
            spread: 0,
            color: shadow_color,
        })
}

fn render_backdrop(state: &AppState, ctx: &egui::Context) {
    egui::Area::new(egui::Id::new("settings_modal_backdrop"))
        .order(egui::Order::Foreground)
        .interactable(true)
        .show(ctx, |ui| {
            let rect = ui.ctx().content_rect();
            let mut backdrop = ui.new_child(
                UiBuilder::new()
                    .sense(Sense::CLICK | Sense::DRAG)
                    .max_rect(rect),
            );
            backdrop.set_min_size(rect.size());

            ui.painter()
                .rect_filled(rect, 0.0, settings_backdrop_tint(state, ctx));
            let _ = backdrop.response();
        });
}

fn settings_backdrop_tint(state: &AppState, ctx: &egui::Context) -> Color32 {
    let dark = ctx.global_style().visuals.dark_mode;
    match (state.ui.glass_active, dark) {
        (true, true) => Color32::from_black_alpha(34),
        (true, false) => Color32::from_white_alpha(42),
        (false, true) => Color32::from_black_alpha(78),
        (false, false) => Color32::from_white_alpha(92),
    }
}

/// The left category rail. Lists every category that still has a matching
/// setting under the active search; clicking one selects it (used when the
/// search box is empty).
fn render_rail(
    state: &mut AppState,
    ui: &mut egui::Ui,
    search: &str,
    pal: &crate::frontend::theme::Palette,
) {
    ui.allocate_ui_with_layout(
        egui::vec2(RAIL_WIDTH, PANE_HEIGHT),
        Layout::top_down_justified(Align::Min),
        |ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            let selected = state.ui.settings.selected_category;
            for category in settings_registry::visible_categories(search) {
                let label = if category == selected {
                    RichText::new(category.label()).color(pal.text_strong)
                } else {
                    RichText::new(category.label())
                };
                if ui.selectable_label(category == selected, label).clicked() {
                    state.ui.settings.selected_category = category;
                }
            }
        },
    );
}
