use super::*;

use eframe::egui::{self, Align, Frame, Layout, Margin, ScrollArea, Stroke};

use crate::frontend::{actions::AppAction, state::AppState};

pub(crate) fn render_output_panel(state: &mut AppState, ui: &mut egui::Ui) {
    ui.set_width(ui.available_width());
    let log_width = ui.available_width();
    let log_content_width = (log_width - ASSISTANT_SCROLLBAR_RESERVE).max(48.0);
    ScrollArea::vertical()
        .max_width(log_width)
        .auto_shrink([false, false])
        .content_margin(Margin::ZERO)
        .stick_to_bottom(true)
        .show(ui, |ui| {
            ui.set_width(log_content_width);
            for line in &state.output_log {
                ui.add(egui::Label::new(console_text(line)).wrap_mode(egui::TextWrapMode::Wrap));
            }
        });
}

pub(crate) fn render_console_panel(
    state: &mut AppState,
    ui: &mut egui::Ui,
    actions: &mut Vec<AppAction>,
) {
    const PROMPT_ROW_HEIGHT: f32 = 34.0;
    const INPUT_OUTER_HEIGHT: f32 = 28.0;
    const INPUT_X_MARGIN: f32 = 8.0;
    const DIVIDER_HEIGHT: f32 = 1.0;
    const BOTTOM_PADDING: f32 = 4.0;

    ui.set_width(ui.available_width());
    // Keep chronological output in top-down visual order while reserving fixed
    // space for the prompt row so the panel cannot grow frame-over-frame.
    let log_height =
        (ui.available_height() - PROMPT_ROW_HEIGHT - DIVIDER_HEIGHT - BOTTOM_PADDING).max(0.0);
    let log_width = ui.available_width();
    let log_content_width = (log_width - ASSISTANT_SCROLLBAR_RESERVE).max(48.0);
    let log_text = state.output_log.join("\n");

    ui.allocate_ui_with_layout(
        egui::vec2(log_width, log_height),
        Layout::top_down(Align::Min),
        |ui| {
            ScrollArea::vertical()
                .max_width(log_width)
                .auto_shrink([false, false])
                .content_margin(Margin::ZERO)
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    ui.set_width(log_content_width);
                    ui.add(
                        egui::Label::new(console_text(log_text))
                            .selectable(true)
                            .wrap_mode(egui::TextWrapMode::Wrap),
                    );
                });
        },
    );
    weak_panel_hairline(ui, 14);
    ui.allocate_ui_with_layout(
        egui::vec2(ui.available_width(), PROMPT_ROW_HEIGHT),
        Layout::left_to_right(Align::Center),
        |ui| {
            let pal = crate::frontend::theme::palette(ui);
            let input_radius =
                crate::frontend::theme::radius::concentric(crate::frontend::theme::radius::CARD, 2);
            ui.spacing_mut().item_spacing.x = 8.0;

            ui.label(console_text("sls>"));

            let button_width = 46.0;
            let text_edit_width = (ui.available_width()
                - button_width
                - ui.spacing().item_spacing.x
                - INPUT_X_MARGIN * 2.0)
                .max(96.0);

            let response = Frame::default()
                .fill(pal.input_fill)
                .stroke(Stroke::new(1.0, pal.hairline))
                .corner_radius(egui::CornerRadius::same(input_radius))
                .inner_margin(Margin::symmetric(INPUT_X_MARGIN as i8, 3))
                .show(ui, |ui| {
                    ui.add_sized(
                        [text_edit_width, INPUT_OUTER_HEIGHT - 8.0],
                        egui::TextEdit::singleline(&mut state.ui.console.input)
                            .desired_width(f32::INFINITY)
                            .frame(Frame::NONE)
                            .margin(Margin::ZERO)
                            .hint_text("view background white"),
                    )
                })
                .inner;

            let mut run = false;
            if ui.button("Run").clicked() {
                run = true;
            }

            if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                run = true;
            }

            if run {
                let command = state.ui.console.input.trim().to_string();
                if !command.is_empty() {
                    actions.push(AppAction::RunConsoleCommand(command));
                    state.ui.console.input.clear();
                }
            }
        },
    );
    ui.add_space(BOTTOM_PADDING);
}
