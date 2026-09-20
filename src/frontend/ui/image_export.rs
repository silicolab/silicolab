use crate::frontend::{actions::AppAction, state::AppState, viewport::ImageExportBackground};
use eframe::egui::{self, Color32};

pub(super) fn render(state: &mut AppState, actions: &mut Vec<AppAction>, ctx: &egui::Context) {
    let Some(prompt) = state.ui.pending_image_export.as_mut() else {
        return;
    };
    let mut open = true;
    egui::Window::new("Export Image")
        .collapsible(false)
        .resizable(false)
        .open(&mut open)
        .show(ctx, |ui| {
            ui.label("PNG output path");
            ui.horizontal(|ui| {
                ui.text_edit_singleline(&mut prompt.path);
                if ui.button("Browse…").clicked() {
                    actions.push(AppAction::ChooseImageExportPath);
                }
            });
            egui::Grid::new("image_export_dimensions").show(ui, |ui| {
                ui.label("Width (px)");
                ui.text_edit_singleline(&mut prompt.width);
                ui.end_row();
                ui.label("Height (px)");
                ui.text_edit_singleline(&mut prompt.height);
                ui.end_row();
            });
            let label = match prompt.background {
                ImageExportBackground::Viewport => "Viewport",
                ImageExportBackground::White => "White",
                ImageExportBackground::Transparent => "Transparent",
                ImageExportBackground::Custom(_) => "Custom",
            };
            egui::ComboBox::from_label("Background")
                .selected_text(label)
                .show_ui(ui, |ui| {
                    for (value, name) in [
                        (ImageExportBackground::Viewport, "Viewport"),
                        (ImageExportBackground::White, "White"),
                        (ImageExportBackground::Transparent, "Transparent"),
                    ] {
                        ui.selectable_value(&mut prompt.background, value, name);
                    }
                    if ui
                        .selectable_label(
                            matches!(prompt.background, ImageExportBackground::Custom(_)),
                            "Custom",
                        )
                        .clicked()
                        && !matches!(prompt.background, ImageExportBackground::Custom(_))
                    {
                        prompt.background = ImageExportBackground::Custom(Color32::WHITE);
                    }
                });
            if let ImageExportBackground::Custom(color) = &mut prompt.background {
                let mut rgb = [color.r(), color.g(), color.b()];
                ui.color_edit_button_srgb(&mut rgb);
                *color = Color32::from_rgb(rgb[0], rgb[1], rgb[2]);
            }
            ui.label("Transparent backgrounds are intended for white-page layouts.");
            let validation = prompt.validate();
            if let Err(error) = &validation {
                ui.colored_label(ui.visuals().error_fg_color, error.to_string());
            }
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(validation.is_ok(), egui::Button::new("Export"))
                    .clicked()
                {
                    actions.push(AppAction::RunImageExport);
                }
                if ui.button("Cancel").clicked() {
                    actions.push(AppAction::CancelImageExport);
                }
            });
        });
    if !open {
        actions.push(AppAction::CancelImageExport);
    }
}
