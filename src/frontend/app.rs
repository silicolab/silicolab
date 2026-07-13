use std::path::PathBuf;

use anyhow::Result;
use eframe::{egui, egui_wgpu, wgpu};

mod fonts;

#[cfg(test)]
use fonts::UI_NAMED_FONT_FAMILIES;
use fonts::install_system_fonts;
pub(crate) use fonts::{ASSISTANT_CJK_FONT, CONSOLE_CJK_MONO_FONT};

use crate::{
    backend::{
        config::{load_config, load_recent_projects},
        housekeeping,
        project::{WorkspaceSession, open_project, remember_opened_project},
    },
    domain::Structure,
    frontend::{
        actions::{AppAction, LeaveIntent},
        dispatcher, shortcuts,
        state::AppState,
        ui,
    },
};

pub fn run(structure: Structure, source_path: Option<PathBuf>) -> Result<()> {
    let options = eframe::NativeOptions {
        // Keep the GUI paced for tooling workloads instead of chasing high-refresh displays.
        vsync: true,
        multisampling: 0,
        renderer: eframe::Renderer::Wgpu,
        // A depth buffer for egui's render pass, so the GPU molecule renderer can
        // depth-test impostors against it. 32 bits → `Depth32Float`, matched by
        // `viewport::gpu::DEPTH_FORMAT`.
        depth_buffer: 32,
        wgpu_options: low_power_wgpu_options(),
        viewport: window_viewport(),
        ..Default::default()
    };

    eframe::run_native(
        "SilicoLab",
        options,
        Box::new(|cc| {
            let mut fonts = egui::FontDefinitions::default();
            egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
            install_system_fonts(&mut fonts);
            cc.egui_ctx.set_fonts(fonts);
            crate::frontend::theme::apply(&cc.egui_ctx);
            let mut app = SilicoLabApp::new(structure, source_path);
            // Kick off the once-per-launch release check (a single background
            // HTTP request); `poll_jobs` drains the result. Honors the
            // "Check for updates" setting, on by default.
            #[cfg(not(feature = "dev-worker"))]
            if app.state.config.check_updates {
                app.state.jobs.update_check = Some(crate::frontend::jobs::spawn_update_check());
            }
            // Restart the utilization sampler when the setting was on at last exit,
            // so the gauges animate from the first frame, seeded with the saved
            // refresh rate (the per-frame poll then drives it live).
            if app.state.config.show_utilization_bars && app.state.jobs.metrics.is_none() {
                app.state.jobs.metrics = Some(crate::frontend::jobs::spawn_metrics_sampler(
                    crate::frontend::jobs::refresh_interval(app.state.config.monitor_refresh),
                ));
            }
            // Debug aid: SILICOLAB_FAKE_UPDATE=<version> (or =1 for a default)
            // injects a fake "update available" so the badge, status-bar link,
            // and message can be previewed without publishing a release.
            if let Ok(fake) = std::env::var("SILICOLAB_FAKE_UPDATE") {
                let version = if fake == "1" {
                    "9.9.9".to_string()
                } else {
                    fake
                };
                app.state.status_neutral(format!(
                    "SilicoLab {version} is available (you have {})",
                    env!("CARGO_PKG_VERSION")
                ));
                app.state.ui.available_update = Some(crate::io::update_check::AvailableUpdate {
                    version,
                    url: crate::io::update_check::RELEASES_URL.to_string(),
                });
            }
            let render_state = cc
                .wgpu_render_state
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("SilicoLab requires the wgpu renderer"))?;
            crate::frontend::viewport::init_gpu_renderer(render_state);
            app.gpu_exporter = Some(crate::frontend::viewport::GpuExporter::new(render_state));
            app.state.ui.gpu_name = Some(render_state.adapter.get_info().name);
            // Enumerate every GPU adapter (not just the LowPower render adapter,
            // which is usually the iGPU on a dual-GPU host) so the hardware
            // monitor can list them all. One-shot; the inventory is cached in the
            // backend and read by the status bar and Compute Hardware panel.
            crate::backend::hardware::set_gpu_inventory(crate::frontend::gpu_inventory::enumerate());
            crate::frontend::theme::set_preference(&cc.egui_ctx, app.state.config.theme);
            // Apply the persisted color scheme (rebuilds visuals); the default
            // (Warm) is already in place from `apply`, so this is a no-op for
            // setups that never changed it.
            crate::frontend::theme::set_scheme(&cc.egui_ctx, app.state.config.color_scheme);
            // Install the OS backdrop effect behind the content (macOS
            // vibrancy, Windows Acrylic) when the platform supports it;
            // `install` is a no-op elsewhere. Runs on the main thread here, as
            // the underlying AppKit/DWM calls require.
            if crate::frontend::glass::supported() {
                crate::frontend::glass::install(cc);
            }
            // Build the native macOS menu bar. Runs on the main thread after the
            // NSApplication exists (eframe created the event loop and window
            // first), which `init_for_nsapp` requires.
            #[cfg(target_os = "macos")]
            {
                app.macos_menu = Some(crate::frontend::menu_macos::MacMenu::install(&cc.egui_ctx));
            }
            Ok(Box::new(app))
        }),
    )
    .map_err(|error| anyhow::anyhow!(error.to_string()))
}

