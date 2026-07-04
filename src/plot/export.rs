use anyhow::{Context, Result, anyhow};

use super::spec::{ChartSpec, ExportFormat, ExportStyle, JournalPreset};
use super::svg::render_svg;

/// Render `spec` and encode it as `format`. `dpi` only affects PNG (SVG/PDF
/// carry physical sizes).
pub fn export_bytes(
    spec: &ChartSpec,
    preset: &JournalPreset,
    style: &ExportStyle,
    format: ExportFormat,
    dpi: u32,
) -> Result<Vec<u8>> {
    let svg = render_svg(spec, preset, style)?;
    match format {
        ExportFormat::Svg => Ok(svg.into_bytes()),
        ExportFormat::Png => png_bytes(&parse_tree(&svg)?, preset, dpi),
        ExportFormat::Pdf => pdf_bytes(&parse_tree(&svg)?),
    }
}

/// Parse the emitted SVG with a fontdb that holds only the bundled font, so
/// text resolves identically on every machine.
fn parse_tree(svg: &str) -> Result<usvg::Tree> {
    let mut fontdb = usvg::fontdb::Database::new();
    fontdb.load_font_data(super::EXPORT_FONT.to_vec());
    let options = usvg::Options {
        fontdb: std::sync::Arc::new(fontdb),
        ..usvg::Options::default()
    };
    usvg::Tree::from_str(svg, &options).context("parse generated chart SVG")
}

/// PNG export DPI bounds, matching the export dialog's `72..=1200` slider.
/// Enforced here too so an out-of-range persisted pref (a hand-edited
/// settings.json with `dpi: 8000`) can't drive a multi-GB Pixmap allocation.
const MIN_DPI: u32 = 72;
const MAX_DPI: u32 = 1200;

/// Ceiling on rasterized pixels. The dialog allows size and DPI combinations
/// (20 in at 1200 dpi = 24000 px a side) whose Pixmap plus RGBA copy would
/// exceed 4 GiB; 100 MP keeps the peak near 800 MB and still admits every
/// journal preset at 1200 dpi.
const MAX_PIXELS: u64 = 100_000_000;

fn png_bytes(tree: &usvg::Tree, preset: &JournalPreset, dpi: u32) -> Result<Vec<u8>> {
    let dpi = dpi.clamp(MIN_DPI, MAX_DPI);
    let px_w = (preset.width_in * f64::from(dpi)).round() as u32;
    let px_h = (preset.height_in * f64::from(dpi)).round() as u32;
    if u64::from(px_w) * u64::from(px_h) > MAX_PIXELS {
        return Err(anyhow!(
            "PNG size {px_w}x{px_h} px is too large to rasterize; reduce the size or DPI"
        ));
    }
    let mut pixmap = resvg::tiny_skia::Pixmap::new(px_w, px_h)
        .ok_or_else(|| anyhow!("invalid raster size {px_w}x{px_h}"))?;
    // usvg parsed physical units at its default 96 dpi, so the tree size is
    // inches × 96; scale it onto the exact target pixel grid.
    let transform = resvg::tiny_skia::Transform::from_scale(
        px_w as f32 / tree.size().width(),
        px_h as f32 / tree.size().height(),
    );
    resvg::render(tree, transform, &mut pixmap.as_mut());

    let rgba: Vec<u8> = pixmap
        .pixels()
        .iter()
        .flat_map(|pixel| {
            let straight = pixel.demultiply();
            [
                straight.red(),
                straight.green(),
                straight.blue(),
                straight.alpha(),
            ]
        })
        .collect();

    let pixels_per_meter = (f64::from(dpi) / 0.0254).round() as u32;
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, px_w, px_h);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_pixel_dims(Some(png::PixelDimensions {
        xppu: pixels_per_meter,
        yppu: pixels_per_meter,
        unit: png::Unit::Meter,
    }));
    let mut writer = encoder.write_header().context("write PNG header")?;
    writer
        .write_image_data(&rgba)
        .context("write PNG image data")?;
    writer.finish().context("finish PNG stream")?;
    Ok(out)
}

