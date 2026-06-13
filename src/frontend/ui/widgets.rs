use super::*;

/// Scroll area used by fixed docked sidebars next to custom resize dividers.
pub(crate) fn docked_sidebar_scroll_area() -> ScrollArea {
    // Keep the floating bar as a non-interactive position indicator. The
    // divider owns drag input on the panel edge.
    ScrollArea::vertical().scroll_source(
        egui::scroll_area::ScrollSource::MOUSE_WHEEL | egui::scroll_area::ScrollSource::DRAG,
    )
}

/// Render sidebar content pinned to the panel's exact width.
///
/// `Panel::exact_size` clips the panel *fill* to the requested width, but a child
/// widget that can't shrink that far (a Settings slider or combo carrying a fixed
/// label) still grows the content frame's `response.rect`. egui advances the
/// parent layout cursor by that grown rect, so the central column — and the
/// bottom panel nested inside it — get pushed out to the content edge while the
/// sidebar fill and our resize divider stay at the requested width, leaving a
/// blank band beside the sidebar (and the bottom panel failing to follow a narrow
/// drag). Rendering the content into a width-bounded, clipped child and advancing
/// the cursor by that fixed rect pins the response rect to the requested width, so
/// the fill, divider, central column, and bottom panel all stay flush at any
/// width. Content too wide to fit is clipped rather than overflowing.
pub(crate) fn render_pinned(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    let rect = ui.max_rect();
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect)
            .layout(Layout::top_down(Align::Min)),
    );
    child.set_clip_rect(rect.intersect(ui.clip_rect()));
    add(&mut child);
    ui.advance_cursor_after_rect(rect);
}

pub(crate) fn window_control_button(
    ui: &mut Ui,
    icon: &'static str,
    hover_fill: egui::Color32,
) -> egui::Response {
    let is_close = icon == egui_phosphor::regular::X;
    let pal = crate::frontend::theme::palette(ui);
    let (rect, response) = ui.allocate_exact_size(Vec2::new(36.0, 24.0), Sense::click());
    let fill = if response.hovered() {
        hover_fill
    } else {
        egui::Color32::TRANSPARENT
    };
    let text_color = if is_close && response.hovered() {
        egui::Color32::WHITE
    } else {
        pal.text_muted
    };

    ui.painter()
        .rect_filled(rect, f32::from(CORE_BUTTON_CORNER_RADIUS), fill);
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        icon,
        egui::FontId::proportional(14.0),
        text_color,
    );
    response
}

pub(crate) fn with_core_button_style<R>(
    ui: &mut Ui,
    selected: bool,
    add_contents: impl FnOnce(&mut Ui) -> R,
) -> R {
    ui.scope(|ui| {
        configure_core_button_visuals(ui, selected);
        add_contents(ui)
    })
    .inner
}

pub(crate) fn configure_core_button_visuals(ui: &mut Ui, selected: bool) {
    let pal = crate::frontend::theme::palette(ui);
    let dark = ui.visuals().dark_mode;
    let inactive_fill = core_button_fill(&pal, dark, selected, false);
    let hovered_fill = core_button_fill(&pal, dark, selected, true);
    let selected_fill = core_button_fill(&pal, dark, true, false);
    let selected_hover_fill = core_button_fill(&pal, dark, true, true);
    let inactive_text = core_button_text_color(&pal, selected);
    let selected_text = core_button_text_color(&pal, true);
    let visuals = &mut ui.style_mut().visuals.widgets;
    // Core buttons round concentrically with the window corner instead of
    // inheriting the global CONTROL radius.
    let corner = egui::CornerRadius::same(CORE_BUTTON_CORNER_RADIUS);

    visuals.inactive.weak_bg_fill = inactive_fill;
    visuals.inactive.bg_fill = inactive_fill;
    visuals.inactive.bg_stroke = Stroke::NONE;
    visuals.inactive.fg_stroke.color = inactive_text;
    visuals.inactive.corner_radius = corner;

    visuals.hovered.weak_bg_fill = hovered_fill;
    visuals.hovered.bg_fill = hovered_fill;
    visuals.hovered.bg_stroke = Stroke::NONE;
    visuals.hovered.fg_stroke.color = inactive_text;
    visuals.hovered.corner_radius = corner;

    visuals.active.weak_bg_fill = selected_hover_fill;
    visuals.active.bg_fill = selected_hover_fill;
    visuals.active.bg_stroke = Stroke::NONE;
    visuals.active.fg_stroke.color = inactive_text;
    visuals.active.corner_radius = corner;

    visuals.open.weak_bg_fill = selected_fill;
    visuals.open.bg_fill = selected_fill;
    visuals.open.bg_stroke = Stroke::NONE;
    visuals.open.fg_stroke.color = selected_text;
    visuals.open.corner_radius = corner;
}

