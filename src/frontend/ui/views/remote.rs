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

/// One launch editor per external engine on `id`, the same widget this machine's
/// engines use. The stored host supplies the status (and its verification); the
/// draft supplies what is being edited.
fn render_host_engines(
    state: &mut AppState,
    ui: &mut egui::Ui,
    id: &str,
    actions: &mut Vec<AppAction>,
) {
    use crate::backend::config::ComputeTarget;
    use crate::engines::registry::{EngineStatus, external_engine_specs};
    use crate::frontend::ui::views::engine_row::{EngineRow, engine_row};

    let target = ComputeTarget::Remote(id.to_string());
    for spec in external_engine_specs() {
        let status = match state
            .config
            .remote_hosts
            .get(id)
            .and_then(|host| host.engines.entry(spec.id))
        {
            Some(entry) => match &entry.verified {
                Some(verified) => EngineStatus::Verified {
                    launch: entry.launch.clone(),
                    version: verified.version.clone(),
                    checked_at: verified.checked_at,
                },
                None => EngineStatus::Unverified {
                    launch: entry.launch.clone(),
                },
            },
            None => EngineStatus::NotConfigured,
        };
        let probe = state
            .ui
            .settings
            .engine_probe
            .get(&(target.clone(), spec.id.as_str()))
            .cloned();
        let Some(draft) = state.ui.settings.remote_host_drafts.get_mut(id) else {
            return;
        };
        let draft = draft
            .engines
            .entry(spec.id.as_str().to_string())
            .or_default();

        engine_row(
            ui,
            EngineRow {
                target: target.clone(),
                engine: spec.id,
                name: spec.name,
                description: spec.description,
                status,
                probe,
                // A remote path is not on this filesystem; there is nothing to browse.
                browsable: false,
            },
            draft,
            actions,
        );
    }
}

/// The Compute ▸ Compute targets group: this machine and each configured remote
/// host as peer collapsers, each owning that target's hardware and engine
/// launches, plus the add-host form.
pub(crate) fn render_compute_targets(
    state: &mut AppState,
    ui: &mut egui::Ui,
    actions: &mut Vec<AppAction>,
) {
    egui::CollapsingHeader::new(RichText::new("This machine").strong())
        .id_salt("compute_target_local")
        .default_open(true)
        .show(ui, |ui| {
            crate::frontend::ui::settings_registry::hardware::render(state, ui, actions);
            ui.add_space(8.0);
            crate::frontend::ui::render_engine_settings(state, ui, actions);
        });
    render_remote_hosts_settings(state, ui, actions);
}

