use std::{
    collections::HashMap,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

use crate::backend::representation::RepresentationPrefs;
use crate::engines::registry::EngineLaunches;

use compute_core::hosts::home_dir;
pub use compute_core::hosts::{
    GpuRequest, JobResources, RemoteHost, ResourceSpec, SchedulerConfig, SlurmGpuSyntax,
    SlurmProfile, config_dir,
};

mod persist;
pub use persist::{
    load_config, load_config_from, load_recent_projects, recent_projects_path,
    remember_recent_project, save_config, save_config_to, save_recent_projects, settings_path,
};

pub use crate::backend::assistant_config::{ApprovalMode, AssistantConfig};

/// How the interface picks its light/dark appearance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ThemeMode {
    /// Follow the operating system appearance, switching live with it.
    #[default]
    System,
    Light,
    Dark,
}

impl ThemeMode {
    pub const fn all() -> [ThemeMode; 3] {
        [ThemeMode::System, ThemeMode::Light, ThemeMode::Dark]
    }

    pub const fn label(self) -> &'static str {
        match self {
            ThemeMode::System => "Follow system",
            ThemeMode::Light => "Light",
            ThemeMode::Dark => "Dark",
        }
    }
}

/// Selectable color scheme — an accent hue plus a neutral surface family,
/// applied on top of the light/dark mode (every scheme works in both). Lets
/// each user pick the palette they prefer rather than a single house style.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ColorScheme {
    /// Warm ivory / charcoal neutrals, blue accent.
    Warm,
    /// Cool blue-gray neutrals, blue accent (SilicoLab's pre-overhaul palette).
    Cool,
    /// Neutral graphite grays, blue accent (the default house look).
    #[default]
    Graphite,
    /// Neutral graphite grays, green accent.
    Green,
    /// Neutral graphite grays, violet accent.
    Violet,
}

impl ColorScheme {
    pub const fn all() -> [ColorScheme; 5] {
        [
            ColorScheme::Warm,
            ColorScheme::Cool,
            ColorScheme::Graphite,
            ColorScheme::Green,
            ColorScheme::Violet,
        ]
    }

    pub const fn label(self) -> &'static str {
        match self {
            ColorScheme::Warm => "Warm",
            ColorScheme::Cool => "Cool (blue)",
            ColorScheme::Graphite => "Graphite",
            ColorScheme::Green => "Green",
            ColorScheme::Violet => "Violet",
        }
    }
}

/// How often the live system monitor samples and repaints while it is shown.
/// Lower rates cut background CPU wakeups and, on a discrete GPU, how often the
/// card is polled (each poll can pull it out of its deepest power state);
/// `Pause` stops sampling entirely and the gauges hold their last values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum MonitorRefresh {
    /// Twice a second.
    High,
    /// Once a second — the default.
    #[default]
    Standard,
    /// Once every few seconds.
    Low,
    /// Suspended: no sampling until resumed.
    Pause,
}

impl MonitorRefresh {
    pub const fn all() -> [MonitorRefresh; 4] {
        [
            MonitorRefresh::High,
            MonitorRefresh::Standard,
            MonitorRefresh::Low,
            MonitorRefresh::Pause,
        ]
    }

    pub const fn label(self) -> &'static str {
        match self {
            MonitorRefresh::High => "High",
            MonitorRefresh::Standard => "Standard",
            MonitorRefresh::Low => "Low",
            MonitorRefresh::Pause => "Pause",
        }
    }
}

/// Where an engine task runs: locally (the historical default) or on a configured
/// remote host, referenced by its [`RemoteHost::id`]. Persisted as the app-wide
/// default and selected per task at launch.
///
/// The id is a loose reference: a [`Remote`](ComputeTarget::Remote) whose host was
/// deleted/renamed resolves leniently back to `Local` (see the dispatcher's target
/// resolver), mirroring how `engine_overrides` falls back to a PATH probe on a miss
/// — a dangling target never panics or silently routes to a non-existent host.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum ComputeTarget {
    /// Run on this machine, exactly as SilicoLab always has.
    #[default]
    Local,
    /// Run on the remote host with this [`RemoteHost::id`].
    Remote(String),
}

