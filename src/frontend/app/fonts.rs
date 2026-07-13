//! System font installation. Loads the platform UI/code faces and binds the
//! named CJK families the UI references, so mixed English/Chinese text renders
//! without missing-glyph boxes and epaint never panics on an unbound family.

use eframe::egui;

/// Named egui font families the UI references via `FontFamily::Name`. Each must
/// resolve to at least one loaded font on every platform — epaint panics at
/// layout time when a referenced family is unbound or empty.
/// `install_system_fonts` enforces this for every entry; the
/// `ui_named_font_families_are_bound` test is the cross-platform regression guard.
pub(crate) const ASSISTANT_CJK_FONT: &str = "assistant-cjk";
pub(crate) const CONSOLE_CJK_MONO_FONT: &str = "console-cjk-mono";

/// Single source of truth: every named family the UI uses, paired with the base
/// family to fall back to when it would otherwise be unbound. The installer's
/// fallback loop and the regression test both read this, so a newly added family
/// cannot silently drift out of sync with its registration.
pub(super) const UI_NAMED_FONT_FAMILIES: &[(&str, egui::FontFamily)] = &[
    (ASSISTANT_CJK_FONT, egui::FontFamily::Proportional),
    (CONSOLE_CJK_MONO_FONT, egui::FontFamily::Monospace),
];

