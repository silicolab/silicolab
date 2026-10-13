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

    #[cfg(target_os = "linux")]
    {
        let mut database = usvg::fontdb::Database::new();
        database.load_system_fonts();
        let mut installed = install_linux_cjk_font(fonts, &database);
        if !installed && super::is_wsl() {
            installed = install_wsl_cjk_font(
                fonts,
                &mut database,
                std::path::Path::new("/mnt/c/Windows/Fonts"),
            );
        }
        if !installed {
            eprintln!(
                "No usable Chinese font found. Install a CJK font (e.g. fonts-noto-cjk) and restart SilicoLab."
            );
        }
    }

    bind_named_font_families(fonts);
}

fn bind_named_font_families(fonts: &mut egui::FontDefinitions) {
    // Named families must remain bound even when no system CJK font is installed.
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

#[cfg(target_os = "linux")]
fn install_wsl_cjk_font(
    fonts: &mut egui::FontDefinitions,
    database: &mut usvg::fontdb::Database,
    directory: &std::path::Path,
) -> bool {
    for filename in ["msyh.ttc", "simsun.ttc", "simhei.ttf"] {
        let path = directory.join(filename);
        match database.load_font_file(&path) {
            Ok(()) => {
                if install_linux_cjk_font(fonts, database) {
                    return true;
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => eprintln!("Could not load WSL host font {}: {error}", path.display()),
        }
    }
    false
}

#[cfg(target_os = "linux")]
fn install_linux_cjk_font(
    fonts: &mut egui::FontDefinitions,
    database: &usvg::fontdb::Database,
) -> bool {
    use usvg::fontdb::{Style, Weight};

    let preferred = [
        "Noto Sans CJK SC",
        "Noto Sans SC",
        "Source Han Sans SC",
        "WenQuanYi Micro Hei",
        "WenQuanYi Zen Hei",
    ];
    let mut faces: Vec<_> = database.faces().collect();
    faces.sort_by_key(|face| {
        let preference = preferred
            .iter()
            .position(|name| face.families.iter().any(|(family, _)| family == name))
            .unwrap_or(preferred.len());
        (
            face.style != Style::Normal,
            face.weight.0.abs_diff(Weight::NORMAL.0),
            preference,
        )
    });
    for face in faces {
        let data = database.with_face_data(face.id, |bytes, index| {
            let parsed = ttf_parser::Face::parse(bytes, index).ok()?;
            if !"中文你好".chars().all(|c| parsed.glyph_index(c).is_some()) {
                return None;
            }
            let mut data = egui::FontData::from_owned(bytes.to_vec());
            // CJK fonts commonly use collections; the matching face need not be index 0.
            data.index = index;
            Some(data)
        });
        let Some(Some(data)) = data else {
            continue;
        };
        let name = "Linux CJK Fallback".to_owned();
        fonts
            .font_data
            .insert(name.clone(), std::sync::Arc::new(data));
        for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            fonts.families.entry(family).or_default().push(name.clone());
        }
        return true;
    }
    false
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

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn linux_cjk_collection_renders_in_every_ui_family() {
        let mut database = usvg::fontdb::Database::new();
        database.load_font_data(
            include_bytes!("../../../tests/fixtures/fonts/cjk-fallback.ttc").to_vec(),
        );
        let mut fonts = egui::FontDefinitions::default();
        install_linux_cjk_font(&mut fonts, &database);
        bind_named_font_families(&mut fonts);

        assert_eq!(fonts.font_data["Linux CJK Fallback"].index, 1);
        assert_chinese_glyphs(fonts);
    }

    #[test]
    fn wsl_loads_host_collection_without_linux_cjk_fonts() {
        let directory =
            std::env::temp_dir().join(format!("silicolab-fonts-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&directory).unwrap();
        std::fs::write(
            directory.join("msyh.ttc"),
            include_bytes!("../../../tests/fixtures/fonts/cjk-fallback.ttc"),
        )
        .unwrap();
        let mut database = usvg::fontdb::Database::new();
        database.load_font_data(crate::plot::EXPORT_FONT.to_vec());
        let mut fonts = egui::FontDefinitions::default();
        let installed = install_wsl_cjk_font(&mut fonts, &mut database, &directory);
        std::fs::remove_dir_all(directory).unwrap();

        assert!(installed);
        assert_eq!(fonts.font_data["Linux CJK Fallback"].index, 1);
        bind_named_font_families(&mut fonts);
        assert_chinese_glyphs(fonts);
    }

    #[test]
    fn installed_wsl_host_font_renders_chinese() {
        if !super::super::is_wsl()
            || !std::path::Path::new("/mnt/c/Windows/Fonts/msyh.ttc").is_file()
        {
            return;
        }
        let mut fonts = egui::FontDefinitions::default();
        egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
        install_system_fonts(&mut fonts);
        assert_chinese_glyphs(fonts);
    }

    fn assert_chinese_glyphs(fonts: egui::FontDefinitions) {
        let context = egui::Context::default();
        context.set_fonts(fonts);
        context.begin_pass(egui::RawInput::default());
        context.fonts_mut(|fonts| {
            for family in [
                egui::FontFamily::Proportional,
                egui::FontFamily::Monospace,
                egui::FontFamily::Name(ASSISTANT_CJK_FONT.into()),
                egui::FontFamily::Name(CONSOLE_CJK_MONO_FONT.into()),
            ] {
                assert!(
                    fonts.has_glyphs(&egui::FontId::new(13.0, family.clone()), "中文你好繁體，。"),
                    "missing Chinese glyphs in {family:?}",
                );
            }
        });
        let _ = context.end_pass();
    }

    #[test]
    fn linux_does_not_treat_latin_fonts_as_cjk_fallbacks() {
        let mut database = usvg::fontdb::Database::new();
        database.load_font_data(crate::plot::EXPORT_FONT.to_vec());
        let mut fonts = egui::FontDefinitions::default();
        install_linux_cjk_font(&mut fonts, &database);
        bind_named_font_families(&mut fonts);

        assert!(!fonts.font_data.contains_key("Linux CJK Fallback"));
        for (name, base) in UI_NAMED_FONT_FAMILIES {
            assert_eq!(
                fonts.families[&egui::FontFamily::Name((*name).into())],
                fonts.families[base],
            );
        }
    }
}
