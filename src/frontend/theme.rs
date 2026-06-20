//! Centralized egui theme, nudged toward a native-leaning desktop appearance.
//!
//! The app previously shipped no global theme (it inherited egui's defaults and
//! patched a few widget colors per panel), which left selection highlights as a
//! hard 1px outline and controls nearly square. The native look instead calls
//! for a coherent appearance, *filled* and rounded selection in the accent
//! color (never a hard border), and softly rounded controls. This module sets
//! that baseline once at startup; per-panel tweaks still layer on top.
//!
//! Light and dark are both registered so egui can follow the system appearance
//! (or an explicit user preference) and switch live. Panels draw their chrome
//! from [`Palette`] — a small set of semantic color roles — so a single
//! `palette(ui)` lookup per frame flips every surface with the theme.

use eframe::egui::{self, Color32, CornerRadius, Shadow, Stroke, Visuals};

use crate::backend::config::{ColorScheme, ThemeMode};

/// Semantic color roles for app-drawn chrome (panels, text, hairlines, hovers).
///
/// Standard egui widgets read their colors from [`Visuals`] and flip for free;
/// this palette covers the surfaces we paint ourselves. Look it up once per
/// draw function with [`palette`] (reads the live, resolved theme), never cache
/// it on app state.
#[derive(Clone, Copy)]
pub struct Palette {
    // Surfaces.
    /// Opaque window backing (macOS `clear_color`) and central panel base.
    pub window_backing: Color32,
    pub title_bar: Color32,
    pub status_bar: Color32,
    pub sidebar: Color32,
    pub central: Color32,
    pub bottom_panel: Color32,
    /// Default background behind the 3D viewport scene.
    pub viewport_bg: Color32,
    // Text.
    pub text_primary: Color32,
    pub text_strong: Color32,
    pub text_muted: Color32,
    pub text_tertiary: Color32,
    // Items / lines.
    pub hairline: Color32,
    pub item_fill: Color32,
    pub item_fill_hover: Color32,
    pub item_fill_active: Color32,
    /// Backing for text inputs (TextEdit, search fields). Distinct from
    /// `item_fill`: in dark mode inputs must sit clearly *lighter* than the
    /// surrounding panel (a card-dark input reads as a black hole), while in
    /// light mode both are white.
    pub input_fill: Color32,
    pub selection_fill: Color32,
    /// Ink used to build low-alpha neutral overlays; dark in light mode and
    /// light in dark mode so hovers read as a highlight either way.
    pub neutral_tint: Color32,
    // Accent / status (saturated; read on either background).
    pub accent: Color32,
    pub selection_blue_tint: Color32,
    pub status_blue: Color32,
    pub status_amber: Color32,
    pub status_green: Color32,
    pub status_red: Color32,
}

impl Palette {
    /// Warm ivory light theme (Claude Desktop family): neutral surfaces sit on
    /// a `#faf9f5` base with warm grays for text and hairlines, while accents
    /// and selection stay macOS system blue.
    pub const fn warm_light() -> Self {
        Self {
            window_backing: Color32::from_rgb(250, 249, 245),
            title_bar: Color32::from_rgb(250, 249, 245),
            status_bar: Color32::from_rgb(240, 239, 235),
            sidebar: Color32::from_rgb(245, 244, 240),
            central: Color32::from_rgb(250, 249, 245),
            bottom_panel: Color32::from_rgb(246, 245, 242),
            viewport_bg: Color32::from_rgb(250, 249, 245),
            text_primary: Color32::from_rgb(54, 51, 44),
            text_strong: Color32::from_rgb(31, 30, 27),
            text_muted: Color32::from_rgb(115, 112, 103),
            text_tertiary: Color32::from_rgb(143, 140, 130),
            hairline: Color32::from_rgb(229, 227, 222),
            item_fill: Color32::WHITE,
            item_fill_hover: Color32::from_rgb(246, 245, 241),
            item_fill_active: Color32::from_rgb(231, 229, 224),
            input_fill: Color32::WHITE,
            selection_fill: Color32::from_rgb(213, 222, 235),
            neutral_tint: Color32::from_rgb(70, 66, 56),
            accent: Color32::from_rgb(0, 122, 255),
            selection_blue_tint: Color32::from_rgb(54, 97, 164),
            status_blue: Color32::from_rgb(120, 146, 184),
            status_amber: Color32::from_rgb(201, 145, 62),
            status_green: Color32::from_rgb(64, 160, 108),
            status_red: Color32::from_rgb(232, 84, 82),
        }
    }

