//! High-level `qm` console commands: the scriptable mirror of the GUI QM panel.
//!
//! * `qm energy`   — single-point energy at the current geometry.
//! * `qm optimize` — relax the geometry on the QM surface; the relaxed structure
//!   is added as a new entry (the original is preserved).
//! * `qm freq`     — harmonic vibrational frequencies at the current geometry.
//!
//! All run synchronously via [`crate::engines::qm`] (pure-Rust chemx), which
//! suits headless `.sls`/CLI use and small molecules; the GUI drives the same
//! engine on a worker thread so long calculations don't block rendering.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, atomic::AtomicBool};

use anyhow::{Result, anyhow, bail};

use crate::{
    engines::qm::{QmKind, QmMethod, QmRequest},
    frontend::state::AppState,
    io::structure_paths::default_structure_save_path,
    workflows::qm::run_qm_calculation,
};

/// Dispatch `qm <subcommand> ...`.
pub fn qm_command(state: &mut AppState, args: &[String]) -> Result<String> {
    let Some(sub) = args.first().map(String::as_str) else {
        bail!(
            "usage: qm <energy|optimize|freq> [--method <m>] [--basis <name>] \
             [--charge <int>] [--spin <2S+1>] [--properties]"
        );
    };
    let kind = match sub {
        "energy" | "sp" | "single-point" => QmKind::SinglePoint,
        "optimize" | "opt" => QmKind::Optimize,
        "freq" | "frequencies" => QmKind::Frequencies,
        other => bail!("unknown qm subcommand `{other}` (expected energy, optimize, or freq)"),
    };
    run(state, kind, &args[1..])
}

fn run(state: &mut AppState, kind: QmKind, args: &[String]) -> Result<String> {
    if state.structure().atoms.is_empty() {
        bail!("no active structure; open one before `qm`");
    }
    let flags = QmFlags::parse(args)?;
    let method = flags
        .str("method")
        .map(QmMethod::parse)
        .unwrap_or_else(|| QmMethod::Dft("b3lyp".to_string()));
    let basis = flags.str("basis").unwrap_or("def2-svp").to_string();
    let charge = flags.int("charge")?.unwrap_or(0);
    let multiplicity = flags.uint("spin")?.or(flags.uint("mult")?).unwrap_or(1);
    let compute_properties = flags.flag("properties");

    let request = QmRequest {
        structure: state.structure().clone(),
        method,
        basis,
        charge,
        multiplicity,
        kind,
        compute_properties,
    };

    // Synchronous: a throwaway cancel flag and a no-op progress sink.
    let cancel = Arc::new(AtomicBool::new(false));
    let result = run_qm_calculation(request, cancel, |_| {})?;
    let outcome = result.outcome;

    // A QM run is a heavy calculation; surface its optimized geometry as a new
    // entry (the original is preserved), matching the GUI task and MD commands.
    if let Some(optimized) = outcome.optimized_structure {
        let save_path = default_structure_save_path(&optimized, None);
        let entry_id = state.entries.add_entry(optimized, None, save_path);
        state.show_entry(entry_id);
    }

    Ok(outcome.summary)
}

/// Minimal `--key value` / `--flag` parser for the `qm` command.
struct QmFlags {
    values: BTreeMap<String, String>,
    flags: BTreeSet<String>,
}

impl QmFlags {
    fn parse(args: &[String]) -> Result<Self> {
        let mut values = BTreeMap::new();
        let mut flags = BTreeSet::new();
        let mut i = 0;
        while i < args.len() {
            let arg = &args[i];
            let Some(key) = arg.strip_prefix("--") else {
                bail!("unexpected argument `{arg}` (expected --key value)");
            };
            if let Some((k, v)) = key.split_once('=') {
                values.insert(k.to_string(), v.to_string());
                i += 1;
            } else if i + 1 < args.len() && !args[i + 1].starts_with("--") {
                values.insert(key.to_string(), args[i + 1].clone());
                i += 2;
            } else {
                flags.insert(key.to_string());
                i += 1;
            }
        }
        Ok(Self { values, flags })
    }

    fn flag(&self, key: &str) -> bool {
        self.flags.contains(key)
    }

    fn str(&self, key: &str) -> Option<&str> {
        self.values.get(key).map(String::as_str)
    }

    fn int(&self, key: &str) -> Result<Option<i32>> {
        self.values
            .get(key)
            .map(|v| {
                v.parse::<i32>()
                    .map_err(|_| anyhow!("--{key} must be an integer, got `{v}`"))
            })
            .transpose()
    }

    fn uint(&self, key: &str) -> Result<Option<u32>> {
        self.values
            .get(key)
            .map(|v| {
                v.parse::<u32>()
                    .map_err(|_| anyhow!("--{key} must be a positive integer, got `{v}`"))
            })
            .transpose()
    }
}

#[cfg(test)]
mod tests {
    use nalgebra::Point3;

    use super::qm_command;
    use crate::{
        domain::{Atom, Structure},
        frontend::state::AppState,
        io::structure_paths::default_structure_save_path,
    };

    fn water() -> Structure {
        Structure::new(
            "water",
            vec![
                Atom {
                    element: "O".to_string(),
                    position: Point3::new(0.0, 0.0, 0.1173),
                    charge: 0.0,
                },
                Atom {
                    element: "H".to_string(),
                    position: Point3::new(0.0, 0.7572, -0.4692),
                    charge: 0.0,
                },
                Atom {
                    element: "H".to_string(),
                    position: Point3::new(0.0, -0.7572, -0.4692),
                    charge: 0.0,
                },
            ],
        )
    }

    #[test]
    fn qm_optimize_creates_new_entry() {
        let mut state = AppState::scratch(Default::default(), Vec::new());
        let structure = water();
        let save_path = default_structure_save_path(&structure, None);
        let original = state.entries.add_entry(structure, None, save_path);

        let summary = qm_command(
            &mut state,
            &[
                "optimize".to_string(),
                "--method".to_string(),
                "rhf".to_string(),
                "--basis".to_string(),
                "sto-3g".to_string(),
            ],
        )
        .expect("qm optimize should succeed");

        // A heavy QM run produces a *new* entry; the original is preserved.
        assert_ne!(
            Some(original),
            state.entries.active_entry_id(),
            "optimize should create and activate a new entry, not edit in place"
        );
        assert!(
            state.structure().title.contains("opt"),
            "new entry title should mark the optimization: {}",
            state.structure().title
        );
        assert!(summary.contains("geometry optimization"));
    }

    #[test]
    fn qm_energy_does_not_create_entry() {
        let mut state = AppState::scratch(Default::default(), Vec::new());
        let structure = water();
        let save_path = default_structure_save_path(&structure, None);
        let original = state.entries.add_entry(structure, None, save_path);

        qm_command(
            &mut state,
            &[
                "energy".to_string(),
                "--basis".to_string(),
                "sto-3g".to_string(),
            ],
        )
        .expect("qm energy should succeed");

        // A single point changes nothing in the entry list.
        assert_eq!(Some(original), state.entries.active_entry_id());
    }
}