/// Where newly opened task panels appear by default. The user can still drag a
/// task tab between dock areas after it opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum TaskPanelPlacement {
    #[default]
    Floating,
    RightSidebar,
    BottomPanel,
}

impl TaskPanelPlacement {
    pub const fn all() -> [TaskPanelPlacement; 3] {
        [
            TaskPanelPlacement::Floating,
            TaskPanelPlacement::RightSidebar,
            TaskPanelPlacement::BottomPanel,
        ]
    }

    pub const fn label(self) -> &'static str {
        match self {
            TaskPanelPlacement::Floating => "Floating window",
            TaskPanelPlacement::RightSidebar => "Right sidebar",
            TaskPanelPlacement::BottomPanel => "Bottom panel",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub default_project_dir: PathBuf,
    pub last_project_path: Option<PathBuf>,
    pub closed_to_scratch: bool,
    /// User-provided overrides for how each engine is launched on this machine.
    /// Lets users point at a GROMACS inside WSL
    /// (`wsl.exe -e /usr/local/gromacs/bin/gmx`) or a non-PATH native install.
    /// The local-target twin of [`RemoteHost::engines`].
    #[serde(default)]
    pub engine_overrides: EngineLaunches,
    /// Remote hosts the user can submit engine jobs to over SSH, keyed by
    /// [`RemoteHost::id`]. Backward compatible (empty default; old `settings.json`
    /// still parses). Preserved across "Reset all settings" like `engine_overrides`.
    #[serde(default)]
    pub remote_hosts: HashMap<String, RemoteHost>,
    /// App-wide default compute target for new tasks. Per-task selection overrides
    /// it at launch. Defaults to [`ComputeTarget::Local`].
    #[serde(default)]
    pub default_compute_target: ComputeTarget,
    /// App-wide default host for newly opened task panels. Per-panel task tabs
    /// remain session state and can still be dragged between hosts after opening.
    #[serde(default)]
    pub default_task_panel_placement: TaskPanelPlacement,
    /// Light/dark preference. Defaults to following the system.
    #[serde(default)]
    pub theme: ThemeMode,
    /// Accent + neutral color scheme, applied on top of light/dark. Defaults to
    /// `Graphite`. The frontend maps it through `Palette::for_scheme`.
    #[serde(default)]
    pub color_scheme: ColorScheme,
    /// Apple-style frosted-glass (vibrancy) material on the window chrome.
    /// Defaults off: the effect costs continuous backdrop-blur compositing while
    /// revealed, so it is opt-in (performance first). Only takes effect where the
    /// platform supports it (macOS for now) and is auto-suppressed when the OS
    /// "Reduce Transparency" setting is on. See the frontend `glass` module.
    #[serde(default = "default_glass")]
    pub glass: bool,
    /// Liquid Glass tint intensity, 0.0 (ultra-clear) ..= 1.0 (fully tinted).
    /// Maps linearly onto the chrome-fill alpha range (see
    /// `theme::glass_alpha`); macOS 27-style user control over
    /// how see-through the frosted chrome reads.
    #[serde(default = "default_glass_intensity")]
    pub glass_intensity: f32,
    /// Whether to check GitHub Releases for a newer SilicoLab once per launch.
    /// On by default; the check is a single anonymous request and only ever
    /// surfaces a notice — nothing is downloaded or installed automatically.
    #[serde(default = "default_check_updates")]
    pub check_updates: bool,
    /// Whether a discovered update is downloaded and installed automatically
    /// (still requiring a restart to apply), rather than waiting for the user to
    /// click "Update". Off by default — the default flow is one-click manual —
    /// and only ever acts when [`check_updates`](Self::check_updates) is on and
    /// the install location is writable.
    #[serde(default = "default_auto_install_updates")]
    pub auto_install_updates: bool,
    /// App-wide default visual appearance applied to newly built or loaded
    /// structures (the Representation settings page). `#[serde(default)]` so a
    /// missing or reshaped field degrades to its own default instead of failing
    /// the whole parse and resetting every other setting.
    #[serde(default)]
    pub representation: RepresentationPrefs,
    /// In-app LLM assistant selection (provider/model/effort). Never stores the
    /// API key. `#[serde(default)]` so an older `settings.json` still parses.
    #[serde(default)]
    pub assistant: AssistantConfig,
    /// Persisted workbench layout: which movable views are docked in which area,
    /// their order, the active view per area, area visibility, and the two area
    /// sizes. A user preference (shared across projects, like the theme), not
    /// project data. `#[serde(default)]` so an older `settings.json` still parses
    /// and falls back to the default layout. Only the fixed views persist here;
    /// per-task panels are session state and are never written. See
    /// the frontend `DockModel`.
    #[serde(default)]
    pub dock_layout: DockLayoutConfig,
    /// Default number of CPU cores a new job starts with — the seed every task
    /// panel's per-run core control inherits (overridable per run). For QM it also
    /// sizes a rayon thread pool that wraps each hartree run (hartree parallelises
    /// over the global rayon pool, so `pool.install(...)` throttles it live).
    /// Defaults to the physical core count; clamped to the logical count at use.
    /// `#[serde(default)]` so older settings.json still parses.
    #[serde(default = "default_compute_core_count")]
    pub compute_core_count: usize,
    /// Show live CPU/GPU utilization gauges in the status bar. Off by default:
    /// while on, the app samples and repaints on the
    /// [`monitor_refresh`](Self::monitor_refresh) cadence to animate the gauges.
    #[serde(default)]
    pub show_utilization_bars: bool,
    /// How often the system monitor samples while shown. Lower rates (or
    /// `Pause`) reduce background wakeups and how often a discrete GPU is
    /// polled. Only takes effect while
    /// [`show_utilization_bars`](Self::show_utilization_bars) is on.
    #[serde(default)]
    pub monitor_refresh: MonitorRefresh,
    /// `#[serde(default)]` so older settings.json still parses.
    #[serde(default)]
    pub chart_export: ChartExportPrefs,
}

/// Persisted state of one dock area (the bottom panel or the right sidebar). The
/// `tabs`/`active` strings are fixed-view tokens (see
/// `StaticView::token`); unknown tokens are skipped on
/// load, so the schema tolerates reordering or removing a view.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DockAreaLayout {
    pub tabs: Vec<String>,
    pub active: Option<String>,
    #[serde(default)]
    pub collapsed: bool,
}