    /// Warm charcoal dark theme (Claude Desktop's `#262624` family). Panels
    /// step up in lightness from the window backing; blue accents unchanged.
    pub const fn warm_dark() -> Self {
        Self {
            window_backing: Color32::from_rgb(33, 33, 31),
            title_bar: Color32::from_rgb(42, 41, 38),
            status_bar: Color32::from_rgb(37, 36, 34),
            sidebar: Color32::from_rgb(44, 43, 40),
            central: Color32::from_rgb(33, 33, 31),
            bottom_panel: Color32::from_rgb(40, 39, 36),
            viewport_bg: Color32::from_rgb(28, 28, 26),
            text_primary: Color32::from_rgb(232, 230, 224),
            text_strong: Color32::from_rgb(247, 245, 240),
            text_muted: Color32::from_rgb(160, 156, 147),
            text_tertiary: Color32::from_rgb(127, 124, 116),
            hairline: Color32::from_rgb(58, 57, 52),
            item_fill: Color32::from_rgb(48, 47, 44),
            item_fill_hover: Color32::from_rgb(66, 65, 60),
            item_fill_active: Color32::from_rgb(80, 78, 72),
            input_fill: Color32::from_rgb(58, 57, 53),
            selection_fill: Color32::from_rgb(44, 58, 82),
            neutral_tint: Color32::from_rgb(222, 219, 212),
            accent: Color32::from_rgb(10, 132, 255),
            selection_blue_tint: Color32::from_rgb(94, 145, 220),
            status_blue: Color32::from_rgb(126, 158, 200),
            status_amber: Color32::from_rgb(216, 162, 84),
            status_green: Color32::from_rgb(98, 184, 122),
            status_red: Color32::from_rgb(238, 104, 102),
        }
    }

    /// The palette for a given scheme and light/dark mode. Warm and Cool are
    /// each hand-tuned; Graphite/Green/Violet share a neutral graphite base and
    /// differ only in accent (see [`Palette::with_accent`]).
    pub fn for_scheme(scheme: ColorScheme, dark: bool) -> Self {
        match scheme {
            ColorScheme::Warm => {
                if dark {
                    Self::warm_dark()
                } else {
                    Self::warm_light()
                }
            }
            ColorScheme::Cool => {
                if dark {
                    Self::cool_dark()
                } else {
                    Self::cool_light()
                }
            }
            ColorScheme::Graphite => Self::graphite(dark),
            ColorScheme::Green => Self::graphite(dark).with_accent(
                if dark {
                    Color32::from_rgb(80, 200, 120)
                } else {
                    Color32::from_rgb(46, 160, 67)
                },
                if dark {
                    Color32::from_rgb(110, 180, 130)
                } else {
                    Color32::from_rgb(60, 130, 80)
                },
                if dark {
                    Color32::from_rgb(40, 70, 50)
                } else {
                    Color32::from_rgb(214, 232, 218)
                },
            ),
            ColorScheme::Violet => Self::graphite(dark).with_accent(
                if dark {
                    Color32::from_rgb(168, 138, 250)
                } else {
                    Color32::from_rgb(116, 92, 232)
                },
                if dark {
                    Color32::from_rgb(150, 130, 210)
                } else {
                    Color32::from_rgb(110, 90, 170)
                },
                if dark {
                    Color32::from_rgb(56, 48, 84)
                } else {
                    Color32::from_rgb(226, 220, 244)
                },
            ),
        }
    }

    /// Re-accent a neutral base: override the accent plus the blue-ish selection
    /// tints so an accent-variant scheme (Green/Violet) recolors selection and
    /// active highlights to match, while keeping the neutral surfaces intact.
    fn with_accent(
        mut self,
        accent: Color32,
        selection_tint: Color32,
        selection_fill: Color32,
    ) -> Self {
        self.accent = accent;
        self.selection_blue_tint = selection_tint;
        self.selection_fill = selection_fill;
        self
    }

    fn graphite(dark: bool) -> Self {
        if dark {
            Self::graphite_dark()
        } else {
            Self::graphite_light()
        }
    }

