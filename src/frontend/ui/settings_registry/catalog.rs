//! The settings catalog: [`registry`] assembles every [`SettingDescriptor`] for
//! the whole Settings panel, applying platform availability gates so unavailable
//! settings are simply absent. The functions in [`super::render`] turn these
//! descriptors into widgets.

use super::*;

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
            subgroup: None,
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
            subgroup: None,
        },
        SettingDescriptor {
            id: "workbench.default_task_panel_location",
            category: SettingCategory::General,
            group: "Workbench",
            title: "Default task panel location",
            description: "Where newly opened task panels appear; open panels can still be dragged to another dock host.",
            keywords: &[
                "task", "panel", "floating", "window", "sidebar", "dock", "bottom", "default",
                "location",
            ],
            control: Control::Choice {
                read: task_panel_placement_read,
                options: &TASK_PANEL_PLACEMENT_OPTIONS,
                on_change: task_panel_placement_change,
            },
            enabled: None,
            indent: false,
            is_default: Some(task_panel_placement_is_default),
            reset: Some(task_panel_placement_reset),
            subgroup: None,
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
            subgroup: None,
        });
        items.push(SettingDescriptor {
            id: "appearance.blur_intensity",
            category: SettingCategory::General,
            group: "Appearance",
            title: "Blur intensity",
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
            subgroup: None,
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
        subgroup: None,
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
        subgroup: None,
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
        subgroup: None,
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
        subgroup: None,
    });

    // Compute targets — this machine and each configured remote host, each a
    // collapser owning that target's hardware and engine launches. Hosts are added
    // at runtime, so per-target sections cannot be static descriptors; the whole
    // group is one Custom body. Strong keywords because search over a Custom
    // descriptor is all-or-nothing on this metadata.
    items.push(SettingDescriptor {
        id: "compute.targets",
        category: SettingCategory::Compute,
        group: "Compute targets",
        title: "Compute targets",
        description: "",
        keywords: &[
            "this machine",
            "local",
            "remote",
            "ssh",
            "host",
            "hpc",
            "cluster",
            "hardware",
            "cpu",
            "gpu",
            "cores",
            "threads",
            "ram",
            "memory",
            "engine",
            "engines",
            "gromacs",
            "lammps",
            "path",
            "binary",
            "wsl",
            "version",
            "verify",
            "detect",
            "passwordless",
            "key",
            "slurm",
            "scheduler",
        ],
        control: Control::Custom(crate::frontend::ui::render_compute_targets),
        enabled: None,
        indent: false,
        is_default: None,
        reset: None,
        subgroup: None,
    });

    // Built-in engines — compiled in, so they belong to no compute target and have
    // nothing to configure. Read-only, and kept out of the launch editors above so
    // a green check there always means "this launch was run and it worked".
    items.push(SettingDescriptor {
        id: "engines.builtin",
        category: SettingCategory::Compute,
        group: "Built-in engines",
        title: "Included with SilicoLab",
        description: "Always available; no setup and no external binary.",
        keywords: &[
            "builtin", "built-in", "hartree", "uff", "docking", "vina", "engine", "engines",
        ],
        control: Control::Custom(crate::frontend::ui::render_builtin_engines),
        enabled: None,
        indent: false,
        is_default: None,
        reset: None,
        subgroup: None,
    });

    // Defaults for new jobs — global seeds every task panel starts from.
    items.push(SettingDescriptor {
        id: "engines.default_compute_target",
        category: SettingCategory::Compute,
        group: "Defaults for new jobs",
        title: "Default compute target",
        description: "Where new QM, docking, and MD panels start; each can override it per run.",
        keywords: &[
            "remote", "compute", "target", "default", "run on", "host", "qm", "docking", "md",
        ],
        control: Control::Custom(render_default_compute_target),
        enabled: None,
        indent: false,
        is_default: None,
        reset: None,
        subgroup: None,
    });
    items.push(SettingDescriptor {
        id: "compute.default_cores",
        category: SettingCategory::Compute,
        group: "Defaults for new jobs",
        title: "Default CPU cores",
        description: "Cores a new job starts with; each task's Run on panel can override it per run.",
        keywords: &[
            "cpu", "cores", "threads", "default", "parallel", "qm", "md",
        ],
        control: Control::Custom(crate::frontend::ui::settings_registry::hardware::render_default_cores),
        enabled: None,
        indent: false,
        is_default: None,
        reset: None,
        subgroup: None,
    });

    // Monitoring — opt-in live system monitor.
    items.push(SettingDescriptor {
        id: "hardware.utilization_bars",
        category: SettingCategory::Compute,
        group: "Monitoring",
        title: "Show system monitor (CPU / Memory / GPU)",
        description: "Live utilization bars in the sidebar footer (or the status bar when the \
                      sidebar is hidden); click them for sparkline details. Set the update \
                      speed below; it pauses on its own while the window is hidden.",
        keywords: &[
            "cpu",
            "memory",
            "ram",
            "gpu",
            "utilization",
            "usage",
            "gauge",
            "monitor",
            "bars",
            "sparkline",
        ],
        control: Control::Toggle {
            read: utilization_bars_read,
            on_change: utilization_bars_change,
        },
        enabled: None,
        indent: false,
        is_default: Some(utilization_bars_is_default),
        reset: Some(utilization_bars_reset),
        subgroup: None,
    });
    items.push(SettingDescriptor {
        id: "hardware.monitor_refresh",
        category: SettingCategory::Compute,
        group: "Monitoring",
        title: "Update speed",
        description: "How often the monitor samples. Lower rates — or Pause — reduce CPU wakeups \
                      and how often a discrete GPU is polled, saving power.",
        keywords: &[
            "refresh",
            "rate",
            "update",
            "speed",
            "interval",
            "frequency",
            "pause",
            "power",
            "battery",
        ],
        control: Control::Choice {
            read: monitor_refresh_read,
            options: &MONITOR_REFRESH_OPTIONS,
            on_change: monitor_refresh_change,
        },
        enabled: Some(monitor_refresh_enabled),
        indent: true,
        is_default: Some(monitor_refresh_is_default),
        reset: Some(monitor_refresh_reset),
        subgroup: None,
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
        subgroup: None,
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
        subgroup: None,
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
        subgroup: None,
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
        subgroup: Some(Subgroup {
            title: "Danger zone",
            default_open: true,
        }),
    });

    // Representation ▸ the molecular-appearance defaults page, defined in its own
    // module (this file is already large, and that page is sizable).
    items.extend(crate::frontend::ui::settings_representation::descriptors());

    items
}
