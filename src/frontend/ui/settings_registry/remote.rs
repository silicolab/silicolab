//! `Control::Custom` body for Hardware ▸ Remote host: pick a configured remote
//! host and fetch its static hardware inventory (CPU / memory / GPU) over SSH.
//! The fetch runs on a worker thread (`AppAction::FetchRemoteHardware`); results
//! are cached per host in `SettingsState::remote_hardware` and shown here.

use eframe::egui::{self, RichText};

use crate::frontend::{actions::AppAction, state::AppState};

/// Render the remote-hardware panel body.
pub(crate) fn render(state: &mut AppState, ui: &mut egui::Ui, actions: &mut Vec<AppAction>) {
    let pal = crate::frontend::theme::palette(ui);

    // Hosts to choose from (id + label), sorted by label for a stable list.
    let mut hosts: Vec<(String, String)> = state
        .config
        .remote_hosts
        .values()
        .map(|host| (host.id.clone(), host.label.clone()))
        .collect();
    hosts.sort_by(|a, b| a.1.cmp(&b.1));

    if hosts.is_empty() {
        ui.label(
            RichText::new("No remote hosts configured. Add one in Engines ▸ Remote hosts.")
                .color(pal.text_tertiary),
        );
        return;
    }

    // Default to the first host until the user picks one.
    if state.ui.settings.remote_hardware_host.is_none() {
        state.ui.settings.remote_hardware_host = Some(hosts[0].0.clone());
    }
    let selected_id = state.ui.settings.remote_hardware_host.clone();
    let selected_label = selected_id
        .as_ref()
        .and_then(|id| hosts.iter().find(|(hid, _)| hid == id))
        .map(|(_, label)| label.clone())
        .unwrap_or_else(|| "Select a host".to_string());

    ui.horizontal(|ui| {
        ui.label("Host:");
        egui::ComboBox::from_id_salt("remote_hardware_host")
            .selected_text(selected_label)
            .show_ui(ui, |ui| {
                crate::frontend::theme::stabilize_selectable_rows(ui);
                for (id, label) in &hosts {
                    ui.selectable_value(
                        &mut state.ui.settings.remote_hardware_host,
                        Some(id.clone()),
                        label,
                    );
                }
            });

        let fetching = state.jobs.remote_hardware.is_some();
        let button_label = if fetching {
            "Fetching…"
        } else {
            "Fetch hardware"
        };
        if ui
            .add_enabled(!fetching, egui::Button::new(button_label))
            .clicked()
            && let Some(id) = state.ui.settings.remote_hardware_host.clone()
        {
            actions.push(AppAction::FetchRemoteHardware(id));
        }
    });

    // Cached inventory for the selected host, if we have fetched it.
    if let Some(id) = state.ui.settings.remote_hardware_host.clone()
        && let Some(info) = state.ui.settings.remote_hardware.get(&id)
    {
        ui.add_space(6.0);
        ui.label(RichText::new(&info.cpu_model).strong());

        let cores_line = match (info.cores, info.threads) {
            (Some(cores), Some(threads)) => format!("{cores} cores, {threads} threads"),
            (Some(cores), None) => format!("{cores} cores"),
            (None, Some(threads)) => format!("{threads} threads"),
            (None, None) => "Core count unavailable".to_string(),
        };
        ui.label(RichText::new(cores_line).color(pal.text_tertiary));

        if let Some(bytes) = info.ram_bytes {
            let gib = bytes as f64 / 1024.0_f64.powi(3);
            ui.label(RichText::new(format!("Memory: {gib:.1} GiB")).color(pal.text_tertiary));
        }

        if info.gpus.is_empty() {
            ui.label(RichText::new("GPU: none detected").color(pal.text_tertiary));
        } else {
            for gpu in &info.gpus {
                ui.label(RichText::new(format!("GPU: {gpu}")).color(pal.text_tertiary));
            }
        }
    }
}
