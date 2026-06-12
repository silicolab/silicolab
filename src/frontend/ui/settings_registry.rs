//! Schema-driven settings registry (VSCode-style).
//!
//! Each user setting is described once as a [`SettingDescriptor`] and the
//! Settings UI is *generated* from those descriptors rather than hand-coded per
//! control. This keeps the single-mutator invariant intact: a descriptor only
//! declares how to **read** the current value and which [`AppAction`] to
//! **emit** on change — the mutation itself still happens in
//! `dispatcher.rs::dispatch`. Controls carry plain function pointers (not
//! closures), so they cannot capture and smuggle in a mutation path.
//!
//! The whole Settings panel is sourced here: a two-level category → group
//! structure (General ▸ Appearance / Startup & Projects; Representation ▸ Base /
//! Cartoon / Surface / Color Schemes; Engines; Tasks; Advanced ▸
//! Configuration). The Engines editor and the Advanced meta-settings are wrapped
//! wholesale as [`Control::Custom`] rather than rebuilt; the Representation page
//! lives in `settings_representation`. The modal (`settings_modal`) renders one
//! category at a time from these descriptors, or a flat cross-category list
//! while a search is active.

use std::ops::RangeInclusive;

use eframe::egui::{self, RichText};

use crate::{
    backend::config::{AppConfig, ColorScheme, ThemeMode},
    frontend::{actions::AppAction, state::AppState},
};

/// Top-level grouping for the Settings panel. `General`, `Representation`,
/// `Engines`, and `Tasks` are populated; `Advanced` carries the meta-settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SettingCategory {
    #[default]
    General,
    Representation,
    Engines,
    Tasks,
    Advanced,
}

impl SettingCategory {
    /// Heading shown above this category's groups, and the label of its entry in
    /// the modal's left rail.
    pub fn label(self) -> &'static str {
        match self {
            SettingCategory::General => "General",
            SettingCategory::Representation => "Representation",
            SettingCategory::Engines => "Engines",
            SettingCategory::Tasks => "Tasks",
            SettingCategory::Advanced => "Advanced",
        }
    }
}

/// Stable iteration order for categories in the rendered panel and the rail.
pub const CATEGORY_ORDER: [SettingCategory; 5] = [
    SettingCategory::General,
    SettingCategory::Representation,
    SettingCategory::Engines,
    SettingCategory::Tasks,
    SettingCategory::Advanced,
];

/// How a single setting is edited. Every variant keeps the Elm flow: it reads
/// from [`AppState`] and returns an [`AppAction`] to emit — it never mutates.
pub enum Control {
    /// A boolean checkbox; the descriptor's title is the checkbox label.
    Toggle {
        read: fn(&AppState) -> bool,
        on_change: fn(bool) -> AppAction,
    },
    /// A one-of-N choice rendered as a combo box. `read` returns the index of
    /// the current value within `options`; `on_change` maps a picked index back
    /// to an action.
    Choice {
        read: fn(&AppState) -> usize,
        options: &'static [&'static str],
        on_change: fn(usize) -> AppAction,
    },
    /// A continuous value. `on_change`'s `bool` is `commit`: `false` while the
    /// slider is mid-drag (live preview, do not persist), `true` on release or a
    /// discrete change — preserving the glass-intensity drag/release pattern.
    Slider {
        read: fn(&AppState) -> f32,
        range: RangeInclusive<f32>,
        on_change: fn(f32, bool) -> AppAction,
        /// Whether the slider draws its numeric value box. `false` reads as a
        /// bare track (matches the pre-registry blur-intensity control, which
        /// used `.show_value(false)` so the slider nests cleanly under its
        /// parent toggle).
        show_value: bool,
    },
    /// A free-typed numeric value rendered as an [`egui::DragValue`] with a unit
    /// suffix (`" Å"`, `" %"`, or `""`). Persists on every discrete change — used
    /// by the Representation cartoon/transparency defaults, which are absolute
    /// preferences with no live-preview drag semantics (unlike [`Self::Slider`]).
    Value {
        read: fn(&AppState) -> f32,
        range: RangeInclusive<f32>,
        unit: &'static str,
        speed: f32,
        on_change: fn(f32) -> AppAction,
    },
    /// Escape hatch for editors too complex to express declaratively (e.g. the
    /// engines table, a path picker). Still confined to emitting actions in
    /// practice — the renderer receives `&mut AppState` only to *read* it.
    Custom(fn(&mut AppState, &mut egui::Ui, &mut Vec<AppAction>)),
}

