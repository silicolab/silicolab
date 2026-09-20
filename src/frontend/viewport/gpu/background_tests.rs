use super::super::camera::ViewCamera;
use super::super::export::*;
use super::*;
use crate::{
    domain::{Atom, Structure},
    frontend::{AtomSelection, ViewportVisualState},
};
use eframe::egui::Color32;
#[test]
#[ignore = "needs a GPU adapter"]
fn gpu_png_backgrounds_and_transparent_compositing() {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
            .unwrap();
    let (device, queue) =
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).unwrap();
    let exporter = GpuExporter { device, queue };
    let structure = Structure::with_bonds(
        "carbon",
        [-3.0, 3.0]
            .into_iter()
            .map(|x| Atom {
                element: "C".into(),
                position: nalgebra::Point3::new(x, 0.0, 0.0),
                charge: 0.0,
            })
            .collect(),
        vec![],
    );
    let empty = Structure::empty();
    let mut visual = ViewportVisualState {
        show_cell: false,
        ..Default::default()
    };
    let directory = std::path::PathBuf::from("target/gpu-background-tests");
    std::fs::create_dir_all(&directory).unwrap();
    let ctx = eframe::egui::Context::default();
    for scheme in [
        crate::backend::config::ColorScheme::Warm,
        crate::backend::config::ColorScheme::Cool,
    ] {
        crate::frontend::theme::set_scheme(&ctx, scheme);
        for theme in [
            eframe::egui::ThemePreference::Light,
            eframe::egui::ThemePreference::Dark,
        ] {
            for color in [None, Some(ViewportVisualState::DEFAULT_BACKGROUND)] {
                let path = directory.join("queued.png");
                let request = PendingViewportPngExport {
                    background: ImageExportBackground::Viewport,
                    structure: structure.clone(),
                    camera: ViewCamera::default(),
                    selection: AtomSelection::default(),
                    visual_state: ViewportVisualState {
                        background_color: color,
                        ..visual.clone()
                    },
                    width: 128,
                    height: 128,
                    output_path: path.clone(),
                };
                ctx.set_theme(theme);
                let mut expected = Color32::TRANSPARENT;
                let _ = ctx.run_ui(Default::default(), |ui| {
                    expected = color.unwrap_or(crate::frontend::theme::palette(ui).viewport_bg);
                });
                request.execute(&exporter, &ctx).unwrap();
                let png = image::open(&path).unwrap().into_rgba8();
                assert_eq!(png.get_pixel(0, 0).0, expected.to_array());
            }
        }
    }
    let render = |structure: &Structure,
                  visual: &ViewportVisualState,
                  background: ImageExportBackground,
                  theme: Color32| {
        let path = directory.join("image.png");
        export_viewport_png(
            &exporter,
            structure,
            ViewportPngExport {
                background: background.resolve(visual, theme),
                camera: ViewCamera {
                    zoom: -0.3,
                    ..Default::default()
                },
                selection: &AtomSelection::default(),
                visual_state: visual,
                width: 128,
                height: 128,
                output_path: &path,
            },
        )
        .unwrap();
        image::open(path).unwrap().into_rgba8()
    };
    for theme in [
        Color32::from_rgb(240, 241, 242),
        Color32::from_rgb(20, 21, 22),
    ] {
        for background in [
            ImageExportBackground::Viewport,
            ImageExportBackground::White,
            ImageExportBackground::Custom(Color32::from_rgb(12, 34, 56)),
            ImageExportBackground::Transparent,
        ] {
            let expected = background.resolve(&visual, theme).clear.to_array();
            let blank = render(&empty, &visual, background, theme);
            assert!(blank.pixels().all(|p| p.0 == expected));
            let molecule = render(&structure, &visual, background, theme);
            assert_eq!(molecule.get_pixel(0, 0).0, expected);
            assert!(molecule.pixels().any(|p| p[3] == 255 && p.0 != expected));
        }
    }
    visual.background_color = Some(Color32::from_rgb(66, 77, 88));
    let custom = render(
        &empty,
        &visual,
        ImageExportBackground::Viewport,
        Color32::BLACK,
    );
    assert_eq!(custom.get_pixel(0, 0).0, [66, 77, 88, 255]);
    visual.surface_overlay.atoms.insert(0, true);
    visual.surface_overlay.atoms.insert(1, true);
    visual.surface.transparency = 0.5;
    for style in [
        crate::frontend::SurfaceStyle::Fill,
        crate::frontend::SurfaceStyle::Mesh,
    ] {
        visual.surface.style = style;
        let transparent = render(
            &structure,
            &visual,
            ImageExportBackground::Transparent,
            Color32::BLACK,
        );
        let white = render(
            &structure,
            &visual,
            ImageExportBackground::White,
            Color32::BLACK,
        );
        assert_eq!(transparent.get_pixel(0, 0).0, [0, 0, 0, 0]);
        transparent
            .save(directory.join(format!("transparent-{style:?}.png")))
            .unwrap();
        white
            .save(directory.join(format!("white-{style:?}.png")))
            .unwrap();
        assert!(
            transparent.pixels().any(|p| p[3] > 0 && p[3] < 255),
            "{style:?}"
        );
        for (pixel, reference) in transparent.pixels().zip(white.pixels()) {
            let alpha = f32::from(pixel[3]) / 255.0;
            for channel in 0..3 {
                let composited =
                    (f32::from(pixel[channel]) * alpha + 255.0 * (1.0 - alpha)).round() as u8;
                assert!(
                    composited.abs_diff(reference[channel]) <= 3,
                    "{style:?}: {pixel:?} vs {reference:?}"
                );
            }
        }
    }
    for (width, height, path, message) in [
        (0, 128, directory.join("zero.png"), "non-zero"),
        (
            exporter.device.limits().max_texture_dimension_2d + 1,
            128,
            directory.join("large.png"),
            "GPU limit",
        ),
        (
            128,
            128,
            directory.join("missing/image.png"),
            "failed to save",
        ),
    ] {
        let error = export_viewport_png(
            &exporter,
            &empty,
            ViewportPngExport {
                background: ImageExportBackground::Transparent.resolve(&visual, Color32::WHITE),
                camera: ViewCamera {
                    zoom: -0.3,
                    ..Default::default()
                },
                selection: &AtomSelection::default(),
                visual_state: &visual,
                width,
                height,
                output_path: &path,
            },
        )
        .unwrap_err();
        assert!(error.to_string().contains(message), "{error}");
        assert!(!path.exists());
    }
}
