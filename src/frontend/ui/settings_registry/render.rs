//! Building the registry and rendering it: `registry()` assembles the
//! descriptors (applying platform availability gates), and the render functions
//! turn descriptors into widgets that emit `AppAction`s.

use super::*;

use eframe::egui::{self, RichText};

use crate::frontend::{actions::AppAction, state::AppState};

/// The current settings registry. Built fresh per call (a handful of cheap
/// descriptors); availability gates that depend on the platform — like glass
/// support — are applied here so unavailable settings are simply absent.
pub fn registry() -> Vec<SettingDescriptor> {
    let mut items = vec![
        SettingDescriptor {
            id: "appearance.theme",
            category: SettingCategory::General,
            group: "Appearance",
            title: "Theme",
            description: "Light or dark interface, or follow the operating system.",
            keywords: &["dark", "light", "mode", "system", "appearance"],
            control: Control::Choice {
                read: theme_read,
                options: &THEME_OPTIONS,
                on_change: theme_change,
            },
            enabled: None,
            indent: false,
            is_default: Some(theme_is_default),
            reset: Some(theme_reset),
        },
        SettingDescriptor {
            id: "appearance.color_scheme",
            category: SettingCategory::General,
            group: "Appearance",
            title: "Color scheme",
            description: "Accent and neutral palette, applied on top of light/dark.",
            keywords: &["accent", "palette", "color", "colour", "scheme"],
            control: Control::Choice {
                read: scheme_read,
                options: &SCHEME_OPTIONS,
                on_change: scheme_change,
            },
            enabled: None,
            indent: false,
            is_default: Some(scheme_is_default),
            reset: Some(scheme_reset),
        },
    ];

    // Frosted-glass settings only exist where the platform supports the material
    // (matches today's `if glass::supported()` guard around both widgets).
    if crate::frontend::glass::supported() {
        items.push(SettingDescriptor {
            id: "appearance.transparency",
            category: SettingCategory::General,
            group: "Appearance",
            title: "Transparency",
            description: "Frosted-glass material on the window chrome.",
            keywords: &["glass", "vibrancy", "frosted", "transparency", "blur"],
            control: Control::Toggle {
                read: glass_read,
                on_change: glass_change,
            },
            enabled: None,
            indent: false,
            is_default: Some(glass_is_default),
            reset: Some(glass_reset),
        });
        items.push(SettingDescriptor {
            id: "appearance.blur_intensity",
            category: SettingCategory::General,
            group: "Appearance",
            title: "Blur Intensity",
            description: "How tinted the frosted chrome reads, from clear to fully tinted.",
            keywords: &["glass", "blur", "intensity", "tint", "vibrancy"],
            control: Control::Slider {
                read: blur_read,
                range: 0.0..=1.0,
                on_change: blur_change,
                // No numeric box, and indented below — reads as nested under the
                // Transparency toggle, matching the pre-registry control.
                show_value: false,
            },
            // Disabled while transparency is off, exactly like the hand-coded UI.
            enabled: Some(glass_enabled),
            indent: true,
            is_default: Some(blur_is_default),
            reset: Some(blur_reset),
        });
    }

    // General ▸ Startup & Projects.
    items.push(SettingDescriptor {
        id: "startup.reopen_last_project",
        category: SettingCategory::General,
        group: "Startup & Projects",
        title: "Reopen last project on launch",
        description: "When off, SilicoLab starts in a blank scratch workspace instead.",
        keywords: &[
            "startup", "launch", "reopen", "project", "scratch", "session",
        ],
        control: Control::Toggle {
            read: reopen_read,
            on_change: reopen_change,
        },
        enabled: None,
        indent: false,
        is_default: Some(reopen_is_default),
        reset: Some(reopen_reset),
    });
    items.push(SettingDescriptor {
        id: "startup.check_updates",
        category: SettingCategory::General,
        group: "Startup & Projects",
        title: "Check for updates on launch",
        description: "Looks up the latest release on GitHub once per launch and shows a notice \
                      in the status bar when a newer version exists. Nothing is downloaded or \
                      installed automatically.",
        keywords: &[
            "update",
            "updates",
            "upgrade",
            "release",
            "version",
            "github",
            "automatic",
        ],
        control: Control::Toggle {
            read: check_updates_read,
            on_change: check_updates_change,
        },
        enabled: None,
        indent: false,
        is_default: Some(check_updates_is_default),
        reset: Some(check_updates_reset),
    });
    items.push(SettingDescriptor {
        id: "startup.auto_install_updates",
        category: SettingCategory::General,
        group: "Startup & Projects",
        title: "Install updates automatically",
        description: "When a newer release is found, download and install it in the background \
                      (a restart still applies it) instead of waiting for you to click \"Update\". \
                      Only applies when the install location is writable.",
        keywords: &[
            "update",
            "updates",
            "upgrade",
            "automatic",
            "auto",
            "install",
            "download",
            "background",
        ],
        control: Control::Toggle {
            read: auto_install_updates_read,
            on_change: auto_install_updates_change,
        },
        enabled: Some(auto_install_updates_enabled),
        indent: true,
        is_default: Some(auto_install_updates_is_default),
        reset: Some(auto_install_updates_reset),
    });
    items.push(SettingDescriptor {
        id: "startup.default_project_dir",
        category: SettingCategory::General,
        group: "Startup & Projects",
        title: "Default project folder",
        description: "Where new projects are created and file dialogs start.",
        keywords: &[
            "project",
            "folder",
            "directory",
            "path",
            "location",
            "default",
        ],
        control: Control::Custom(render_default_project_dir),
        enabled: None,
        indent: false,
        // A user-chosen path has no meaningful "default" to restore to.
        is_default: None,
        reset: None,
    });

    // Engines: wrap the existing hand-coded editor wholesale. Strong keywords
    // because search on a Custom descriptor is all-or-nothing on this metadata.
    // Empty description so no help line renders under the whole editor.
    items.push(SettingDescriptor {
        id: "engines.registry",
        category: SettingCategory::Engines,
        group: "Engines",
        title: "Compute engines",
        description: "",
        keywords: &[
            "engine", "engines", "gromacs", "lammps", "path", "binary", "wsl", "version", "detect",
        ],
        control: Control::Custom(crate::frontend::ui::render_engine_settings),
        enabled: None,
        indent: false,
        is_default: None,
        reset: None,
    });

    // Remote Hosts: SSH execution targets. Its own group under Engines so the
    // local-engine editor above stays uncluttered.
    items.push(SettingDescriptor {
        id: "engines.remote_hosts",
        category: SettingCategory::Engines,
        group: "Remote Hosts",
        title: "Remote hosts (SSH)",
        description: "",
        keywords: &[
            "remote",
            "ssh",
            "host",
            "hpc",
            "cluster",
            "scp",
            "submit",
            "compute",
            "gromacs",
            "passwordless",
            "key",
        ],
        control: Control::Custom(crate::frontend::ui::render_remote_hosts_settings),
        enabled: None,
        indent: false,
        is_default: None,
        reset: None,
    });

    // Assistant: the in-app LLM agent. Custom editor (provider/model/effort/key),
    // data-driven from the agent registry. Strong keywords for search.
    items.push(SettingDescriptor {
        id: "assistant.config",
        category: SettingCategory::Assistant,
        group: "Assistant",
        title: "AI assistant",
        description: "",
        keywords: &[
            "assistant",
            "ai",
            "llm",
            "agent",
            "claude",
            "anthropic",
            "model",
            "assistant",
            "provider",
            "api key",
            "effort",
        ],
        control: Control::Custom(render_assistant_settings),
        enabled: None,
        indent: false,
        is_default: None,
        reset: None,
    });

    // Tasks: no real global task preferences exist yet — surface a note instead
    // of inventing a setting (see `render_tasks_placeholder`).
    items.push(SettingDescriptor {
        id: "tasks.info",
        category: SettingCategory::Tasks,
        group: "Background tasks",
        title: "Background tasks",
        description: "",
        keywords: &[
            "task",
            "tasks",
            "job",
            "jobs",
            "concurrency",
            "timeout",
            "parallel",
        ],
        control: Control::Custom(render_tasks_placeholder),
        enabled: None,
        indent: false,
        is_default: None,
        reset: None,
    });

    // Hardware ▸ Compute — detected CPU/GPU/RAM inventory and the core-count cap
    // for QM jobs.
    items.push(SettingDescriptor {
        id: "hardware.compute",
        category: SettingCategory::Hardware,
        group: "Compute",
        title: "Compute hardware",
        description: "Detected CPU, GPU, and memory; cap the cores QM uses.",
        keywords: &[
            "cpu", "gpu", "cores", "threads", "ram", "memory", "hardware", "parallel",
        ],
        control: Control::Custom(crate::frontend::ui::settings_registry::hardware::render),
        enabled: None,
        indent: false,
        is_default: None,
        reset: None,
    });

    // Hardware ▸ Monitoring — opt-in live utilization gauges.
    items.push(SettingDescriptor {
        id: "hardware.utilization_bars",
        category: SettingCategory::Hardware,
        group: "Monitoring",
        title: "Show CPU/GPU utilization",
        description: "Live gauges in the status bar. Repaints continuously while on.",
        keywords: &[
            "cpu",
            "gpu",
            "utilization",
            "usage",
            "gauge",
            "monitor",
            "bars",
        ],
        control: Control::Toggle {
            read: utilization_bars_read,
            on_change: utilization_bars_change,
        },
        enabled: None,
        indent: false,
        is_default: Some(utilization_bars_is_default),
        reset: Some(utilization_bars_reset),
    });

    // Advanced ▸ Configuration — meta-settings that operate on the settings file
    // itself. All Custom (buttons / confirmations), so none carry a reset.
    items.push(SettingDescriptor {
        id: "advanced.settings_location",
        category: SettingCategory::Advanced,
        group: "Configuration",
        title: "Settings file location",
        description: "",
        keywords: &[
            "settings", "file", "location", "path", "reveal", "json", "config",
        ],
        control: Control::Custom(render_settings_location),
        enabled: None,
        indent: false,
        is_default: None,
        reset: None,
    });
    items.push(SettingDescriptor {
        id: "advanced.export_import",
        category: SettingCategory::Advanced,
        group: "Configuration",
        title: "Backup & restore",
        description: "Export the current settings to a file, or import them from one.",
        keywords: &[
            "export", "import", "backup", "restore", "settings", "json", "config",
        ],
        control: Control::Custom(render_export_import),
        enabled: None,
        indent: false,
        is_default: None,
        reset: None,
    });
    items.push(SettingDescriptor {
        id: "advanced.reset_all",
        category: SettingCategory::Advanced,
        group: "Configuration",
        title: "Reset all settings",
        description: "Restore every setting to its default value.",
        keywords: &["reset", "default", "defaults", "all", "restore", "factory"],
        control: Control::Custom(render_reset_all),
        enabled: None,
        indent: false,
        is_default: None,
        reset: None,
    });

    // Representation ▸ the molecular-appearance defaults page, defined in its own
    // module (this file is already large, and that page is sizable).
    items.extend(crate::frontend::ui::settings_representation::descriptors());

    items
}

