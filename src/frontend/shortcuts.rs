use eframe::egui;

use crate::{
    domain::AtomCategory,
    frontend::{
        actions::{AppAction, VisibilityCommand},
        dispatcher,
        state::{AppState, AtomStyle, DockArea, PrimaryView, StaticView},
    },
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum ShortcutScope {
    Global,
    Workbench,
    Viewport,
    Selection,
    Trajectory,
    Modal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ShortcutGuard {
    Always,
    HasActiveEntry,
    HasSelection,
    HistoryNavigation,
    PrimarySidebarVisible,
    TrajectoryLoaded,
    ActiveMdEntryWithTrajectory,
}

#[derive(Debug, Clone)]
pub(crate) enum ShortcutCommand {
    Action(AppAction),
    ToggleSettings,
    SetPrimaryView(PrimaryView),
    RevealStaticView(StaticView),
    OpenPrimarySearch,
    ResetViewportCamera,
    ZoomViewport(f32),
    LoadDefaultTrajectory,
    Escape,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ShortcutModifiers {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub command: bool,
    pub mac_cmd: bool,
}

impl ShortcutModifiers {
    const NONE: Self = Self {
        ctrl: false,
        shift: false,
        alt: false,
        command: false,
        mac_cmd: false,
    };

    const SHIFT: Self = Self {
        shift: true,
        ..Self::NONE
    };

    const ALT: Self = Self {
        alt: true,
        ..Self::NONE
    };

    const MOD: Self = Self {
        command: true,
        ..Self::NONE
    };

    const MOD_SHIFT: Self = Self {
        command: true,
        shift: true,
        ..Self::NONE
    };
}

#[derive(Debug, Clone)]
pub(crate) struct ShortcutBinding {
    pub id: &'static str,
    pub scope: ShortcutScope,
    pub key: egui::Key,
    pub modifiers: ShortcutModifiers,
    pub command: ShortcutCommand,
    pub label: &'static str,
    pub when: ShortcutGuard,
}

impl ShortcutBinding {
    fn new(
        id: &'static str,
        scope: ShortcutScope,
        key: egui::Key,
        modifiers: ShortcutModifiers,
        command: ShortcutCommand,
        label: &'static str,
        when: ShortcutGuard,
    ) -> Self {
        Self {
            id,
            scope,
            key,
            modifiers,
            command,
            label,
            when,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ShortcutEffect {
    pub command: ShortcutCommand,
}

pub(crate) fn handle_frame(state: &mut AppState, ctx: &egui::Context) {
    let effects = collect_effects(state, ctx);
    for effect in effects {
        match effect.command {
            ShortcutCommand::Action(action) => dispatcher::dispatch(state, action, ctx),
            command => apply_ui_command(state, command, ctx),
        }
    }
}

pub(crate) fn label_for(id: &str) -> Option<String> {
    registry()
        .into_iter()
        .find(|binding| binding.id == id)
        .map(|binding| binding.shortcut_label())
}

pub(crate) fn menu_text(id: &str, label: &str) -> String {
    match registry().into_iter().find(|binding| binding.id == id) {
        Some(binding) => {
            let label = if label.is_empty() {
                binding.label
            } else {
                label
            };
            format!("{label}\t{}", binding.shortcut_label())
        }
        None => label.to_string(),
    }
}

fn collect_effects(state: &AppState, ctx: &egui::Context) -> Vec<ShortcutEffect> {
    let wants_keyboard_input = ctx.egui_wants_keyboard_input();
    let Some(binding) = ctx.input_mut(|input| {
        let bindings = registry();
        let key_pressed = |key| {
            input
                .events
                .iter()
                .any(|event| key_event_matches(event, key))
        };
        resolve_binding(
            &bindings,
            ShortcutInput {
                key_pressed: &key_pressed,
                modifiers: input.modifiers,
                wants_keyboard_input,
            },
            state,
        )
        .cloned()
    }) else {
        return Vec::new();
    };

    ctx.input_mut(|input| consume_key_events(input, binding.key));
    vec![ShortcutEffect {
        command: binding.command,
    }]
}

fn resolve_binding<'a>(
    bindings: &'a [ShortcutBinding],
    input: ShortcutInput<'_>,
    state: &AppState,
) -> Option<&'a ShortcutBinding> {
    bindings
        .iter()
        .filter(|binding| binding_matches(binding, input, state))
        .max_by_key(|binding| (binding.scope, binding.modifier_specificity()))
}

#[derive(Clone, Copy)]
struct ShortcutInput<'a> {
    key_pressed: &'a dyn Fn(egui::Key) -> bool,
    modifiers: egui::Modifiers,
    wants_keyboard_input: bool,
}

fn binding_matches(binding: &ShortcutBinding, input: ShortcutInput<'_>, state: &AppState) -> bool {
    (input.key_pressed)(binding.key)
        && modifiers_match(binding.modifiers, input.modifiers)
        && text_input_allows(binding, input.wants_keyboard_input)
        && guard_allows(binding.when, state)
}

fn text_input_allows(binding: &ShortcutBinding, wants_keyboard_input: bool) -> bool {
    !wants_keyboard_input
        || matches!(
            binding.command,
            ShortcutCommand::Escape | ShortcutCommand::ToggleSettings
        )
}

fn modifiers_match(expected: ShortcutModifiers, actual: egui::Modifiers) -> bool {
    expected.shift == actual.shift
        && expected.alt == actual.alt
        && if expected.command {
            actual.command || actual.ctrl
        } else {
            !actual.command && !actual.ctrl
        }
        && if expected.command {
            true
        } else {
            expected.ctrl == actual.ctrl && expected.mac_cmd == actual.mac_cmd
        }
}

fn key_event_matches(event: &egui::Event, key: egui::Key) -> bool {
    matches!(
        event,
        egui::Event::Key {
            key: event_key,
            pressed: true,
            ..
        } if *event_key == key
    )
}

fn consume_key_events(input: &mut egui::InputState, key: egui::Key) {
    input.events.retain(|event| !key_event_matches(event, key));
}

fn guard_allows(guard: ShortcutGuard, state: &AppState) -> bool {
    match guard {
        ShortcutGuard::Always => true,
        ShortcutGuard::HasActiveEntry => state.has_active_entry(),
        ShortcutGuard::HasSelection => state.has_active_entry() && !state.ui.selection.is_empty(),
        ShortcutGuard::HistoryNavigation => state.history_navigation_enabled(),
        ShortcutGuard::PrimarySidebarVisible => state.ui.layout.show_primary_sidebar,
        ShortcutGuard::TrajectoryLoaded => state.ui.trajectory.is_some(),
        ShortcutGuard::ActiveMdEntryWithTrajectory => active_md_entry_with_trajectory(state),
    }
}

fn active_md_entry_with_trajectory(state: &AppState) -> bool {
    let Some(entry) = state.entries.active_entry() else {
        return false;
    };
    entry.origin.trajectory().is_some()
        && state
            .ui
            .trajectory
            .as_ref()
            .is_none_or(|playback| playback.entry_id != entry.id)
}

fn apply_ui_command(state: &mut AppState, command: ShortcutCommand, ctx: &egui::Context) {
    match command {
        ShortcutCommand::ToggleSettings => {
            state.ui.layout.settings_open = !state.ui.layout.settings_open;
        }
        ShortcutCommand::SetPrimaryView(view) => {
            state.ui.layout.show_primary_sidebar = true;
            state.ui.layout.active_primary_view = view;
        }
        ShortcutCommand::RevealStaticView(view) => {
            state.ui.layout.dock.reveal_static(view);
            state.mark_layout_dirty(ctx.input(|input| input.time));
        }
        ShortcutCommand::OpenPrimarySearch => {
            state.ui.entry_list.search_open = true;
        }
        ShortcutCommand::ResetViewportCamera => {
            state.ui.camera = Default::default();
            state.status_neutral("Reset viewport camera".to_string());
            ctx.request_repaint();
        }
        ShortcutCommand::ZoomViewport(delta) => {
            state.ui.camera.zoom = (state.ui.camera.zoom + delta).clamp(-0.85, 3.0);
            ctx.request_repaint();
        }
        ShortcutCommand::LoadDefaultTrajectory => {
            if let Some(entry_id) = state.entries.active_entry_id() {
                dispatcher::dispatch(state, AppAction::LoadTrajectory(entry_id, None), ctx);
            }
        }
        ShortcutCommand::Escape => apply_escape(state, ctx),
        ShortcutCommand::Action(_) => {}
    }
}

fn apply_escape(state: &mut AppState, ctx: &egui::Context) {
    if state.ui.entry_list.search_open {
        state.ui.entry_list.search_open = false;
        return;
    }
    if state.ui.layout.settings_open {
        state.ui.layout.settings_open = false;
        return;
    }
    if state.ui.layout.about_open {
        state.ui.layout.about_open = false;
        return;
    }
    if state.ui.text_viewer.is_some() {
        state.ui.text_viewer = None;
        return;
    }
    if state.ui.notification.is_some() {
        state.ui.notification = None;
        return;
    }
    if let Some(playback) = state.ui.trajectory.as_ref() {
        if playback.playing {
            dispatcher::dispatch(state, AppAction::ToggleTrajectoryPlay, ctx);
        } else {
            dispatcher::dispatch(state, AppAction::StopTrajectory, ctx);
        }
        return;
    }
    if !state.ui.selection.is_empty() {
        dispatcher::dispatch(state, AppAction::ClearSelection, ctx);
    }
}

mod registry;

use self::registry::registry;

impl ShortcutBinding {
    fn shortcut_label(&self) -> String {
        shortcut_label(self.modifiers, self.key)
    }

    fn modifier_specificity(&self) -> u8 {
        self.modifiers.ctrl as u8
            + self.modifiers.shift as u8
            + self.modifiers.alt as u8
            + self.modifiers.command as u8
            + self.modifiers.mac_cmd as u8
    }
}

fn shortcut_label(modifiers: ShortcutModifiers, key: egui::Key) -> String {
    let mut parts = Vec::new();
    if modifiers.command {
        parts.push(platform_command_label());
    }
    if modifiers.ctrl {
        parts.push("Ctrl");
    }
    if modifiers.alt {
        parts.push("Alt");
    }
    if modifiers.shift {
        parts.push("Shift");
    }
    if modifiers.mac_cmd {
        parts.push("Cmd");
    }
    parts.push(key_label(key));
    parts.join("+")
}

fn platform_command_label() -> &'static str {
    if cfg!(target_os = "macos") {
        "Cmd"
    } else {
        "Ctrl"
    }
}

fn key_label(key: egui::Key) -> &'static str {
    match key {
        egui::Key::Comma => ",",
        egui::Key::Backtick => "`",
        egui::Key::Plus => "+",
        egui::Key::Equals => "=",
        egui::Key::Minus => "-",
        egui::Key::Space => "Space",
        egui::Key::Escape => "Esc",
        _ => key.name(),
    }
}

#[cfg(test)]
mod tests;
