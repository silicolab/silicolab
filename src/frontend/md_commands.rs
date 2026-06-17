//! High-level `md` console commands: the scriptable mirror of the GUI MD flow.
//!
//! The flow's steps and the commands match them:
//! * `md build`    — the MD System Builder: wrap the active structure in a
//!   simulation box (if it has none) and capture the engine-neutral
//!   [`MdTopology`] (species + nonbonded parameters) for the project.
//! * `md solvate`  — fill the box with water and ions (SilicoLab-native), replacing
//!   the structure with the solvated system and extending the captured topology.
//! * `md simulate` — run the fixed EM → NVT → NPT → production protocol (legacy
//!   physical-intent command).
//! * `md presets`  — list the preset library, marking the one recommended for the
//!   active system.
//! * `md run`      — the scriptable mirror of the GUI Run MD panel: pick a preset
//!   (recommended by default) for the inherited system context, apply overrides
//!   (`--temperature`, `--length`, `--set`, `--raw`, system-type toggles),
//!   validate, then realize and run the GROMACS pipeline (PME for biomolecular
//!   systems).
//!
//! `md simulate` surfaces only physical choices; `md run` additionally exposes the
//! preset library and tiered parameters. The commands run synchronously, which
//! suits headless `.sls`/CLI use; the GUI drives the same engine functions on a
//! worker thread.

use anyhow::{Result, bail};

use crate::frontend::state::AppState;

mod agent;
mod build;
mod parsing;
mod run;
mod simulate;
mod support;
mod task_runs;

pub use agent::*;
pub use build::*;
pub use parsing::*;
pub use run::*;
pub use simulate::*;
pub use support::*;
pub use task_runs::*;

#[cfg(test)]
mod tests;

/// Dispatch `md <subcommand> ...`.
pub fn md_command(state: &mut AppState, args: &[String]) -> Result<String> {
    let Some(sub) = args.first().map(String::as_str) else {
        bail!("usage: md <build|solvate|simulate|presets|run> [options]");
    };
    match sub {
        "build" => md_build(state, &args[1..]),
        "solvate" => md_solvate(state, &args[1..]),
        "simulate" => md_simulate(state, &args[1..]),
        "presets" => md_presets(state, &args[1..]),
        "run" => md_run(state, &args[1..]),
        other => bail!(
            "unknown md subcommand `{other}` (expected build, solvate, simulate, presets, or run)"
        ),
    }
}
