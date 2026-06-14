//! Build script. Windows-only: embed the app icon into the executable so the
//! taskbar and Explorer show it. A no-op on every other platform.

fn main() {
    #[cfg(target_os = "windows")]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon/silicolab.ico");
        if let Err(error) = res.compile() {
            eprintln!("warning: failed to embed Windows icon: {error}");
        }
    }
}
