//! The shared "where and how does this job run" controls, reused by every task
//! panel (and the Settings default). [`execution_section`] renders the compute
//! target plus the resource envelope in one consistent block; a task passes
//! [`ExecutionCaps`] to say which resource knobs it honours, and the rest render
//! greyed out rather than hidden so every panel reads the same. The widgets only
//! edit the passed-in [`ExecutionPrefs`] / emit actions — the dispatcher owns the
//! translation into each engine's request.

use eframe::egui;

use crate::backend::config::ComputeTarget;
use crate::frontend::{
    actions::AppAction,
    state::{AppState, ExecutionCaps, ExecutionPrefs},
};

/// Generous upper bounds for the resource fields. They cap accidental fat-finger
/// entries, not real limits — the engine (or the host probe) clamps for real.
const MAX_SUBTASKS: u32 = 256;
const MAX_GPUS: u32 = 16;
const MAX_MEMORY_MIB: u32 = 1_048_576; // 1 TiB

/// `(id, label)` for every configured remote host, sorted by label — the option
/// list behind the compute-target picker.
pub(crate) fn remote_host_options(state: &AppState) -> Vec<(String, String)> {
    let mut hosts: Vec<(String, String)> = state
        .config
        .remote_hosts
        .values()
        .map(|host| (host.id.clone(), host.label.clone()))
        .collect();
    hosts.sort_by_key(|host| host.1.to_lowercase());
    hosts
}

/// The compute-target picker: "This machine" plus every configured remote host,
/// and an always-visible "Add host…" button that opens the Remote Hosts settings
/// so a user can discover host configuration without already knowing where it
/// lives. Mutates `target` in place; `actions` carries the open-settings request.
/// `id_salt` distinguishes instances that can be on screen at once (e.g. a task
/// panel's picker and the Settings default-target picker over it).
pub(crate) fn compute_target_picker(
    ui: &mut egui::Ui,
    target: &mut ComputeTarget,
    hosts: &[(String, String)],
    actions: &mut Vec<AppAction>,
    id_salt: &str,
) {
    let selected = match target {
        ComputeTarget::Local => "This machine".to_string(),
        ComputeTarget::Remote(id) => hosts
            .iter()
            .find(|(host_id, _)| host_id == id)
            .map(|(_, label)| label.clone())
            .unwrap_or_else(|| "(unconfigured host)".to_string()),
    };
    ui.horizontal(|ui| {
        ui.label("Run on:");
        egui::ComboBox::from_id_salt(id_salt)
            .selected_text(selected)
            .show_ui(ui, |ui| {
                crate::frontend::theme::stabilize_selectable_rows(ui);
                ui.selectable_value(target, ComputeTarget::Local, "This machine");
                for (id, label) in hosts {
                    ui.selectable_value(target, ComputeTarget::Remote(id.clone()), label);
                }
            });
        if ui
            .button(format!("{}  Add host…", egui_phosphor::regular::PLUS))
            .on_hover_text("Add or manage remote hosts in Settings")
            .clicked()
        {
            actions.push(AppAction::OpenRemoteHostsSettings);
        }
    });
}

/// One labelled integer resource field. Disabled (greyed) fields still render —
/// with a hover note explaining why — so the section keeps a uniform shape across
/// tasks. `zero_is_auto` shows `0` as "Auto" (the engine's own default).
fn resource_field(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut u32,
    enabled: bool,
    range: std::ops::RangeInclusive<u32>,
    zero_is_auto: bool,
    disabled_hint: &str,
) {
    ui.horizontal(|ui| {
        ui.label(label);
        let mut drag = egui::DragValue::new(value).range(range);
        if zero_is_auto {
            drag = drag.custom_formatter(|n, _| {
                if n <= 0.0 {
                    "Auto".to_string()
                } else {
                    (n as i64).to_string()
                }
            });
        }
        ui.add_enabled(enabled, drag)
            .on_disabled_hover_text(disabled_hint);
    });
}

/// Render the compute target plus the resource envelope as one consistent block.
/// `hosts` is precomputed by the caller (via [`remote_host_options`]) so this can
/// take `prefs` mutably without also borrowing the whole [`AppState`].
pub(crate) fn execution_section(
    ui: &mut egui::Ui,
    prefs: &mut ExecutionPrefs,
    caps: ExecutionCaps,
    hosts: &[(String, String)],
    actions: &mut Vec<AppAction>,
) {
    compute_target_picker(ui, &mut prefs.target, hosts, actions, "compute_target");

    let max_cores = (crate::backend::hardware::info().logical_cores as u32).max(1);
    resource_field(
        ui,
        "Subtasks",
        &mut prefs.subtasks,
        caps.subtasks,
        1..=MAX_SUBTASKS,
        false,
        "Parallel subtasks aren't available for this task.",
    );
    resource_field(
        ui,
        "CPU cores / subtask",
        &mut prefs.cores_per_subtask,
        caps.cores,
        0..=max_cores,
        true,
        "This task doesn't expose a core count.",
    );
    resource_field(
        ui,
        "GPUs",
        &mut prefs.gpu_count,
        caps.gpu,
        0..=MAX_GPUS,
        true,
        "GPU selection isn't available for this task yet.",
    );
    resource_field(
        ui,
        "Memory (MiB)",
        &mut prefs.memory_mib,
        caps.memory,
        0..=MAX_MEMORY_MIB,
        true,
        "This task doesn't expose a memory cap.",
    );
}
