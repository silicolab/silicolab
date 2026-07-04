use super::spec::Series;

/// Smallest 1/2/5 × 10ⁿ step at least `span / target`, so the range splits
/// into at most `target` intervals.
pub fn nice_step(span: f64, target: usize) -> f64 {
    // A ±f64::MAX-scale span overflows `max - min` to +inf, and a sub-5e-323
    // span underflows `base` to 0; either makes `raw.log10()` non-finite and
    // the result inf/0, which drives the `ticks` loop into a NaN spin. Fall
    // back to a finite unit step so callers stay bounded.
    if !span.is_finite() || span <= 0.0 {
        return 1.0;
    }
    let raw = span / target.max(1) as f64;
    let base = 10f64.powi(raw.log10().floor() as i32);
    let normalized = raw / base;
    let factor = if normalized <= 1.0 {
        1.0
    } else if normalized <= 2.0 {
        2.0
    } else if normalized <= 5.0 {
        5.0
    } else {
        10.0
    };
    factor * base
}

/// Round tick positions covering `[min, max]`; empty when the range is not a
/// forward finite interval.
pub fn ticks(min: f64, max: f64, target: usize) -> Vec<f64> {
    let span = max - min;
    // `max - min` can overflow to +inf even when both endpoints are finite
    // (±f64::MAX scale); guard the span itself, not just the endpoints, or the
    // loop below spins on a NaN break test and grows the Vec without bound.
    if !min.is_finite() || !max.is_finite() || !span.is_finite() || span <= 0.0 {
        return Vec::new();
    }
    let step = nice_step(span, target);
    if !step.is_finite() || step <= 0.0 {
        return Vec::new();
    }
    let first = (min / step).ceil() * step;
    let mut out = Vec::new();
    let mut index = 0u32;
    loop {
        let value = first + step * f64::from(index);
        if value > max + step * 1e-9 {
            break;
        }
        // `first` is computed by division, so an intended zero can come out as
        // ±1e-17; snap it.
        out.push(if value.abs() < step * 1e-9 {
            0.0
        } else {
            value
        });
        index += 1;
    }
    out
}

/// Finite data extent as `[x_range, y_range]`, each padded if degenerate.
/// `None` when no series has a fully finite point.
pub fn data_bounds(series: &[Series]) -> Option<[[f64; 2]; 2]> {
    let mut x = [f64::INFINITY, f64::NEG_INFINITY];
    let mut y = [f64::INFINITY, f64::NEG_INFINITY];
    for s in series {
        for point in &s.points {
            if point[0].is_finite() && point[1].is_finite() {
                x[0] = x[0].min(point[0]);
                x[1] = x[1].max(point[0]);
                y[0] = y[0].min(point[1]);
                y[1] = y[1].max(point[1]);
            }
        }
    }
    if x[0] > x[1] {
        return None;
    }
    Some([pad_degenerate(x), pad_degenerate(y)])
}

/// Widen a zero-width range so a single-point dataset still gets a drawable
/// axis; forward ranges pass through unchanged.
pub fn pad_degenerate(range: [f64; 2]) -> [f64; 2] {
    if range[0] < range[1] {
        return range;
    }
    let pad = if range[0] == 0.0 {
        1.0
    } else {
        range[0].abs() * 0.05
    };
    [range[0] - pad, range[1] + pad]
}

/// Extend an auto-fit range to include the zero baseline. Stick charts draw
/// each peak from `y = 0`, so a data-fit range like `[0.2, 0.9]` would clip the
/// shortest stick to zero length and make every height encode intensity minus
/// the floor. A no-op once 0 is already inside the range.
pub fn extend_to_zero(range: [f64; 2]) -> [f64; 2] {
    [range[0].min(0.0), range[1].max(0.0)]
}

