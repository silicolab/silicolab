//! Native macOS menu bar (NSMenu via `muda`).
//!
//! macOS hides the in-window egui menus (`ui/mod.rs` `show_inline_menus`)
//! because the window draws its own borderless title bar; the system menu bar
//! at the top of the screen is where Mac users expect File/Edit/etc. This
//! module builds that native bar once at startup, mirroring the egui menus
//! item-for-item, and routes clicks back through the same [`AppAction`]
//! dispatcher.
//!
//! Flow: AppKit fires a [`MenuEvent`] on the main thread → our global handler
//! forwards the [`MenuId`] into an mpsc channel and wakes the (reactive) eframe
//! loop with `request_repaint` → [`MacMenu::drain`] turns pending ids into
//! [`MenuCommand`]s each frame → the app applies them. [`MacMenu::sync`]
//! reconciles enabled/checked state and the dynamic Recent Projects submenu
//! from [`AppState`].
//!
//! Everything here is `cfg(target_os = "macos")`. muda's menu objects are `Rc`
//! based and therefore `!Send`; [`MacMenu`] must stay on the main thread (it
//! lives in `SilicoLabApp`, which the event loop only ever touches there).

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender};

use eframe::egui;
use muda::accelerator::{Accelerator, Code, Modifiers};
use muda::{CheckMenuItem, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem, Submenu};
use objc2_foundation::{NSProcessInfo, NSString};

use crate::backend::config::{ColorScheme, ThemeMode};
use crate::domain::AtomCategory;
use crate::frontend::actions::AppAction;
use crate::frontend::state::AppState;

/// What a menu click asks the app loop to do.
///
/// `Action` is dispatched through `dispatcher::dispatch` exactly like an egui
/// menu click; the remaining variants are the transient-chrome mutations the
/// egui menus perform inline (sidebar flags, settings panel, layout reset) and
/// which `ARCHITECTURE.md` explicitly exempts from the dispatcher.
pub enum MenuCommand {
    Action(AppAction),
    ShowAbout,
    ToggleSettings,
    TogglePrimarySidebar,
    ToggleSecondarySidebar,
    TogglePanel,
    ToggleAtomLabels,
    ResetWorkbenchLayout,
    Quit,
}

/// Cached copies of the last values pushed into AppKit, so `sync` only calls
/// across the FFI boundary when something actually changed.
struct SyncCache {
    is_project: bool,
    has_entry: bool,
    can_undo: bool,
    can_redo: bool,
    primary_sidebar: bool,
    secondary_sidebar: bool,
    panel: bool,
    atom_labels: bool,
    theme: ThemeMode,
    scheme: ColorScheme,
    recent: Vec<(PathBuf, String)>,
}

/// Owns the native menu and the handles whose state tracks [`AppState`].
///
/// `_menu` is retained only to keep the NSMenu (and everything hung off it)
/// alive for the process lifetime; `NSApp.setMainMenu:` retains it too, but we
/// hold it so the Rust side never drops the root.
pub struct MacMenu {
    _menu: Menu,
    rx: Receiver<MenuId>,

    // File
    recent_submenu: Submenu,
    recent_paths: Vec<PathBuf>,
    close_project: MenuItem,
    save: MenuItem,
    save_as: MenuItem,

    // Edit
    undo: MenuItem,
    redo: MenuItem,
    edit_structure: MenuItem,

    // View
    primary_sidebar: CheckMenuItem,
    secondary_sidebar: CheckMenuItem,
    panel: CheckMenuItem,
    atom_labels: CheckMenuItem,
    theme_items: Vec<CheckMenuItem>,
    scheme_items: Vec<CheckMenuItem>,

    cache: SyncCache,
}

fn accel(mods: Modifiers, code: Code) -> Option<Accelerator> {
    Some(Accelerator::new(Some(mods), code))
}