/// Build the main window's viewport.
///
/// On macOS we use the *native* window frame — standard traffic-light buttons,
/// continuous-curvature (squircle) corners, and the system drop shadow — via a
/// transparent titlebar plus a full-size content view, so our custom title bar
/// draws behind the native buttons.
/// Windows/Linux keep a borderless, transparent window with app-drawn chrome
/// (custom controls, rounded corners, resize handles).
fn window_viewport() -> egui::ViewportBuilder {
    let viewport = egui::ViewportBuilder::default()
        .with_inner_size([1180.0, 760.0])
        .with_min_inner_size([860.0, 560.0])
        .with_icon(app_icon());

    #[cfg(target_os = "macos")]
    {
        let viewport = viewport
            .with_fullsize_content_view(true)
            .with_titlebar_shown(false)
            .with_title_shown(false)
            .with_titlebar_buttons_shown(true)
            .with_has_shadow(true);
        // Only make the NSWindow non-opaque when the vibrancy path is enabled
        // (see `glass::supported`). A transparent surface without a correctly
        // layered effect view behind it renders blank, so the default stays
        // opaque.
        if crate::frontend::glass::supported() {
            viewport.with_transparent(true)
        } else {
            viewport
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        viewport.with_decorations(false).with_transparent(true)
    }
}

/// Decode the embedded 256² window icon into egui's `IconData` (straight RGBA).
/// Used by `window_viewport` for the title-bar/taskbar/Dock icon. Panics only if
/// the committed asset is corrupt, which a unit test guards against.
///
/// macOS shows this runtime icon in the Dock — eframe's `setApplicationIconImage`
/// overrides even a bundled `.icns` — so use the padded squircle there to sit
/// correctly in the native Dock grid. Windows/Linux taskbars have no such grid and
/// want the full-bleed icon. Both assets are 256² (see `scripts/gen-icons.py`).
fn app_icon() -> egui::IconData {
    #[cfg(target_os = "macos")]
    let bytes: &[u8] = include_bytes!("../../assets/icon/window-256-mac.png");
    #[cfg(not(target_os = "macos"))]
    let bytes: &[u8] = include_bytes!("../../assets/icon/window-256.png");
    let image = image::load_from_memory(bytes)
        .expect("decode embedded window icon")
        .to_rgba8();
    let (width, height) = image.dimensions();
    egui::IconData {
        rgba: image.into_raw(),
        width,
        height,
    }
}

fn low_power_wgpu_options() -> egui_wgpu::WgpuConfiguration {
    let mut options = egui_wgpu::WgpuConfiguration::default();
    if let egui_wgpu::WgpuSetup::CreateNew(create_new) = &mut options.wgpu_setup {
        create_new.power_preference =
            wgpu::PowerPreference::from_env().unwrap_or(wgpu::PowerPreference::LowPower);
    }
    options
}

/// Join startup notices into the one-message banner so an earlier warning (e.g.
/// "settings were reset") is not clobbered by a later one (crash recovery).
fn append_message(existing: Option<String>, next: &str) -> String {
    match existing {
        Some(prev) if !prev.is_empty() => format!("{prev} — {next}"),
        _ => next.to_string(),
    }
}

pub struct SilicoLabApp {
    state: AppState,
    last_viewport_title: String,
    gpu_exporter: Option<crate::frontend::viewport::GpuExporter>,
    /// Native macOS menu bar; `None` until installed in the app-creator closure
    /// (it needs `cc.egui_ctx` and a live NSApplication). Other platforms keep
    /// the in-window egui menus instead.
    #[cfg(target_os = "macos")]
    macos_menu: Option<crate::frontend::menu_macos::MacMenu>,
}

impl SilicoLabApp {
    fn new(structure: Structure, source_path: Option<PathBuf>) -> Self {
        // `load_config` may return a warning (corrupt settings backed up) to show.
        let (mut config, mut startup_message) = load_config();
        let mut recent_projects = load_recent_projects();
        let mut state = if !config.closed_to_scratch {
            if let Some(last_project_path) = config.last_project_path.clone() {
                match open_project(&last_project_path) {
                    Ok((project, snapshot)) => {
                        let _ =
                            remember_opened_project(&mut config, &mut recent_projects, &project);
                        let recovered_from_crash = housekeeping::acquire_lock(&project);
                        if !snapshot.warnings.is_empty() {
                            startup_message = Some(append_message(
                                startup_message.take(),
                                &snapshot.warnings.join(" — "),
                            ));
                        }
                        let mut state = AppState::new(
                            structure,
                            source_path,
                            WorkspaceSession::Project(project),
                            config,
                            recent_projects,
                            Some(snapshot),
                        );
                        if recovered_from_crash {
                            startup_message = Some(append_message(
                                startup_message.take(),
                                "Recovered project: previous session did not close cleanly",
                            ));
                        }
                        if let Some(message) = startup_message.take() {
                            state.status_neutral(message);
                        }
                        return Self {
                            state,
                            last_viewport_title: String::new(),
                            gpu_exporter: None,
                            #[cfg(target_os = "macos")]
                            macos_menu: None,
                        };
                    }
                    Err(error) => {
                        startup_message = Some(append_message(
                            startup_message.take(),
                            &format!("Last project unavailable; opened Scratch: {error}"),
                        ));
                        config.last_project_path = None;
                        AppState::scratch(config, recent_projects)
                    }
                }
            } else {
                AppState::scratch(config, recent_projects)
            }
        } else {
            AppState::scratch(config, recent_projects)
        };
        if let Some(message) = startup_message {
            state.status_neutral(message);
        }
        Self {
            state,
            last_viewport_title: String::new(),
            gpu_exporter: None,
            #[cfg(target_os = "macos")]
            macos_menu: None,
        }
    }

    fn open_dropped_files(&mut self, ctx: &egui::Context) {
        let dropped_paths = ctx.input(|input| {
            input
                .raw
                .dropped_files
                .iter()
                .filter_map(|file| file.path.clone())
                .collect::<Vec<_>>()
        });
        if dropped_paths.is_empty() {
            return;
        }

        dispatcher::open_paths(&mut self.state, dropped_paths);
    }

    fn show_file_drop_overlay(&self, ctx: &egui::Context) {
        let hovered_count = ctx.input(|input| {
            input
                .raw
                .hovered_files
                .iter()
                .filter(|file| file.path.is_some())
                .count()
        });
        if hovered_count == 0 {
            return;
        }

        egui::Area::new(egui::Id::new("file_drop_overlay"))
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .interactable(false)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.set_max_width(260.0);
                    ui.vertical_centered(|ui| {
                        ui.heading("Drop to open");
                        if hovered_count == 1 {
                            ui.label("Release to open the structure file");
                        } else {
                            ui.label(format!("Release to open {hovered_count} structure files"));
                        }
                    });
                });
            });
    }
}