    /// Cool blue-gray light theme — SilicoLab's pre-overhaul light palette.
    pub const fn cool_light() -> Self {
        Self {
            window_backing: Color32::from_rgb(245, 247, 249),
            title_bar: Color32::from_rgb(246, 248, 251),
            status_bar: Color32::from_rgb(229, 236, 244),
            sidebar: Color32::from_rgb(252, 252, 253),
            central: Color32::from_rgb(245, 247, 249),
            bottom_panel: Color32::from_rgb(248, 249, 251),
            viewport_bg: Color32::from_rgb(245, 247, 249),
            text_primary: Color32::from_rgb(32, 37, 43),
            text_strong: Color32::from_rgb(18, 22, 30),
            text_muted: Color32::from_rgb(92, 100, 112),
            text_tertiary: Color32::from_rgb(120, 128, 138),
            hairline: Color32::from_rgb(226, 232, 240),
            item_fill: Color32::from_rgb(249, 251, 253),
            item_fill_hover: Color32::from_rgb(242, 247, 252),
            item_fill_active: Color32::from_rgb(221, 226, 233),
            input_fill: Color32::WHITE,
            selection_fill: Color32::from_rgb(216, 223, 233),
            neutral_tint: Color32::from_rgb(64, 70, 82),
            accent: Color32::from_rgb(0, 122, 255),
            selection_blue_tint: Color32::from_rgb(54, 97, 164),
            status_blue: Color32::from_rgb(120, 146, 184),
            status_amber: Color32::from_rgb(201, 145, 62),
            status_green: Color32::from_rgb(64, 160, 108),
            status_red: Color32::from_rgb(232, 84, 82),
        }
    }

    /// Cool near-black dark theme — SilicoLab's pre-overhaul dark palette.
    pub const fn cool_dark() -> Self {
        Self {
            window_backing: Color32::from_rgb(22, 22, 24),
            title_bar: Color32::from_rgb(30, 30, 33),
            status_bar: Color32::from_rgb(26, 26, 29),
            sidebar: Color32::from_rgb(32, 32, 36),
            central: Color32::from_rgb(22, 22, 24),
            bottom_panel: Color32::from_rgb(28, 28, 31),
            viewport_bg: Color32::from_rgb(18, 18, 20),
            text_primary: Color32::from_rgb(228, 231, 236),
            text_strong: Color32::from_rgb(244, 246, 249),
            text_muted: Color32::from_rgb(150, 157, 167),
            text_tertiary: Color32::from_rgb(120, 127, 137),
            hairline: Color32::from_rgb(52, 54, 60),
            item_fill: Color32::from_rgb(40, 40, 45),
            item_fill_hover: Color32::from_rgb(50, 50, 56),
            item_fill_active: Color32::from_rgb(60, 60, 67),
            input_fill: Color32::from_rgb(52, 54, 62),
            selection_fill: Color32::from_rgb(44, 58, 82),
            neutral_tint: Color32::from_rgb(210, 215, 222),
            accent: Color32::from_rgb(10, 132, 255),
            selection_blue_tint: Color32::from_rgb(94, 145, 220),
            status_blue: Color32::from_rgb(126, 158, 200),
            status_amber: Color32::from_rgb(216, 162, 84),
            status_green: Color32::from_rgb(98, 184, 122),
            status_red: Color32::from_rgb(238, 104, 102),
        }
    }

    /// Neutral graphite light theme — pure grays with a blue accent. Shared base
    /// for the Graphite/Green/Violet schemes.
    pub const fn graphite_light() -> Self {
        Self {
            window_backing: Color32::from_rgb(247, 247, 247),
            title_bar: Color32::from_rgb(247, 247, 247),
            status_bar: Color32::from_rgb(238, 238, 238),
            sidebar: Color32::from_rgb(242, 242, 242),
            central: Color32::from_rgb(247, 247, 247),
            bottom_panel: Color32::from_rgb(244, 244, 244),
            viewport_bg: Color32::from_rgb(247, 247, 247),
            text_primary: Color32::from_rgb(45, 45, 47),
            text_strong: Color32::from_rgb(24, 24, 26),
            text_muted: Color32::from_rgb(110, 110, 112),
            text_tertiary: Color32::from_rgb(140, 140, 142),
            hairline: Color32::from_rgb(225, 225, 227),
            item_fill: Color32::WHITE,
            item_fill_hover: Color32::from_rgb(243, 243, 244),
            item_fill_active: Color32::from_rgb(228, 228, 230),
            input_fill: Color32::WHITE,
            selection_fill: Color32::from_rgb(214, 222, 234),
            neutral_tint: Color32::from_rgb(60, 60, 64),
            accent: Color32::from_rgb(0, 122, 255),
            selection_blue_tint: Color32::from_rgb(54, 97, 164),
            status_blue: Color32::from_rgb(120, 146, 184),
            status_amber: Color32::from_rgb(201, 145, 62),
            status_green: Color32::from_rgb(64, 160, 108),
            status_red: Color32::from_rgb(232, 84, 82),
        }
    }