/// The Hardware section of a remote host's card: fetch and show its static
/// CPU / memory / GPU inventory over SSH, cached per host in `remote_hardware`.
fn render_host_hardware(
    state: &mut AppState,
    ui: &mut egui::Ui,
    id: &str,
    actions: &mut Vec<AppAction>,
) {
    let pal = crate::frontend::theme::palette(ui);

    let fetching = state.jobs.remote_hardware.is_some();
    let button_label = if fetching {
        "Fetching…"
    } else {
        "Fetch hardware"
    };
    if ui
        .add_enabled(!fetching, egui::Button::new(button_label))
        .clicked()
    {
        actions.push(AppAction::FetchRemoteHardware(id.to_string()));
    }

    if let Some(info) = state.ui.settings.remote_hardware.get(id) {
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

    ui.add_space(8.0);
    ui.label(
        RichText::new("Live GPU monitoring moved to the sidebar system monitor.")
            .color(pal.text_tertiary),
    );
}

/// The remote hosts within Compute targets: each configured host as a collapser
/// with editable connection fields, its hardware probe, its engine launches, an
/// optional scheduler block, a Test/Set-up-passwordless row, and an add-host form.
/// Every action here commits the host's draft first, so what it contacts is what
/// the user is looking at. Network work runs on worker threads (see the
/// dispatcher), so the panel stays responsive.
pub(crate) fn render_remote_hosts_settings(
    state: &mut AppState,
    ui: &mut egui::Ui,
    actions: &mut Vec<AppAction>,
) {
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
        // A custom header rather than `CollapsingHeader`: whether a host is reachable
        // must be readable without expanding it, which a plain-text header cannot show.
        let header_id = ui.make_persistent_id(("compute_target_host", id.as_str()));
        egui::collapsing_header::CollapsingState::load_with_default_open(
            ui.ctx(),
            header_id,
            false,
        )
        .show_header(ui, |ui| {
            ui.label(RichText::new(label).strong());
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                remote_status_indicator(ui, &status, &pal);
            });
        })
        .body(|ui| {
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
                ui.horizontal(|ui| {
                    ui.label("Execution backend:");
                    ui.selectable_value(&mut draft.slurm, false, "Direct SSH");
                    ui.selectable_value(&mut draft.slurm, true, "Slurm");
                });
                if draft.slurm {
                    ui.label(caption_text(
                        "Scheduler setup commands (one per line):",
                        pal.text_muted,
                    ));
                    ui.add(
                        egui::TextEdit::multiline(&mut draft.scheduler_prelude)
                            .desired_rows(2)
                            .desired_width(f32::INFINITY),
                    );
                    remote_host_field(ui, "Partition:", &mut draft.partition);
                    remote_host_field(ui, "Account:", &mut draft.account);
                    remote_host_field(ui, "QOS:", &mut draft.qos);
                    remote_host_field(ui, "Default CPUs:", &mut draft.default_cpus);
                    remote_host_field(ui, "Default memory (MiB):", &mut draft.default_memory_mib);
                    remote_host_field(
                        ui,
                        "Default walltime (minutes):",
                        &mut draft.default_walltime_minutes,
                    );
                    default_gpu_fields(ui, draft);
                    ui.horizontal(|ui| {
                        ui.label("GPU syntax:");
                        ui.selectable_value(&mut draft.gpu_syntax, "gres".to_string(), "GRES");
                        ui.selectable_value(&mut draft.gpu_syntax, "gpus".to_string(), "--gpus");
                        ui.selectable_value(&mut draft.gpu_syntax, "custom".to_string(), "Custom");
                    });
                    if draft.gpu_syntax == "gres" {
                        remote_host_field(ui, "GRES name:", &mut draft.gres_name);
                    } else if draft.gpu_syntax == "custom" {
                        remote_host_field(
                            ui,
                            "GPU argument template:",
                            &mut draft.custom_gpu_argument,
                        );
                    }
                    ui.collapsing("Advanced", |ui| {
                        remote_host_field(ui, "Reservation:", &mut draft.reservation);
                        remote_host_field(ui, "Constraint:", &mut draft.constraint);
                        ui.label(caption_text(
                            "Extra arguments (one argv token per line):",
                            pal.text_muted,
                        ));
                        ui.add(
                            egui::TextEdit::multiline(&mut draft.extra_args)
                                .desired_rows(2)
                                .desired_width(f32::INFINITY),
                        );
                    });
                }
            }

            render_host_hardware(state, ui, id, actions);

            render_host_engines(state, ui, id, actions);

            ui.horizontal(|ui| {
                if ui.button("Save").clicked() {
                    actions.push(AppAction::SaveRemoteHost(id.clone()));
                }
                if ui.button("Test connection").clicked() {
                    actions.push(AppAction::CheckRemoteHost(id.clone()));
                }
                if ui.button("Set up passwordless login").clicked() {
                    actions.push(AppAction::SetupRemoteHostKey(id.clone()));
                }
                if state.config.remote_hosts.get(id).is_some_and(|host| {
                    matches!(
                        host.scheduler,
                        crate::backend::config::SchedulerConfig::Slurm(_)
                    )
                }) {
                    if ui.button("Detect Slurm").clicked() {
                        actions.push(AppAction::DetectRemoteSlurm(id.clone()));
                    }
                    if ui.button("Refresh cluster").clicked() {
                        actions.push(AppAction::RefreshSlurmCapabilities(id.clone()));
                    }
                    if ui.button("Test scheduler").clicked() {
                        actions.push(AppAction::TestRemoteSlurm(id.clone()));
                    }
                }
            });

            if let Some(capabilities) = state.ui.settings.slurm_capabilities.get(id) {
                ui.label(caption_text(
                    format!(
                        "Partitions: {} · GPU types: {} · Features: {}",
                        capabilities.partitions.join(", "),
                        capabilities.gpu_types.join(", "),
                        capabilities.features.join(", ")
                    ),
                    pal.text_muted,
                ));
            }

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
        });
    }

    egui::CollapsingHeader::new(RichText::new("Add a host").strong())
        .id_salt("compute_target_add_host")
        .default_open(false)
        .show(ui, |ui| {
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
            // No engine editor here: verifying one needs a host to reach, so engines are
            // configured on the host's own card once it exists.
            ui.horizontal(|ui| {
                ui.label("Execution backend:");
                ui.selectable_value(&mut draft.slurm, false, "Direct SSH");
                ui.selectable_value(&mut draft.slurm, true, "Slurm");
            });
            if draft.slurm {
                ui.label(caption_text(
                    "Scheduler setup commands (one per line):",
                    pal.text_muted,
                ));
                ui.add(
                    egui::TextEdit::multiline(&mut draft.scheduler_prelude)
                        .desired_rows(2)
                        .desired_width(f32::INFINITY),
                );
                remote_host_field(ui, "Partition:", &mut draft.partition);
                remote_host_field(ui, "Account:", &mut draft.account);
                remote_host_field(ui, "QOS:", &mut draft.qos);
                remote_host_field(ui, "Default CPUs:", &mut draft.default_cpus);
                remote_host_field(ui, "Default memory (MiB):", &mut draft.default_memory_mib);
                remote_host_field(
                    ui,
                    "Default walltime (minutes):",
                    &mut draft.default_walltime_minutes,
                );
                default_gpu_fields(ui, draft);
                if draft.gpu_syntax.is_empty() {
                    draft.gpu_syntax = "gres".to_string();
                }
                if draft.gres_name.is_empty() {
                    draft.gres_name = "gpu".to_string();
                }
                ui.horizontal(|ui| {
                    ui.label("GPU syntax:");
                    ui.selectable_value(&mut draft.gpu_syntax, "gres".to_string(), "GRES");
                    ui.selectable_value(&mut draft.gpu_syntax, "gpus".to_string(), "--gpus");
                    ui.selectable_value(&mut draft.gpu_syntax, "custom".to_string(), "Custom");
                });
                if draft.gpu_syntax == "gres" {
                    remote_host_field(ui, "GRES name:", &mut draft.gres_name);
                } else if draft.gpu_syntax == "custom" {
                    remote_host_field(ui, "GPU argument template:", &mut draft.custom_gpu_argument);
                }
            }
            if ui.button("Add host").clicked() {
                actions.push(AppAction::AddRemoteHost);
            }
        });
}

fn default_gpu_fields(ui: &mut egui::Ui, draft: &mut crate::frontend::state::RemoteHostDraft) {
    if draft.default_gpu_kind.is_empty() {
        draft.default_gpu_kind = "none".to_string();
    }
    ui.horizontal(|ui| {
        ui.label("Default GPU:");
        egui::ComboBox::from_id_salt("host_default_gpu")
            .selected_text(match draft.default_gpu_kind.as_str() {
                "any" => "Any available",
                "typed" => "Specific type",
                _ => "No GPU",
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut draft.default_gpu_kind, "none".to_string(), "No GPU");
                ui.selectable_value(
                    &mut draft.default_gpu_kind,
                    "any".to_string(),
                    "Any available",
                );
                ui.selectable_value(
                    &mut draft.default_gpu_kind,
                    "typed".to_string(),
                    "Specific type",
                );
            });
        if draft.default_gpu_kind == "typed" {
            ui.label("Type:");
            ui.add(egui::TextEdit::singleline(&mut draft.default_gpu_type).desired_width(90.0));
        }
        if draft.default_gpu_kind != "none" {
            ui.label("Count:");
            ui.add(egui::TextEdit::singleline(&mut draft.default_gpu_count).desired_width(45.0));
        }
    });
}