/// On native desktop targets, prefer the platform UI/code font and keep system
/// CJK faces as fallbacks so mixed English/Chinese assistant output does not
/// render as missing-glyph boxes. On every platform, the tail loop guarantees
/// the named families above resolve to real fonts.
pub(super) fn install_system_fonts(fonts: &mut egui::FontDefinitions) {
    #[cfg(target_os = "macos")]
    {
        let assistant_cjk = egui::FontFamily::Name(ASSISTANT_CJK_FONT.into());
        let console_cjk_mono = egui::FontFamily::Name(CONSOLE_CJK_MONO_FONT.into());
        fonts.families.insert(assistant_cjk.clone(), Vec::new());
        fonts.families.insert(console_cjk_mono.clone(), Vec::new());
        install_font(
            fonts,
            "SF Pro",
            "/System/Library/Fonts/SFNS.ttf",
            egui::FontFamily::Proportional,
            true,
        );
        install_font(
            fonts,
            "SF Mono",
            "/System/Library/Fonts/SFNSMono.ttf",
            egui::FontFamily::Monospace,
            true,
        );
        install_font(
            fonts,
            "Hiragino Sans GB Assistant",
            "/System/Library/Fonts/Hiragino Sans GB.ttc",
            assistant_cjk.clone(),
            false,
        );
        install_font(
            fonts,
            "STHeiti Medium Assistant",
            "/System/Library/Fonts/STHeiti Medium.ttc",
            assistant_cjk.clone(),
            false,
        );
        install_font(
            fonts,
            "SF Pro Assistant Fallback",
            "/System/Library/Fonts/SFNS.ttf",
            assistant_cjk,
            false,
        );
        install_font(
            fonts,
            "Hiragino Sans GB Console",
            "/System/Library/Fonts/Hiragino Sans GB.ttc",
            console_cjk_mono.clone(),
            false,
        );
        install_font(
            fonts,
            "STHeiti Medium Console",
            "/System/Library/Fonts/STHeiti Medium.ttc",
            console_cjk_mono.clone(),
            false,
        );
        install_font(
            fonts,
            "Menlo Console Fallback",
            "/System/Library/Fonts/Menlo.ttc",
            console_cjk_mono,
            false,
        );
        install_font(
            fonts,
            "Hiragino Sans GB",
            "/System/Library/Fonts/Hiragino Sans GB.ttc",
            egui::FontFamily::Proportional,
            false,
        );
        install_font(
            fonts,
            "STHeiti Medium",
            "/System/Library/Fonts/STHeiti Medium.ttc",
            egui::FontFamily::Proportional,
            false,
        );
        install_font(
            fonts,
            "Hiragino Sans GB Mono Fallback",
            "/System/Library/Fonts/Hiragino Sans GB.ttc",
            egui::FontFamily::Monospace,
            false,
        );
        install_font(
            fonts,
            "STHeiti Medium Mono Fallback",
            "/System/Library/Fonts/STHeiti Medium.ttc",
            egui::FontFamily::Monospace,
            false,
        );
    }

    #[cfg(target_os = "windows")]
    {
        let assistant_cjk = egui::FontFamily::Name(ASSISTANT_CJK_FONT.into());
        let console_cjk_mono = egui::FontFamily::Name(CONSOLE_CJK_MONO_FONT.into());
        fonts.families.insert(assistant_cjk.clone(), Vec::new());
        fonts.families.insert(console_cjk_mono.clone(), Vec::new());
        let windir = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".to_string());
        let font_path = |file: &str| format!("{windir}\\Fonts\\{file}");

        install_font(
            fonts,
            "Segoe UI",
            font_path("segoeui.ttf"),
            egui::FontFamily::Proportional,
            true,
        );
        install_font(
            fonts,
            "Consolas",
            font_path("consola.ttf"),
            egui::FontFamily::Monospace,
            true,
        );
        install_font(
            fonts,
            "Microsoft YaHei Assistant",
            font_path("msyh.ttc"),
            assistant_cjk.clone(),
            false,
        );
        install_font(
            fonts,
            "SimSun Assistant",
            font_path("simsun.ttc"),
            assistant_cjk.clone(),
            false,
        );
        install_font(
            fonts,
            "Segoe UI Assistant Fallback",
            font_path("segoeui.ttf"),
            assistant_cjk,
            false,
        );
        install_font(
            fonts,
            "Microsoft YaHei Console",
            font_path("msyh.ttc"),
            console_cjk_mono.clone(),
            false,
        );
        install_font(
            fonts,
            "SimSun Console",
            font_path("simsun.ttc"),
            console_cjk_mono.clone(),
            false,
        );
        install_font(
            fonts,
            "Consolas Console Fallback",
            font_path("consola.ttf"),
            console_cjk_mono,
            false,
        );
        install_font(
            fonts,
            "Microsoft YaHei",
            font_path("msyh.ttc"),
            egui::FontFamily::Proportional,
            false,
        );
        install_font(
            fonts,
            "SimSun",
            font_path("simsun.ttc"),
            egui::FontFamily::Proportional,
            false,
        );
        install_font(
            fonts,
            "Microsoft YaHei Mono Fallback",
            font_path("msyh.ttc"),
            egui::FontFamily::Monospace,
            false,
        );
        install_font(
            fonts,
            "SimSun Mono Fallback",
            font_path("simsun.ttc"),
            egui::FontFamily::Monospace,
            false,
        );
    }

    // epaint panics when a `FontFamily::Name` resolves to no fonts. The macOS
    // and Windows blocks above bind the named families to system CJK faces;
    // elsewhere they are still missing here (and a failed platform font read
    // could leave one empty). Alias every unbound or empty named family to its
    // default stack so layout always has real fonts and the app starts.
    for (name, base) in UI_NAMED_FONT_FAMILIES {
        let family = egui::FontFamily::Name((*name).into());
        if fonts
            .families
            .get(&family)
            .is_none_or(|list| list.is_empty())
        {
            let fallback = fonts.families.get(base).cloned().unwrap_or_default();
            fonts.families.insert(family, fallback);
        }
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn install_font(
    fonts: &mut egui::FontDefinitions,
    name: &str,
    path: impl AsRef<std::path::Path>,
    family: egui::FontFamily,
    prepend: bool,
) {
    if let Ok(bytes) = std::fs::read(path) {
        fonts.font_data.insert(
            name.to_owned(),
            std::sync::Arc::new(egui::FontData::from_owned(bytes)),
        );
        if let Some(list) = fonts.families.get_mut(&family) {
            if prepend {
                list.insert(0, name.to_owned());
            } else {
                list.push(name.to_owned());
            }
        }
    }
}