    /// Neutral graphite dark theme — pure grays with a blue accent.
    pub const fn graphite_dark() -> Self {
        Self {
            window_backing: Color32::from_rgb(28, 28, 30),
            title_bar: Color32::from_rgb(38, 38, 40),
            status_bar: Color32::from_rgb(33, 33, 35),
            sidebar: Color32::from_rgb(40, 40, 42),
            central: Color32::from_rgb(28, 28, 30),
            bottom_panel: Color32::from_rgb(36, 36, 38),
            viewport_bg: Color32::from_rgb(24, 24, 26),
            text_primary: Color32::from_rgb(230, 230, 232),
            text_strong: Color32::from_rgb(246, 246, 248),
            text_muted: Color32::from_rgb(156, 156, 160),
            text_tertiary: Color32::from_rgb(124, 124, 128),
            hairline: Color32::from_rgb(56, 56, 60),
            item_fill: Color32::from_rgb(44, 44, 47),
            item_fill_hover: Color32::from_rgb(62, 62, 66),
            item_fill_active: Color32::from_rgb(76, 76, 80),
            input_fill: Color32::from_rgb(54, 54, 58),
            selection_fill: Color32::from_rgb(44, 58, 82),
            neutral_tint: Color32::from_rgb(216, 216, 220),
            accent: Color32::from_rgb(10, 132, 255),
            selection_blue_tint: Color32::from_rgb(94, 145, 220),
            status_blue: Color32::from_rgb(126, 158, 200),
            status_amber: Color32::from_rgb(216, 162, 84),
            status_green: Color32::from_rgb(98, 184, 122),
            status_red: Color32::from_rgb(238, 104, 102),
        }
    }

    /// Low-alpha neutral overlay (hover/press) that inverts with the theme:
    /// dark ink over light surfaces, light ink over dark ones.
    pub fn neutral_overlay(&self, alpha: u8) -> Color32 {
        let [r, g, b, _] = self.neutral_tint.to_array();
        Color32::from_rgba_unmultiplied(r, g, b, alpha)
    }

    /// Low-alpha blue overlay for selection/active tints.
    pub fn blue_overlay(&self, alpha: u8) -> Color32 {
        let [r, g, b, _] = self.selection_blue_tint.to_array();
        Color32::from_rgba_unmultiplied(r, g, b, alpha)
    }

    /// A pastel form of the theme accent — same hue, roughly half the
    /// saturation and a touch brighter. Decorative accent glyphs (e.g. the
    /// assistant's empty-state sparkle) read as harsh at full accent
    /// saturation; this keeps them on-theme across every scheme (blue, violet,
    /// green, …) without a per-scheme color.
    pub fn accent_soft(&self) -> Color32 {
        let hsva = egui::ecolor::Hsva::from(self.accent);
        egui::ecolor::Hsva {
            s: hsva.s * 0.5,
            v: (hsva.v + 0.1).min(1.0),
            ..hsva
        }
        .into()
    }
}

/// The palette for the active color scheme and the theme egui currently
/// resolves for this `Ui`. The scheme is read from the egui context (set by
/// [`set_scheme`]); the light/dark axis from the resolved visuals.
pub fn palette(ui: &egui::Ui) -> Palette {
    Palette::for_scheme(active_scheme(ui.ctx()), ui.visuals().dark_mode)
}

/// Stop "selectable" rows (`selectable_label` / `selectable_value` / ComboBox
/// menu rows, all backed by `egui::Button::selectable`) from jumping 1px sideways
/// on hover. Call it on the `Ui` that *hosts the rows* — the first line inside a
/// ComboBox `show_ui` closure, or inside a dedicated `ui.scope`/`ui.horizontal`
/// that wraps only the selectable rows.
///
/// Why: egui 0.34.3 derives a selectable button's content offset from
/// `Frame::total_margin` (= inner_margin + stroke.width + outer_margin). An
/// unselected row at rest takes the `frame_when_inactive(false)` branch, which
/// wraps the inner_margin — already pre-reduced by `inactive.bg_stroke.width` — in
/// a `Frame::new()` whose stroke width is 0, so it never adds that width back. The
/// hover/selected branch draws the styled frame and does add it back. The delta is
/// exactly `inactive.bg_stroke.width`, so a resting row sits 1px left of its
/// hovered self. We keep the global `inactive.bg_stroke` at 1px because that hairline
/// is the *only* resting outline our inputs / checkboxes / radios / closed combos
/// draw (stock egui leaves it `NONE`); zeroing it here, scoped to a row-only `Ui`,
/// removes the jump without touching those borders. **Do not** call this on a `Ui`
/// that also hosts a `TextEdit`/`DragValue`/`Checkbox`/plain `Button`, or those
/// neighbors lose their resting hairline too.
pub fn stabilize_selectable_rows(ui: &mut egui::Ui) {
    ui.visuals_mut().widgets.inactive.bg_stroke.width = 0.0;
}

