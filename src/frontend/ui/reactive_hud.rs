//! Heads-up overlay for reactive-trajectory playback: what produced the
//! trajectory (and how far to trust it), an energy / bond-distance strip synced
//! to the current frame, and a callout when a bond forms or breaks.

use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Stroke, StrokeKind, Vec2};

use crate::frontend::trajectory::TrajectoryPlayback;

const HARTREE_TO_KCAL: f64 = 627.509_474_063;
const CALLOUT_FRAMES: usize = 40;
const ENERGY_COLOR: Color32 = Color32::from_rgb(120, 170, 255);
const DISTANCE_COLOR: Color32 = Color32::from_rgb(255, 150, 70);
const FORMED_COLOR: Color32 = Color32::from_rgb(64, 214, 255);
const BROKEN_COLOR: Color32 = Color32::from_rgb(255, 120, 48);
const EDGE_MARGIN: f32 = 14.0;
/// Distance of the bond-event callout below the viewport's top edge (clears the
/// badge and the clock line under it).
const CALLOUT_TOP: f32 = 64.0;
const STRIP_WIDTH_FRACTION: f32 = 0.34;
const STRIP_MIN_WIDTH: f32 = 240.0;
const STRIP_MAX_WIDTH: f32 = 440.0;
const STRIP_HEIGHT: f32 = 112.0;

/// Draw the overlay over `rect` (the viewport). Does nothing for a classical,
/// fixed-topology trajectory.
pub(crate) fn draw_reactive_hud(ui: &egui::Ui, rect: Rect, playback: &TrajectoryPlayback) {
    let Some(reactive) = &playback.reactive else {
        return;
    };
    let painter = ui.painter_at(rect);
    let pal = crate::frontend::theme::palette(ui);
    let frame = playback.current_frame;
    let trajectory = &playback.trajectory;
    let elements: Vec<&str> = playback
        .scratch
        .atoms
        .iter()
        .map(|atom| atom.element.as_str())
        .collect();
    let pair_label = |a: usize, b: usize| {
        format!(
            "{}–{}",
            elements.get(a).unwrap_or(&"?"),
            elements.get(b).unwrap_or(&"?")
        )
    };

    let method = trajectory.method().unwrap_or("reactive trajectory");
    let semi_empirical = method.to_ascii_lowercase().contains("xtb");
    let badge = if semi_empirical {
        format!("{method} · semi-empirical MD · qualitative")
    } else {
        method.to_string()
    };
    let badge_pos = rect.left_top() + Vec2::new(EDGE_MARGIN, 12.0);
    let galley = painter.layout_no_wrap(badge, FontId::proportional(13.0), pal.text_strong);
    let badge_rect = Rect::from_min_size(badge_pos, galley.size() + Vec2::new(16.0, 8.0));
    painter.rect_filled(badge_rect, 6.0, pal.window_backing.gamma_multiply(0.85));
    painter.rect_stroke(
        badge_rect,
        6.0,
        Stroke::new(1.0_f32, pal.status_amber),
        StrokeKind::Inside,
    );
    painter.galley(badge_pos + Vec2::new(8.0, 4.0), galley, pal.text_strong);
    let clock = format!(
        "t = {:.3} ps   frame {}/{}",
        trajectory.time(frame),
        frame + 1,
        trajectory.frame_count()
    );
    painter.text(
        badge_rect.left_bottom() + Vec2::new(2.0, 6.0),
        Align2::LEFT_TOP,
        clock,
        FontId::monospace(12.0),
        pal.text_muted,
    );

    let callout = reactive
        .headlines()
        .iter()
        .rev()
        .find(|event| event.frame <= frame && frame - event.frame < CALLOUT_FRAMES);
    if let Some(event) = callout {
        let fade = 1.0 - (frame - event.frame) as f32 / CALLOUT_FRAMES as f32;
        let (verb, color) = if event.formed {
            ("bond formed", FORMED_COLOR)
        } else {
            ("bond broken", BROKEN_COLOR)
        };
        let text = format!("{} {verb}", pair_label(event.a, event.b));
        let galley =
            painter.layout_no_wrap(text, FontId::proportional(22.0), color.gamma_multiply(fade));
        let pos = Pos2::new(
            rect.center().x - galley.size().x / 2.0,
            rect.top() + CALLOUT_TOP,
        );
        let back = Rect::from_min_size(
            pos - Vec2::new(14.0, 6.0),
            galley.size() + Vec2::new(28.0, 12.0),
        );
        painter.rect_filled(back, 8.0, pal.window_backing.gamma_multiply(0.8 * fade));
        painter.galley(pos, galley, color.gamma_multiply(fade));
    }

    draw_strip(
        &painter,
        rect,
        playback,
        reactive.key_bond(),
        &pair_label,
        &pal,
    );
}

