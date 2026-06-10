//! Orchestration of GROMACS analysis tools.
//!
//! `gmx energy` reads thermodynamic terms from a run's `.edr` and writes them as
//! a plain `.xvg` (via `-xvg none`) that [`super::xvg`] parses directly. The
//! term selection is driven on stdin (mirroring the tutorial's `printf "...\n" |`
//! idiom).

use std::{
    path::{Path, PathBuf},
    sync::{Arc, atomic::AtomicBool},
    time::Duration,
};

use anyhow::Result;

use crate::engines::{
    gromacs::{
        analysis::{Xvg, energy::energy_term_selection, xvg::parse_xvg},
        exec::run_gmx,
        runner::{GromacsProgress, subprocess_failure},
    },
    registry::EngineLaunch,
};

/// Shared context for an analysis invocation.
#[derive(Debug, Clone)]
pub struct AnalysisContext {
    pub working_dir: PathBuf,
    pub gmx_launch: EngineLaunch,
    pub max_duration: Duration,
}

fn name(path: &Path, fallback: &str) -> String {
    path.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(fallback)
        .to_string()
}

/// `gmx energy` argument vector (pure).
pub fn build_energy_args(edr: &str, output_xvg: &str) -> Vec<String> {
    vec![
        "energy".to_string(),
        "-f".to_string(),
        edr.to_string(),
        "-o".to_string(),
        output_xvg.to_string(),
        "-xvg".to_string(),
        "none".to_string(),
    ]
}

/// Run `gmx energy`, extracting the named terms (e.g. "Temperature",
/// "Potential") into a parsed [`Xvg`].
pub fn gmx_energy<F>(
    ctx: &AnalysisContext,
    edr: &Path,
    output_name: &str,
    terms: &[&str],
    cancel: Arc<AtomicBool>,
    mut report: F,
) -> Result<Xvg>
where
    F: FnMut(GromacsProgress),
{
    let outcome = run_gmx(
        &ctx.gmx_launch,
        &ctx.working_dir,
        build_energy_args(&name(edr, "ener.edr"), output_name),
        Some(energy_term_selection(terms)),
        ctx.max_duration,
        cancel,
        &mut report,
    )?;
    if !outcome.success() {
        return Err(subprocess_failure("energy", &outcome));
    }
    read_xvg(&ctx.working_dir, output_name)
}

fn read_xvg(dir: &Path, name: &str) -> Result<Xvg> {
    let path = dir.join(name);
    let text = std::fs::read_to_string(&path)
        .map_err(|e| anyhow::anyhow!("failed to read {}: {e}", path.display()))?;
    parse_xvg(&text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn energy_args_request_plain_xvg() {
        let args = build_energy_args("md.edr", "energy.xvg");
        let joined = args.join(" ");
        assert!(joined.contains("energy -f md.edr -o energy.xvg"));
        assert!(joined.contains("-xvg none"));
    }
}
