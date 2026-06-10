//! Apple-style frosted-glass (vibrancy) support.
//!
//! On macOS the real effect is an `NSVisualEffectView` placed *behind* the
//! window's content, installed once at startup with the `window-vibrancy` crate.
//! The window itself is made transparent (see [`crate::frontend::app`]'s
//! `window_viewport`), and the effect is *revealed* per frame by painting a
//! transparent clear color plus semi-transparent chrome fills (see
//! [`crate::frontend::theme::chrome_fill`]). When glass is off, the clear color
//! and chrome fills go opaque and fully cover the effect view — so the same
//! installed view can be toggled live, with no window recreation.
//!
//! Other platforms have no effect yet: [`supported`] gates the settings UI and
//! [`glass_active`] returns `false` there, so the app draws its normal opaque
//! chrome. Windows (Mica/Acrylic) and a translucent-fill fallback are planned.

/// Master switch for the real macOS vibrancy path.
///
/// `window-vibrancy` inserts its `NSVisualEffectView` as a *subview* of the
/// layer-backed view wgpu renders its `CAMetalLayer` into, so by default it
/// composites *over* the egui content and the window reads as blank. [`install`]
/// fixes this by re-parenting the effect view *behind* the Metal content (see
/// `reparent_effect_behind_content`), so the glass shows through the transparent
/// areas of the Metal layer instead of covering it.
const VIBRANCY_ENABLED: bool = true;

/// Whether this platform can show *some* translucent glass effect — gating the
/// settings toggle and whether [`glass_active`] may return true.
///
/// - **macOS**: a real `NSVisualEffectView` frost (vibrancy).
/// - **Windows**: an Acrylic backdrop blur (Win10 1803+/Win11).
/// - **Linux**: no portable compositor-blur API exists, so it falls back to a
///   plain translucent *tint* — the transparent window simply lets the desktop
///   show through the semi-transparent chrome fills (no blur).
pub const fn supported() -> bool {
    cfg!(any(
        target_os = "macos",
        target_os = "windows",
        target_os = "linux"
    )) && VIBRANCY_ENABLED
}

/// Whether the frosted glass should be *revealed* this frame: the user enabled
/// it, the platform supports it, and the OS "Reduce Transparency" accessibility
/// setting is off. Resolved once per frame (see `SilicoLabApp::ui`).
pub fn glass_active(enabled: bool) -> bool {
    enabled && supported() && !reduce_transparency()
}

/// macOS "Reduce Transparency" accessibility setting (always `false` elsewhere).
///
/// Resolved once per frame by [`glass_active`], so it must stay cheap. The
/// underlying value changes only when the user flips it in System Settings, so
/// the AppKit query is cached and refreshed at most a couple of times a second
/// (see `TTL`) rather than issued on every repaint — keeping the per-frame cost
/// effectively zero while still reflecting a change within half a second (no
/// restart needed).
#[cfg(target_os = "macos")]
pub fn reduce_transparency() -> bool {
    use std::cell::Cell;
    use std::time::{Duration, Instant};

    // Only ever touched from the main (UI) thread, so a thread-local Cell needs
    // no locking.
    thread_local! {
        static CACHE: Cell<Option<(Instant, bool)>> = const { Cell::new(None) };
    }
    const TTL: Duration = Duration::from_millis(500);

    if let Some((read_at, value)) = CACHE.with(Cell::get) {
        if read_at.elapsed() < TTL {
            return value;
        }
    }
    let value = query_reduce_transparency();
    CACHE.with(|cache| cache.set(Some((Instant::now(), value))));
    value
}

/// One uncached AppKit read of the Reduce Transparency setting. See the cache in
/// [`reduce_transparency`].
#[cfg(target_os = "macos")]
fn query_reduce_transparency() -> bool {
    use objc2::runtime::AnyObject;
    // SAFETY: NSWorkspace is part of AppKit (linked via window-vibrancy). We only
    // fetch the shared instance and read a BOOL property on it.
    unsafe {
        let workspace: *mut AnyObject =
            objc2::msg_send![objc2::class!(NSWorkspace), sharedWorkspace];
        if workspace.is_null() {
            return false;
        }
        objc2::msg_send![workspace, accessibilityDisplayShouldReduceTransparency]
    }
}

#[cfg(not(target_os = "macos"))]
pub fn reduce_transparency() -> bool {
    false
}