/// The categories that currently have at least one setting matching `search`
/// (already lower-cased). With an empty search, every category that owns a
/// setting. Drives the modal's left rail — categories with nothing to show
/// under the active filter are hidden.
pub fn visible_categories(search: &str) -> Vec<SettingCategory> {
    let registry = registry();
    CATEGORY_ORDER
        .into_iter()
        .filter(|category| {
            registry
                .iter()
                .any(|descriptor| descriptor.category == *category && descriptor.matches(search))
        })
        .collect()
}

/// Render one category's groups and settings (the selected-category view, shown
/// while the search box is empty). Groups appear in first-appearance order as
/// open collapsing sections.
pub fn render_category(
    state: &mut AppState,
    ui: &mut egui::Ui,
    actions: &mut Vec<AppAction>,
    category: SettingCategory,
) {
    let registry = registry();
    let pal = crate::frontend::theme::palette(ui);

    let mut groups: Vec<&'static str> = Vec::new();
    for descriptor in &registry {
        if descriptor.category == category && !groups.contains(&descriptor.group) {
            groups.push(descriptor.group);
        }
    }

    for group in groups {
        let matching: Vec<&SettingDescriptor> = registry
            .iter()
            .filter(|descriptor| descriptor.category == category && descriptor.group == group)
            .collect();

        egui::CollapsingHeader::new(RichText::new(group).strong())
            .default_open(true)
            .show(ui, |ui| {
                for descriptor in matching {
                    render_descriptor(descriptor, state, ui, actions, &pal);
                }
            });
    }
}

