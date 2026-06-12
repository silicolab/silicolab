use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::backend::representation::RepresentationPrefs;
use crate::engines::registry::EngineLaunch;

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
    /// Warm ivory / charcoal neutrals, blue accent (the current house look).
    #[default]
    Warm,
    /// Cool blue-gray neutrals, blue accent (SilicoLab's pre-overhaul palette).
    Cool,
    /// Neutral graphite grays, blue accent.
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub default_project_dir: PathBuf,
    pub last_project_path: Option<PathBuf>,
    pub closed_to_scratch: bool,
    /// User-provided overrides for how each engine is launched, keyed by
    /// [`crate::engines::registry::EngineId`] string. Lets users point at a
    /// GROMACS inside WSL (`wsl.exe -e /usr/local/gromacs/bin/gmx`) or a
    /// non-PATH native install.
    #[serde(default)]
    pub engine_overrides: HashMap<String, EngineLaunch>,
    /// Light/dark preference. Defaults to following the system.
    #[serde(default)]
    pub theme: ThemeMode,
    /// Accent + neutral color scheme, applied on top of light/dark. Defaults to
    /// `Warm`, so existing setups keep the current look until the user changes
    /// it. See [`crate::frontend::theme::Palette::for_scheme`].
    #[serde(default)]
    pub color_scheme: ColorScheme,
    /// Apple-style frosted-glass (vibrancy) material on the window chrome.
    /// Defaults off: the effect costs continuous backdrop-blur compositing while
    /// revealed, so it is opt-in (performance first). Only takes effect where the
    /// platform supports it (macOS for now) and is auto-suppressed when the OS
    /// "Reduce Transparency" setting is on. See [`crate::frontend::glass`].
    #[serde(default = "default_glass")]
    pub glass: bool,
    /// Liquid Glass tint intensity, 0.0 (ultra-clear) ..= 1.0 (fully tinted).
    /// Maps linearly onto the chrome-fill alpha range (see
    /// [`crate::frontend::theme::glass_alpha`]); macOS 27-style user control over
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
    /// structures (the Representation settings page). Deliberately **not**
    /// `#[serde(default)]` (pre-release cleanliness): a `settings.json` written
    /// before this field existed fails to parse and falls back to
    /// `AppConfig::default()` once. See [`crate::backend::representation`].
    pub representation: RepresentationPrefs,
}

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
            engine_overrides: HashMap::default(),
            theme: ThemeMode::default(),
            color_scheme: ColorScheme::default(),
            glass: default_glass(),
            glass_intensity: default_glass_intensity(),
            check_updates: default_check_updates(),
            auto_install_updates: default_auto_install_updates(),
            representation: RepresentationPrefs::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecentProject {
    pub path: PathBuf,
    pub name: String,
    pub last_accessed: u64,
}

pub fn config_dir() -> PathBuf {
    home_dir().join(".silicolab")
}

pub fn settings_path() -> PathBuf {
    config_dir().join("settings.json")
}

pub fn recent_projects_path() -> PathBuf {
    config_dir().join("recent_projects.json")
}

pub fn load_config() -> AppConfig {
    load_config_from(&settings_path()).unwrap_or_default()
}

pub fn save_config(config: &AppConfig) -> Result<()> {
    save_config_to(&settings_path(), config)
}

pub fn load_recent_projects() -> Vec<RecentProject> {
    load_recent_projects_from(&recent_projects_path()).unwrap_or_default()
}

pub fn save_recent_projects(projects: &[RecentProject]) -> Result<()> {
    save_recent_projects_to(&recent_projects_path(), projects)
}

pub fn remember_recent_project(projects: &mut Vec<RecentProject>, path: &Path, name: &str) {
    let now = current_timestamp();
    if let Some(project) = projects.iter_mut().find(|project| project.path == path) {
        project.name = name.to_string();
        project.last_accessed = now;
    } else {
        projects.push(RecentProject {
            path: path.to_path_buf(),
            name: name.to_string(),
            last_accessed: now,
        });
    }
    projects.sort_by_key(|project| std::cmp::Reverse(project.last_accessed));
    projects.truncate(12);
}

/// Read and parse an `AppConfig` from an arbitrary path. Used by the settings
/// loader and by Advanced ▸ Import; the `Result` lets the importer report
/// malformed input non-fatally rather than panic.
pub fn load_config_from(path: &Path) -> Result<AppConfig> {
    let source =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    serde_json::from_str(&source).with_context(|| format!("failed to parse {}", path.display()))
}

/// Serialize an `AppConfig` to an arbitrary path. Used by the settings saver and
/// by Advanced ▸ Export.
pub fn save_config_to(path: &Path, config: &AppConfig) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let source = serde_json::to_string_pretty(config)?;
    fs::write(path, source).with_context(|| format!("failed to write {}", path.display()))
}

fn load_recent_projects_from(path: &Path) -> Result<Vec<RecentProject>> {
    let source =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    serde_json::from_str(&source).with_context(|| format!("failed to parse {}", path.display()))
}

fn save_recent_projects_to(path: &Path, projects: &[RecentProject]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let source = serde_json::to_string_pretty(projects)?;
    fs::write(path, source).with_context(|| format!("failed to write {}", path.display()))
}

fn home_dir() -> PathBuf {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
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

    use super::{AppConfig, ColorScheme, RecentProject, load_config_from, remember_recent_project};
    use crate::engines::registry::EngineLaunch;

    #[test]
    fn reset_preferences_keeps_engine_overrides_and_last_project() {
        let mut config = AppConfig {
            color_scheme: ColorScheme::Violet,
            glass: true,
            closed_to_scratch: true,
            last_project_path: Some(PathBuf::from("/work/lysozyme")),
            ..AppConfig::default()
        };
        config.engine_overrides.insert(
            "gromacs".to_string(),
            EngineLaunch {
                command_prefix: vec!["wsl.exe".to_string(), "-e".to_string()],
                program: "/usr/local/gromacs/bin/gmx".to_string(),
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
        // ...while environment / session state is carried over.
        assert_eq!(reset.engine_overrides, config.engine_overrides);
        assert_eq!(reset.last_project_path, config.last_project_path);
    }

    #[test]
    fn missing_config_uses_default() {
        let loaded = load_config_from(&PathBuf::from("target/no-such-settings.json"));

        assert!(loaded.is_err());
        assert!(
            !AppConfig::default()
                .default_project_dir
                .as_os_str()
                .is_empty()
        );
    }

    #[test]
    fn remember_recent_project_updates_existing() {
        let mut projects = vec![RecentProject {
            path: PathBuf::from("old"),
            name: "Old".to_string(),
            last_accessed: 1,
        }];

        remember_recent_project(&mut projects, &PathBuf::from("old"), "Renamed");

        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].name, "Renamed");
    }
}
