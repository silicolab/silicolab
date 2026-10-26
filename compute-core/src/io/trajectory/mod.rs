//! Readers for molecular-dynamics trajectory files.
//!
//! These decode coordinate frames produced by external MD engines (kept in the
//! task run directory, not in the project database) into a
//! [`crate::domain::Trajectory`] for in-app playback.

pub mod xtc;
pub mod xyz;

use std::path::Path;

use anyhow::Result;

use crate::domain::Trajectory;

pub use xtc::read_xtc;
pub use xyz::read_xyz_trajectory;

/// Frame spacing assumed for an XYZ trajectory whose comments carry no time.
pub const XYZ_FALLBACK_DT_PS: f32 = 0.005;

/// Whether `path` names a trajectory format [`read_trajectory`] can decode.
pub fn is_trajectory_path(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|ext| ext.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("xtc" | "trj" | "xyz")
    )
}

/// Decode a trajectory by extension: GROMACS `.xtc`, or a multi-frame XYZ
/// (`.trj`, `.xyz`) as written by quantum and semi-empirical MD codes.
pub fn read_trajectory(path: &Path) -> Result<Trajectory> {
    match path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("trj" | "xyz") => read_xyz_trajectory(path, XYZ_FALLBACK_DT_PS),
        _ => read_xtc(path),
    }
}
