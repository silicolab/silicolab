//! Cross-platform, device-adaptive viewport navigation.
//!
//! Mouse vs. trackpad is distinguished by the *device*, not the OS: egui tags
//! every scroll event as [`MouseWheelUnit::Line`] (a real mouse wheel) or
//! [`MouseWheelUnit::Point`] (a precision-touchpad pixel scroll), so the same
//! code gives a Windows Precision Touchpad the same feel as a Mac trackpad. The
//! one genuine OS difference is the pinch gesture: winit only delivers a pinch
//! event (egui's [`Event::Zoom`]) on macOS, so on Windows/Linux the touchpad
//! zoom path is Ctrl + two-finger scroll instead.
//!
//! The per-OS knobs live in [`InputProfile`] (chosen by [`platform_profile`]);
//! [`route_events`] is the shared router used by both the 3D viewport and the 2D
//! sketcher.

use eframe::egui::{Event, MouseWheelUnit, Vec2};

/// Per-OS navigation tuning. Selected at compile time by [`platform_profile`].
#[derive(Clone, Copy, Debug)]
pub struct InputProfile {
    /// Read trackpad pinch ([`Event::Zoom`]) as zoom. Only macOS delivers it.
    pub pinch_zoom: bool,
    /// Gain on two-finger trackpad pan (camera-pan points per point scrolled).
    pub pan_speed: f32,
    /// Zoom gain per mouse-wheel notch ([`MouseWheelUnit::Line`]).
    pub wheel_zoom_speed: f32,
    /// Zoom gain per point of Ctrl/Cmd + trackpad scroll ([`MouseWheelUnit::Point`]).
    pub trackpad_zoom_speed: f32,
    /// Exponent applied to the raw pinch factor.
    pub pinch_zoom_speed: f32,
    /// Flip the horizontal pan direction.
    pub invert_pan_x: bool,
    /// Flip the vertical pan direction.
    pub invert_pan_y: bool,
}

/// The navigation profile for the OS this binary was built for.
pub const fn platform_profile() -> InputProfile {
    #[cfg(target_os = "macos")]
    {
        InputProfile {
            pinch_zoom: true,
            pan_speed: 1.0,
            wheel_zoom_speed: 0.1,
            trackpad_zoom_speed: 0.0015,
            pinch_zoom_speed: 1.0,
            invert_pan_x: false,
            invert_pan_y: false,
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        // Windows + Linux: winit delivers no pinch event, so Ctrl + two-finger
        // scroll is the touchpad zoom path. The two share a profile for now;
        // split this `cfg` if their feel ever needs to diverge.
        InputProfile {
            pinch_zoom: false,
            pan_speed: 1.0,
            wheel_zoom_speed: 0.1,
            trackpad_zoom_speed: 0.0015,
            pinch_zoom_speed: 1.0,
            invert_pan_x: false,
            invert_pan_y: false,
        }
    }
}

/// Navigation intent accumulated from one frame's wheel + pinch events.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScrollNav {
    /// Two-finger trackpad pan, in points (already gain- and sign-adjusted).
    pub pan: Vec2,
    /// Multiplicative zoom factor for the frame: `1.0` is no change, `> 1.0`
    /// zooms in, `< 1.0` zooms out.
    pub zoom: f32,
}