/// Keep the native appearance — and with it the vibrancy material — in step
/// with the app's theme preference.
///
/// AppKit resolves the `NSVisualEffectView` material from the window's
/// *effective appearance*, which follows the system unless overridden. With the
/// app forced Dark on a light-mode system the window frosts with the *light*
/// material, and the dark translucent chrome over a bright blur reads washed
/// out and wrong (the reverse mismatch looks just as bad). Forcing the
/// application-wide appearance keeps the glass matched to the UI theme;
/// `System` clears the override so AppKit follows the OS again. Must run on the
/// main thread (called from [`crate::frontend::theme::set_preference`]).
#[cfg(target_os = "macos")]
pub fn sync_appearance(mode: crate::backend::config::ThemeMode) {
    use crate::backend::config::ThemeMode;
    use objc2::runtime::AnyObject;
    use objc2_foundation::NSString;

    let appearance_name = match mode {
        ThemeMode::System => None,
        ThemeMode::Light => Some("NSAppearanceNameAqua"),
        ThemeMode::Dark => Some("NSAppearanceNameDarkAqua"),
    };
    // SAFETY: reads the shared NSApplication and assigns an NSAppearance (or nil
    // to clear the override); both are plain AppKit property accesses on the
    // main thread.
    unsafe {
        let app: *mut AnyObject = objc2::msg_send![objc2::class!(NSApplication), sharedApplication];
        if app.is_null() {
            return;
        }
        let appearance: *mut AnyObject = match appearance_name {
            Some(name) => {
                let name = NSString::from_str(name);
                objc2::msg_send![objc2::class!(NSAppearance), appearanceNamed: &*name]
            }
            None => core::ptr::null_mut(),
        };
        let _: () = objc2::msg_send![app, setAppearance: appearance];
    }
}

/// Install any one-time OS backdrop effect for the glass material, at startup.
/// Dispatched per platform (macOS vibrancy, Windows Acrylic, Linux none). Safe to
/// call regardless of the user's current preference: when glass is off, the
/// opaque clear color and chrome fills cover the effect entirely, so it costs
/// nothing visible. Must run on the main thread (called from eframe's creation
/// closure).
pub fn install(handle: impl raw_window_handle::HasWindowHandle) {
    #[cfg(target_os = "macos")]
    install_macos(handle);
    #[cfg(target_os = "windows")]
    install_windows(handle);
    // Linux: no portable backdrop-blur API. The translucent-tint fallback needs
    // no install — the transparent window and semi-transparent fills do the work.
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let _ = handle;
}

/// Windows Acrylic backdrop blur (Win10 1803+/Win11). On failure (older Windows)
/// this is a no-op and the app degrades to the plain translucent tint, since the
/// window is already transparent.
#[cfg(target_os = "windows")]
fn install_windows(handle: impl raw_window_handle::HasWindowHandle) {
    // Acrylic fills the window's full rectangle; without DWM rounding it shows
    // through the transparent corner cutouts as dark wedges.
    round_window_corners(&handle);
    // Near-transparent neutral tint; the app's chrome fills supply the actual
    // color, so Acrylic only contributes the blur.
    let _ = window_vibrancy::apply_acrylic(&handle, Some((18, 18, 20, 16)));
}

/// Ask DWM to clip the window to its rounded-corner shape. Fails silently on
/// Windows 10, which predates the attribute.
#[cfg(target_os = "windows")]
fn round_window_corners(handle: &impl raw_window_handle::HasWindowHandle) {
    use raw_window_handle::RawWindowHandle;

    #[link(name = "dwmapi")]
    unsafe extern "system" {
        fn DwmSetWindowAttribute(
            hwnd: *mut core::ffi::c_void,
            attribute: u32,
            value: *const core::ffi::c_void,
            size: u32,
        ) -> i32;
    }
    const DWMWA_WINDOW_CORNER_PREFERENCE: u32 = 33;
    const DWMWCP_ROUND: i32 = 2;

    let Ok(window_handle) = handle.window_handle() else {
        return;
    };
    let RawWindowHandle::Win32(win32) = window_handle.as_raw() else {
        return;
    };
    let hwnd = win32.hwnd.get() as *mut core::ffi::c_void;
    let preference: i32 = DWMWCP_ROUND;
    // SAFETY: `hwnd` is the live window; DWM only reads the 4-byte value.
    unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            (&raw const preference).cast(),
            size_of::<i32>() as u32,
        );
    }
}