pub(crate) fn core_button_fill(
    pal: &crate::frontend::theme::Palette,
    dark: bool,
    selected: bool,
    hovered: bool,
) -> egui::Color32 {
    let alpha = match (dark, selected, hovered) {
        (_, false, false) => 0,
        (false, false, true) => 18,
        (false, true, false) => 42,
        (false, true, true) => 50,
        (true, false, true) => 34,
        (true, true, false) => 52,
        (true, true, true) => 72,
    };
    match (selected, hovered) {
        (false, false) => egui::Color32::TRANSPARENT,
        _ => pal.neutral_overlay(alpha),
    }
}

pub(crate) fn core_button_text_color(
    pal: &crate::frontend::theme::Palette,
    selected: bool,
) -> egui::Color32 {
    if selected {
        pal.text_primary
    } else {
        pal.text_muted
    }
}

/// Color for a view-switcher segment's icon. Keep it neutral so the selected
/// card follows the active theme instead of reading as a fixed blue accent.
pub(crate) fn segment_icon_color(
    view: PrimaryView,
    pal: &crate::frontend::theme::Palette,
    selected: bool,
) -> egui::Color32 {
    let _ = view;
    if selected {
        pal.text_strong
    } else {
        pal.text_muted
    }
}

/// Whether the cartoon / surface overlays are enabled across the active scope —
/// the selection, or all atoms when nothing is selected. Drives the overlay
/// checkboxes: returns `true` only when *every* atom in the scope has the
/// overlay, so the box reflects a uniform state.
pub(crate) fn overlay_state_for_scope(state: &AppState) -> (bool, bool) {
    let structure = state.structure();
    let atom_count = structure.atoms.len();
    if atom_count == 0 {
        return (false, false);
    }
    let indices: Vec<usize> = if state.ui.selection.is_empty() {
        (0..atom_count).collect()
    } else {
        state.ui.selection.ordered_indices()
    };
    if indices.is_empty() {
        return (false, false);
    }
    let cartoon = indices
        .iter()
        .all(|&index| state.ui.viewport.cartoon_enabled(structure, index));
    let surface = indices
        .iter()
        .all(|&index| state.ui.viewport.surface_enabled(structure, index));
    (cartoon, surface)
}

/// A small rounded status pill drawn inline (e.g. "Built-in"). Mirrors the
/// entry-row origin chip: a CHIP-radius filled rect sized snugly to its label.
pub(crate) fn status_pill(
    ui: &mut egui::Ui,
    label: &str,
    fill: egui::Color32,
    text_color: egui::Color32,
) {
    let galley = ui.painter().layout_no_wrap(
        label.to_owned(),
        egui::FontId::proportional(11.0),
        text_color,
    );
    let pad = egui::vec2(7.0, 2.5);
    let (rect, _) = ui.allocate_exact_size(galley.size() + pad * 2.0, Sense::hover());
    ui.painter()
        .rect_filled(rect, f32::from(crate::frontend::theme::radius::CHIP), fill);
    ui.painter()
        .galley(rect.center() - galley.size() / 2.0, galley, text_color);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{backend::config::ColorScheme, frontend::theme::Palette};

    fn fill_alpha(scheme: ColorScheme, dark: bool, selected: bool, hovered: bool) -> u8 {
        let pal = Palette::for_scheme(scheme, dark);
        core_button_fill(&pal, dark, selected, hovered).to_array()[3]
    }

    #[test]
    fn core_button_hover_is_lighter_in_light_mode_across_schemes() {
        for scheme in ColorScheme::all() {
            assert!(
                fill_alpha(scheme, false, false, true) < fill_alpha(scheme, true, false, true),
                "{scheme:?} unselected hover should be lighter in light mode"
            );
            assert!(
                fill_alpha(scheme, false, true, true) < fill_alpha(scheme, true, true, true),
                "{scheme:?} selected hover should be lighter in light mode"
            );
        }
    }

    #[test]
    fn core_button_light_hover_keeps_selected_and_unselected_states_distinct() {
        for scheme in ColorScheme::all() {
            let unselected_hover = fill_alpha(scheme, false, false, true);
            let selected_idle = fill_alpha(scheme, false, true, false);
            let selected_hover = fill_alpha(scheme, false, true, true);

            assert_eq!(fill_alpha(scheme, false, false, false), 0);
            assert!(unselected_hover < selected_idle);
            assert!(selected_idle < selected_hover);
        }
    }
}
