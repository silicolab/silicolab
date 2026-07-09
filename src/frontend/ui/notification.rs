//! Renders the app's single active [`Notification`] as a floating card.
//!
//! A notification is non-modal chrome: it floats over the workspace (above the
//! viewport, below the Settings/About modals) and never steals the whole screen.
//! It is the one place the app offers the user a *choice* outside a task panel —
//! a short message plus optional action buttons — so a transient suggestion can
//! carry a "do this / not now" without a blocking dialog.

use eframe::egui::{self, Align, Align2, Button, Color32, Frame, Layout, Margin, RichText, Stroke};

use crate::frontend::{
    actions::{AppAction, NotificationSeverity},
    state::AppState,
    theme,
};

/// Draw the active notification (if any) and push any button presses onto
/// `actions`. Every button — and the dismiss "×" — first emits
/// [`AppAction::DismissNotification`], so taking an action always clears the card
/// (and a button's own action may then post a fresh notification).
pub(super) fn render_notification(state: &AppState, ui: &egui::Ui, actions: &mut Vec<AppAction>) {
    let Some(notification) = &state.ui.notification else {
        return;
    };
    let pal = theme::palette(ui);
    let accent = match notification.severity {
        NotificationSeverity::Info => pal.status_blue,
        NotificationSeverity::Warning => pal.status_amber,
    };

    egui::Area::new(egui::Id::new("workspace_notification"))
        .order(egui::Order::Foreground)
        .anchor(Align2::CENTER_TOP, egui::vec2(0.0, 22.0))
        .show(ui.ctx(), |ui| {
            ui.set_max_width(460.0);
            Frame::default()
                .fill(pal.window_backing)
                .stroke(Stroke::new(1.0_f32, pal.hairline))
                .corner_radius(egui::CornerRadius::same(theme::radius::MODAL))
                .inner_margin(Margin::same(14))
                .shadow(egui::Shadow {
                    offset: [0, 6],
                    blur: 18,
                    spread: 0,
                    color: Color32::from_black_alpha(60),
                })
                .show(ui, |ui| {
                    // Title row: a colored accent dot, the title, then the dismiss
                    // "×" pushed to the far right.
                    ui.horizontal(|ui| {
                        let (dot, _) =
                            ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
                        ui.painter().circle_filled(dot.center(), 4.0, accent);
                        ui.add_space(2.0);
                        ui.label(
                            RichText::new(&notification.title)
                                .strong()
                                .color(pal.text_strong),
                        );
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui
                                .add(
                                    Button::new(RichText::new("×").color(pal.text_muted))
                                        .frame(false),
                                )
                                .on_hover_text("Dismiss")
                                .clicked()
                            {
                                actions.push(AppAction::DismissNotification);
                            }
                        });
                    });

                    if !notification.body.is_empty() {
                        ui.add_space(6.0);
                        ui.label(RichText::new(&notification.body).color(pal.text_muted));
                    }

                    if !notification.buttons.is_empty() {
                        ui.add_space(12.0);
                        // Right-aligned button row; iterate reversed so the first
                        // declared button ends up leftmost (the primary/recommended
                        // one conventionally sits rightmost).
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            for button in notification.buttons.iter().rev() {
                                let widget = if button.primary {
                                    Button::new(RichText::new(&button.label).color(Color32::WHITE))
                                        .fill(accent)
                                } else {
                                    Button::new(RichText::new(&button.label).color(pal.text_strong))
                                };
                                if ui.add(widget).clicked() {
                                    actions.push(AppAction::DismissNotification);
                                    actions.push(button.action.clone());
                                }
                            }
                        });
                    }
                });
        });
}

#[cfg(test)]
mod tests {
    use crate::frontend::actions::{AppAction, Notification, NotificationSeverity};

    #[test]
    fn builder_collects_buttons_in_order() {
        let notification = Notification::new(NotificationSeverity::Warning, "Big", "Lots of atoms")
            .button("Use wireframe", true, AppAction::DismissNotification)
            .button("Keep detail", false, AppAction::DismissNotification);

        assert_eq!(notification.severity, NotificationSeverity::Warning);
        assert_eq!(notification.buttons.len(), 2);
        assert_eq!(notification.buttons[0].label, "Use wireframe");
        assert!(notification.buttons[0].primary);
        assert!(!notification.buttons[1].primary);
    }
}