fn pdf_bytes(tree: &usvg::Tree) -> Result<Vec<u8>> {
    // The tree's pixel size was parsed at 96 dpi; telling svg2pdf the same dpi
    // makes the PDF page exactly the preset's physical size.
    let page = svg2pdf::PageOptions { dpi: 96.0 };
    svg2pdf::to_pdf(tree, svg2pdf::ConversionOptions::default(), page)
        .map_err(|error| anyhow!("convert chart SVG to PDF: {error:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plot::spec::{AxisSpec, Mark, PresetChoice, Series};

    fn spec() -> ChartSpec {
        ChartSpec {
            title: "SCF convergence".to_string(),
            x: AxisSpec::new("Iteration", ""),
            y: AxisSpec::new("Energy", "Eh"),
            series: vec![Series {
                name: "SCF energy".to_string(),
                points: vec![[1.0, -74.1], [2.0, -74.9], [3.0, -74.96]],
                mark: Mark::Line,
            }],
        }
    }

    #[test]
    fn png_dimensions_are_inches_times_dpi() {
        for (choice, dpi, expect) in [
            (PresetChoice::SingleColumn, 300, (990, 750)),
            (PresetChoice::DoubleColumn, 600, (4200, 2520)),
        ] {
            let bytes = export_bytes(
                &spec(),
                &choice.preset(),
                &ExportStyle::default(),
                ExportFormat::Png,
                dpi,
            )
            .unwrap();
            let decoder = png::Decoder::new(std::io::Cursor::new(&bytes));
            let reader = decoder.read_info().unwrap();
            let info = reader.info();
            assert_eq!((info.width, info.height), expect);
        }
    }

    #[test]
    fn png_dpi_is_clamped_to_the_dialog_range() {
        let preset = PresetChoice::SingleColumn.preset();
        // Below the floor clamps to 72 dpi, absurdly high clamps to 1200 — both
        // succeed with bounded pixel dimensions, no allocation abort.
        for (dpi, expect) in [(1u32, (238u32, 180u32)), (20_000u32, (3960u32, 3000u32))] {
            let bytes = export_bytes(
                &spec(),
                &preset,
                &ExportStyle::default(),
                ExportFormat::Png,
                dpi,
            )
            .unwrap();
            let decoder = png::Decoder::new(std::io::Cursor::new(&bytes));
            let reader = decoder.read_info().unwrap();
            assert_eq!((reader.info().width, reader.info().height), expect);
        }
    }

    #[test]
    fn png_oversized_raster_errors_instead_of_allocating() {
        // The dialog's extremes (20 in × 20 in at 1200 dpi) would be a
        // 24000×24000 Pixmap — refuse before allocation.
        let preset = PresetChoice::Custom {
            width_in: 20.0,
            height_in: 20.0,
        }
        .preset();
        let error = export_bytes(
            &spec(),
            &preset,
            &ExportStyle::default(),
            ExportFormat::Png,
            1200,
        )
        .unwrap_err();
        assert!(error.to_string().contains("too large"), "{error}");
    }

    #[test]
    fn png_carries_physical_dpi_metadata() {
        let bytes = export_bytes(
            &spec(),
            &PresetChoice::SingleColumn.preset(),
            &ExportStyle::default(),
            ExportFormat::Png,
            300,
        )
        .unwrap();
        let decoder = png::Decoder::new(std::io::Cursor::new(&bytes));
        let reader = decoder.read_info().unwrap();
        let dims = reader.info().pixel_dims.expect("pHYs chunk present");
        // 300 dpi = 11811 pixels per meter.
        assert_eq!(dims.xppu, 11811);
        assert_eq!(dims.yppu, 11811);
        assert!(matches!(dims.unit, png::Unit::Meter));
    }

    #[test]
    fn pdf_export_embeds_a_font_subset() {
        let bytes = export_bytes(
            &spec(),
            &PresetChoice::SingleColumn.preset(),
            &ExportStyle::default(),
            ExportFormat::Pdf,
            300,
        )
        .unwrap();
        assert!(bytes.starts_with(b"%PDF-"));
        assert!(bytes.len() > 1000);
        assert!(
            bytes.windows(9).any(|window| window == b"FontFile2"),
            "expected an embedded TrueType font subset"
        );
    }

    #[test]
    fn svg_export_is_the_emitter_output() {
        let preset = PresetChoice::SingleColumn.preset();
        let style = ExportStyle::default();
        let bytes = export_bytes(&spec(), &preset, &style, ExportFormat::Svg, 300).unwrap();
        let direct = render_svg(&spec(), &preset, &style).unwrap();
        assert_eq!(bytes, direct.into_bytes());
    }
}
