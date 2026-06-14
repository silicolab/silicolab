//! Build script. Windows-only: embed the app icon into the executable so the
//! taskbar and Explorer show it. A no-op on every other target.

fn main() {
    // Gate on the *target*, not the host: `#[cfg(target_os = ...)]` in a build
    // script resolves against the machine running the script, which disagrees
    // with the target-gated `winresource` build-dependency and breaks
    // cross-compilation (the icon is silently skipped, or the crate is missing
    // entirely). `CARGO_CFG_TARGET_OS` reflects what we're building *for*.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        // Re-run (and re-embed) when the icon changes; without this Cargo only
        // tracks build.rs itself, so editing the .ico alone leaves the old icon
        // baked into the executable until a `cargo clean`.
        println!("cargo:rerun-if-changed=assets/icon/silicolab.ico");
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon/silicolab.ico");
        if let Err(error) = res.compile() {
            // `cargo:warning=` surfaces in build output; a plain `eprintln!` is
            // swallowed by Cargo.
            println!("cargo:warning=failed to embed Windows icon: {error}");
        }
    }
}