impl MacMenu {
    /// Build the menu, attach it to NSApp, and install the global click handler.
    ///
    /// Must run **on the main thread, after the NSApplication exists** — i.e.
    /// from eframe's app-creator closure, which is exactly where it is called.
    /// Call at most once per process: muda's event handler is a `OnceCell`, and
    /// a second `init_for_nsapp` would replace the bar.
    pub fn install(ctx: &egui::Context) -> Self {
        set_process_name();

        let (tx, rx): (Sender<MenuId>, Receiver<MenuId>) = std::sync::mpsc::channel();
        let ctx = ctx.clone();
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            // Runs on the main thread during menu tracking. Forward the id and
            // wake the reactive loop so the click is handled even when idle.
            let _ = tx.send(event.id);
            ctx.request_repaint();
        }));

        let menu = Menu::new();

        // --- Application menu (must be the first submenu: muda promotes it to
        // the macOS app menu) ---
        let app_menu = Submenu::new("SilicoLab", true);
        app_menu.set_text("SilicoLab");
        let about = MenuItem::with_id("app.about", "About SilicoLab", true, None);
        let settings = MenuItem::with_id(
            "app.settings",
            "Settings…",
            true,
            accel(Modifiers::META, Code::Comma),
        );
        let quit = MenuItem::with_id(
            "app.quit",
            "Quit SilicoLab",
            true,
            accel(Modifiers::META, Code::KeyQ),
        );
        app_menu
            .append_items(&[
                &about,
                &PredefinedMenuItem::separator(),
                &settings,
                &PredefinedMenuItem::separator(),
                &PredefinedMenuItem::services(None),
                &PredefinedMenuItem::hide(None),
                &PredefinedMenuItem::hide_others(None),
                &PredefinedMenuItem::show_all(None),
                &PredefinedMenuItem::separator(),
                &quit,
            ])
            .expect("build app menu");

        // --- File ---
        let recent_submenu = Submenu::new("Recent Projects", false);
        let close_project = MenuItem::with_id("file.close_project", "Close Project", true, None);
        let save = MenuItem::with_id(
            "file.save",
            "Save",
            true,
            accel(Modifiers::META, Code::KeyS),
        );
        let save_as = MenuItem::with_id(
            "file.save_as",
            "Save As…",
            true,
            accel(Modifiers::META | Modifiers::SHIFT, Code::KeyS),
        );
        let file_menu = Submenu::new("File", true);
        file_menu
            .append_items(&[
                &MenuItem::with_id("file.new_project", "Create a new project…", true, None),
                &MenuItem::with_id(
                    "file.open_project",
                    "Open Project…",
                    true,
                    accel(Modifiers::META | Modifiers::SHIFT, Code::KeyO),
                ),
                &MenuItem::with_id("file.save_project", "Save Project", true, None),
                &close_project,
                &PredefinedMenuItem::separator(),
                &recent_submenu,
                &PredefinedMenuItem::separator(),
                &MenuItem::with_id(
                    "file.new_entry",
                    "New Empty Entry",
                    true,
                    accel(Modifiers::META, Code::KeyN),
                ),
                &MenuItem::with_id("file.sketch_molecule", "Sketch Molecule…", true, None),
                &MenuItem::with_id(
                    "file.open_file",
                    "Open File…",
                    true,
                    accel(Modifiers::META, Code::KeyO),
                ),
                &MenuItem::with_id("file.fetch_pdb", "Fetch from PDB ID…", true, None),
                &PredefinedMenuItem::separator(),
                &save,
                &save_as,
            ])
            .expect("build file menu");

        // --- Edit ---
        let undo = MenuItem::with_id(
            "edit.undo",
            "Undo",
            true,
            accel(Modifiers::META, Code::KeyZ),
        );
        let redo = MenuItem::with_id(
            "edit.redo",
            "Redo",
            true,
            accel(Modifiers::META | Modifiers::SHIFT, Code::KeyZ),
        );
        let edit_structure = MenuItem::with_id("edit.structure", "Edit Structure…", true, None);
        let edit_menu = Submenu::new("Edit", true);
        edit_menu
            .append_items(&[
                &undo,
                &redo,
                &PredefinedMenuItem::separator(),
                &edit_structure,
            ])
            .expect("build edit menu");

        // --- Selection ---
        let selection_menu = Submenu::new("Selection", true);
        // `Select All` deliberately gets no accelerator: Cmd+A must stay with
        // egui text fields. (The egui menu has no accelerator here either.)
        selection_menu
            .append_items(&[
                &MenuItem::with_id("selection.all", "Select All", true, None),
                &MenuItem::with_id("selection.invert", "Invert Selection", true, None),
                &MenuItem::with_id("selection.clear", "Clear Selection", true, None),
                &PredefinedMenuItem::separator(),
            ])
            .expect("build selection menu");
        for (index, category) in AtomCategory::selectable().iter().enumerate() {
            selection_menu
                .append(&MenuItem::with_id(
                    format!("selection.cat.{index}"),
                    category.label(),
                    true,
                    None,
                ))
                .expect("build selection category");
        }

        // --- View ---
        let primary_sidebar = CheckMenuItem::with_id(
            "view.primary_sidebar",
            "Primary Side Bar",
            true,
            false,
            None,
        );
        let secondary_sidebar = CheckMenuItem::with_id(
            "view.secondary_sidebar",
            "Secondary Side Bar",
            true,
            false,
            None,
        );
        let panel = CheckMenuItem::with_id("view.panel", "Panel", true, false, None);
        let atom_labels =
            CheckMenuItem::with_id("view.atom_labels", "Show Atom Labels", true, false, None);

        let appearance = Submenu::new("Appearance", true);
        let mut theme_items = Vec::new();
        for (index, mode) in ThemeMode::all().into_iter().enumerate() {
            let item = CheckMenuItem::with_id(
                format!("view.theme.{index}"),
                mode.label(),
                true,
                false,
                None,
            );
            appearance.append(&item).expect("append theme item");
            theme_items.push(item);
        }
        appearance
            .append(&PredefinedMenuItem::separator())
            .expect("append appearance separator");
        let mut scheme_items = Vec::new();
        for (index, scheme) in ColorScheme::all().into_iter().enumerate() {
            let item = CheckMenuItem::with_id(
                format!("view.scheme.{index}"),
                scheme.label(),
                true,
                false,
                None,
            );
            appearance.append(&item).expect("append scheme item");
            scheme_items.push(item);
        }

        let view_menu = Submenu::new("View", true);
        view_menu
            .append_items(&[
                &primary_sidebar,
                &secondary_sidebar,
                &panel,
                &atom_labels,
                &PredefinedMenuItem::separator(),
                &MenuItem::with_id("view.reset_layout", "Reset Workbench Layout", true, None),
                &PredefinedMenuItem::separator(),
                &appearance,
            ])
            .expect("build view menu");

        menu.append_items(&[
            &app_menu,
            &file_menu,
            &edit_menu,
            &selection_menu,
            &view_menu,
        ])
        .expect("assemble menu bar");
        menu.init_for_nsapp();
        app_menu.set_text("SilicoLab");

        Self {
            _menu: menu,
            rx,
            recent_submenu,
            recent_paths: Vec::new(),
            close_project,
            save,
            save_as,
            undo,
            redo,
            edit_structure,
            primary_sidebar,
            secondary_sidebar,
            panel,
            atom_labels,
            theme_items,
            scheme_items,
            // Seed with impossible values so the first `sync` pushes the real
            // state across to AppKit.
            cache: SyncCache {
                is_project: false,
                has_entry: false,
                can_undo: false,
                can_redo: false,
                primary_sidebar: false,
                secondary_sidebar: false,
                panel: false,
                atom_labels: false,
                theme: ThemeMode::System,
                scheme: ColorScheme::default(),
                recent: Vec::new(),
            },
        }
    }

    /// Drain all menu clicks queued since the last frame into commands.
    pub fn drain(&mut self) -> Vec<MenuCommand> {
        // Collect first so the borrow on `self.rx` is released before `map_id`
        // borrows `self`.
        let ids: Vec<MenuId> = self.rx.try_iter().collect();
        ids.iter().filter_map(|id| self.map_id(id)).collect()
    }

    fn map_id(&self, id: &MenuId) -> Option<MenuCommand> {
        let id = id.as_ref();
        let command = match id {
            "app.about" => MenuCommand::ShowAbout,
            "app.settings" => MenuCommand::ToggleSettings,
            "app.quit" => MenuCommand::Quit,
            "file.new_project" => MenuCommand::Action(AppAction::CreateProject),
            "file.open_project" => MenuCommand::Action(AppAction::OpenProject),
            "file.save_project" => MenuCommand::Action(AppAction::SaveProject),
            "file.close_project" => MenuCommand::Action(AppAction::CloseProject),
            "file.new_entry" => MenuCommand::Action(AppAction::NewEmptyEntry),
            "file.sketch_molecule" => MenuCommand::Action(AppAction::SketchMolecule),
            "file.open_file" => MenuCommand::Action(AppAction::OpenFile),
            "file.fetch_pdb" => MenuCommand::Action(AppAction::OpenPdbFetchDialog),
            "file.save" => MenuCommand::Action(AppAction::Save),
            "file.save_as" => MenuCommand::Action(AppAction::SaveAs),
            "edit.undo" => MenuCommand::Action(AppAction::Undo),
            "edit.redo" => MenuCommand::Action(AppAction::Redo),
            "edit.structure" => MenuCommand::Action(AppAction::EditStructure),
            "selection.all" => MenuCommand::Action(AppAction::SelectAll),
            "selection.invert" => MenuCommand::Action(AppAction::InvertSelection),
            "selection.clear" => MenuCommand::Action(AppAction::ClearSelection),
            "view.primary_sidebar" => MenuCommand::TogglePrimarySidebar,
            "view.secondary_sidebar" => MenuCommand::ToggleSecondarySidebar,
            "view.panel" => MenuCommand::TogglePanel,
            "view.atom_labels" => MenuCommand::ToggleAtomLabels,
            "view.reset_layout" => MenuCommand::ResetWorkbenchLayout,
            _ => {
                if let Some(rest) = id.strip_prefix("file.recent.") {
                    let index: usize = rest.parse().ok()?;
                    let path = self.recent_paths.get(index)?.clone();
                    return Some(MenuCommand::Action(AppAction::OpenRecentProject(path)));
                }
                if let Some(rest) = id.strip_prefix("selection.cat.") {
                    let index: usize = rest.parse().ok()?;
                    let category = *AtomCategory::selectable().get(index)?;
                    return Some(MenuCommand::Action(AppAction::SelectCategory(category)));
                }
                if let Some(rest) = id.strip_prefix("view.theme.") {
                    let index: usize = rest.parse().ok()?;
                    let mode = *ThemeMode::all().get(index)?;
                    return Some(MenuCommand::Action(AppAction::SetThemeMode(mode)));
                }
                if let Some(rest) = id.strip_prefix("view.scheme.") {
                    let index: usize = rest.parse().ok()?;
                    let scheme = *ColorScheme::all().get(index)?;
                    return Some(MenuCommand::Action(AppAction::SetColorScheme(scheme)));
                }
                return None;
            }
        };
        Some(command)
    }

    /// Reconcile the native menu with `state`. Call once per frame after the
    /// dispatch loop. Cheap: every AppKit call is gated on a changed value.
    pub fn sync(&mut self, state: &AppState, ctx: &egui::Context) {
        let is_project = state.workspace.is_project();
        if is_project != self.cache.is_project {
            self.close_project.set_enabled(is_project);
            self.cache.is_project = is_project;
        }

        let has_entry = state.has_active_entry();
        if has_entry != self.cache.has_entry {
            self.save.set_enabled(has_entry);
            self.save_as.set_enabled(has_entry);
            self.edit_structure.set_enabled(has_entry);
            self.cache.has_entry = has_entry;
        }

        // Disable Undo/Redo while a text field is focused so their Cmd+Z /
        // Cmd+Shift+Z key equivalents fall through to egui's own text editing
        // (a disabled NSMenuItem does not consume its key equivalent). Mirrors
        // the `egui_wants_keyboard_input` guard in `dispatcher::handle_history_shortcuts`.
        let typing = ctx.egui_wants_keyboard_input();
        let can_undo = state.can_undo() && !typing;
        if can_undo != self.cache.can_undo {
            self.undo.set_enabled(can_undo);
            self.cache.can_undo = can_undo;
        }
        let can_redo = state.can_redo() && !typing;
        if can_redo != self.cache.can_redo {
            self.redo.set_enabled(can_redo);
            self.cache.can_redo = can_redo;
        }

        let layout = &state.ui.layout;
        if layout.show_primary_sidebar != self.cache.primary_sidebar {
            self.primary_sidebar
                .set_checked(layout.show_primary_sidebar);
            self.cache.primary_sidebar = layout.show_primary_sidebar;
        }
        // The dock areas' visibility is derived (non-empty and not collapsed), so
        // the checkmark tracks whether the area is shown. It can flip without a
        // menu click — e.g. the last tab is dragged out — and the value-gated
        // cache reconciles it next sync.
        let secondary_visible = layout
            .dock
            .is_visible(crate::frontend::state::DockArea::Right);
        if secondary_visible != self.cache.secondary_sidebar {
            self.secondary_sidebar.set_checked(secondary_visible);
            self.cache.secondary_sidebar = secondary_visible;
        }
        let panel_visible = layout
            .dock
            .is_visible(crate::frontend::state::DockArea::Bottom);
        if panel_visible != self.cache.panel {
            self.panel.set_checked(panel_visible);
            self.cache.panel = panel_visible;
        }
        let show_labels = state.ui.viewport.show_atom_labels;
        if show_labels != self.cache.atom_labels {
            self.atom_labels.set_checked(show_labels);
            self.cache.atom_labels = show_labels;
        }

        let theme = state.config.theme;
        if theme != self.cache.theme {
            for (item, mode) in self.theme_items.iter().zip(ThemeMode::all()) {
                item.set_checked(mode == theme);
            }
            self.cache.theme = theme;
        }
        let scheme = state.config.color_scheme;
        if scheme != self.cache.scheme {
            for (item, option) in self.scheme_items.iter().zip(ColorScheme::all()) {
                item.set_checked(option == scheme);
            }
            self.cache.scheme = scheme;
        }

        self.sync_recent(state);
    }

    fn sync_recent(&mut self, state: &AppState) {
        let fingerprint: Vec<(PathBuf, String)> = state
            .recent_projects
            .iter()
            .map(|project| (project.path.clone(), project.name.clone()))
            .collect();
        if fingerprint == self.cache.recent {
            return;
        }

        while !self.recent_submenu.items().is_empty() {
            self.recent_submenu.remove_at(0);
        }
        self.recent_paths.clear();
        for (index, (path, name)) in fingerprint.iter().enumerate() {
            self.recent_submenu
                .append(&MenuItem::with_id(
                    format!("file.recent.{index}"),
                    format!("{name} — {}", path.display()),
                    true,
                    None,
                ))
                .expect("append recent project");
            self.recent_paths.push(path.clone());
        }
        self.recent_submenu.set_enabled(!fingerprint.is_empty());
        self.cache.recent = fingerprint;
    }
}

fn set_process_name() {
    let name = NSString::from_str("SilicoLab");
    NSProcessInfo::processInfo().setProcessName(&name);
}