/// A declarative description of one setting.
pub struct SettingDescriptor {
    /// Stable dotted key, e.g. `"appearance.theme"`. Used as a widget id salt
    /// and matched by search.
    pub id: &'static str,
    pub category: SettingCategory,
    /// Section heading the setting renders under, e.g. `"Appearance"`.
    pub group: &'static str,
    pub title: &'static str,
    /// Help text shown beneath the control and matched by search.
    pub description: &'static str,
    /// Extra search terms not present in the title/description.
    pub keywords: &'static [&'static str],
    pub control: Control,
    /// Optional gate: when present and it returns `false`, the control renders
    /// disabled (e.g. blur intensity while transparency is off). `None` =
    /// always enabled. (Availability — whether a setting is registered at all,
    /// e.g. glass support — is decided in [`registry`], not here.)
    pub enabled: Option<fn(&AppState) -> bool>,
    /// When `true`, the control is indented one step, so it reads as nested
    /// under the setting directly above it (the blur slider beneath the
    /// Transparency toggle).
    pub indent: bool,
    /// Whether the current value differs from the default. Together with
    /// [`reset`](Self::reset) it drives the inline "reset to default"
    /// affordance: the button appears only while this returns `false`. `None`
    /// for settings with no meaningful default (path pickers, the engines
    /// table, informational placeholders), which opt out of reset entirely.
    pub is_default: Option<fn(&AppState) -> bool>,
    /// Action that restores this setting's default value, emitted when the
    /// reset affordance is clicked. Paired with [`is_default`](Self::is_default);
    /// both are present or both `None`.
    pub reset: Option<fn() -> AppAction>,
}

impl SettingDescriptor {
    /// Whether this setting matches a (already lower-cased) search query across
    /// its id, title, description, and keywords. Empty query matches everything.
    fn matches(&self, search: &str) -> bool {
        if search.is_empty() {
            return true;
        }
        let hit = |text: &str| text.to_lowercase().contains(search);
        hit(self.id)
            || hit(self.title)
            || hit(self.description)
            || self.keywords.iter().any(|keyword| hit(keyword))
    }
}

// --- Appearance group accessors (read state / build action) --------------- //
//
// These are free functions, not closures, so they coerce to the `fn(...)`
// pointers the controls hold.

const THEME_OPTIONS: [&str; 3] = {
    let modes = ThemeMode::all();
    [modes[0].label(), modes[1].label(), modes[2].label()]
};

const SCHEME_OPTIONS: [&str; 5] = {
    let schemes = ColorScheme::all();
    [
        schemes[0].label(),
        schemes[1].label(),
        schemes[2].label(),
        schemes[3].label(),
        schemes[4].label(),
    ]
};

fn theme_read(state: &AppState) -> usize {
    ThemeMode::all()
        .iter()
        .position(|mode| *mode == state.config.theme)
        .unwrap_or(0)
}

fn theme_change(index: usize) -> AppAction {
    AppAction::SetThemeMode(ThemeMode::all().get(index).copied().unwrap_or_default())
}

fn scheme_read(state: &AppState) -> usize {
    ColorScheme::all()
        .iter()
        .position(|scheme| *scheme == state.config.color_scheme)
        .unwrap_or(0)
}

fn scheme_change(index: usize) -> AppAction {
    AppAction::SetColorScheme(ColorScheme::all().get(index).copied().unwrap_or_default())
}

fn glass_read(state: &AppState) -> bool {
    state.config.glass
}

fn glass_change(on: bool) -> AppAction {
    AppAction::SetGlass(on)
}

