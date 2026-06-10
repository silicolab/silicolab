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

use std::sync::Arc;

use eframe::egui::{
    self, Align, Color32, ImageData, Layout, RichText, Sense, TextureOptions, UiBuilder,
};

use super::settings_registry;
use crate::frontend::{actions::AppAction, state::AppState};

/// Default dialog size — comfortably wider than the old sidebar so the two-pane
/// layout breathes and the blur slider has room.
const MODAL_WIDTH: f32 = 720.0;
const PANE_HEIGHT: f32 = 440.0;
const RAIL_WIDTH: f32 = 165.0;
const BACKDROP_MAX_DIMENSION: u32 = 900;
const BACKDROP_BLUR_SIGMA: f32 = 1.8;

#[derive(Debug)]
struct SettingsBackdropCapture;

/// Render the Settings modal when open. A no-op while closed.
pub fn show(state: &mut AppState, ctx: &egui::Context, actions: &mut Vec<AppAction>) {
    if !state.ui.layout.settings_open {
        state.ui.settings_backdrop.reset();
        return;
    }

    collect_backdrop_capture(state, ctx);

    // A theme/scheme change makes the captured backdrop stale (it still shows
    // the old appearance); drop it so it's re-captured.
    let appearance = current_appearance(state, ctx);
    if state.ui.settings_backdrop.texture.is_some()
        && state.ui.settings_backdrop.captured_appearance != Some(appearance)
    {
        state.ui.settings_backdrop.reset();
    }

    if state.ui.settings_backdrop.texture.is_none() {
        render_capture_backdrop(ctx);
        request_backdrop_capture(state, ctx);
        return;
    }

    render_blurred_backdrop(state, ctx);

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
        .inner_margin(egui::Margin::same(12))
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

fn collect_backdrop_capture(state: &mut AppState, ctx: &egui::Context) {
    let screenshots = ctx.input(|input| {
        input
            .raw
            .events
            .iter()
            .filter_map(|event| match event {
                egui::Event::Screenshot {
                    user_data, image, ..
                } if is_settings_backdrop_capture(user_data) => Some(Arc::clone(image)),
                _ => None,
            })
            .collect::<Vec<_>>()
    });

    for screenshot in screenshots {
        let image = blurred_backdrop_image(&screenshot);
        state.ui.settings_backdrop.texture = Some(ctx.load_texture(
            "settings_modal_blurred_backdrop",
            ImageData::Color(Arc::new(image)),
            TextureOptions::LINEAR,
        ));
        state.ui.settings_backdrop.capture_pending = false;
        state.ui.settings_backdrop.captured_appearance = Some(current_appearance(state, ctx));
    }
}

fn current_appearance(
    state: &AppState,
    ctx: &egui::Context,
) -> (bool, crate::backend::config::ColorScheme) {
    (
        ctx.global_style().visuals.dark_mode,
        state.config.color_scheme,
    )
}

fn is_settings_backdrop_capture(user_data: &egui::UserData) -> bool {
    user_data
        .data
        .as_ref()
        .is_some_and(|data| data.downcast_ref::<SettingsBackdropCapture>().is_some())
}

fn request_backdrop_capture(state: &mut AppState, ctx: &egui::Context) {
    if state.ui.settings_backdrop.capture_pending {
        ctx.request_repaint();
        return;
    }

    state.ui.settings_backdrop.capture_pending = true;
    ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::new(
        SettingsBackdropCapture,
    )));
    ctx.request_repaint();
}

fn render_capture_backdrop(ctx: &egui::Context) {
    render_backdrop(ctx, None);
}

fn render_blurred_backdrop(state: &mut AppState, ctx: &egui::Context) {
    render_backdrop(ctx, state.ui.settings_backdrop.texture.as_ref());
}

fn render_backdrop(ctx: &egui::Context, texture: Option<&egui::TextureHandle>) {
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

            if let Some(texture) = texture {
                ui.painter().image(
                    texture.id(),
                    rect,
                    egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                    Color32::WHITE,
                );
            }
            ui.painter()
                .rect_filled(rect, 0.0, settings_backdrop_tint(ctx));
            let _ = backdrop.response();
        });
}

fn settings_backdrop_tint(ctx: &egui::Context) -> Color32 {
    if ctx.global_style().visuals.dark_mode {
        Color32::from_black_alpha(58)
    } else {
        Color32::from_white_alpha(72)
    }
}

fn blurred_backdrop_image(image: &egui::ColorImage) -> egui::ColorImage {
    let [width, height] = image.size;
    if width == 0 || height == 0 {
        return image.clone();
    }

    let max_dimension = width.max(height) as f32;
    let scale = (BACKDROP_MAX_DIMENSION as f32 / max_dimension).min(1.0);
    let scaled_width = ((width as f32 * scale).round() as u32).max(1);
    let scaled_height = ((height as f32 * scale).round() as u32).max(1);

    let mut rgba = Vec::with_capacity(width * height * 4);
    for pixel in &image.pixels {
        rgba.extend_from_slice(&[pixel.r(), pixel.g(), pixel.b(), pixel.a()]);
    }

    let Some(source) = image::RgbaImage::from_raw(width as u32, height as u32, rgba) else {
        return image.clone();
    };
    let small = image::imageops::resize(
        &source,
        scaled_width,
        scaled_height,
        image::imageops::FilterType::Triangle,
    );
    let blurred = image::imageops::blur(&small, BACKDROP_BLUR_SIGMA);

    egui::ColorImage::from_rgba_unmultiplied(
        [scaled_width as usize, scaled_height as usize],
        blurred.as_raw(),
    )
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
