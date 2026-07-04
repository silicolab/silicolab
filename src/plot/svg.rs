use std::fmt::Write as _;

use anyhow::{Context, Result, bail};

use super::layout;
use super::spec::{ChartSpec, ExportStyle, JournalPreset, Mark};

const TICK_LEN_PT: f64 = 3.0;
const GAP_PT: f64 = 4.0;
const X_TICK_TARGET: usize = 6;
const Y_TICK_TARGET: usize = 5;

/// Advance-sum text measurement of the bundled export font. Kerning is
/// ignored — margin sizing only needs a close estimate.
pub struct TextMeasure {
    face: ttf_parser::Face<'static>,
}

impl TextMeasure {
    pub fn new() -> Result<Self> {
        let face =
            ttf_parser::Face::parse(super::EXPORT_FONT, 0).context("parse bundled export font")?;
        Ok(Self { face })
    }

    pub fn width_pt(&self, text: &str, size_pt: f64) -> f64 {
        let upem = f64::from(self.face.units_per_em());
        let units: f64 = text
            .chars()
            .map(|ch| {
                self.face
                    .glyph_index(ch)
                    .and_then(|glyph| self.face.glyph_hor_advance(glyph))
                    .map_or(upem * 0.5, f64::from)
            })
            .sum();
        units / upem * size_pt
    }
}

