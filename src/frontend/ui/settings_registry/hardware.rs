//! `Control::Custom` body for the Compute Hardware settings panel: a read-only
//! inventory of the detected CPU, GPU, and total RAM, followed by a slider that
//! caps how many cores QM jobs may use.

use eframe::egui::{self, RichText};

use crate::frontend::{actions::AppAction, state::AppState};

/// Render the Compute Hardware panel body.
pub(crate) fn render(state: &mut AppState, ui: &mut egui::Ui, actions: &mut Vec<AppAction>) {
    let hw = crate::backend::hardware::info();
    let pal = crate::frontend::theme::palette(ui);

    ui.label(RichText::new(&hw.cpu_brand).strong());

    let cores_line = match (hw.performance_cores, hw.efficiency_cores) {
        (Some(p), Some(e)) => format!(
            "{} cores ({p}P + {e}E), {} threads",
            hw.physical_cores, hw.logical_cores
        ),
        _ => format!("{} cores, {} threads", hw.physical_cores, hw.logical_cores),
    };
    ui.label(RichText::new(cores_line).color(pal.text_tertiary));

    let gpu = state
        .ui
        .gpu_name
        .clone()
        .unwrap_or_else(|| "Unknown GPU".to_string());
    ui.label(RichText::new(format!("GPU: {gpu}")).color(pal.text_tertiary));

    let ram_gib = hw.total_ram_bytes as f64 / 1024.0_f64.powi(3);
    ui.label(RichText::new(format!("Memory: {ram_gib:.1} GiB")).color(pal.text_tertiary));

    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.label("Cores for QM:");
        let mut cores = state.config.compute_core_count as f32;
        let resp = ui.add(egui::Slider::new(&mut cores, 1.0..=hw.logical_cores as f32).integer());
        if resp.changed() {
            actions.push(AppAction::SetComputeCoreCount(cores.round() as usize));
        }
    });
}
