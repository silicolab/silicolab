//! Downloading and applying a newer SilicoLab build in place.
//!
//! Where [`update_check`](crate::io::update_check) only *detects* a newer
//! GitHub release, this module *performs* the update: it downloads the release
//! asset matching the running platform, replaces the current executable (via
//! the `self_update` crate, which handles the Windows "can't overwrite a
//! running exe" case through an atomic self-replace), and offers a restart into
//! the freshly installed binary.
//!
//! The actual download/replace is blocking and must run off the UI thread; the
//! frontend drives it from a worker thread and reports progress through the
//! usual job channel.

use anyhow::{Context, Result};

/// Repository the released binaries are published under. Kept in sync with the
/// API endpoint in [`update_check`](crate::io::update_check).
const REPO_OWNER: &str = "silicolab";
const REPO_NAME: &str = "silicolab";

/// Name of the SilicoLab executable *inside* a release archive. On Windows the
/// archive carries `silicolab.exe`; elsewhere it is the bare binary. `self_update`
/// uses this to locate the binary after extracting the `.zip`/`.tar.gz`.
#[cfg(target_os = "windows")]
const ARCHIVE_BIN_NAME: &str = "silicolab.exe";
#[cfg(not(target_os = "windows"))]
const ARCHIVE_BIN_NAME: &str = "silicolab";

/// Download the latest release asset for this platform and replace the running
/// executable with it. Returns the version that was installed.
///
/// Blocking: performs network I/O and filesystem replacement. Call from a
/// worker thread. A successful return means the new binary is on disk in place
/// of the old one; the process is still running the *old* code until it is
/// restarted (see [`restart_into_new_binary`]).
pub fn perform_update() -> Result<String> {
    let status = self_update::backends::github::Update::configure()
        .repo_owner(REPO_OWNER)
        .repo_name(REPO_NAME)
        .bin_name(ARCHIVE_BIN_NAME)
        .current_version(env!("CARGO_PKG_VERSION"))
        // The GUI owns the confirmation step and draws its own progress, so the
        // crate's interactive prompt and CLI progress bar are both suppressed.
        .no_confirm(true)
        .show_download_progress(false)
        .build()
        .context("failed to configure the updater")?
        .update()
        .context("failed to download and apply the update")?;

    Ok(status.version().to_string())
}

/// Relaunch the application from the (now replaced) executable and exit the
/// current process. Only call after [`perform_update`] reports success.
///
/// Never returns on success. On Windows the running image has already been
/// swapped out by the self-replace, so the path resolved here points at the new
/// binary.
pub fn restart_into_new_binary() -> Result<std::convert::Infallible> {
    let exe = std::env::current_exe().context("could not locate the current executable")?;
    std::process::Command::new(&exe)
        .spawn()
        .with_context(|| format!("failed to relaunch {}", exe.display()))?;
    std::process::exit(0);
}

/// Whether an in-place self-update can plausibly succeed for this install.
///
/// Fails closed: package-manager, portable, or read-only installs (where the
/// executable's directory is not writable) report `false`, and the frontend
/// falls back to pointing the user at the releases page instead of offering a
/// one-click update that would only error out.
pub fn is_self_update_supported() -> bool {
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let Some(dir) = exe.parent() else {
        return false;
    };
    is_dir_writable(dir)
}

/// Best-effort writability probe: try to create (and immediately remove) a
/// uniquely named temp file in `dir`. A read-only or permission-restricted
/// directory makes this fail, which is exactly the case we want to detect.
fn is_dir_writable(dir: &std::path::Path) -> bool {
    let probe = dir.join(format!(".silicolab-write-probe-{}", std::process::id()));
    match std::fs::File::create(&probe) {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}
