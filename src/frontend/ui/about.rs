//! The custom, cross-platform About window.
//!
//! Replaces the macOS-only native About panel (which can show the app icon but
//! not a styled wordmark) and gives Windows/Linux an About for the first time.
//! Opened by setting `state.ui.layout.about_open`; closed by Esc or the close
//! button. Shares its window frame and backdrop with Settings via [`super::modal`].

use eframe::egui::{self, Align, Color32, Layout, RichText};

use crate::frontend::{actions::AppAction, state::AppState};

const REPO_URL: &str = "https://github.com/silicolab/silicolab";
const DOCS_URL: &str = "https://github.com/silicolab/silicolab#readme";
const ICON_PX: f32 = 96.0;
const WINDOW_WIDTH: f32 = 420.0;

/// Render the About window when open. A no-op while closed.
///
/// Takes `actions` for signature parity with the other `ui::*::show` entry
/// points (the dialog currently emits none), so wiring it into `show_workbench`
/// matches `settings_modal::show`.
pub fn show(state: &mut AppState, ctx: &egui::Context, _actions: &mut Vec<AppAction>) {
    if !state.ui.layout.about_open {
        return;
    }

    super::modal::render_backdrop(state, ctx, "about_backdrop");

    let mut close = false;
    let texture = icon_texture(ctx);

    egui::Window::new("about_window")
        .id(egui::Id::new("about_window"))
        .title_bar(false)
        .resizable(false)
        .collapsible(false)
        .order(egui::Order::Foreground)
        .frame(super::modal::window_frame(ctx, egui::Margin::same(18)))
        .pivot(egui::Align2::CENTER_CENTER)
        .default_pos(ctx.content_rect().center())
        .show(ctx, |ui| {
            ui.set_width(WINDOW_WIDTH);
            let dark = ctx.global_style().visuals.dark_mode;

            // Close button, top-right.
            ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                if ui
                    .button(RichText::new(egui_phosphor::regular::X))
                    .on_hover_text("Close (Esc)")
                    .clicked()
                {
                    close = true;
                }
            });

            ui.vertical_centered(|ui| {
                ui.add_space(2.0);
                ui.image(egui::load::SizedTexture::new(
                    texture.id(),
                    egui::vec2(ICON_PX, ICON_PX),
                ));
                ui.add_space(14.0);

                // Wordmark: "Silico" neutral + "Lab" violet, built as one galley
                // so vertical_centered centers it on its real width (a hardcoded
                // width guess drifts off-center when the font or text changes).
                let (silico, lab) = wordmark_colors(dark);
                let font = egui::FontId::proportional(30.0);
                let mut wordmark = egui::text::LayoutJob::default();
                wordmark.append(
                    "Silico",
                    0.0,
                    egui::TextFormat {
                        font_id: font.clone(),
                        color: silico,
                        ..Default::default()
                    },
                );
                wordmark.append(
                    "Lab",
                    0.0,
                    egui::TextFormat {
                        font_id: font,
                        color: lab,
                        ..Default::default()
                    },
                );
                ui.label(wordmark);

                ui.add_space(4.0);
                ui.label(
                    RichText::new(format!("Version {}", env!("CARGO_PKG_VERSION")))
                        .color(crate::frontend::theme::palette(ui).text_muted),
                );
                ui.add_space(8.0);
                ui.label(env!("CARGO_PKG_DESCRIPTION"));
                ui.add_space(14.0);

                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    let gap = 18.0;
                    let width =
                        link_width(ui, "Repository") + gap + link_width(ui, "Documentation");
                    ui.add_space((ui.available_width() - width).max(0.0) / 2.0);
                    ui.hyperlink_to("Repository", REPO_URL);
                    ui.add_space(gap);
                    ui.hyperlink_to("Documentation", DOCS_URL);
                });
                ui.add_space(4.0);
            });
        });

    if close || ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
        state.ui.layout.about_open = false;
    }
}

/// Wordmark accent colors (spec §5): "Lab" violet, "Silico" neutral.
fn wordmark_colors(dark: bool) -> (Color32, Color32) {
    if dark {
        (
            Color32::from_rgb(0xE8, 0xE6, 0xE0),
            Color32::from_rgb(0xB7, 0x9C, 0xFF),
        )
    } else {
        (
            Color32::from_rgb(0x36, 0x33, 0x2C),
            Color32::from_rgb(0x7B, 0x5C, 0xFF),
        )
    }
}

/// Rendered width of body-style `text`, used to center the link row on its real
/// width instead of a hardcoded guess.
fn link_width(ui: &egui::Ui, text: &str) -> f32 {
    let font_id = egui::TextStyle::Body.resolve(ui.style());
    ui.painter()
        .layout_no_wrap(text.to_owned(), font_id, Color32::WHITE)
        .size()
        .x
}

/// Upload the committed 256² icon once and cache the handle in egui temp memory
/// (re-uploading every frame would thrash the GPU texture).
fn icon_texture(ctx: &egui::Context) -> egui::TextureHandle {
    let id = egui::Id::new("about_icon_texture");
    if let Some(handle) = ctx.data(|d| d.get_temp::<egui::TextureHandle>(id)) {
        return handle;
    }
    let bytes = include_bytes!("../../../assets/icon/window-256.png");
    let image = image::load_from_memory(bytes)
        .expect("decode embedded window-256.png")
        .to_rgba8();
    let (w, h) = image.dimensions();
    let color = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &image);
    let handle = ctx.load_texture("about_icon", color, egui::TextureOptions::LINEAR);
    ctx.data_mut(|d| d.insert_temp(id, handle.clone()));
    handle
}