fn glass_enabled(state: &AppState) -> bool {
    state.config.glass
}

fn blur_read(state: &AppState) -> f32 {
    state.config.glass_intensity
}

fn blur_change(value: f32, commit: bool) -> AppAction {
    AppAction::SetGlassIntensity { value, commit }
}

// --- Reset-to-default detectors & actions --------------------------------- //
//
// Defaults are read from `AppConfig::default()` rather than hard-coded, so they
// stay in sync with the canonical defaults in `backend::config`. Each pair is a
// detector (`is value == default?`) and the action that restores that default.

fn theme_is_default(state: &AppState) -> bool {
    state.config.theme == AppConfig::default().theme
}

fn theme_reset() -> AppAction {
    AppAction::SetThemeMode(AppConfig::default().theme)
}

fn scheme_is_default(state: &AppState) -> bool {
    state.config.color_scheme == AppConfig::default().color_scheme
}

fn scheme_reset() -> AppAction {
    AppAction::SetColorScheme(AppConfig::default().color_scheme)
}

fn glass_is_default(state: &AppState) -> bool {
    state.config.glass == AppConfig::default().glass
}

fn glass_reset() -> AppAction {
    AppAction::SetGlass(AppConfig::default().glass)
}

fn blur_is_default(state: &AppState) -> bool {
    state.config.glass_intensity == AppConfig::default().glass_intensity
}

fn blur_reset() -> AppAction {
    AppAction::SetGlassIntensity {
        value: AppConfig::default().glass_intensity,
        commit: true,
    }
}

fn reopen_is_default(state: &AppState) -> bool {
    state.config.closed_to_scratch == AppConfig::default().closed_to_scratch
}

fn reopen_reset() -> AppAction {
    AppAction::SetReopenLastProject(!AppConfig::default().closed_to_scratch)
}

fn check_updates_read(state: &AppState) -> bool {
    state.config.check_updates
}

fn check_updates_change(on: bool) -> AppAction {
    AppAction::SetCheckUpdates(on)
}

fn check_updates_is_default(state: &AppState) -> bool {
    state.config.check_updates == AppConfig::default().check_updates
}

fn check_updates_reset() -> AppAction {
    AppAction::SetCheckUpdates(AppConfig::default().check_updates)
}

fn auto_install_updates_read(state: &AppState) -> bool {
    state.config.auto_install_updates
}

fn auto_install_updates_change(on: bool) -> AppAction {
    AppAction::SetAutoInstallUpdates(on)
}

fn auto_install_updates_is_default(state: &AppState) -> bool {
    state.config.auto_install_updates == AppConfig::default().auto_install_updates
}

fn auto_install_updates_reset() -> AppAction {
    AppAction::SetAutoInstallUpdates(AppConfig::default().auto_install_updates)
}

/// The auto-install toggle is meaningful only while update checks run, so it is
/// disabled (and visually nested) under "Check for updates on launch".
fn auto_install_updates_enabled(state: &AppState) -> bool {
    state.config.check_updates
}

// --- Startup & Projects group --------------------------------------------- //

fn reopen_read(state: &AppState) -> bool {
    // The stored flag is the inverse ("closed to scratch"); present it the way a
    // user thinks about it.
    !state.config.closed_to_scratch
}

fn reopen_change(on: bool) -> AppAction {
    AppAction::SetReopenLastProject(on)
}

/// Path settings are not a scalar control: show the current default project
/// folder and a button that emits the picker action (the dialog itself runs in
/// the dispatcher, the only place allowed to touch state).
fn render_default_project_dir(
    state: &mut AppState,
    ui: &mut egui::Ui,
    actions: &mut Vec<AppAction>,
) {
    let pal = crate::frontend::theme::palette(ui);
    ui.horizontal(|ui| {
        ui.label("Default project folder");
        if ui.button("Choose…").clicked() {
            actions.push(AppAction::PickDefaultProjectDir);
        }
    });
    ui.label(
        RichText::new(state.config.default_project_dir.display().to_string())
            .small()
            .color(pal.text_tertiary),
    );
}