/// macOS vibrancy: install an `NSVisualEffectView` behind the window content.
#[cfg(target_os = "macos")]
fn install_macos(handle: impl raw_window_handle::HasWindowHandle) {
    use window_vibrancy::{NSVisualEffectMaterial, NSVisualEffectState, apply_vibrancy};

    // `Sidebar` is the translucent material AppKit uses behind Finder/Mail source
    // lists — noticeably more see-through than the flatter `UnderWindowBackground`,
    // so the frosted blur actually reads through the chrome. `Active` keeps it
    // vibrant even when the window is not focused, so the glass doesn't flatten on
    // blur.
    let _ = apply_vibrancy(
        &handle,
        NSVisualEffectMaterial::Sidebar,
        Some(NSVisualEffectState::Active),
        None,
    );

    // window-vibrancy adds the NSVisualEffectView as a *subview* of the Metal
    // content view, so its layer is a sublayer that composites *above* the egui
    // content — the window reads as blank frosted desktop. Move it up to the
    // content view's parent, positioned *below* the content view, so the Metal
    // content draws on top and the glass only shows through its transparent areas.
    reparent_effect_behind_content(&handle);

    // egui-wgpu prefers a PreMultiplied composite-alpha mode on Metal, but
    // wgpu-hal only clears the CAMetalLayer's `opaque` flag for PostMultiplied —
    // so the layer can stay opaque and hide the effect view even when everything
    // else is configured. Force it non-opaque directly. (Harmless when glass is
    // off: an opaque clear color still paints fully opaque pixels over it.)
    set_metal_layer_opaque(&handle, false);
}

/// Move window-vibrancy's `NSVisualEffectView` out of the Metal content view and
/// into the content view's parent, positioned directly *below* it. As a subview
/// the effect view's layer composites above the Metal content (blank window); as a
/// sibling behind it, the Metal content draws on top and the glass shows through
/// its transparent pixels. See [`install`].
#[cfg(target_os = "macos")]
fn reparent_effect_behind_content(handle: &impl raw_window_handle::HasWindowHandle) {
    use objc2::runtime::AnyObject;
    use objc2_foundation::NSRect;
    use raw_window_handle::RawWindowHandle;

    let Ok(window_handle) = handle.window_handle() else {
        return;
    };
    let RawWindowHandle::AppKit(appkit) = window_handle.as_raw() else {
        return;
    };
    let content = appkit.ns_view.as_ptr() as *mut AnyObject;

    // SAFETY: `content` is the live content NSView. We read its parent and
    // subviews, find the NSVisualEffectView window-vibrancy just added, and
    // re-insert it into the parent below the content view. All on the main thread.
    unsafe {
        let superview: *mut AnyObject = objc2::msg_send![content, superview];
        if superview.is_null() {
            return;
        }
        let subviews: *mut AnyObject = objc2::msg_send![content, subviews];
        if subviews.is_null() {
            return;
        }
        let count: usize = objc2::msg_send![subviews, count];
        let effect_class = objc2::class!(NSVisualEffectView);
        let mut effect: *mut AnyObject = core::ptr::null_mut();
        for i in 0..count {
            let view: *mut AnyObject = objc2::msg_send![subviews, objectAtIndex: i];
            let is_effect: bool = objc2::msg_send![view, isKindOfClass: effect_class];
            if is_effect {
                effect = view;
                break;
            }
        }
        if effect.is_null() {
            return;
        }

        // Retain across the re-parent so removing it from the content view can't
        // drop the last strong reference and free it mid-move.
        let _: *mut AnyObject = objc2::msg_send![effect, retain];
        let _: () = objc2::msg_send![effect, removeFromSuperview];

        // Match the content view's frame (now interpreted in the parent's
        // coordinate space) and keep it pinned to that rect on resize.
        let frame: NSRect = objc2::msg_send![content, frame];
        let _: () = objc2::msg_send![effect, setFrame: frame];
        // NSViewWidthSizable | NSViewHeightSizable.
        let _: () = objc2::msg_send![effect, setAutoresizingMask: 18usize];

        // NSWindowBelow == -1: insert the effect view directly beneath the content
        // view within their shared parent.
        let below: isize = -1;
        let _: () =
            objc2::msg_send![superview, addSubview: effect, positioned: below, relativeTo: content];
        let _: () = objc2::msg_send![effect, release];
    }
}

/// Clear the CAMetalLayer's `opaque` flag so a transparent clear color actually
/// composites over the vibrancy view. See the note in [`install`].
#[cfg(target_os = "macos")]
fn set_metal_layer_opaque(handle: &impl raw_window_handle::HasWindowHandle, opaque: bool) {
    use objc2::runtime::AnyObject;
    use raw_window_handle::RawWindowHandle;

    let Ok(window_handle) = handle.window_handle() else {
        return;
    };
    let RawWindowHandle::AppKit(appkit) = window_handle.as_raw() else {
        return;
    };
    let ns_view = appkit.ns_view.as_ptr() as *mut AnyObject;
    // SAFETY: `ns_view` is the live, layer-backed NSView the window renders into;
    // `-layer` returns its CAMetalLayer, on which we set the `opaque` BOOL.
    unsafe {
        let layer: *mut AnyObject = objc2::msg_send![ns_view, layer];
        if !layer.is_null() {
            let _: () = objc2::msg_send![layer, setOpaque: opaque];
        }
    }
}
