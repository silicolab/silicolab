//! `Control::Custom` body for the Compute Hardware settings panel: a read-only
//! inventory of the detected CPU, GPU, and total RAM. The core-count cap for QM
//! jobs lives next to the QM panel's Run button now, not here, so it is set where
//! the job is launched.

use eframe::egui::{self, RichText};

use crate::frontend::{actions::AppAction, state::AppState};

/// Render the Compute Hardware panel body.
pub(crate) fn render(state: &mut AppState, ui: &mut egui::Ui, _actions: &mut Vec<AppAction>) {
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

    let gpus = crate::backend::hardware::gpus();
    if gpus.is_empty() {
        // No enumerated inventory (headless / software renderer): fall back to the
        // render-adapter name captured at startup.
        let gpu = state
            .ui
            .gpu_name
            .clone()
            .unwrap_or_else(|| "Unknown GPU".to_string());
        ui.label(RichText::new(format!("GPU: {gpu}")).color(pal.text_tertiary));
    } else {
        for gpu in gpus {
            let rendering = state.ui.gpu_name.as_deref() == Some(gpu.name.as_str());
            let mut line = format!("GPU: {} ({})", gpu.name, gpu.kind.label());
            if rendering {
                line.push_str(" · rendering");
            }
            ui.label(RichText::new(line).color(pal.text_tertiary));
            if let Some(detail) = crate::frontend::gpu_monitor::find_sample(&state.ui.gpus, gpu)
                .and_then(crate::frontend::gpu_monitor::live_line)
            {
                ui.label(
                    RichText::new(format!("    {detail}"))
                        .small()
                        .color(pal.text_tertiary),
                );
            }
        }
    }

    let ram_gib = hw.total_ram_bytes as f64 / 1024.0_f64.powi(3);
    ui.label(RichText::new(format!("Memory: {ram_gib:.1} GiB")).color(pal.text_tertiary));
}