/// Persisted workbench layout mirror of the frontend `DockModel`.
/// Lives in the backend layer (no dependency on `frontend`), so the default
/// placement is spelled out here with literal view tokens and sizes; a test in
/// `state.rs` asserts it stays in lock-step with `DockModel::default()`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DockLayoutConfig {
    pub bottom: DockAreaLayout,
    pub right: DockAreaLayout,
    pub right_width: f32,
    pub bottom_height: f32,
}

impl Default for DockLayoutConfig {
    fn default() -> Self {
        Self {
            // Bottom shows console / monitor / output (console active), matching
            // the frontend's `DockModel::default`.
            bottom: DockAreaLayout {
                tabs: vec![
                    "console".to_string(),
                    "sequence".to_string(),
                    "task_monitor".to_string(),
                    "output".to_string(),
                    "plot".to_string(),
                ],
                active: Some("console".to_string()),
                collapsed: false,
            },
            // Assistant's home is the right sidebar and the area is shown at rest, so
            // a first run opens straight into the assistant.
            right: DockAreaLayout {
                tabs: vec!["assistant".to_string()],
                active: Some("assistant".to_string()),
                collapsed: false,
            },
            // Mirror SIDEBAR_DEFAULT_WIDTH_SECONDARY / PANEL_DEFAULT_HEIGHT
            // (frontend consts, kept in sync by the state.rs lock-step test).
            right_width: 320.0,
            bottom_height: 180.0,
        }
    }
}

pub use crate::backend::chart_export::ChartExportPrefs;

fn default_glass() -> bool {
    false
}

fn default_check_updates() -> bool {
    true
}

fn default_auto_install_updates() -> bool {
    false
}

fn default_glass_intensity() -> f32 {
    // Maps to a chrome alpha of ~110 — the historical fixed tint, so existing
    // setups look unchanged until the user moves the slider.
    0.35
}

