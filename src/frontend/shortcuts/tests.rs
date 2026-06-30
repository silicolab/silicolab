use super::*;
use std::path::PathBuf;

use eframe::egui;
use nalgebra::Point3;

use crate::{
    backend::project::WorkspaceSession,
    domain::{Atom, Structure},
};

fn state_with_entry() -> AppState {
    AppState::new(
        Structure::new(
            "mol",
            vec![Atom {
                element: "C".to_string(),
                position: Point3::origin(),
                charge: 0.0,
            }],
        ),
        Some(PathBuf::from("mol.xyz")),
        WorkspaceSession::Scratch,
        Default::default(),
        Vec::new(),
        None,
    )
}

fn shortcut_input(
    key: egui::Key,
    modifiers: egui::Modifiers,
    wants_keyboard_input: bool,
) -> ShortcutInput<'static> {
    ShortcutInput {
        key_pressed: Box::leak(Box::new(move |candidate| candidate == key)),
        modifiers,
        wants_keyboard_input,
    }
}

#[test]
fn matches_specific_shortcut() {
    let state = state_with_entry();
    let bindings = registry();
    let binding = resolve_binding(
        &bindings,
        shortcut_input(egui::Key::S, mod_key(true, false, false), false),
        &state,
    )
    .expect("shortcut matches");

    assert_eq!(binding.id, "file.save");
}

#[test]
fn text_input_blocks_global_shortcuts() {
    let state = state_with_entry();
    let bindings = registry();
    let binding = resolve_binding(
        &bindings,
        shortcut_input(egui::Key::S, mod_key(true, false, false), true),
        &state,
    );

    assert!(binding.is_none());
}

#[test]
fn text_input_allows_escape() {
    let state = state_with_entry();
    let bindings = registry();
    let binding = resolve_binding(
        &bindings,
        shortcut_input(egui::Key::Escape, egui::Modifiers::NONE, true),
        &state,
    )
    .expect("escape still matches");

    assert_eq!(binding.id, "app.escape");
}

#[test]
fn text_input_allows_settings_shortcut() {
    let state = state_with_entry();
    let bindings = registry();
    let binding = resolve_binding(
        &bindings,
        shortcut_input(egui::Key::Comma, mod_key(true, false, false), true),
        &state,
    )
    .expect("settings shortcut still matches while typing");

    assert_eq!(binding.id, "app.settings");
}

#[test]
fn guard_blocks_entry_shortcuts_without_active_entry() {
    let state = AppState::scratch(Default::default(), Vec::new());
    let bindings = registry();
    let binding = resolve_binding(
        &bindings,
        shortcut_input(egui::Key::S, mod_key(true, false, false), false),
        &state,
    );

    assert!(binding.is_none());
}

#[test]
fn exact_modifiers_prevent_shift_conflict() {
    let state = state_with_entry();
    let bindings = registry();
    let binding = resolve_binding(
        &bindings,
        shortcut_input(egui::Key::S, mod_key(true, true, false), false),
        &state,
    )
    .expect("shifted shortcut matches");

    assert_eq!(binding.id, "file.save_as");
}

#[test]
fn higher_scope_wins_same_key_conflict() {
    let state = state_with_entry();
    let bindings = vec![
        ShortcutBinding::new(
            "global",
            ShortcutScope::Global,
            egui::Key::Escape,
            ShortcutModifiers::NONE,
            ShortcutCommand::ToggleSettings,
            "Global",
            ShortcutGuard::Always,
        ),
        ShortcutBinding::new(
            "modal",
            ShortcutScope::Modal,
            egui::Key::Escape,
            ShortcutModifiers::NONE,
            ShortcutCommand::Escape,
            "Modal",
            ShortcutGuard::Always,
        ),
    ];
    let binding = resolve_binding(
        &bindings,
        shortcut_input(egui::Key::Escape, egui::Modifiers::NONE, false),
        &state,
    )
    .expect("shortcut matches");

    assert_eq!(binding.id, "modal");
}

#[test]
fn label_generation_uses_registry() {
    let label = label_for("edit.redo_alt").expect("label");

    if cfg!(target_os = "macos") {
        assert_eq!(label, "Cmd+Shift+Z");
    } else {
        assert_eq!(label, "Ctrl+Shift+Z");
    }
}

fn mod_key(command: bool, shift: bool, alt: bool) -> egui::Modifiers {
    egui::Modifiers {
        alt,
        ctrl: command,
        shift,
        mac_cmd: false,
        command,
    }
}