/// Context-data key holding the active [`ColorScheme`]. Stored in egui's
/// per-context data so [`palette`] (which only has a `&Ui`) can resolve it
/// without threading the scheme through every draw call.
fn color_scheme_id() -> egui::Id {
    egui::Id::new("silicolab.color_scheme")
}

/// The active color scheme, or the default when none has been set yet.
pub fn active_scheme(ctx: &egui::Context) -> ColorScheme {
    ctx.data(|data| data.get_temp::<ColorScheme>(color_scheme_id()))
        .unwrap_or_default()
}

/// Register Light + Dark `Visuals` built from the active color scheme.
fn register_visuals(ctx: &egui::Context) {
    let scheme = active_scheme(ctx);
    ctx.set_visuals_of(egui::Theme::Light, build_visuals(scheme, false));
    ctx.set_visuals_of(egui::Theme::Dark, build_visuals(scheme, true));
}

/// Switch the active color scheme and rebuild both Light and Dark visuals
/// live. Preserves the light/dark *preference* (unlike [`apply`], it does not
/// touch `theme_preference`), so the scheme can be changed without disturbing
/// the user's Light/Dark/System choice.
pub fn set_scheme(ctx: &egui::Context, scheme: ColorScheme) {
    ctx.data_mut(|data| data.insert_temp(color_scheme_id(), scheme));
    register_visuals(ctx);
}

/// Apple-style corner-radius scale, aligned with the macOS 27 (Golden Gate)
/// corner system. Every rounded rect in the app draws from these steps, and
/// the three radius semantics of AppKit's `NSViewCornerConfiguration` map
/// directly onto this module:
/// - `fixed(r)` — the constants below;
/// - `containerConcentric(minimum:)` — [`concentric`] (inner = outer − inset,
///   floored at [`MIN`]) so stacked corners share a center;
/// - `capsule(maximumRadius:)` — epaint already clamps a radius to half the
///   rect's short side at paint time, so an oversized constant *is* a capsule.
pub mod radius {
    /// Tiny inline chips/badges (e.g. the "MD" origin chip on entry rows).
    pub const CHIP: u8 = 5;
    /// Standard controls: buttons, inputs, list-row highlights, menu items.
    pub const CONTROL: u8 = 8;
    /// Cards, menus, tooltips, popovers: task cards, recent-project rows.
    pub const CARD: u8 = 10;
    /// Large in-window containers and 44px call-to-action buttons.
    pub const LARGE: u8 = 12;
    /// Floating windows and modals (`window_corner_radius`).
    pub const MODAL: u8 = 14;
    /// The OS window corner. Drawn by the app on Windows/Linux; on macOS the
    /// native frame owns the corner, so this is the design constant feeding
    /// concentric math for corner-adjacent widgets. Measured 16.2pt on a
    /// macOS 27 (Golden Gate) native window via alpha-mask circle fit.
    /// On Windows the app-drawn arc must match DWM's 8pt rounding, which clips
    /// the window (see `glass::install_windows`).
    pub const WINDOW: u8 = if cfg!(target_os = "windows") { 8 } else { 16 };
    /// Floor for concentric results so deeply inset widgets never go square.
    pub const MIN: u8 = 2;

    /// Apple's concentric rule: inner radius = outer radius − inset, floored
    /// at [`MIN`]. Mirror of AppKit's `containerConcentric(minimum:)`.
    pub const fn concentric(outer: u8, inset: u8) -> u8 {
        let r = outer.saturating_sub(inset);
        if r < MIN { MIN } else { r }
    }
}

/// Linear blend between two colors (`t` = 0 -> `a`, `t` = 1 -> `b`), done in
/// egui's linear `Rgba` space so the midpoint reads naturally.
pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let a = egui::Rgba::from(a);
    let b = egui::Rgba::from(b);
    let t = t.clamp(0.0, 1.0);
    Color32::from(a * (1.0 - t) + b * t)
}

/// Chrome-fill alpha range for the Liquid Glass tint, mapped from the user's
/// persisted `glass_intensity` (0..=1) by [`glass_alpha`]. The minimum stays
/// clearly translucent ("ultra-clear"); the maximum is heavily tinted but never
/// fully opaque, so the backdrop blur always reads at least faintly.
pub const GLASS_ALPHA_MIN: f32 = 45.0;
pub const GLASS_ALPHA_MAX: f32 = 230.0;

/// Map the persisted 0..=1 Liquid Glass intensity onto the effective chrome
/// alpha (macOS 27-style "Clear ↔ Tinted" slider).
pub fn glass_alpha(intensity: f32) -> u8 {
    egui::lerp(GLASS_ALPHA_MIN..=GLASS_ALPHA_MAX, intensity.clamp(0.0, 1.0)).round() as u8
}

