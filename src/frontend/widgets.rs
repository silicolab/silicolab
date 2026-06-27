use eframe::egui;

mod geometry;
mod reticular;

pub use geometry::{bond_geometry_summary, status_text};
pub(crate) use reticular::{
    atom_index_combo, component_preview, component_source_combo_box, default_substitution_site,
    functionalization_selector, preview_substitutions_for,
};

pub fn cell_value(ui: &mut egui::Ui, label: &str, value: &mut f32) -> bool {
    ui.label(label);
    let changed = drag_value(ui, value);
    ui.end_row();
    changed
}

pub fn drag_value(ui: &mut egui::Ui, value: &mut f32) -> bool {
    ui.add_sized(
        [104.0, 20.0],
        egui::DragValue::new(value).speed(0.01).max_decimals(6),
    )
    .changed()
}

pub fn charge_value(ui: &mut egui::Ui, value: &mut f32) -> bool {
    ui.add_sized(
        [72.0, 20.0],
        egui::DragValue::new(value).speed(0.01).max_decimals(4),
    )
    .changed()
}

pub fn supercell_value(ui: &mut egui::Ui, value: &mut u32) -> bool {
    ui.add_sized([44.0, 20.0], egui::DragValue::new(value).range(1..=6))
        .changed()
}

pub fn combo_box<T>(
    ui: &mut egui::Ui,
    label: &str,
    selected: &mut T,
    options: &[T],
    label_for: fn(T) -> &'static str,
) where
    T: Copy + PartialEq,
{
    ui.horizontal(|ui| {
        ui.label(label);
        egui::ComboBox::from_id_salt(label)
            .selected_text(label_for(*selected))
            .show_ui(ui, |ui| {
                crate::frontend::theme::stabilize_selectable_rows(ui);
                for option in options {
                    ui.selectable_value(selected, *option, label_for(*option));
                }
            });
    });
}
