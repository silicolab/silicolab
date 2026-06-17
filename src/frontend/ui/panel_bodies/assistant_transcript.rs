use super::*;

use eframe::egui::{self, Color32, CornerRadius, Frame, Margin, RichText, Stroke};

/// A small "icon + role" header above an assistant or user message.
pub(super) fn message_role(ui: &mut egui::Ui, icon: &str, label: &str, color: Color32) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 5.0;
        ui.label(RichText::new(icon).small().color(color));
        ui.label(RichText::new(label).small().strong().color(color));
    });
}

pub(super) fn agent_message_header(ui: &mut egui::Ui, pal: &crate::frontend::theme::Palette) {
    ui.add_space(10.0);
    message_role(
        ui,
        egui_phosphor::regular::SPARKLE,
        "SilicoLab Agent",
        pal.accent,
    );
    ui.add_space(2.0);
}

pub(super) fn render_transcript_entry(
    ui: &mut egui::Ui,
    pal: &crate::frontend::theme::Palette,
    markdown_cache: &mut egui_commonmark::CommonMarkCache,
    entry: &crate::frontend::agent::TranscriptEntry,
    content_width: f32,
    show_agent_header: bool,
) {
    use crate::frontend::agent::TranscriptEntry;
    use crate::frontend::theme::radius;
    match entry {
        TranscriptEntry::User(text) => {
            ui.set_width(content_width);
            ui.add_space(10.0);
            // The user's turn reads as a soft bubble so it stands apart from the
            // assistant's flush-left prose.
            let frame_inner_width = (content_width - 20.0).max(48.0);
            Frame::default()
                .fill(pal.neutral_overlay(20))
                .corner_radius(CornerRadius::same(radius::CARD))
                .inner_margin(Margin::symmetric(10, 8))
                .show(ui, |ui| {
                    ui.set_width(frame_inner_width);
                    ui.add(
                        egui::Label::new(assistant_text(text).color(pal.text_primary))
                            .wrap_mode(egui::TextWrapMode::Wrap),
                    );
                });
        }
        TranscriptEntry::Assistant(text) => {
            ui.set_width(content_width);
            if show_agent_header {
                agent_message_header(ui, pal);
            } else {
                ui.add_space(6.0);
            }
            render_markdown(ui, pal, markdown_cache, text);
        }
        TranscriptEntry::Tool {
            summary,
            result,
            is_error,
        } => {
            ui.set_width(content_width);
            if show_agent_header {
                agent_message_header(ui, pal);
            } else {
                ui.add_space(4.0);
            }
            render_tool_block(ui, pal, summary, result.as_deref(), *is_error);
        }
        TranscriptEntry::Notice(text) => {
            ui.set_width(content_width);
            ui.add_space(6.0);
            ui.label(
                assistant_small_text(text)
                    .small()
                    .italics()
                    .color(pal.text_tertiary),
            );
        }
    }
}

/// Render an assistant reply as formatted Markdown via `egui_commonmark`.
///
/// The viewer has no font/color setters of its own — it lays everything out
/// from the ambient `ui` style — so we scope the body, monospace, and heading
/// text styles to the Assistant tab's own fonts (keeping the CJK fallback and
/// 13px body sizing identical to the surrounding prose) and set the prose text
/// color before showing it. Without this it would fall back to egui's default
/// 14px proportional face.
pub(super) fn render_markdown(
    ui: &mut egui::Ui,
    pal: &crate::frontend::theme::Palette,
    cache: &mut egui_commonmark::CommonMarkCache,
    text: &str,
) {
    ui.scope(|ui| {
        let style = ui.style_mut();
        style
            .text_styles
            .insert(egui::TextStyle::Body, assistant_body_font_id());
        style
            .text_styles
            .insert(egui::TextStyle::Monospace, console_font_id());
        style
            .text_styles
            .insert(egui::TextStyle::Heading, assistant_font_id(18.0));
        // Color prose through the *default* text color, not `override_text_color`:
        // an override bakes its color into every glyph, including links — whose
        // accent `hyperlink_color` only paints over un-colored (placeholder)
        // glyphs — so links would render indistinguishable from body text. The
        // noninteractive stroke sets only the prose color; links and code keep
        // the colors the theme assigns them.
        style.visuals.widgets.noninteractive.fg_stroke.color = pal.text_primary;
        egui_commonmark::CommonMarkViewer::new().show(ui, cache, text);
    });
}

pub(super) fn render_tool_block(
    ui: &mut egui::Ui,
    pal: &crate::frontend::theme::Palette,
    command: &str,
    result: Option<&str>,
    is_error: bool,
) {
    use crate::frontend::theme::radius;
    let frame_inner_width = (ui.available_width() - 16.0).max(48.0);
    Frame::default()
        .fill(pal.neutral_overlay(12))
        .corner_radius(CornerRadius::same(radius::CONTROL))
        .inner_margin(Margin::symmetric(8, 6))
        .show(ui, |ui| {
            ui.set_width(frame_inner_width);
            ui.vertical(|ui| {
                ui.set_width(frame_inner_width);
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    ui.label(
                        RichText::new(egui_phosphor::regular::TERMINAL)
                            .small()
                            .color(pal.text_tertiary),
                    );
                    ui.add(
                        egui::Label::new(assistant_small_text(command).color(pal.text_muted))
                            .wrap_mode(egui::TextWrapMode::Wrap),
                    );
                });

                ui.add_space(5.0);
                let (line_rect, _) = ui
                    .allocate_exact_size(egui::vec2(frame_inner_width, 1.0), egui::Sense::hover());
                ui.painter().hline(
                    line_rect.left()..=line_rect.right(),
                    line_rect.center().y,
                    Stroke::new(1.0, pal.neutral_overlay(18)),
                );
                ui.add_space(5.0);

                match result {
                    Some(result) => {
                        let (icon, color) = if is_error {
                            (egui_phosphor::regular::X_CIRCLE, pal.status_red)
                        } else {
                            (egui_phosphor::regular::CHECK_CIRCLE, pal.text_tertiary)
                        };
                        ui.horizontal_top(|ui| {
                            ui.spacing_mut().item_spacing.x = 6.0;
                            ui.label(RichText::new(icon).small().color(color));
                            ui.add(
                                egui::Label::new(
                                    RichText::new(result).small().monospace().color(color),
                                )
                                .wrap_mode(egui::TextWrapMode::Wrap),
                            );
                        });
                    }
                    None => {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 6.0;
                            ui.add(egui::Spinner::new().size(12.0));
                            ui.label(RichText::new("Running…").small().color(pal.text_tertiary));
                        });
                    }
                }
            });
        });
}

pub(super) fn completed_badge(ui: &mut egui::Ui, pal: &crate::frontend::theme::Palette) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 5.0;
        ui.label(
            RichText::new(egui_phosphor::regular::CHECK_CIRCLE)
                .small()
                .color(pal.status_green),
        );
        ui.label(
            RichText::new("Completed")
                .small()
                .strong()
                .color(pal.status_green),
        );
    });
}