/// Render a flat list of every setting matching `search` (non-empty,
/// lower-cased) across all categories, with category labels as section
/// separators (a flat cross-category search — the selected category is ignored).
pub fn render_search_results(
    state: &mut AppState,
    ui: &mut egui::Ui,
    actions: &mut Vec<AppAction>,
    search: &str,
) {
    let registry = registry();
    let pal = crate::frontend::theme::palette(ui);

    let mut first = true;
    for category in CATEGORY_ORDER {
        let matching: Vec<&SettingDescriptor> = registry
            .iter()
            .filter(|descriptor| descriptor.category == category && descriptor.matches(search))
            .collect();
        if matching.is_empty() {
            continue;
        }

        if !first {
            ui.add_space(8.0);
        }
        first = false;
        ui.label(
            RichText::new(category.label())
                .strong()
                .color(pal.text_tertiary),
        );
        for descriptor in matching {
            render_descriptor(descriptor, state, ui, actions, &pal);
        }
    }

    if first {
        ui.add_space(4.0);
        ui.label(
            RichText::new("No settings match your search.")
                .italics()
                .color(pal.text_tertiary),
        );
    }
}

/// The inline "reset to default" affordance, right-aligned on the control's row.
/// Rendered only when the setting declares a default and currently differs from
/// it. Reads `AppState` to detect non-default; the reset itself flows through an
/// `AppAction`, preserving the single-mutator invariant.
fn reset_affordance(
    descriptor: &SettingDescriptor,
    state: &AppState,
    ui: &mut egui::Ui,
    actions: &mut Vec<AppAction>,
) {
    let (Some(is_default), Some(reset)) = (descriptor.is_default, descriptor.reset) else {
        return;
    };
    if is_default(state) {
        return;
    }
    if ui
        .add(egui::Button::new(
            RichText::new(egui_phosphor::regular::ARROW_COUNTER_CLOCKWISE).small(),
        ))
        .on_hover_text("Reset to default")
        .clicked()
    {
        actions.push(reset());
    }
}

