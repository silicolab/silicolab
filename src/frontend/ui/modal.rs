//! Shared chrome for the app's floating modal dialogs (Settings, About).
//!
//! Both are draggable, centered `egui::Window`s wearing the same shadowed,
//! hairline-stroked frame, each over the same dimming backdrop. Only two things
//! differ: the frame's inner margin (Settings insets its right edge to make room
//! for the scrollbar; About is symmetric) and the backdrop's `Area` id. Both are
//! parameters here, so the two dialogs stay in visual lockstep.

use eframe::egui::{self, Color32, Sense, UiBuilder};

use crate::frontend::state::AppState;

/// The shared modal window frame: shadowed, hairline-stroked, MODAL corner
/// radius, with an 18px outer margin. The caller supplies `inner_margin`.
pub(super) fn window_frame(ctx: &egui::Context, inner_margin: egui::Margin) -> egui::Frame {
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
        .inner_margin(inner_margin)
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

/// Paint the dimming backdrop behind a modal. `id_source` namespaces the `Area`
/// so two stacked modals never collide. The backdrop captures clicks/drags only
/// to block the workspace beneath — clicking it is intentionally *not* a
/// dismissal (so a stray click can't lose unsaved edits); Esc and the close
/// button dismiss.
pub(super) fn render_backdrop(state: &AppState, ctx: &egui::Context, id_source: &str) {
    egui::Area::new(egui::Id::new(id_source))
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
                .rect_filled(rect, 0.0, backdrop_tint(state, ctx));
            let _ = backdrop.response();
        });
}

/// Backdrop dimming, tuned per glass/theme so the dialog reads as foreground
/// without fully hiding the workspace.
fn backdrop_tint(state: &AppState, ctx: &egui::Context) -> Color32 {
    let dark = ctx.global_style().visuals.dark_mode;
    match (state.ui.glass_active, dark) {
        (true, true) => Color32::from_black_alpha(34),
        (true, false) => Color32::from_white_alpha(42),
        (false, true) => Color32::from_black_alpha(78),
        (false, false) => Color32::from_white_alpha(92),
    }
}
