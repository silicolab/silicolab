use super::*;

/// Apply and persist the light/dark appearance preference. egui switches the
/// active theme immediately; the choice is written to the global settings file.
pub(crate) fn set_theme_mode(
    state: &mut AppState,
    mode: crate::backend::config::ThemeMode,
    ctx: &egui::Context,
) {
    state.config.theme = mode;
    crate::frontend::theme::set_preference(ctx, mode);
    if let Err(error) = save_config(&state.config) {
        state.set_message(format!("Could not save theme preference: {error}"));
    }
}

pub(crate) fn set_color_scheme(
    state: &mut AppState,
    scheme: crate::backend::config::ColorScheme,
    ctx: &egui::Context,
) {
    state.config.color_scheme = scheme;
    // Rebuild the visuals live; the scheme is read back by `theme::palette`.
    crate::frontend::theme::set_scheme(ctx, scheme);
    if let Err(error) = save_config(&state.config) {
        state.set_message(format!("Could not save color scheme: {error}"));
    }
}

/// Apply one Representation default edit and persist. These defaults only seed
/// the appearance of *future* new entries/surfaces, so there is no live
/// re-render here — the change lands the next time a structure is built/loaded
/// or a surface is first enabled.
pub(crate) fn set_representation(
    state: &mut AppState,
    edit: crate::backend::representation::RepresentationEdit,
) {
    state.config.representation.apply(edit);
    if let Err(error) = save_config(&state.config) {
        state.set_message(format!("Could not save representation defaults: {error}"));
    }
}

/// Restore one Representation group to its defaults and persist.
pub(crate) fn reset_representation_group(
    state: &mut AppState,
    group: crate::backend::representation::RepresentationGroup,
) {
    state.config.representation.reset_group(group);
    if let Err(error) = save_config(&state.config) {
        state.set_message(format!("Could not save representation defaults: {error}"));
    }
}

/// Restore every Representation default and persist.
pub(crate) fn reset_representation_defaults(state: &mut AppState) {
    state.config.representation = crate::backend::representation::RepresentationPrefs::default();
    if let Err(error) = save_config(&state.config) {
        state.set_message(format!("Could not save representation defaults: {error}"));
    }
}

/// Persist whether launches check GitHub Releases for a newer version. Turning
/// the check on also runs one immediately (unless one is already in flight), so
/// the user sees the result without restarting.
pub(crate) fn set_check_updates(state: &mut AppState, on: bool) {
    state.config.check_updates = on;
    if let Err(error) = save_config(&state.config) {
        state.set_message(format!("Could not save update preference: {error}"));
    }
    if on && state.jobs.update_check.is_none() && state.ui.available_update.is_none() {
        state.jobs.update_check = Some(spawn_update_check());
    }
}

/// Persist whether discovered updates install themselves automatically. If a
/// newer release is already known and the install is writable, switching this
/// on starts the download right away (unless one is already running), so the
/// toggle gives immediate effect rather than waiting for the next launch.
pub(crate) fn set_auto_install_updates(state: &mut AppState, on: bool) {
    state.config.auto_install_updates = on;
    if let Err(error) = save_config(&state.config) {
        state.set_message(format!("Could not save update preference: {error}"));
    }
    if on {
        maybe_auto_install_update(state);
    }
}

/// Start an automatic update install when one is warranted: the user opted in,
/// a newer release was found, the install location is writable, and no
/// self-update is already in flight or finished. Shared by the settings toggle
/// and the background update-check poll so both honor the same gate.
pub(crate) fn maybe_auto_install_update(state: &mut AppState) {
    if !state.config.auto_install_updates
        || state.ui.available_update.is_none()
        || state.jobs.self_update.is_some()
        || !matches!(state.ui.self_update, SelfUpdateStatus::Idle)
        || !crate::io::self_update::is_self_update_supported()
    {
        return;
    }
    let version = state
        .ui
        .available_update
        .as_ref()
        .map(|update| update.version.clone())
        .unwrap_or_default();
    state.ui.self_update = SelfUpdateStatus::Downloading;
    state.jobs.self_update = Some(spawn_self_update());
    state.set_message(format!("Downloading SilicoLab {version}…"));
}