// --- Tasks group ---------------------------------------------------------- //

/// Informational placeholder: there are no user-tunable global task preferences
/// wired to anything yet (the job manager spawns a thread per job with no
/// concurrency cap, and timeouts are fixed per operation), so we surface a note
/// rather than invent a setting that controls nothing.
fn render_tasks_placeholder(
    _state: &mut AppState,
    ui: &mut egui::Ui,
    _actions: &mut Vec<AppAction>,
) {
    let pal = crate::frontend::theme::palette(ui);
    ui.label(
        RichText::new(
            "No configurable task preferences yet — background jobs run concurrently \
             and each engine step uses a fixed timeout.",
        )
        .small()
        .color(pal.text_tertiary),
    );
}

// --- Advanced ▸ Configuration --------------------------------------------- //

/// Show the settings.json path with a button that reveals it in the OS file
/// manager. The blocking shell-out runs in the dispatcher; this only emits the
/// action.
fn render_settings_location(
    _state: &mut AppState,
    ui: &mut egui::Ui,
    actions: &mut Vec<AppAction>,
) {
    let pal = crate::frontend::theme::palette(ui);
    let path = crate::backend::config::settings_path();
    ui.horizontal(|ui| {
        ui.label("Settings file");
        if ui
            .button(format!("{}  Reveal", egui_phosphor::regular::FOLDER_OPEN))
            .clicked()
        {
            actions.push(AppAction::RevealSettingsFile);
        }
    });
    ui.label(
        RichText::new(path.display().to_string())
            .small()
            .color(pal.text_tertiary),
    );
}

/// Reset-everything, gated behind an explicit inline confirmation so a single
/// click can't wipe the user's settings. The confirm flag is parked in egui's
/// per-widget temp memory (transient UI, never persisted), keeping this renderer
/// free of any persisted-state mutation.
fn render_reset_all(_state: &mut AppState, ui: &mut egui::Ui, actions: &mut Vec<AppAction>) {
    let pal = crate::frontend::theme::palette(ui);
    let confirm_id = ui.id().with("settings_reset_all_confirm");
    let confirming = ui
        .data(|data| data.get_temp::<bool>(confirm_id))
        .unwrap_or(false);

    if confirming {
        ui.horizontal(|ui| {
            ui.label(RichText::new("Reset every setting to its default?").color(pal.status_red));
            if ui.button("Reset all").clicked() {
                actions.push(AppAction::ResetAllSettings);
                ui.data_mut(|data| data.insert_temp(confirm_id, false));
            }
            if ui.button("Cancel").clicked() {
                ui.data_mut(|data| data.insert_temp(confirm_id, false));
            }
        });
    } else if ui.button("Reset all settings to defaults…").clicked() {
        ui.data_mut(|data| data.insert_temp(confirm_id, true));
    }
}

/// Export / import the whole settings file via native dialogs (run in the
/// dispatcher, like the other pickers).
fn render_export_import(_state: &mut AppState, ui: &mut egui::Ui, actions: &mut Vec<AppAction>) {
    ui.horizontal(|ui| {
        if ui
            .button(format!(
                "{}  Export…",
                egui_phosphor::regular::UPLOAD_SIMPLE
            ))
            .clicked()
        {
            actions.push(AppAction::ExportSettings);
        }
        if ui
            .button(format!(
                "{}  Import…",
                egui_phosphor::regular::DOWNLOAD_SIMPLE
            ))
            .clicked()
        {
            actions.push(AppAction::ImportSettings);
        }
    });
}

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
            description: "Apple-style frosted-glass material on the window chrome.",
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
        control: Control::Custom(super::render_engine_settings),
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
    items.extend(super::settings_representation::descriptors());

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
/// separators (VSCode's search behaviour — the selected category is ignored).
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
            ui.horizontal(|ui| {
                let mut value = read(state);
                if ui.checkbox(&mut value, descriptor.title).changed() {
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
        ui.label(
            RichText::new(descriptor.description)
                .small()
                .color(pal.text_tertiary),
        );
    }
}