fn rgb(color: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", color[0], color[1], color[2])
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Fixed-precision coordinate formatting keeps the output deterministic.
fn num(value: f64) -> String {
    format!("{value:.2}")
}

/// Render `spec` as a standalone publication SVG at the preset's physical
/// size. One user unit = 1 pt (the viewBox spans `width_in × 72`).
pub fn render_svg(spec: &ChartSpec, preset: &JournalPreset, style: &ExportStyle) -> Result<String> {
    let measure = TextMeasure::new()?;
    let Some([x_data, y_data]) = layout::data_bounds(&spec.series) else {
        bail!("chart has no finite data points");
    };
    let mut x_range = spec.x.range.map(layout::pad_degenerate).unwrap_or(x_data);
    let mut y_range = spec.y.range.map(layout::pad_degenerate).unwrap_or(y_data);
    // A reversed explicit range (e.g. [0.9, 0.2]) survives pad_degenerate; sort
    // the endpoints so the `clamp(lo, hi)` baseline below can't panic. Axis
    // direction is the separate `inverted` flag, not endpoint order.
    if x_range[0] > x_range[1] {
        x_range.swap(0, 1);
    }
    if y_range[0] > y_range[1] {
        y_range.swap(0, 1);
    }
    // Sticks are drawn from the zero line; when the y-range is auto-fit, pull it
    // to include 0 so the shortest peak isn't flattened to a zero-length line.
    // An explicit user range wins as-is.
    if spec.y.range.is_none() && spec.series.iter().any(|s| matches!(s.mark, Mark::Sticks)) {
        y_range = layout::extend_to_zero(y_range);
    }
    // Finite endpoints can still yield a non-finite SPAN: two ±f64::MAX-scale
    // points overflow `max - min` to +inf, which would feed inf into every
    // coordinate map. Treat it as undrawable (same path as no finite data).
    for range in [x_range, y_range] {
        if !range[0].is_finite() || !range[1].is_finite() || !(range[1] - range[0]).is_finite() {
            bail!("chart range is not finite");
        }
    }
    let x_step = layout::nice_step(x_range[1] - x_range[0], X_TICK_TARGET);
    let y_step = layout::nice_step(y_range[1] - y_range[0], Y_TICK_TARGET);
    let x_ticks = layout::ticks(x_range[0], x_range[1], X_TICK_TARGET);
    let y_ticks = layout::ticks(y_range[0], y_range[1], Y_TICK_TARGET);

    let width_pt = preset.width_in * 72.0;
    let height_pt = preset.height_in * 72.0;
    let title_pt = preset.base_pt + 2.0;

    let y_tick_width = y_ticks
        .iter()
        .map(|&tick| measure.width_pt(&layout::format_tick(tick, y_step), preset.tick_pt))
        .fold(0.0_f64, f64::max);
    let left = GAP_PT + preset.base_pt * 1.2 + GAP_PT + y_tick_width + 2.0 + TICK_LEN_PT;
    let right = width_pt - 6.0;
    let top = if spec.title.is_empty() {
        6.0
    } else {
        GAP_PT + title_pt * 1.4
    };
    let bottom = height_pt
        - (TICK_LEN_PT + 2.0 + preset.tick_pt * 1.2 + GAP_PT + preset.base_pt * 1.2 + GAP_PT);
    if right <= left || bottom <= top {
        bail!("preset too small for the chart margins");
    }

    let map_x = |value: f64| {
        let mut t = (value - x_range[0]) / (x_range[1] - x_range[0]);
        if spec.x.inverted {
            t = 1.0 - t;
        }
        left + t * (right - left)
    };
    let map_y = |value: f64| {
        let mut t = (value - y_range[0]) / (y_range[1] - y_range[0]);
        if spec.y.inverted {
            t = 1.0 - t;
        }
        bottom - t * (bottom - top)
    };

    let ink = rgb(style.ink);
    let font = super::EXPORT_FONT_FAMILY;
    let mut svg = String::new();
    let _ = writeln!(
        svg,
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{}in" height="{}in" viewBox="0 0 {} {}">"#,
        preset.width_in,
        preset.height_in,
        num(width_pt),
        num(height_pt),
    );
    let _ = writeln!(
        svg,
        r#"<rect x="0" y="0" width="{}" height="{}" fill="{}"/>"#,
        num(width_pt),
        num(height_pt),
        rgb(style.background),
    );
    let _ = writeln!(
        svg,
        r#"<clipPath id="plot-area"><rect x="{}" y="{}" width="{}" height="{}"/></clipPath>"#,
        num(left),
        num(top),
        num(right - left),
        num(bottom - top),
    );

    if !spec.title.is_empty() {
        let _ = writeln!(
            svg,
            r#"<text x="{}" y="{}" font-family="{font}" font-size="{}" fill="{ink}" text-anchor="middle">{}</text>"#,
            num((left + right) / 2.0),
            num(GAP_PT + title_pt),
            num(title_pt),
            escape(&spec.title),
        );
    }

    for &tick in &y_ticks {
        let y = num(map_y(tick));
        let _ = writeln!(
            svg,
            r#"<line x1="{}" y1="{y}" x2="{}" y2="{y}" stroke="{}" stroke-width="0.4"/>"#,
            num(left),
            num(right),
            rgb(style.grid),
        );
    }

    let _ = writeln!(svg, r#"<g clip-path="url(#plot-area)">"#);
    for (index, series) in spec.series.iter().enumerate() {
        let color = rgb(style.series_color(index));
        let finite = || {
            series
                .points
                .iter()
                .filter(|p| p[0].is_finite() && p[1].is_finite())
        };
        match series.mark {
            Mark::Line => {
                let points: Vec<String> = finite()
                    .map(|p| format!("{},{}", num(map_x(p[0])), num(map_y(p[1]))))
                    .collect();
                let _ = writeln!(
                    svg,
                    r#"<polyline points="{}" fill="none" stroke="{color}" stroke-width="1"/>"#,
                    points.join(" "),
                );
            }
            Mark::Sticks => {
                let baseline = num(map_y(0.0_f64.clamp(y_range[0], y_range[1])));
                for point in finite() {
                    let x = num(map_x(point[0]));
                    let _ = writeln!(
                        svg,
                        r#"<line x1="{x}" y1="{baseline}" x2="{x}" y2="{}" stroke="{color}" stroke-width="0.9"/>"#,
                        num(map_y(point[1])),
                    );
                }
            }
        }
    }
    let _ = writeln!(svg, "</g>");

    let _ = writeln!(
        svg,
        r#"<rect x="{}" y="{}" width="{}" height="{}" fill="none" stroke="{ink}" stroke-width="0.75"/>"#,
        num(left),
        num(top),
        num(right - left),
        num(bottom - top),
    );
    for &tick in &x_ticks {
        let x = num(map_x(tick));
        let _ = writeln!(
            svg,
            r#"<line x1="{x}" y1="{}" x2="{x}" y2="{}" stroke="{ink}" stroke-width="0.75"/>"#,
            num(bottom),
            num(bottom + TICK_LEN_PT),
        );
        let _ = writeln!(
            svg,
            r#"<text x="{x}" y="{}" font-family="{font}" font-size="{}" fill="{ink}" text-anchor="middle">{}</text>"#,
            num(bottom + TICK_LEN_PT + 2.0 + preset.tick_pt),
            num(preset.tick_pt),
            escape(&layout::format_tick(tick, x_step)),
        );
    }
    for &tick in &y_ticks {
        let y = map_y(tick);
        let _ = writeln!(
            svg,
            r#"<line x1="{}" y1="{}" x2="{}" y2="{}" stroke="{ink}" stroke-width="0.75"/>"#,
            num(left - TICK_LEN_PT),
            num(y),
            num(left),
            num(y),
        );
        let _ = writeln!(
            svg,
            r#"<text x="{}" y="{}" font-family="{font}" font-size="{}" fill="{ink}" text-anchor="end">{}</text>"#,
            num(left - TICK_LEN_PT - 2.0),
            num(y + preset.tick_pt * 0.35),
            num(preset.tick_pt),
            escape(&layout::format_tick(tick, y_step)),
        );
    }

    let _ = writeln!(
        svg,
        r#"<text x="{}" y="{}" font-family="{font}" font-size="{}" fill="{ink}" text-anchor="middle">{}</text>"#,
        num((left + right) / 2.0),
        num(height_pt - GAP_PT),
        num(preset.base_pt),
        escape(&spec.x.display_label()),
    );
    let y_label_x = GAP_PT + preset.base_pt;
    let y_label_y = (top + bottom) / 2.0;
    let _ = writeln!(
        svg,
        r#"<text x="{x}" y="{y}" font-family="{font}" font-size="{}" fill="{ink}" text-anchor="middle" transform="rotate(-90 {x} {y})">{}</text>"#,
        num(preset.base_pt),
        escape(&spec.y.display_label()),
        x = num(y_label_x),
        y = num(y_label_y),
    );

    if spec.series.len() > 1 {
        for (index, series) in spec.series.iter().enumerate() {
            let row_y = top + 8.0 + index as f64 * preset.tick_pt * 1.6;
            let name_width = measure.width_pt(&series.name, preset.tick_pt);
            let text_x = right - 6.0;
            let swatch_end = text_x - name_width - 4.0;
            let color = rgb(style.series_color(index));
            let _ = writeln!(
                svg,
                r#"<line x1="{}" y1="{}" x2="{}" y2="{}" stroke="{color}" stroke-width="1"/>"#,
                num(swatch_end - 10.0),
                num(row_y - preset.tick_pt * 0.35),
                num(swatch_end),
                num(row_y - preset.tick_pt * 0.35),
            );
            let _ = writeln!(
                svg,
                r#"<text x="{}" y="{}" font-family="{font}" font-size="{}" fill="{ink}" text-anchor="end">{}</text>"#,
                num(text_x),
                num(row_y),
                num(preset.tick_pt),
                escape(&series.name),
            );
        }
    }

    svg.push_str("</svg>\n");
    Ok(svg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plot::spec::{AxisSpec, PresetChoice, Series};

    pub(crate) fn line_spec() -> ChartSpec {
        ChartSpec {
            title: "SCF convergence".to_string(),
            x: AxisSpec::new("Iteration", ""),
            y: AxisSpec::new("Energy", "Eh"),
            series: vec![
                Series {
                    name: "SCF energy".to_string(),
                    points: vec![
                        [1.0, -74.10],
                        [2.0, -74.72],
                        [3.0, -74.91],
                        [4.0, -74.958],
                        [5.0, -74.963],
                        [6.0, -74.9634],
                    ],
                    mark: Mark::Line,
                },
                Series {
                    name: "Reference".to_string(),
                    points: vec![[1.0, -74.9634], [6.0, -74.9634]],
                    mark: Mark::Line,
                },
            ],
        }
    }

    pub(crate) fn sticks_spec() -> ChartSpec {
        let mut x = AxisSpec::new("Wavenumber", "cm⁻¹");
        x.inverted = true;
        ChartSpec {
            title: "IR spectrum".to_string(),
            x,
            y: AxisSpec::new("Intensity", ""),
            series: vec![Series {
                name: "Peaks".to_string(),
                points: vec![[600.0, 0.2], [1650.0, 0.9], [3400.0, 0.5]],
                mark: Mark::Sticks,
            }],
        }
    }

    /// Byte-for-byte golden comparison. `SILICOLAB_BLESS=1` rewrites the
    /// fixture instead, for intentional emitter changes.
    fn assert_matches_fixture(name: &str, rendered: &str) {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src/plot/fixtures")
            .join(name);
        if std::env::var_os("SILICOLAB_BLESS").is_some() {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, rendered).unwrap();
            return;
        }
        let expected = std::fs::read_to_string(&path).unwrap_or_else(|_| {
            panic!(
                "missing fixture {}; create it with SILICOLAB_BLESS=1",
                path.display()
            )
        });
        assert_eq!(
            rendered, expected,
            "SVG output changed; re-bless with SILICOLAB_BLESS=1 if intended"
        );
    }

    #[test]
    fn golden_line_chart() {
        let svg = render_svg(
            &line_spec(),
            &PresetChoice::SingleColumn.preset(),
            &ExportStyle::default(),
        )
        .unwrap();
        assert_matches_fixture("line.svg", &svg);
    }

    #[test]
    fn golden_sticks_chart() {
        let svg = render_svg(
            &sticks_spec(),
            &PresetChoice::DoubleColumn.preset(),
            &ExportStyle::default(),
        )
        .unwrap();
        assert_matches_fixture("sticks.svg", &svg);
    }

    #[test]
    fn svg_declares_physical_size_font_and_editable_text() {
        let svg = render_svg(
            &line_spec(),
            &PresetChoice::SingleColumn.preset(),
            &ExportStyle::default(),
        )
        .unwrap();
        assert!(svg.contains(r#"width="3.3in""#));
        assert!(svg.contains(r#"height="2.5in""#));
        assert!(svg.contains("Liberation Sans"));
        assert!(svg.contains("<text"), "text must stay editable, not paths");
        assert!(svg.contains("Energy (Eh)"));
    }

    #[test]
    fn empty_or_nonfinite_data_is_an_error() {
        let mut spec = line_spec();
        spec.series.clear();
        let preset = PresetChoice::SingleColumn.preset();
        assert!(render_svg(&spec, &preset, &ExportStyle::default()).is_err());
        spec.series = vec![Series {
            name: "bad".to_string(),
            points: vec![[f64::NAN, f64::NAN]],
            mark: Mark::Line,
        }];
        assert!(render_svg(&spec, &preset, &ExportStyle::default()).is_err());
    }

    #[test]
    fn nonfinite_span_dataset_falls_to_the_placeholder_without_hanging() {
        // Finite endpoints whose difference overflows to +inf. Before the guard
        // this spun `ticks` forever on the synchronous export path.
        let mut spec = line_spec();
        spec.series = vec![Series {
            name: "extreme".to_string(),
            points: vec![[1.0, 9e307], [2.0, -9e307]],
            mark: Mark::Line,
        }];
        let result = render_svg(
            &spec,
            &PresetChoice::SingleColumn.preset(),
            &ExportStyle::default(),
        );
        assert!(result.is_err(), "non-finite span must not render/hang");
    }

    #[test]
    fn text_measure_tracks_glyph_widths() {
        let measure = TextMeasure::new().unwrap();
        let narrow = measure.width_pt("iii", 8.0);
        let wide = measure.width_pt("MMM", 8.0);
        assert!(narrow > 0.0 && wide > narrow);
        assert!(measure.width_pt("MMM", 16.0) > wide * 1.9);
    }
}
