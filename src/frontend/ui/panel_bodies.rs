use eframe::egui::{self, FontFamily, FontId, RichText, Stroke};

use crate::frontend::app::{ASSISTANT_CJK_FONT, CONSOLE_CJK_MONO_FONT};

mod activity;
mod assistant;
mod assistant_composer;
mod assistant_transcript;
mod console;
mod monitor;
mod task_monitor;

pub(crate) use activity::*;
pub(crate) use assistant::*;
pub(crate) use assistant_composer::*;
use assistant_transcript::*;
pub(crate) use console::*;
pub(crate) use monitor::*;
pub(crate) use task_monitor::*;

/// Width reserved at the right edge of the log/transcript scroll areas so their
/// content never slides under the scrollbar.
const ASSISTANT_SCROLLBAR_RESERVE: f32 = 12.0;

/// A frameless ✕ button in the tertiary color; `true` when clicked. Shared by
/// the composer strips and the Activity panel so the remove/cancel affordance
/// stays a single widget.
pub(super) fn assistant_remove_button(ui: &mut egui::Ui, hover: &str) -> bool {
    let pal = crate::frontend::theme::palette(ui);
    ui.add(
        egui::Button::new(
            RichText::new(egui_phosphor::regular::X)
                .small()
                .color(pal.text_tertiary),
        )
        .frame(false),
    )
    .on_hover_text(hover)
    .clicked()
}

pub(super) fn weak_panel_hairline(ui: &mut egui::Ui, alpha: u8) {
    let pal = crate::frontend::theme::palette(ui);
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 1.0), egui::Sense::hover());
    ui.painter().hline(
        rect.left()..=rect.right(),
        rect.center().y,
        Stroke::new(1.0, pal.neutral_overlay(alpha)),
    );
}

fn assistant_font_id(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(ASSISTANT_CJK_FONT.into()))
}

fn assistant_body_font_id() -> FontId {
    assistant_font_id(13.0)
}

fn assistant_text(text: impl Into<String>) -> RichText {
    RichText::new(text).font(assistant_body_font_id())
}

fn assistant_small_text(text: impl Into<String>) -> RichText {
    RichText::new(text).font(assistant_font_id(11.0))
}

fn console_font_id() -> FontId {
    FontId::new(13.0, FontFamily::Name(CONSOLE_CJK_MONO_FONT.into()))
}

fn console_text(text: impl Into<String>) -> RichText {
    RichText::new(text).font(console_font_id())
}