/// Tick label with just enough decimals for `step`; scientific notation for
/// extreme magnitudes.
pub fn format_tick(value: f64, step: f64) -> String {
    if value == 0.0 {
        return "0".to_string();
    }
    let magnitude = value.abs();
    if !(1e-4..1e5).contains(&magnitude) {
        return format!("{value:.2e}");
    }
    // A non-finite or non-positive step makes `-step.log10()` inf/NaN, which
    // `as usize` saturates to usize::MAX decimals (an OOM-scale format string);
    // fall back to integer precision.
    let decimals = if !step.is_finite() || step >= 1.0 || step <= 0.0 {
        0
    } else {
        (-step.log10()).ceil().max(0.0) as usize
    };
    format!("{value:.decimals$}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plot::spec::Mark;

    fn series(points: Vec<[f64; 2]>) -> Series {
        Series {
            name: "s".to_string(),
            points,
            mark: Mark::Line,
        }
    }

    #[test]
    fn nice_step_picks_1_2_5_multiples() {
        assert_eq!(nice_step(10.0, 5), 2.0);
        assert_eq!(nice_step(1.0, 5), 0.2);
        assert_eq!(nice_step(0.07, 5), 0.02);
        assert_eq!(nice_step(1_000_000.0, 5), 200_000.0);
        assert_eq!(nice_step(3.0, 5), 1.0);
    }

    #[test]
    fn ticks_cover_the_range_at_round_values() {
        assert_eq!(ticks(0.0, 10.0, 5), vec![0.0, 2.0, 4.0, 6.0, 8.0, 10.0]);
        let energy = ticks(-74.97, -74.90, 5);
        assert!(energy.len() >= 3);
        assert!(energy.iter().all(|v| (-74.97..=-74.90).contains(v)));
    }

    #[test]
    fn ticks_on_degenerate_or_nonfinite_ranges_are_empty() {
        assert!(ticks(1.0, 1.0, 5).is_empty());
        assert!(ticks(2.0, 1.0, 5).is_empty());
        assert!(ticks(f64::NAN, 1.0, 5).is_empty());
    }

    #[test]
    fn data_bounds_span_all_series_and_skip_nonfinite_points() {
        let bounds = data_bounds(&[
            series(vec![[1.0, -2.0], [2.0, f64::NAN], [3.0, 5.0]]),
            series(vec![[0.0, 1.0], [f64::INFINITY, 9.0]]),
        ])
        .unwrap();
        assert_eq!(bounds[0], [0.0, 3.0]);
        assert_eq!(bounds[1], [-2.0, 5.0]);
    }

    #[test]
    fn data_bounds_pad_single_points_and_reject_empty_input() {
        assert!(data_bounds(&[]).is_none());
        assert!(data_bounds(&[series(vec![[f64::NAN, f64::NAN]])]).is_none());
        let bounds = data_bounds(&[series(vec![[2.0, -74.9]])]).unwrap();
        assert!(bounds[0][0] < 2.0 && bounds[0][1] > 2.0);
        assert!(bounds[1][0] < -74.9 && bounds[1][1] > -74.9);
    }

    #[test]
    fn pad_degenerate_widens_zero_width_ranges_only() {
        assert_eq!(pad_degenerate([1.0, 2.0]), [1.0, 2.0]);
        let padded = pad_degenerate([0.0, 0.0]);
        assert!(padded[0] < 0.0 && padded[1] > 0.0);
    }

    #[test]
    fn extreme_finite_span_stays_bounded_and_finite() {
        // The two endpoints are finite, but their difference overflows to +inf.
        let bounds = data_bounds(&[series(vec![[1.0, 9e307], [2.0, -9e307]])]).unwrap();
        let [lo, hi] = bounds[1];
        assert!((hi - lo).is_infinite(), "precondition: span overflows");
        let out = ticks(lo, hi, 5);
        assert!(out.len() < 100, "must not spin the tick loop");
        assert!(out.iter().all(|v| v.is_finite()));
        let step = nice_step(hi - lo, 5);
        assert!(step.is_finite() && step > 0.0);
    }

    #[test]
    fn subnormal_span_stays_bounded() {
        // Two nearly-equal subnormals: `nice_step`'s base underflows to 0.
        let lo = f64::from_bits(1);
        let hi = f64::from_bits(3);
        assert!(hi > lo && (hi - lo) > 0.0);
        let out = ticks(lo, hi, 5);
        assert!(out.len() < 100);
        assert!(out.iter().all(|v| v.is_finite()));
    }

    #[test]
    fn extend_to_zero_pulls_the_sticks_range_to_the_baseline() {
        assert_eq!(extend_to_zero([0.2, 0.9]), [0.0, 0.9]);
        assert_eq!(extend_to_zero([-0.5, -0.1]), [-0.5, 0.0]);
        // Already spanning zero: unchanged.
        assert_eq!(extend_to_zero([-0.3, 0.4]), [-0.3, 0.4]);
    }

    #[test]
    fn format_tick_tolerates_nonpositive_or_nonfinite_step() {
        // step 0 would otherwise ask for usize::MAX decimals.
        assert_eq!(format_tick(3.5, 0.0), "4");
        assert_eq!(format_tick(3.5, f64::INFINITY), "4");
        assert_eq!(format_tick(3.5, -1.0), "4");
    }

    #[test]
    fn format_tick_matches_the_step_precision() {
        assert_eq!(format_tick(4.0, 2.0), "4");
        assert_eq!(format_tick(-74.96, 0.02), "-74.96");
        assert_eq!(format_tick(0.0, 0.5), "0");
        assert_eq!(format_tick(250_000.0, 50_000.0), "2.50e5");
        assert_eq!(format_tick(0.00002, 0.00001), "2.00e-5");
    }
}
