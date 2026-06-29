use eframe::egui::{self, Align, Layout, RichText};

use crate::frontend::actions::AppAction;
use crate::frontend::state::AppState;
use crate::frontend::ui::settings_registry::caption_text;

/// One labeled single-line text field row for a remote-host draft.
fn remote_host_field(ui: &mut egui::Ui, label: &str, value: &mut String) {
    ui.horizontal(|ui| {
        ui.label(label);
        ui.add(egui::TextEdit::singleline(value).desired_width(f32::INFINITY));
    });
}

/// The connection-status indicator for a remote host.
fn remote_status_indicator(
    ui: &mut egui::Ui,
    status: &crate::frontend::state::RemoteHostStatus,
    pal: &crate::frontend::theme::Palette,
) {
    use crate::frontend::state::RemoteHostStatus;
    match status {
        RemoteHostStatus::Unknown => {
            ui.label(caption_text("Not checked", pal.text_muted));
        }
        RemoteHostStatus::Checking => {
            ui.label(caption_text("Checking…", pal.text_muted));
        }
        RemoteHostStatus::Ready => {
            ui.label(caption_text(
                format!("{}  Connected", egui_phosphor::regular::CHECK_CIRCLE),
                pal.status_green,
            ));
        }
        RemoteHostStatus::NeedsSetup => {
            ui.label(caption_text(
                format!(
                    "{}  Needs passwordless setup",
                    egui_phosphor::regular::WARNING
                ),
                pal.text_muted,
            ));
        }
        RemoteHostStatus::Unreachable(reason) => {
            ui.label(caption_text(
                format!("{}  Unreachable", egui_phosphor::regular::X_CIRCLE),
                pal.text_muted,
            ))
            .on_hover_text(reason);
        }
    }
}

/// The Remote Hosts editor (Settings → Engines → Remote Hosts). Lists configured
/// hosts with editable connection fields, a Detect/Test/Set-up-passwordless row,
/// and an "add host" form. Network actions run on worker threads (see the
/// dispatcher), so the panel stays responsive.
pub(crate) fn render_remote_hosts_settings(
    state: &mut AppState,
    ui: &mut egui::Ui,
    actions: &mut Vec<AppAction>,
) {
    use crate::engines::registry::EngineId;
    use crate::frontend::state::RemoteHostDraft;

    let pal = crate::frontend::theme::palette(ui);
    ui.label(caption_text(
        "Run external engines (GROMACS) on a remote host over SSH. Login is key-based — no \
             passwords are stored. Pick a host per task with the Compute selector in the Run/Build \
             panels.",
        pal.text_muted,
    ));

    // Owned, sorted (id, label) list so the loop doesn't borrow config while the
    // per-host draft is edited mutably.
    let mut hosts: Vec<(String, String)> = state
        .config
        .remote_hosts
        .values()
        .map(|host| (host.id.clone(), host.label.clone()))
        .collect();
    hosts.sort_by_key(|host| host.1.to_lowercase());

    if hosts.is_empty() {
        ui.label(caption_text(
            "No remote hosts configured yet.",
            pal.text_muted,
        ));
    }

    for (id, label) in &hosts {
        // Seed the editable draft from the stored host on first show.
        if !state.ui.settings.remote_host_drafts.contains_key(id)
            && let Some(host) = state.config.remote_hosts.get(id)
        {
            state
                .ui
                .settings
                .remote_host_drafts
                .insert(id.clone(), RemoteHostDraft::from_host(host));
        }
        // Snapshots taken before the mutable draft borrow below.
        let status = state
            .ui
            .settings
            .remote_status
            .get(id)
            .cloned()
            .unwrap_or_default();
        let bootstrap_cmd = match &state.ui.settings.remote_bootstrap {
            Some((bid, cmd)) if bid == id => Some(cmd.clone()),
            _ => None,
        };
        let version = state.config.remote_hosts.get(id).and_then(|host| {
            host.engine_versions
                .get(EngineId::GROMACS.as_str())
                .cloned()
        });

        ui.separator();
        ui.horizontal(|ui| {
            ui.label(RichText::new(label).strong());
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                remote_status_indicator(ui, &status, &pal);
            });
        });

        if let Some(draft) = state.ui.settings.remote_host_drafts.get_mut(id) {
            remote_host_field(ui, "Label:", &mut draft.label);
            remote_host_field(ui, "Host:", &mut draft.hostname);
            remote_host_field(ui, "User:", &mut draft.username);
            remote_host_field(ui, "Port:", &mut draft.port);
            remote_host_field(ui, "Work dir:", &mut draft.work_root);
            ui.label(caption_text(
                "Setup commands (one per line, e.g. module load gromacs):",
                pal.text_muted,
            ));
            ui.add(
                egui::TextEdit::multiline(&mut draft.prelude)
                    .desired_rows(2)
                    .desired_width(f32::INFINITY),
            );
            remote_host_field(ui, "GROMACS path:", &mut draft.gmx_program);
        }
        if let Some(version) = &version {
            ui.label(caption_text(
                format!("Detected GROMACS {version}"),
                pal.status_green,
            ));
        }

        ui.horizontal(|ui| {
            if ui.button("Save").clicked() {
                actions.push(AppAction::SaveRemoteHost(id.clone()));
            }
            if ui.button("Detect GROMACS").clicked() {
                actions.push(AppAction::DetectRemoteGromacs(id.clone()));
            }
            if ui.button("Test connection").clicked() {
                actions.push(AppAction::CheckRemoteHost(id.clone()));
            }
            if ui.button("Set up passwordless login").clicked() {
                actions.push(AppAction::SetupRemoteHostKey(id.clone()));
            }
        });

        ui.add_space(8.0);
        if crate::frontend::ui::widgets::confirm_destructive(
            ui,
            ("remove_remote_host", id.as_str()),
            "Remove this host and its connection settings?",
            "Remove",
            |ui| ui.button(RichText::new("Remove").color(pal.status_red)),
        ) {
            actions.push(AppAction::RemoveRemoteHost(id.clone()));
        }

        if let Some(mut command) = bootstrap_cmd {
            ui.label(caption_text(
                "Run this once on the remote host (paste into a terminal, or type \
                     `! <command>` in this prompt), then Verify:",
                pal.text_muted,
            ));
            ui.add(
                egui::TextEdit::multiline(&mut command)
                    .desired_rows(2)
                    .desired_width(f32::INFINITY)
                    .font(egui::TextStyle::Monospace),
            );
            if ui.button("Verify").clicked() {
                actions.push(AppAction::CheckRemoteHost(id.clone()));
            }
        }
        ui.add_space(8.0);
    }

    // ---- Add a host ------------------------------------------------------
    ui.separator();
    ui.label(RichText::new("Add a remote host").strong());
    let draft = &mut state.ui.settings.new_remote_host;
    remote_host_field(ui, "Label:", &mut draft.label);
    remote_host_field(ui, "Host:", &mut draft.hostname);
    remote_host_field(ui, "User:", &mut draft.username);
    remote_host_field(ui, "Port:", &mut draft.port);
    remote_host_field(ui, "Work dir:", &mut draft.work_root);
    ui.label(caption_text(
        "Setup commands (one per line):",
        pal.text_muted,
    ));
    ui.add(
        egui::TextEdit::multiline(&mut draft.prelude)
            .desired_rows(2)
            .desired_width(f32::INFINITY),
    );
    remote_host_field(
        ui,
        "GROMACS path (optional — use Detect after adding):",
        &mut draft.gmx_program,
    );
    if ui.button("Add host").clicked() {
        actions.push(AppAction::AddRemoteHost);
    }
}