fn render_descriptor(
    descriptor: &SettingDescriptor,
    state: &mut AppState,
    ui: &mut egui::Ui,
    actions: &mut Vec<AppAction>,
    pal: &crate::frontend::theme::Palette,
) {
    if descriptor.indent {
        ui.indent(descriptor.id, |ui| {
            render_descriptor_body(descriptor, state, ui, actions, pal);
        });
    } else {
        render_descriptor_body(descriptor, state, ui, actions, pal);
    }
}

fn render_descriptor_body(
    descriptor: &SettingDescriptor,
    state: &mut AppState,
    ui: &mut egui::Ui,
    actions: &mut Vec<AppAction>,
    pal: &crate::frontend::theme::Palette,
) {
    match &descriptor.control {
        Control::Toggle { read, on_change } => {
            // Control first (so it sits flush left, like every other row), then
            // a right-to-left region filling the remainder pins the reset button
            // to the row's right edge without disturbing the control.
            let enabled = descriptor.enabled.is_none_or(|gate| gate(state));
            ui.horizontal(|ui| {
                let mut value = read(state);
                // A setting is a persistent, immediately-applied on/off, so it
                // gets a sliding toggle switch rather than a checkbox. Centralizing
                // the toggle render here means every `Control::Toggle` in the
                // registry picks up the switch from this one site.
                //
                // Honour `descriptor.enabled` like the Slider/Value branches:
                // `toggle_switch` returns a bare `Response`, so gate it through
                // an `add_enabled_ui` wrapper rather than `add_enabled`.
                let changed = ui
                    .add_enabled_ui(enabled, |ui| {
                        crate::frontend::ui::toggle_switch(ui, &mut value, descriptor.title, pal)
                            .changed()
                    })
                    .inner;
                if changed {
                    actions.push(on_change(value));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    reset_affordance(descriptor, state, ui, actions);
                });
            });
        }
        Control::Choice {
            read,
            options,
            on_change,
        } => {
            let current = read(state);
            ui.horizontal(|ui| {
                ui.label(descriptor.title);
                let selected = options.get(current).copied().unwrap_or_default();
                egui::ComboBox::from_id_salt(descriptor.id)
                    .selected_text(selected)
                    .show_ui(ui, |ui| {
                        for (index, option) in options.iter().enumerate() {
                            if ui.selectable_label(index == current, *option).clicked() {
                                actions.push(on_change(index));
                            }
                        }
                    });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    reset_affordance(descriptor, state, ui, actions);
                });
            });
        }
        Control::Slider {
            read,
            range,
            on_change,
            show_value,
        } => {
            let enabled = descriptor.enabled.is_none_or(|gate| gate(state));
            let mut value = read(state);
            ui.horizontal(|ui| {
                let slider = egui::Slider::new(&mut value, range.clone())
                    .text(descriptor.title)
                    .show_value(*show_value);
                let response = ui.add_enabled(enabled, slider);
                // Mirror the glass-intensity pattern: live preview while
                // dragging (commit = false), persist on a discrete change
                // or on release.
                if response.changed() {
                    actions.push(on_change(value, !response.dragged()));
                } else if response.drag_stopped() {
                    actions.push(on_change(value, true));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    reset_affordance(descriptor, state, ui, actions);
                });
            });
        }
        Control::Value {
            read,
            range,
            unit,
            speed,
            on_change,
        } => {
            let enabled = descriptor.enabled.is_none_or(|gate| gate(state));
            let mut value = read(state);
            ui.horizontal(|ui| {
                ui.label(descriptor.title);
                let drag = egui::DragValue::new(&mut value)
                    .range(range.clone())
                    .speed(*speed)
                    .suffix(*unit);
                if ui.add_enabled(enabled, drag).changed() {
                    actions.push(on_change(value));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    reset_affordance(descriptor, state, ui, actions);
                });
            });
        }
        Control::Custom(render) => render(state, ui, actions),
    }

    if !descriptor.description.is_empty() {
        ui.label(caption_text(descriptor.description, pal.text_muted));
    }
}