impl eframe::App for SilicoLabApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let viewport_title = format!(
            "SilicoLab - {} - {}",
            self.state.workspace_label(),
            self.state.current_entry_label()
        );
        if viewport_title != self.last_viewport_title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(viewport_title.clone()));
            self.last_viewport_title = viewport_title;
        }
        if self.state.ui.request_window_close {
            self.state.ui.request_window_close = false;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        if ctx.input(|input| input.viewport().close_requested())
            && !self.state.ui.allow_window_close
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            dispatcher::dispatch(
                &mut self.state,
                AppAction::RequestLeave(LeaveIntent::Quit),
                &ctx,
            );
        }
        self.open_dropped_files(&ctx);
        dispatcher::poll_jobs(&mut self.state, &ctx);
        shortcuts::handle_frame(&mut self.state, &ctx);

        // Resolve once per frame whether the frosted glass is revealed; read by
        // the chrome fills below and by `clear_color`. Re-evaluated every frame
        // so toggling the preference or the OS "Reduce Transparency" setting
        // takes effect live.
        self.state.ui.glass_active = crate::frontend::glass::glass_active(self.state.config.glass);
        self.state.ui.glass_alpha = self
            .state
            .ui
            .glass_active
            .then(|| crate::frontend::theme::glass_alpha(self.state.config.glass_intensity));

        let mut actions = Vec::<AppAction>::new();
        // Fold native macOS menu clicks into this frame's actions before the UI
        // runs, so a click renders the same frame the repaint wake delivers it.
        #[cfg(target_os = "macos")]
        if let Some(menu) = self.macos_menu.as_mut() {
            use crate::frontend::menu_macos::MenuCommand;
            for command in menu.drain() {
                match command {
                    MenuCommand::Action(action) => actions.push(action),
                    MenuCommand::ShowAbout => {
                        self.state.ui.layout.about_open = true;
                    }
                    MenuCommand::ToggleSettings => {
                        let open = &mut self.state.ui.layout.settings_open;
                        *open = !*open;
                    }
                    MenuCommand::TogglePrimarySidebar => {
                        actions.push(AppAction::TogglePrimarySidebar)
                    }
                    MenuCommand::ToggleSecondarySidebar => actions.push(AppAction::ToggleDockArea(
                        crate::frontend::state::DockArea::Right,
                    )),
                    MenuCommand::TogglePanel => actions.push(AppAction::ToggleDockArea(
                        crate::frontend::state::DockArea::Bottom,
                    )),
                    MenuCommand::ToggleAtomLabels => actions.push(AppAction::ToggleAtomLabels),
                    MenuCommand::ResetWorkbenchLayout => {
                        actions.push(AppAction::ResetWorkbenchLayout)
                    }
                    MenuCommand::Quit => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                }
            }
        }
        ui::show_workbench(&mut self.state, ui, &mut actions);
        self.show_file_drop_overlay(&ctx);
        for action in actions {
            dispatcher::dispatch(&mut self.state, action, &ctx);
        }
        while let Some(request) = self.state.ui.pending_viewport_exports.pop_front() {
            let output_path = request.output_path.clone();
            let result = self
                .gpu_exporter
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("GPU exporter is not initialized"))
                .and_then(|exporter| request.execute(exporter));
            match result {
                Ok(()) => self
                    .state
                    .status_success(format!("Saved image to {}", output_path.display())),
                Err(error) => self.state.status_error(format!(
                    "Failed to save image to {}: {error}",
                    output_path.display()
                )),
            }
        }
        // Reconcile the native menu (enabled/checked state, Recent Projects)
        // with the post-dispatch state.
        #[cfg(target_os = "macos")]
        if let Some(menu) = self.macos_menu.as_mut() {
            menu.sync(&self.state, &ctx);
        }
        dispatcher::flush_dirty_task_runs(&mut self.state);
        dispatcher::flush_dirty_run_graph(&mut self.state);
        dispatcher::flush_pending_autosave(&mut self.state, &ctx);
        dispatcher::flush_pending_layout_save(&mut self.state, &ctx);
        self.state.tick_status(std::time::Instant::now());
    }

    /// Backing color behind the UI.
    ///
    /// macOS backing is opaque and matched to the active theme's window backing
    /// (which equals the central panel fill), so the native title bar shows no
    /// seam and the native shadow stays intact, in light or dark. Other
    /// platforms use a transparent backing so the app-drawn rounded corners read
    /// as empty (revealing the desktop behind them).
    fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
        #[cfg(target_os = "macos")]
        {
            // With glass revealed, clear fully transparent so the vibrancy
            // material behind the window shows through the semi-transparent
            // chrome. Otherwise keep the opaque backing matched to the central
            // panel fill (seamless native title bar, intact shadow).
            if self.state.ui.glass_active {
                return [0.0, 0.0, 0.0, 0.0];
            }
            crate::frontend::theme::Palette::for_scheme(
                self.state.config.color_scheme,
                visuals.dark_mode,
            )
            .window_backing
            .to_normalized_gamma_f32()
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = visuals;
            [0.0, 0.0, 0.0, 0.0]
        }
    }

    /// Persist only the window geometry, not egui's transient widget memory.
    ///
    /// The `eframe` "persistence" feature (enabled for window size/position recall)
    /// otherwise also serializes the entire egui `Memory` typemap — collapsing-header
    /// open/closed state, scroll offsets, text-edit undo buffers, focus, etc. — which
    /// we don't want surviving restarts. Window geometry is saved separately (gated on
    /// `persist_window`, default true), so it is unaffected by returning false here.
    fn persist_egui_memory(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use eframe::egui;

    use super::{UI_NAMED_FONT_FAMILIES, app_icon, install_system_fonts};

    #[test]
    fn embedded_window_icon_is_256_rgba() {
        let icon = app_icon();
        assert_eq!(icon.width, 256);
        assert_eq!(icon.height, 256);
        assert_eq!(icon.rgba.len(), 256 * 256 * 4);
    }

    /// Regression for #35: the Assistant panel renders text with
    /// `FontFamily::Name("assistant-cjk")`, but the family was bound only on
    /// macOS — so Windows and Linux passed every logic-only test yet panicked at
    /// the first rendered frame. Build the font set exactly as `run()` does and
    /// assert every UI-referenced named family resolves to real, loaded fonts on
    /// this host. Guards the whole "unbound named family" class, not just the two
    /// families that exist today.
    #[test]
    fn ui_named_font_families_are_bound() {
        let mut fonts = egui::FontDefinitions::default();
        egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
        install_system_fonts(&mut fonts);

        for (name, _base) in UI_NAMED_FONT_FAMILIES {
            let family = egui::FontFamily::Name((*name).into());
            let list = fonts
                .families
                .get(&family)
                .unwrap_or_else(|| panic!("named font family {name:?} is not registered"));
            assert!(!list.is_empty(), "named font family {name:?} has no fonts");
            for font in list {
                assert!(
                    fonts.font_data.contains_key(font),
                    "named family {name:?} references {font:?} missing from font_data",
                );
            }
        }
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_assistant_fonts_include_system_cjk_fallbacks() {
        use super::{ASSISTANT_CJK_FONT, CONSOLE_CJK_MONO_FONT};

        let windir = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".to_string());
        if !std::path::Path::new(&windir)
            .join("Fonts")
            .join("msyh.ttc")
            .exists()
        {
            return;
        }

        let mut fonts = egui::FontDefinitions::default();
        egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
        install_system_fonts(&mut fonts);

        let assistant = egui::FontFamily::Name(ASSISTANT_CJK_FONT.into());
        let console = egui::FontFamily::Name(CONSOLE_CJK_MONO_FONT.into());
        let assistant_fonts = fonts.families.get(&assistant).unwrap();
        let console_fonts = fonts.families.get(&console).unwrap();

        assert!(
            assistant_fonts
                .iter()
                .any(|font| font == "Microsoft YaHei Assistant"),
            "assistant-cjk should use Microsoft YaHei as a Windows CJK fallback",
        );
        assert!(
            console_fonts
                .iter()
                .any(|font| font == "Microsoft YaHei Console"),
            "console-cjk-mono should use Microsoft YaHei as a Windows CJK fallback",
        );
    }
}