/// Route this frame's wheel/pinch events into pan + zoom according to `profile`.
///
/// Reads the *raw* [`Event::MouseWheel`] / [`Event::Zoom`] events (egui leaves
/// them in the event list after processing) so we own the mouse-vs-trackpad
/// split and avoid `InputState::zoom_delta`'s pinch + ctrl-scroll bundling.
///
/// Routing:
/// - [`Event::Zoom`] (pinch, macOS only) → zoom, when `profile.pinch_zoom`.
/// - [`MouseWheelUnit::Line`] / [`MouseWheelUnit::Page`] (mouse wheel) → zoom.
/// - [`MouseWheelUnit::Point`] with Ctrl/Cmd held → zoom (the Win/Linux pinch
///   substitute; also an extra zoom path on macOS).
/// - [`MouseWheelUnit::Point`] with no modifier (two-finger trackpad) → pan.
pub fn route_events(events: &[Event], profile: &InputProfile) -> ScrollNav {
    let mut pan = Vec2::ZERO;
    let mut zoom = 1.0_f32;

    for event in events {
        match event {
            Event::Zoom(factor) if profile.pinch_zoom => {
                // Trackpad pinch (macOS): `factor > 1` = spread = zoom in.
                zoom *= factor.powf(profile.pinch_zoom_speed);
            }
            Event::MouseWheel {
                unit,
                delta,
                modifiers,
                ..
            } => {
                let zoom_mod = modifiers.command || modifiers.ctrl;
                match unit {
                    // A real mouse wheel (or page key) → zoom. Positive `delta.y`
                    // zooms out, matching the previous wheel behavior.
                    MouseWheelUnit::Line | MouseWheelUnit::Page => {
                        zoom *= (-delta.y * profile.wheel_zoom_speed).exp();
                    }
                    MouseWheelUnit::Point => {
                        if zoom_mod {
                            // Ctrl/Cmd + two-finger scroll → zoom.
                            zoom *= (-delta.y * profile.trackpad_zoom_speed).exp();
                        } else {
                            // Plain two-finger trackpad scroll → pan.
                            let mut d = *delta * profile.pan_speed;
                            if profile.invert_pan_x {
                                d.x = -d.x;
                            }
                            if profile.invert_pan_y {
                                d.y = -d.y;
                            }
                            pan += d;
                        }
                    }
                }
            }
            _ => {}
        }
    }

    ScrollNav { pan, zoom }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::{Modifiers, TouchPhase, vec2};

    fn profile(pinch_zoom: bool) -> InputProfile {
        InputProfile {
            pinch_zoom,
            pan_speed: 1.0,
            wheel_zoom_speed: 0.1,
            trackpad_zoom_speed: 0.0015,
            pinch_zoom_speed: 1.0,
            invert_pan_x: false,
            invert_pan_y: false,
        }
    }

    fn wheel(unit: MouseWheelUnit, delta: Vec2, modifiers: Modifiers) -> Event {
        Event::MouseWheel {
            unit,
            delta,
            phase: TouchPhase::Move,
            modifiers,
        }
    }

    fn ctrl() -> Modifiers {
        Modifiers {
            ctrl: true,
            ..Modifiers::default()
        }
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-5
    }

    #[test]
    fn two_finger_trackpad_scroll_pans() {
        let events = [wheel(
            MouseWheelUnit::Point,
            vec2(7.0, -3.0),
            Modifiers::default(),
        )];
        let nav = route_events(&events, &profile(true));
        assert_eq!(nav.pan, vec2(7.0, -3.0));
        assert!(close(nav.zoom, 1.0), "trackpad pan must not zoom");
    }

    #[test]
    fn mouse_wheel_zooms_not_pans() {
        let events = [wheel(
            MouseWheelUnit::Line,
            vec2(0.0, 1.0),
            Modifiers::default(),
        )];
        let nav = route_events(&events, &profile(true));
        assert_eq!(nav.pan, Vec2::ZERO);
        // Positive delta.y → zoom out → factor < 1.
        assert!(close(nav.zoom, (-0.1_f32).exp()));
        assert!(nav.zoom < 1.0);
    }

    #[test]
    fn pinch_zooms_on_macos_profile_only() {
        let events = [Event::Zoom(1.2)];

        let mac = route_events(&events, &profile(true));
        assert!(
            close(mac.zoom, 1.2),
            "pinch should zoom when pinch_zoom is on"
        );
        assert_eq!(mac.pan, Vec2::ZERO);

        let win = route_events(&events, &profile(false));
        assert!(
            close(win.zoom, 1.0),
            "pinch must be ignored when pinch_zoom is off (Windows/Linux)"
        );
    }

    #[test]
    fn ctrl_trackpad_scroll_zooms_instead_of_panning() {
        let events = [wheel(MouseWheelUnit::Point, vec2(0.0, 10.0), ctrl())];
        // Works regardless of pinch availability (covers Windows/Linux).
        let nav = route_events(&events, &profile(false));
        assert_eq!(nav.pan, Vec2::ZERO, "ctrl+scroll is a zoom, not a pan");
        assert!(close(nav.zoom, (-10.0_f32 * 0.0015).exp()));
        assert!(nav.zoom < 1.0);
    }

    #[test]
    fn invert_pan_flags_flip_direction() {
        let mut p = profile(true);
        p.invert_pan_x = true;
        p.invert_pan_y = true;
        let events = [wheel(
            MouseWheelUnit::Point,
            vec2(5.0, -2.0),
            Modifiers::default(),
        )];
        let nav = route_events(&events, &p);
        assert_eq!(nav.pan, vec2(-5.0, 2.0));
    }

    #[test]
    fn accumulates_pan_and_zoom_across_events() {
        let events = [
            wheel(MouseWheelUnit::Point, vec2(2.0, 1.0), Modifiers::default()),
            wheel(MouseWheelUnit::Point, vec2(3.0, 4.0), Modifiers::default()),
            wheel(MouseWheelUnit::Line, vec2(0.0, -1.0), Modifiers::default()),
        ];
        let nav = route_events(&events, &profile(true));
        assert_eq!(nav.pan, vec2(5.0, 5.0));
        // One wheel notch up → zoom in.
        assert!(close(nav.zoom, (0.1_f32).exp()));
        assert!(nav.zoom > 1.0);
    }

    #[test]
    fn no_events_means_no_change() {
        let nav = route_events(&[], &profile(true));
        assert_eq!(nav.pan, Vec2::ZERO);
        assert!(close(nav.zoom, 1.0));
    }
}
