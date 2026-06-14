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

use eframe::egui::{self, Align, Layout, RichText};

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

    super::modal::render_backdrop(state, ctx, "settings_modal_backdrop");

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
        .frame(super::modal::window_frame(
            ctx,
            egui::Margin {
                left: FRAME_MARGIN,
                right: FRAME_RIGHT_MARGIN,
                top: FRAME_MARGIN,
                bottom: FRAME_MARGIN,
            },
        ))
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