/// Fill for an app-drawn chrome surface (title bar, sidebars, status bar).
/// `glass` is `Some(alpha)` while Liquid Glass is revealed this frame (resolved
/// once per frame into `ui.glass_alpha`): the opaque palette color is made
/// semi-transparent at that alpha so the window's vibrancy material shows
/// through. `None` returns the opaque color unchanged. The central panel and 3D
/// viewport keep their opaque fills, so the glass never sits behind dense
/// content or the GPU scene.
pub fn chrome_fill(base: Color32, glass: Option<u8>) -> Color32 {
    match glass {
        Some(alpha) => {
            let [r, g, b, _] = base.to_array();
            Color32::from_rgba_unmultiplied(r, g, b, alpha)
        }
        None => base,
    }
}

/// Register the light and dark themes and start following the system.
///
/// Both visual sets are installed so egui can switch live when the OS
/// appearance changes (or when an explicit preference is set via
/// [`set_preference`]). The actual preference is applied afterwards once the
/// stored config is available.
pub fn apply(ctx: &egui::Context) {
    register_visuals(ctx);

    // A slim, auto-hiding overlay scroll bar (macOS behaviour): the bar floats
    // above the content without reserving layout space, fades out entirely
    // when idle, and fades a thin foreground-colored handle back in while
    // scrolling — widening slightly under the cursor. A dormant scroll area is
    // indistinguishable from a static one.
    let mut scroll = egui::style::ScrollStyle::floating();
    scroll.floating_width = 4.0; // resting width while scrolling
    scroll.bar_width = 6.0; // expanded width when the cursor is over the bar
    scroll.bar_inner_margin = 4.0;
    // Hug the edge for a more compact read, without sitting flush on it.
    scroll.bar_outer_margin = 1.0;
    scroll.handle_min_length = 24.0;
    // `floating()` already hides the dormant handle/track. While merely
    // scrolling no track is drawn; bringing the cursor near the bar fades a
    // faint track in (and back out when it leaves).
    scroll.active_background_opacity = 0.0;
    scroll.interact_background_opacity = 0.1;
    scroll.active_handle_opacity = 0.22;
    scroll.interact_handle_opacity = 0.35;

    for theme in [egui::Theme::Light, egui::Theme::Dark] {
        ctx.style_mut_of(theme, |style| {
            // Slightly roomier controls, closer to macOS metrics.
            style.spacing.button_padding = egui::vec2(8.0, 4.0);
            style.spacing.scroll = scroll;
            // A wider grab zone for panel resize, so the sidebar divider is easy
            // to catch (and reliably beats the inset scroll bar next to it).
            style.interaction.resize_grab_radius_side = 7.0;
            // Type scale aligned with AppKit: 13pt body/controls, an 11pt floor
            // for secondary text (`.small()`), monospace bumped to match body.
            // egui's defaults (Body/Button 12.5, Small 9, Monospace 12) leave
            // captions and the `.small()` chrome too small to read on macOS; one
            // central override here lifts every named-TextStyle call site at once
            // (visuals — set via `set_visuals_of` on theme/scheme change — never
            // touch `text_styles`, so this survives live theme switches).
            use egui::{FontFamily, FontId, TextStyle};
            style.text_styles = [
                (
                    TextStyle::Small,
                    FontId::new(11.0, FontFamily::Proportional),
                ),
                (TextStyle::Body, FontId::new(13.0, FontFamily::Proportional)),
                (
                    TextStyle::Button,
                    FontId::new(13.0, FontFamily::Proportional),
                ),
                (
                    TextStyle::Heading,
                    FontId::new(18.0, FontFamily::Proportional),
                ),
                (
                    TextStyle::Monospace,
                    FontId::new(13.0, FontFamily::Monospace),
                ),
            ]
            .into();
        });
    }

    ctx.options_mut(|options| {
        options.theme_preference = egui::ThemePreference::System;
        // If the OS appearance can't be detected, fall back to light (the
        // app's historical default) rather than egui's dark default.
        options.fallback_theme = egui::Theme::Light;
    });
}

/// Apply a user theme preference (Light / Dark / follow System).
pub fn set_preference(ctx: &egui::Context, mode: ThemeMode) {
    let preference = match mode {
        ThemeMode::System => egui::ThemePreference::System,
        ThemeMode::Light => egui::ThemePreference::Light,
        ThemeMode::Dark => egui::ThemePreference::Dark,
    };
    ctx.set_theme(preference);
    // Keep the native window appearance (and the vibrancy material behind the
    // Liquid Glass) in step with a forced theme; see `glass::sync_appearance`.
    #[cfg(target_os = "macos")]
    crate::frontend::glass::sync_appearance(mode);
}