fn default_compute_core_count() -> usize {
    crate::backend::hardware::info().physical_cores.max(1)
}

impl AppConfig {
    /// A copy with every user-facing *preference* reset to its default, while
    /// preserving non-preference state: `engine_overrides` (user-configured
    /// engine/WSL binary paths, expensive to recreate — their loss reads as
    /// "engines mysteriously stopped working") and `last_project_path` (session
    /// state, not a preference). Everything else — theme, color scheme, glass +
    /// intensity, `closed_to_scratch`, `default_project_dir` — returns to
    /// default. `default_project_dir` is treated as a preference (the user picks
    /// where new projects land), so it is reset.
    ///
    /// Backs "Reset all settings to defaults"; Export/Import deliberately keep
    /// round-tripping the *full* config, including `engine_overrides`.
    pub fn reset_preferences(&self) -> AppConfig {
        AppConfig {
            engine_overrides: self.engine_overrides.clone(),
            // Configured remote hosts are environment, not a preference: losing them
            // reads as "my HPC hosts mysteriously vanished" (same rationale as
            // engine_overrides). The app-wide default *target*, however, is a
            // preference and resets to Local.
            remote_hosts: self.remote_hosts.clone(),
            last_project_path: self.last_project_path.clone(),
            ..AppConfig::default()
        }
    }
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            default_project_dir: home_dir().join("silicolab_project"),
            last_project_path: None,
            closed_to_scratch: false,
            engine_overrides: EngineLaunches::new(),
            remote_hosts: HashMap::default(),
            default_compute_target: ComputeTarget::default(),
            default_task_panel_placement: TaskPanelPlacement::default(),
            theme: ThemeMode::default(),
            color_scheme: ColorScheme::default(),
            glass: default_glass(),
            glass_intensity: default_glass_intensity(),
            check_updates: default_check_updates(),
            auto_install_updates: default_auto_install_updates(),
            representation: RepresentationPrefs::default(),
            assistant: AssistantConfig::default(),
            dock_layout: DockLayoutConfig::default(),
            compute_core_count: default_compute_core_count(),
            show_utilization_bars: false,
            monitor_refresh: MonitorRefresh::default(),
            chart_export: ChartExportPrefs::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecentProject {
    pub path: PathBuf,
    pub name: String,
    pub last_accessed: u64,
}

pub fn current_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {

    use std::path::PathBuf;

    use super::{
        AppConfig, ColorScheme, ComputeTarget, MonitorRefresh, RemoteHost, ThemeMode,
        load_config_from,
    };
    use crate::engines::registry::{EngineId, EngineLaunch};

    #[test]
    fn reset_preferences_keeps_engine_overrides_and_last_project() {
        let mut config = AppConfig {
            color_scheme: ColorScheme::Violet,
            glass: true,
            closed_to_scratch: true,
            last_project_path: Some(PathBuf::from("/work/lysozyme")),
            default_compute_target: ComputeTarget::Remote("hpc".to_string()),
            ..AppConfig::default()
        };
        config.engine_overrides.insert(
            EngineId::GROMACS,
            EngineLaunch {
                command_prefix: vec!["wsl.exe".to_string(), "-e".to_string()],
                program: "/usr/local/gromacs/bin/gmx".to_string(),
            },
        );
        config.remote_hosts.insert(
            "hpc".to_string(),
            RemoteHost {
                id: "hpc".to_string(),
                label: "Cluster".to_string(),
                hostname: "login.example.edu".to_string(),
                username: "alice".to_string(),
                prelude: vec!["module load gromacs".to_string()],
                ..Default::default()
            },
        );

        let reset = config.reset_preferences();

        // Preferences fall back to default...
        assert_eq!(reset.color_scheme, AppConfig::default().color_scheme);
        assert_eq!(reset.glass, AppConfig::default().glass);
        assert_eq!(
            reset.closed_to_scratch,
            AppConfig::default().closed_to_scratch
        );
        assert_eq!(
            reset.default_project_dir,
            AppConfig::default().default_project_dir
        );
        // ...the app-wide default target is a preference, so it resets to Local.
        assert_eq!(reset.default_compute_target, ComputeTarget::Local);
        // ...while environment / session state is carried over.
        assert_eq!(reset.engine_overrides, config.engine_overrides);
        assert_eq!(reset.remote_hosts.len(), 1);
        assert!(reset.remote_hosts.contains_key("hpc"));
        assert_eq!(reset.last_project_path, config.last_project_path);
    }

    #[test]
    fn compute_target_defaults_to_local_and_round_trips() {
        // #[serde(default)] -> Local lets an old settings.json without the field
        // parse, and a remote target survives a round-trip.
        assert_eq!(ComputeTarget::default(), ComputeTarget::Local);
        let target = ComputeTarget::Remote("hpc".to_string());
        let json = serde_json::to_string(&target).expect("serialize");
        let back: ComputeTarget = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, target);
    }

