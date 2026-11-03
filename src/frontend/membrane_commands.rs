//! `membrane diffusion` / `membrane free-volume`: transport analysis of a dense
//! phase from an entry and its MD trajectory. Both run in the foreground: the
//! MSD is a few seconds even for thousands of frames, and the grid insertion
//! is bounded by the cell size.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};

use crate::{
    domain::Trajectory,
    frontend::{
        console::{DiffusionArgs, FreeVolumeArgs, ScriptContext},
        entry_ref::{entry_structure, resolve_entry_id},
        state::AppState,
    },
    io::trajectory::{XYZ_FALLBACK_DT_PS, read_trajectory},
    md::analysis::{
        FreeVolumeSpec, MsdSpec, free_volume, free_volume_over_frames, mean_squared_displacement,
        molecules_of, msd_csv, select_atoms,
    },
};

pub fn diffusion_command(
    state: &mut AppState,
    context: &ScriptContext,
    args: DiffusionArgs,
) -> Result<String> {
    let entry_id = resolve_entry_id(state, &args.entry)?;
    let structure = entry_structure(state, entry_id, "membrane diffusion")?;
    let path = trajectory_path(state, entry_id, args.trajectory.as_deref())?;
    let trajectory = load_trajectory(&path)?;
    let atoms = select_atoms(&structure, &args.select)?;
    let molecules = molecules_of(&structure, &atoms);
    let fit_window = parse_fit(&args.fit)?;
    let max_lag_frames = match args.max_lag {
        Some(lag_ps) => Some(frames_for_lag(&trajectory, lag_ps)?),
        None => None,
    };
    let report = mean_squared_displacement(
        &trajectory,
        &structure,
        &MsdSpec {
            molecules,
            max_lag_frames,
            fit_window,
        },
    )?;
    let mut summary = format!(
        "D = {:.3e} cm²/s for {} molecule{} (`{}`) over {} frames of {}; MSD exponent {:.2} on \
         {:.0}–{:.0} ps",
        report.diffusion_cm2_s,
        report.molecules,
        if report.molecules == 1 { "" } else { "s" },
        args.select,
        report.frames,
        path.display(),
        report.exponent,
        report.fit_range_ps.0,
        report.fit_range_ps.1
    );
    if report.exponent < 0.8 {
        summary.push_str(" — sub-diffusive: the run is too short for a Fickian estimate");
    }
    if xyz_without_times(&path, &trajectory) {
        summary.push_str(&format!(
            " — the XYZ comments record no times, so frames were taken as {XYZ_FALLBACK_DT_PS} ps \
             apart and D is only as right as that spacing"
        ));
    }
    if !report.unwrapped {
        summary.push_str(" — no box found, so wrapped coordinates were used as is");
    }
    if let Some(out) = &args.out {
        let out = context.output_path(state, out)?;
        std::fs::write(&out, msd_csv(&report))
            .with_context(|| format!("could not write {}", out.display()))?;
        summary.push_str(&format!("; MSD curve written to {}", out.display()));
    }
    Ok(summary)
}

pub fn free_volume_command(state: &mut AppState, args: FreeVolumeArgs) -> Result<String> {
    let entry_id = resolve_entry_id(state, &args.entry)?;
    let structure = entry_structure(state, entry_id, "membrane free-volume")?;
    let spec = FreeVolumeSpec {
        probe_radius: args.probe,
        grid_spacing: args.grid,
    };
    if args.probe < 0.0 {
        bail!("the probe radius cannot be negative");
    }
    let (report, source) = match (args.trajectory.as_deref(), args.frames) {
        (None, None) => (free_volume(&structure, spec)?, String::new()),
        (trajectory, frames) => {
            let path = trajectory_path(state, entry_id, trajectory)?;
            let trajectory = load_trajectory(&path)?;
            let count = frames.unwrap_or(10).max(1).min(trajectory.frame_count());
            let first = trajectory.frame_count() - count;
            let frames: Vec<usize> = (first..trajectory.frame_count()).collect();
            (
                free_volume_over_frames(&structure, &trajectory, &frames, spec)?,
                format!(" over the last {count} frames of {}", path.display()),
            )
        }
    };
    Ok(format!(
        "free volume {:.1}% of {:.0} Å³ ({:.0} Å³) with a {:.1} Å probe on a {:.2} Å grid{source}; \
         density {:.3} g/cm³",
        report.fraction * 100.0,
        report.cell_volume_a3,
        report.free_volume_a3,
        report.probe_radius,
        args.grid,
        report.density_g_cm3
    ))
}

/// The trajectory to analyse: an explicit path (relative to the project root,
/// else the working directory), or the entry's recorded MD trajectory.
fn trajectory_path(state: &AppState, entry_id: u64, explicit: Option<&Path>) -> Result<PathBuf> {
    let project_root = state
        .workspace
        .project()
        .map(|project| project.root.clone());
    let relative = match explicit {
        Some(path) => path.to_path_buf(),
        None => {
            let entry = state
                .entries
                .entry(entry_id)
                .ok_or_else(|| anyhow!("entry #{entry_id} not found"))?;
            entry
                .origin
                .trajectory()
                .map(Path::to_path_buf)
                .ok_or_else(|| {
                    anyhow!(
                        "entry #{entry_id} has no recorded MD trajectory; pass the trajectory \
                         path explicitly"
                    )
                })?
        }
    };
    if relative.is_absolute() {
        return Ok(relative);
    }
    match project_root {
        Some(root) if root.join(&relative).exists() => Ok(root.join(relative)),
        _ => Ok(relative),
    }
}

fn load_trajectory(path: &Path) -> Result<Trajectory> {
    if !path.exists() {
        bail!("trajectory file {} does not exist", path.display());
    }
    let trajectory = read_trajectory(path)?;
    if trajectory.is_empty() {
        bail!("trajectory {} has no frames", path.display());
    }
    Ok(trajectory)
}

/// An XYZ reader fills missing times at the fallback spacing; that spacing is
/// rarely the real one for an MD run, so the caller has to say so.
fn xyz_without_times(path: &Path, trajectory: &Trajectory) -> bool {
    let is_xyz = path
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("xyz"));
    is_xyz
        && (0..trajectory.frame_count()).all(|frame| {
            (trajectory.time(frame) as f64 - frame as f64 * XYZ_FALLBACK_DT_PS as f64).abs()
                < 1.0e-4
        })
}

fn parse_fit(text: &str) -> Result<(f64, f64)> {
    let parse = |part: &str| {
        part.trim()
            .parse::<f64>()
            .map_err(|_| anyhow!("--fit expects two fractions like `0.2,0.8`, got `{text}`"))
    };
    match text.split_once(',') {
        Some((from, to)) => Ok((parse(from)?, parse(to)?)),
        None => bail!("--fit expects two fractions like `0.2,0.8`, got `{text}`"),
    }
}

fn frames_for_lag(trajectory: &Trajectory, lag_ps: f64) -> Result<usize> {
    let frames = trajectory.frame_count();
    let span = trajectory.time(frames - 1) as f64 - trajectory.time(0) as f64;
    if lag_ps <= 0.0 {
        bail!("--max-lag must be positive");
    }
    if frames < 2 || span <= 0.0 {
        return Ok(frames / 2);
    }
    let dt = span / (frames - 1) as f64;
    Ok(((lag_ps / dt).round() as usize).clamp(2, frames - 1))
}
