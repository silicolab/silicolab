//! Multi-frame XYZ trajectories, the format quantum and semi-empirical MD codes
//! (xtb's `xtb.trj`, ORCA, CP2K) write: one XYZ record per frame over a fixed
//! atom list, with the frame's energy and sometimes its time in the comment line.

use std::path::Path;

use anyhow::{Context, Result, bail};

use crate::domain::Trajectory;

/// Read a multi-frame XYZ file as a trajectory. Coordinates are Angstrom.
///
/// Each comment line is scanned for an energy (`energy: -60.75`, `E= -60.75`,
/// xtb's and ASE's spellings, Hartree) and a time (`t= 0.5`, `time= 0.5`, ps);
/// the first one also for the producing method (`method=GFN2-xTB`, or xtb's
/// `xtb: 6.7.1` version tag).
/// Frames without a time are spaced `fallback_dt_ps` apart, so playback still
/// has a monotonic clock.
pub fn read_xyz_trajectory(path: &Path, fallback_dt_ps: f32) -> Result<Trajectory> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read XYZ trajectory {}", path.display()))?;
    parse_xyz_trajectory(&text, fallback_dt_ps)
        .with_context(|| format!("failed to read XYZ trajectory {}", path.display()))
}

pub fn parse_xyz_trajectory(text: &str, fallback_dt_ps: f32) -> Result<Trajectory> {
    let lines: Vec<&str> = text.lines().collect();
    let mut natoms: Option<usize> = None;
    let mut elements: Vec<String> = Vec::new();
    let mut coords = Vec::new();
    let mut times = Vec::new();
    let mut energies = Vec::new();
    let mut method = None;
    let mut cursor = 0;

    while cursor < lines.len() {
        if lines[cursor].trim().is_empty() {
            cursor += 1;
            continue;
        }
        let count: usize = lines[cursor].trim().parse().with_context(|| {
            format!(
                "XYZ frame at line {} must begin with an atom count",
                cursor + 1
            )
        })?;
        match natoms {
            None => natoms = Some(count),
            Some(expected) if expected != count => bail!(
                "XYZ frame at line {} has {count} atoms; earlier frames have {expected}",
                cursor + 1
            ),
            Some(_) => {}
        }
        if cursor + 2 + count > lines.len() {
            bail!("XYZ frame at line {} is truncated", cursor + 1);
        }
        let frame = times.len();
        let comment = lines[cursor + 1];
        if frame == 0 {
            method = comment_method(comment);
        }
        times.push(
            comment_value(comment, &["t=", "time="]).unwrap_or(frame as f64 * fallback_dt_ps as f64)
                as f32,
        );
        if let Some(energy) = comment_value(comment, &["energy:", "energy=", "E="]) {
            energies.push(energy);
        }
        for (offset, line) in lines[cursor + 2..cursor + 2 + count].iter().enumerate() {
            let mut fields = line.split_whitespace();
            let symbol = fields.next().unwrap_or_default();
            let xyz: Vec<f32> = fields
                .take(3)
                .map(str::parse)
                .collect::<Result<_, _>>()
                .with_context(|| format!("bad coordinates on line {}", cursor + 3 + offset))?;
            if xyz.len() != 3 {
                bail!("line {} needs three coordinates", cursor + 3 + offset);
            }
            if frame == 0 {
                elements.push(symbol.to_string());
            } else if !elements[offset].eq_ignore_ascii_case(symbol) {
                bail!(
                    "atom {} changes element between frames ({} → {symbol})",
                    offset + 1,
                    elements[offset]
                );
            }
            coords.extend_from_slice(&xyz);
        }
        cursor += 2 + count;
    }

    let Some(natoms) = natoms else {
        bail!("the file contains no XYZ frames");
    };
    let frames = times.len();
    Ok(Trajectory::from_parts(natoms, coords, times, 1, frames)
        .with_energies(energies)
        .with_method(method))
}

fn comment_method(comment: &str) -> Option<String> {
    let mut tokens = comment.split_whitespace();
    while let Some(token) = tokens.next() {
        if let Some(name) = token.strip_prefix("method=") {
            return (!name.is_empty()).then(|| name.to_string());
        }
        if token == "xtb:" {
            return Some(
                format!("xtb {}", tokens.next().unwrap_or_default())
                    .trim()
                    .to_string(),
            );
        }
    }
    None
}

fn comment_value(comment: &str, keys: &[&str]) -> Option<f64> {
    let lower = comment.to_ascii_lowercase();
    keys.iter().find_map(|key| {
        let key = key.to_ascii_lowercase();
        let start = lower.find(&key)? + key.len();
        lower[start..].split_whitespace().next()?.parse().ok()
    })
}

#[cfg(test)]
mod tests {
    use super::parse_xyz_trajectory;

    #[test]
    fn reads_xtb_frames_with_energies() {
        let text = "2\n energy: -1.5 gnorm: 0.1 xtb: 6.7.1\nH 0 0 0\nH 0 0 0.74\n\
                    2\n energy: -1.4 gnorm: 0.2 xtb: 6.7.1\nH 0 0 0\nH 0 0 0.90\n";
        let trajectory = parse_xyz_trajectory(text, 0.005).expect("trajectory");
        assert_eq!(trajectory.frame_count(), 2);
        assert_eq!(trajectory.energy(1), Some(-1.4));
        assert!((trajectory.time(1) - 0.005).abs() < 1e-6);
        assert!((trajectory.position(1, 1).z - 0.90).abs() < 1e-6);
        assert_eq!(trajectory.method(), Some("xtb 6.7.1"));
    }

    #[test]
    fn prefers_recorded_times() {
        let text = "1\nt= 0.25 E= -3.0 method=GFN2-xTB\nHe 0 0 0\n1\nt= 0.50 E= -3.1\nHe 0 0 1\n";
        let trajectory = parse_xyz_trajectory(text, 1.0).expect("trajectory");
        assert_eq!(trajectory.method(), Some("GFN2-xTB"));
        assert!((trajectory.time(1) - 0.5).abs() < 1e-6);
        assert_eq!(trajectory.energies(), &[-3.0, -3.1]);
    }

    #[test]
    fn rejects_changing_atom_lists() {
        assert!(parse_xyz_trajectory("1\n\nH 0 0 0\n2\n\nH 0 0 0\nH 0 0 1\n", 1.0).is_err());
        assert!(parse_xyz_trajectory("1\n\nH 0 0 0\n1\n\nO 0 0 0\n", 1.0).is_err());
    }
}