/// Energy (ΔE vs frame 0, kcal/mol) and the key bond's length over the run, with
/// a cursor at the current frame and markers where that bond broke or formed.
fn draw_strip(
    painter: &egui::Painter,
    rect: Rect,
    playback: &TrajectoryPlayback,
    key: Option<(usize, usize)>,
    pair_label: &dyn Fn(usize, usize) -> String,
    pal: &crate::frontend::theme::Palette,
) {
    let trajectory = &playback.trajectory;
    let frames = trajectory.frame_count();
    if frames < 2 {
        return;
    }
    let energies = trajectory.energies();
    let distances: Vec<f32> = key
        .map(|key| {
            (0..frames)
                .map(|frame| {
                    nalgebra::distance(
                        &trajectory.position(frame, key.0),
                        &trajectory.position(frame, key.1),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    if energies.is_empty() && distances.is_empty() {
        return;
    }

    let width = (rect.width() * STRIP_WIDTH_FRACTION).clamp(STRIP_MIN_WIDTH, STRIP_MAX_WIDTH);
    let strip = Rect::from_min_size(
        rect.left_bottom() + Vec2::new(EDGE_MARGIN, -(STRIP_HEIGHT + EDGE_MARGIN)),
        Vec2::new(width, STRIP_HEIGHT),
    );
    painter.rect_filled(strip, 8.0, pal.window_backing.gamma_multiply(0.85));
    painter.rect_stroke(
        strip,
        8.0,
        Stroke::new(1.0_f32, pal.hairline),
        StrokeKind::Inside,
    );
    let plot = Rect::from_min_max(
        strip.min + Vec2::new(10.0, 26.0),
        strip.max - Vec2::new(10.0, 18.0),
    );
    let x_at = |frame: usize| plot.left() + plot.width() * frame as f32 / (frames - 1) as f32;

    let mut legend = Vec::new();
    if !energies.is_empty() {
        let delta: Vec<f32> = energies
            .iter()
            .map(|e| ((e - energies[0]) * HARTREE_TO_KCAL) as f32)
            .collect();
        draw_series(painter, plot, &delta, ENERGY_COLOR, &x_at);
        legend.push((
            format!(
                "ΔE {:+.1} kcal/mol",
                delta[playback.current_frame.min(frames - 1)]
            ),
            ENERGY_COLOR,
        ));
    }
    if let Some(key) = key {
        draw_series(painter, plot, &distances, DISTANCE_COLOR, &x_at);
        legend.push((
            format!(
                "{} {:.2} Å",
                pair_label(key.0, key.1),
                distances[playback.current_frame.min(frames - 1)]
            ),
            DISTANCE_COLOR,
        ));
        if let Some(reactive) = &playback.reactive {
            for event in reactive
                .events()
                .iter()
                .filter(|event| (event.a, event.b) == key)
            {
                let x = x_at(event.frame);
                let color = if event.formed {
                    FORMED_COLOR
                } else {
                    BROKEN_COLOR
                };
                painter.line_segment(
                    [Pos2::new(x, plot.top()), Pos2::new(x, plot.bottom())],
                    Stroke::new(1.0_f32, color.gamma_multiply(0.7)),
                );
            }
        }
    }

    let mut x = strip.left() + 10.0;
    for (text, color) in legend {
        let galley = painter.layout_no_wrap(text, FontId::monospace(12.0), color);
        let advance = galley.size().x + 18.0;
        painter.galley(Pos2::new(x, strip.top() + 6.0), galley, color);
        x += advance;
    }
    let cursor = x_at(playback.current_frame.min(frames - 1));
    painter.line_segment(
        [
            Pos2::new(cursor, plot.top() - 2.0),
            Pos2::new(cursor, plot.bottom() + 2.0),
        ],
        Stroke::new(2.0_f32, pal.text_strong),
    );
    painter.text(
        Pos2::new(plot.left(), strip.bottom() - 4.0),
        Align2::LEFT_BOTTOM,
        format!("{:.2} ps", trajectory.time(0)),
        FontId::monospace(10.0),
        pal.text_muted,
    );
    painter.text(
        Pos2::new(plot.right(), strip.bottom() - 4.0),
        Align2::RIGHT_BOTTOM,
        format!("{:.2} ps", trajectory.time(frames - 1)),
        FontId::monospace(10.0),
        pal.text_muted,
    );
}

fn draw_series(
    painter: &egui::Painter,
    plot: Rect,
    values: &[f32],
    color: Color32,
    x_at: &dyn Fn(usize) -> f32,
) {
    let (min, max) = values
        .iter()
        .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), &v| {
            (lo.min(v), hi.max(v))
        });
    let span = (max - min).max(1e-6);
    let points: Vec<Pos2> = values
        .iter()
        .enumerate()
        .map(|(frame, &v)| {
            Pos2::new(
                x_at(frame),
                plot.bottom() - plot.height() * (v - min) / span,
            )
        })
        .collect();
    painter.add(egui::Shape::line(points, Stroke::new(1.5_f32, color)));
}
