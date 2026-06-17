//! General-group accessors and reset detectors.
//!
//! These are free functions (not closures) so they coerce to the `fn(...)`
//! pointers the controls hold. Defaults are read from `AppConfig::default()`
//! rather than hard-coded, so they stay in sync with the canonical defaults in
//! `backend::config`. Each reset pair is a detector (`is value == default?`)
//! and the action that restores that default.

use crate::{
    backend::config::{AppConfig, ColorScheme, ThemeMode},
    frontend::{actions::AppAction, state::AppState},
};

pub(crate) const THEME_OPTIONS: [&str; 3] = {
    let modes = ThemeMode::all();
    [modes[0].label(), modes[1].label(), modes[2].label()]
};

pub(crate) const SCHEME_OPTIONS: [&str; 5] = {
    let schemes = ColorScheme::all();
    [
        schemes[0].label(),
        schemes[1].label(),
        schemes[2].label(),
        schemes[3].label(),
        schemes[4].label(),
    ]
};

pub(crate) fn theme_read(state: &AppState) -> usize {
    ThemeMode::all()
        .iter()
        .position(|mode| *mode == state.config.theme)
        .unwrap_or(0)
}

pub(crate) fn theme_change(index: usize) -> AppAction {
    AppAction::SetThemeMode(ThemeMode::all().get(index).copied().unwrap_or_default())
}

pub(crate) fn scheme_read(state: &AppState) -> usize {
    ColorScheme::all()
        .iter()
        .position(|scheme| *scheme == state.config.color_scheme)
        .unwrap_or(0)
}

pub(crate) fn scheme_change(index: usize) -> AppAction {
    AppAction::SetColorScheme(ColorScheme::all().get(index).copied().unwrap_or_default())
}

pub(crate) fn glass_read(state: &AppState) -> bool {
    state.config.glass
}

pub(crate) fn glass_change(on: bool) -> AppAction {
    AppAction::SetGlass(on)
}

pub(crate) fn glass_enabled(state: &AppState) -> bool {
    state.config.glass
}

pub(crate) fn blur_read(state: &AppState) -> f32 {
    state.config.glass_intensity
}

pub(crate) fn blur_change(value: f32, commit: bool) -> AppAction {
    AppAction::SetGlassIntensity { value, commit }
}

pub(crate) fn theme_is_default(state: &AppState) -> bool {
    state.config.theme == AppConfig::default().theme
}

pub(crate) fn theme_reset() -> AppAction {
    AppAction::SetThemeMode(AppConfig::default().theme)
}

pub(crate) fn scheme_is_default(state: &AppState) -> bool {
    state.config.color_scheme == AppConfig::default().color_scheme
}

pub(crate) fn scheme_reset() -> AppAction {
    AppAction::SetColorScheme(AppConfig::default().color_scheme)
}

pub(crate) fn glass_is_default(state: &AppState) -> bool {
    state.config.glass == AppConfig::default().glass
}

pub(crate) fn glass_reset() -> AppAction {
    AppAction::SetGlass(AppConfig::default().glass)
}

pub(crate) fn blur_is_default(state: &AppState) -> bool {
    state.config.glass_intensity == AppConfig::default().glass_intensity
}

pub(crate) fn blur_reset() -> AppAction {
    AppAction::SetGlassIntensity {
        value: AppConfig::default().glass_intensity,
        commit: true,
    }
}

pub(crate) fn reopen_is_default(state: &AppState) -> bool {
    state.config.closed_to_scratch == AppConfig::default().closed_to_scratch
}

pub(crate) fn reopen_reset() -> AppAction {
    AppAction::SetReopenLastProject(!AppConfig::default().closed_to_scratch)
}

pub(crate) fn check_updates_read(state: &AppState) -> bool {
    state.config.check_updates
}

pub(crate) fn check_updates_change(on: bool) -> AppAction {
    AppAction::SetCheckUpdates(on)
}

pub(crate) fn check_updates_is_default(state: &AppState) -> bool {
    state.config.check_updates == AppConfig::default().check_updates
}

pub(crate) fn check_updates_reset() -> AppAction {
    AppAction::SetCheckUpdates(AppConfig::default().check_updates)
}

pub(crate) fn auto_install_updates_read(state: &AppState) -> bool {
    state.config.auto_install_updates
}

pub(crate) fn auto_install_updates_change(on: bool) -> AppAction {
    AppAction::SetAutoInstallUpdates(on)
}

pub(crate) fn auto_install_updates_is_default(state: &AppState) -> bool {
    state.config.auto_install_updates == AppConfig::default().auto_install_updates
}

pub(crate) fn auto_install_updates_reset() -> AppAction {
    AppAction::SetAutoInstallUpdates(AppConfig::default().auto_install_updates)
}

/// The auto-install toggle is meaningful only while update checks run, so it is
/// disabled (and visually nested) under "Check for updates on launch".
pub(crate) fn auto_install_updates_enabled(state: &AppState) -> bool {
    state.config.check_updates
}

pub(crate) fn reopen_read(state: &AppState) -> bool {
    // The stored flag is the inverse ("closed to scratch"); present it the way a
    // user thinks about it.
    !state.config.closed_to_scratch
}

pub(crate) fn reopen_change(on: bool) -> AppAction {
    AppAction::SetReopenLastProject(on)
}