/// Persist whether the next launch reopens the last project. The stored field
/// is `closed_to_scratch` (set when the user closes to scratch); the setting is
/// its inverse so it reads naturally as "reopen last project on launch".
pub(crate) fn set_reopen_last_project(state: &mut AppState, reopen: bool) {
    state.config.closed_to_scratch = !reopen;
    if let Err(error) = save_config(&state.config) {
        state.set_message(format!("Could not save startup preference: {error}"));
    }
}

/// Pick and persist the default project directory via a native folder picker.
/// The blocking dialog lives here (the mutation layer), matching the other
/// project file pickers; the settings widget only emits the action.
pub(crate) fn pick_default_project_dir(state: &mut AppState) {
    let Some(path) = rfd::FileDialog::new()
        .set_directory(&state.config.default_project_dir)
        .pick_folder()
    else {
        return;
    };
    state.config.default_project_dir = path;
    if let Err(error) = save_config(&state.config) {
        state.set_message(format!("Could not save default project folder: {error}"));
    } else {
        state.set_message(format!(
            "Default project folder set to {}",
            state.config.default_project_dir.display()
        ));
    }
}

/// Reveal the global settings.json in the OS file manager, selecting the file
/// where the platform supports it. Falls back to opening the containing folder
/// when the file doesn't exist yet, and to a path message if the shell-out
/// fails — never an error path the user can't recover from.
pub(crate) fn reveal_settings_file(state: &mut AppState) {
    let path = crate::backend::config::settings_path();
    let revealed = if path.exists() {
        reveal_in_file_manager(&path, true)
    } else if let Some(parent) = path.parent() {
        reveal_in_file_manager(parent, false)
    } else {
        false
    };
    if revealed {
        state.set_message(format!("Revealed {}", path.display()));
    } else {
        state.set_message(format!("Settings file: {}", path.display()));
    }
}

/// Open the OS file manager at `path`. With `select`, the file is highlighted in
/// its folder (Explorer `/select`, Finder `-R`); otherwise `path` is opened
/// directly. Returns whether the launcher process spawned.
pub(crate) fn reveal_in_file_manager(path: &Path, select: bool) -> bool {
    use std::process::Command;
    #[cfg(target_os = "windows")]
    {
        let mut command = Command::new("explorer");
        if select {
            command.arg(format!("/select,{}", path.display()));
        } else {
            command.arg(path);
        }
        command.spawn().is_ok()
    }
    #[cfg(target_os = "macos")]
    {
        let mut command = Command::new("open");
        if select {
            command.arg("-R");
        }
        command.arg(path);
        command.spawn().is_ok()
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        // No portable "reveal and select"; open the containing folder.
        let target = if select {
            path.parent().unwrap_or(path)
        } else {
            path
        };
        Command::new("xdg-open").arg(target).spawn().is_ok()
    }
}

/// Restore user-facing preferences to their defaults and persist. Preserves
/// `engine_overrides` and `last_project_path` (see
/// [`AppConfig::reset_preferences`]). Mirrors the live-refresh the individual
/// setters do (theme + color scheme are pushed into egui so the change is
/// visible immediately; glass reads `config` each frame).
pub(crate) fn reset_all_settings(state: &mut AppState, ctx: &egui::Context) {
    state.config = state.config.reset_preferences();
    crate::frontend::theme::set_preference(ctx, state.config.theme);
    crate::frontend::theme::set_scheme(ctx, state.config.color_scheme);
    if let Err(error) = save_config(&state.config) {
        state.set_message(format!("Could not save reset settings: {error}"));
    } else {
        state.set_message("All settings reset to defaults".to_string());
    }
}

/// Export the current settings to a user-chosen JSON file. The blocking dialog
/// runs here (the mutation layer), matching the other pickers.
pub(crate) fn export_settings(state: &mut AppState) {
    let Some(path) = rfd::FileDialog::new()
        .set_file_name("settings.json")
        .add_filter("JSON", &["json"])
        .save_file()
    else {
        return;
    };
    match crate::backend::config::save_config_to(&path, &state.config) {
        Ok(()) => state.set_message(format!("Exported settings to {}", path.display())),
        Err(error) => state.set_message(format!("Could not export settings: {error}")),
    }
}