/// Build the native-leaning [`Visuals`] for one scheme + theme, sourced from
/// its palette.
fn build_visuals(scheme: ColorScheme, dark: bool) -> Visuals {
    let pal = Palette::for_scheme(scheme, dark);
    let mut visuals = if dark {
        Visuals::dark()
    } else {
        Visuals::light()
    };

    // Re-base every default egui surface on the scheme palette. egui's stock
    // `Visuals::light()/dark()` fills are cool grays; left untouched they leak
    // through wherever a widget isn't explicitly painted (TextEdit backing,
    // popups, plain buttons) and read as cold patches on the ivory/charcoal
    // surfaces.
    visuals.panel_fill = pal.central;
    visuals.window_fill = pal.central;
    visuals.extreme_bg_color = pal.input_fill;
    visuals.faint_bg_color = pal.item_fill_hover;
    visuals.widgets.noninteractive.bg_fill = pal.central;
    visuals.widgets.noninteractive.fg_stroke.color = pal.text_primary;
    visuals.widgets.inactive.weak_bg_fill = pal.item_fill;
    // `bg_fill` is used by filled controls such as slider rails, checkboxes,
    // and radio buttons. In dark mode those rails can disappear against a
    // translucent sidebar, so lift only this filled-control layer; keep
    // `weak_bg_fill` at the quieter item color so regular buttons do not all
    // become visually louder.
    visuals.widgets.inactive.bg_fill = if dark {
        mix(pal.item_fill, pal.text_primary, 0.08)
    } else {
        pal.item_fill
    };
    visuals.widgets.inactive.fg_stroke.color = pal.text_primary;
    visuals.widgets.open.weak_bg_fill = pal.item_fill_active;
    visuals.widgets.open.bg_fill = pal.item_fill_active;

    // Selection is a soft, *filled*, rounded highlight — not a hard outline.
    // The stroke width is 0 (so no outline is drawn), but its *color* must stay
    // visible: egui uses `selection.stroke.color` as the text color of selected
    // buttons / `selectable_label`s (see `Style::button_style`). A transparent
    // color there makes the selected combo/menu item's text invisible.
    visuals.selection.bg_fill = pal.selection_fill;
    visuals.selection.stroke = Stroke::new(0.0, pal.text_strong);
    visuals.hyperlink_color = pal.accent;

    // Controls have gently rounded corners. egui defaults to 2-3px; nudge up.
    let control = CornerRadius::same(radius::CONTROL);
    for widget in [
        &mut visuals.widgets.noninteractive,
        &mut visuals.widgets.inactive,
        &mut visuals.widgets.hovered,
        &mut visuals.widgets.active,
        &mut visuals.widgets.open,
    ] {
        widget.corner_radius = control;
    }
    visuals.window_corner_radius = CornerRadius::same(radius::MODAL);
    // Menus, tooltips and popovers sit one step rounder than controls so they
    // read as floating cards (Golden Gate capsule-ish feel on short tooltips).
    visuals.menu_corner_radius = CornerRadius::same(radius::CARD);

    // Inputs keep a faint resting hairline, but hover and press become a soft
    // *filled* block with no visible outline, rather than the hard wireframe
    // egui draws by default. Keep the stroke width at 1px in every interactive
    // state: egui subtracts `bg_stroke.width` from button/selectable inner
    // margins, so switching to `Stroke::NONE` makes ComboBox menu rows and
    // selectable labels shift by a pixel while hovering.
    visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, pal.hairline);
    let invisible_stable_stroke = Stroke::new(1.0, Color32::TRANSPARENT);

    visuals.widgets.hovered.weak_bg_fill = pal.item_fill_hover;
    visuals.widgets.hovered.bg_fill = pal.item_fill_hover;
    visuals.widgets.hovered.bg_stroke = invisible_stable_stroke;

    visuals.widgets.active.weak_bg_fill = pal.item_fill_active;
    visuals.widgets.active.bg_fill = pal.item_fill_active;
    visuals.widgets.active.bg_stroke = invisible_stable_stroke;
    visuals.widgets.open.bg_stroke = invisible_stable_stroke;

    // Dark translucent panels need a stronger read on range controls. The rail
    // remains neutral; the filled portion gives the user a clear value cue.
    visuals.slider_trailing_fill = dark;

    // Keep label/icon color stable across hover and press; only the fill changes.
    // The divider hover line also reads from this stroke, but preserving text
    // contrast is more important than tinting the foreground on routine buttons.
    visuals.widgets.hovered.fg_stroke.color = pal.text_primary;
    visuals.widgets.active.fg_stroke.color = pal.text_primary;
    visuals.widgets.open.fg_stroke.color = pal.text_primary;

    visuals.window_stroke = Stroke::new(1.0, pal.hairline);

    // Popups (tooltips, menus, combo lists) and windows get a soft, mostly
    // *vertical* drop shadow. egui's default shoves the popup shadow well to the
    // right ([6, 10] offset with only 8px of blur), which reads as a hard band
    // pushed to one corner rather than a shadow cast by light from above. A
    // near-centered offset with a wider blur diffuses it into a natural ambient
    // halo; the dark theme needs a heavier alpha to register against near-black.
    let shadow_alpha = if dark { 120 } else { 36 };
    visuals.popup_shadow = Shadow {
        offset: [0, 5],
        blur: 18,
        spread: 0,
        color: Color32::from_black_alpha(shadow_alpha),
    };
    visuals.window_shadow = Shadow {
        offset: [0, 10],
        blur: 28,
        spread: 0,
        color: Color32::from_black_alpha(shadow_alpha),
    };

    visuals
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glass_alpha_spans_the_documented_range() {
        assert_eq!(glass_alpha(0.0), GLASS_ALPHA_MIN as u8);
        assert_eq!(glass_alpha(1.0), GLASS_ALPHA_MAX as u8);
        // The default intensity (0.35) maps to the historical fixed tint (~110),
        // so existing setups look unchanged until the user moves the slider.
        assert_eq!(glass_alpha(0.35), 110);
    }

    #[test]
    fn glass_alpha_clamps_out_of_range_intensity() {
        assert_eq!(glass_alpha(-1.0), GLASS_ALPHA_MIN as u8);
        assert_eq!(glass_alpha(2.0), GLASS_ALPHA_MAX as u8);
    }

    #[test]
    fn chrome_fill_keeps_rgb_and_applies_requested_alpha() {
        let base = Color32::from_rgb(10, 20, 30);
        // No glass: the opaque base color is returned unchanged.
        assert_eq!(chrome_fill(base, None), base);
        // Glass: same RGB, made semi-transparent at the requested alpha.
        assert_eq!(
            chrome_fill(base, Some(128)),
            Color32::from_rgba_unmultiplied(10, 20, 30, 128)
        );
    }

    #[test]
    fn mix_returns_endpoints_and_clamps_t() {
        let a = Color32::from_rgb(0, 0, 0);
        let b = Color32::from_rgb(255, 255, 255);
        assert_eq!(mix(a, b, 0.0), a);
        assert_eq!(mix(a, b, 1.0), b);
        // `t` clamps to 0..=1.
        assert_eq!(mix(a, b, -1.0), a);
        assert_eq!(mix(a, b, 2.0), b);
        // The midpoint lands strictly between the endpoints on every channel.
        let [r, g, bl, _] = mix(a, b, 0.5).to_array();
        assert!((1..255).contains(&r));
        assert!((1..255).contains(&g));
        assert!((1..255).contains(&bl));
    }

    #[test]
    fn interactive_widget_stroke_widths_keep_selectable_rows_stable() {
        for scheme in ColorScheme::all() {
            for dark in [false, true] {
                let visuals = build_visuals(scheme, dark);
                let width = visuals.widgets.inactive.bg_stroke.width;
                assert_eq!(visuals.widgets.hovered.bg_stroke.width, width);
                assert_eq!(visuals.widgets.active.bg_stroke.width, width);
                assert_eq!(visuals.widgets.open.bg_stroke.width, width);
            }
        }
    }

    #[test]
    fn dark_slider_visuals_read_above_button_fill() {
        for scheme in ColorScheme::all() {
            let pal = Palette::for_scheme(scheme, true);
            let visuals = build_visuals(scheme, true);
            assert_eq!(visuals.widgets.inactive.weak_bg_fill, pal.item_fill);
            assert_ne!(visuals.widgets.inactive.bg_fill, pal.item_fill);
            assert!(
                relative_luminance(visuals.widgets.inactive.bg_fill)
                    > relative_luminance(pal.item_fill)
            );
            assert!(visuals.slider_trailing_fill);
        }
    }

    #[test]
    fn light_sliders_keep_quiet_neutral_rail() {
        for scheme in ColorScheme::all() {
            let pal = Palette::for_scheme(scheme, false);
            let visuals = build_visuals(scheme, false);
            assert_eq!(visuals.widgets.inactive.bg_fill, pal.item_fill);
            assert!(!visuals.slider_trailing_fill);
        }
    }

    fn relative_luminance(color: Color32) -> f32 {
        let [r, g, b, _] = color.to_array();
        0.2126 * f32::from(r) + 0.7152 * f32::from(g) + 0.0722 * f32::from(b)
    }
}