    #[test]
    fn appearance_defaults_are_system_graphite() {
        let config = AppConfig::default();

        assert_eq!(config.theme, ThemeMode::System);
        assert_eq!(config.color_scheme, ColorScheme::Graphite);
    }

    #[test]
    fn config_parses_without_representation_field() {
        // A file missing `representation` must still parse (field defaults) rather
        // than failing the whole load and resetting every other setting.
        let json = r#"{
            "default_project_dir": "/tmp/p",
            "last_project_path": null,
            "closed_to_scratch": false,
            "assistant": { "enabled": true, "provider": "openai",
                           "model": "gpt-5.1", "effort": "high", "base_url": null }
        }"#;
        let dir = std::env::temp_dir().join("silicolab-cfg-norepr");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        let path = dir.join("settings.json");
        std::fs::write(&path, json).expect("write");

        let config = load_config_from(&path).expect("parse despite missing representation");
        assert_eq!(config.assistant.model, "gpt-5.1");
        assert_eq!(config.representation, super::RepresentationPrefs::default());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn compute_core_count_defaults_and_back_compat() {
        let cfg = AppConfig::default();
        assert!(cfg.compute_core_count >= 1);
        assert_eq!(
            cfg.compute_core_count,
            crate::backend::hardware::info().physical_cores.max(1)
        );
        // A settings.json written before this field still parses and yields the
        // hardware default. Reuse the JSON shape from
        // `config_parses_without_representation_field` — it includes every
        // required non-default field and is already known to parse.
        let json = r#"{
            "default_project_dir": "/tmp/p",
            "last_project_path": null,
            "closed_to_scratch": false,
            "assistant": { "enabled": true, "provider": "openai",
                           "model": "gpt-5.1", "effort": "high", "base_url": null }
        }"#;
        let parsed: AppConfig = serde_json::from_str(json).expect("legacy config should parse");
        assert_eq!(
            parsed.compute_core_count,
            crate::backend::hardware::info().physical_cores.max(1)
        );
    }

    #[test]
    fn show_utilization_bars_defaults_false() {
        assert!(!AppConfig::default().show_utilization_bars);
    }

    #[test]
    fn monitor_refresh_defaults_to_standard() {
        assert_eq!(
            AppConfig::default().monitor_refresh,
            MonitorRefresh::Standard
        );
    }

    #[test]
    fn monitor_refresh_all_lists_every_variant_in_order() {
        assert_eq!(
            MonitorRefresh::all(),
            [
                MonitorRefresh::High,
                MonitorRefresh::Standard,
                MonitorRefresh::Low,
                MonitorRefresh::Pause,
            ]
        );
    }

    #[test]
    fn config_parses_without_utilization_bars_field() {
        // A settings.json written before this field must still parse (backward
        // compat via #[serde(default)]) and yield the default (false).
        let json = r#"{
            "default_project_dir": "/tmp/p",
            "last_project_path": null,
            "closed_to_scratch": false,
            "assistant": { "enabled": true, "provider": "openai",
                           "model": "gpt-5.1", "effort": "high", "base_url": null }
        }"#;
        let parsed: AppConfig = serde_json::from_str(json).expect("legacy config should parse");
        assert!(!parsed.show_utilization_bars);
        // The refresh-rate field is likewise absent in pre-existing files and
        // must fall back to the default rather than failing the parse.
        assert_eq!(parsed.monitor_refresh, MonitorRefresh::Standard);
    }
}