/// Import settings from a user-chosen JSON file. Malformed input is reported
/// non-fatally (mirroring `load_config`'s graceful fallback) and leaves the
/// current settings untouched. On success the config is applied, saved, and the
/// live theme/scheme are refreshed the way the individual setters do.
pub(crate) fn import_settings(state: &mut AppState, ctx: &egui::Context) {
    let Some(path) = rfd::FileDialog::new()
        .add_filter("JSON", &["json"])
        .pick_file()
    else {
        return;
    };
    match crate::backend::config::load_config_from(&path) {
        Ok(config) => {
            state.config = config;
            crate::frontend::theme::set_preference(ctx, state.config.theme);
            crate::frontend::theme::set_scheme(ctx, state.config.color_scheme);
            if let Err(error) = save_config(&state.config) {
                state.set_message(format!("Imported settings, but could not save: {error}"));
            } else {
                state.set_message(format!("Imported settings from {}", path.display()));
            }
        }
        Err(error) => state.set_message(format!("Could not import settings: {error}")),
    }
}

pub(crate) fn resize_sidebar(state: &mut AppState, side: Side, delta: f32, ctx: &egui::Context) {
    let max_w = sidebar_max_width(ctx.viewport_rect().width());
    let (width, min) = match side {
        Side::Primary => (
            &mut state.ui.layout.primary_sidebar_width,
            SIDEBAR_MIN_WIDTH_PRIMARY,
        ),
        Side::Secondary => (
            &mut state.ui.layout.secondary_sidebar_width,
            SIDEBAR_MIN_WIDTH_SECONDARY,
        ),
    };
    *width = (*width + delta).clamp(min, max_w);
}

pub(crate) fn reset_sidebar(state: &mut AppState, side: Side, ctx: &egui::Context) {
    let max_w = sidebar_max_width(ctx.viewport_rect().width());
    let (width, min, default) = match side {
        Side::Primary => (
            &mut state.ui.layout.primary_sidebar_width,
            SIDEBAR_MIN_WIDTH_PRIMARY,
            SIDEBAR_DEFAULT_WIDTH_PRIMARY,
        ),
        Side::Secondary => (
            &mut state.ui.layout.secondary_sidebar_width,
            SIDEBAR_MIN_WIDTH_SECONDARY,
            SIDEBAR_DEFAULT_WIDTH_SECONDARY,
        ),
    };
    *width = default.clamp(min, max_w);
}

pub(crate) fn resize_panel(state: &mut AppState, delta: f32, ctx: &egui::Context) {
    let max_h = (ctx.viewport_rect().height() * 0.6).max(160.0);
    state.ui.layout.panel_height =
        (state.ui.layout.panel_height + delta).clamp(PANEL_MIN_HEIGHT, max_h);
}

pub(crate) fn reset_panel(state: &mut AppState) {
    state.ui.layout.panel_height = PANEL_DEFAULT_HEIGHT;
}

/// Persist the frosted-glass preference. The clear color and chrome fills read
/// `config.glass` each frame (resolved into `ui.glass_active`), so the change is
/// visible immediately; only the stored setting needs writing here.
pub(crate) fn set_glass(state: &mut AppState, on: bool) {
    state.config.glass = on;
    if let Err(error) = save_config(&state.config) {
        state.set_message(format!("Could not save glass preference: {error}"));
    }
}

/// Update the Liquid Glass tint intensity. The chrome alpha is re-resolved from
/// `config.glass_intensity` next frame, so a mid-drag update (`commit == false`)
/// previews live without writing settings.json dozens of times per second; the
/// release event commits once.
pub(crate) fn set_glass_intensity(state: &mut AppState, value: f32, commit: bool) {
    state.config.glass_intensity = value.clamp(0.0, 1.0);
    if commit && let Err(error) = save_config(&state.config) {
        state.set_message(format!("Could not save glass intensity: {error}"));
    }
}
