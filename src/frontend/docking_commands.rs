//! High-level `dock` / `score` console commands: the scriptable mirror of the
//! Molecular Docking panel, shared by the GUI console and the CLI.
//!
//! * `dock  --receptor <entry> --ligand <entry> [options]` — full Vina search.
//! * `score --receptor <entry> --ligand <entry> [options]` — single-point score
//!   of the ligand's input pose (`--score_only`).
//!
//! An `<entry>` is `active`, a numeric id (`#2` or `2`), or an entry name. Both
//! run synchronously via [`crate::engines::docking`] (pure-Rust Vina), which suits
//! headless `.sls`/CLI use; the GUI drives the same engine on a worker thread so a
//! long search doesn't block rendering. Inputs are prepared heuristically — import
//! already-prepared `.pdbqt` for production-quality results.

use std::collections::BTreeMap;
use std::sync::{Arc, atomic::AtomicBool};

use anyhow::{Result, anyhow, bail};

use crate::{
    domain::Structure,
    engines::docking::{DockingConfig, DockingInput, DockingKind, DockingOutcome, DockingRequest},
    frontend::state::AppState,
    io::structure_paths::default_structure_save_path,
    workflows::docking::run_docking_calculation,
};

/// `dock ...` — run the full Monte-Carlo docking search.
pub fn dock_command(state: &mut AppState, args: &[String]) -> Result<String> {
    run_docking_command(state, DockingKind::Dock, args)
}

/// `score ...` — score the ligand's input pose without searching.
pub fn score_command(state: &mut AppState, args: &[String]) -> Result<String> {
    run_docking_command(state, DockingKind::ScoreOnly, args)
}

fn run_docking_command(state: &mut AppState, kind: DockingKind, args: &[String]) -> Result<String> {
    let request = assemble_request(state, kind, args)?;
    // Synchronous: a throwaway cancel flag and a no-op progress sink.
    let cancel = Arc::new(AtomicBool::new(false));
    let result = run_docking_calculation(request, cancel, |_| {})?;
    add_pose_entries(state, &result.outcome);
    Ok(result.outcome.summary)
}

/// Build a docking request for the agent's async (off-thread) `dock` tool, so the
/// GUI assistant runs the same code path a human types — mirrors
/// [`crate::frontend::qm_commands::build_agent_qm_request`].
pub fn build_agent_dock_request(state: &mut AppState, args: &[String]) -> Result<DockingRequest> {
    assemble_request(state, DockingKind::Dock, args)
}

/// Resolve the receptor/ligand entries and the search box from `--flags` into a
/// [`DockingRequest`]. Shared by the synchronous command and the agent tool.
fn assemble_request(
    state: &mut AppState,
    kind: DockingKind,
    args: &[String],
) -> Result<DockingRequest> {
    let flags = DockFlags::parse(args)?;
    let verb = match kind {
        DockingKind::Dock => "dock",
        DockingKind::ScoreOnly => "score",
    };
    let receptor_ref = flags
        .str("receptor")
        .ok_or_else(|| anyhow!("{verb} requires --receptor <entry>"))?;
    let ligand_ref = flags
        .str("ligand")
        .ok_or_else(|| anyhow!("{verb} requires --ligand <entry>"))?;
    let receptor_id = resolve_entry_id(state, receptor_ref)?;
    let ligand_id = resolve_entry_id(state, ligand_ref)?;
    if receptor_id == ligand_id {
        bail!("the receptor and ligand must be different entries");
    }
    state.ensure_entry_loaded(receptor_id);
    state.ensure_entry_loaded(ligand_id);
    let receptor = entry_structure(state, receptor_id, "receptor")?;
    let ligand = entry_structure(state, ligand_id, "ligand")?;

    // Default the box to the receptor centroid and a 22.5 Å cube.
    let box_center = match flags.str("center") {
        Some(spec) => parse_triple(spec, "center")?,
        None => {
            let center = receptor.center();
            [center.x as f64, center.y as f64, center.z as f64]
        }
    };
    let box_size = match flags.str("size") {
        Some(spec) => parse_triple(spec, "size")?,
        None => [22.5, 22.5, 22.5],
    };
    if box_size.iter().any(|&size| size <= 0.0) {
        bail!("--size must be positive on every axis");
    }

    let config = DockingConfig {
        exhaustiveness: flags.uint("exhaustiveness")?.unwrap_or(8).max(1) as usize,
        num_modes: flags.uint("modes")?.unwrap_or(9).max(1) as usize,
        seed: flags.uint("seed")?.unwrap_or(0),
    };
    Ok(DockingRequest {
        receptor: DockingInput::Structure(Box::new(receptor)),
        ligand: DockingInput::Structure(Box::new(ligand)),
        box_center,
        box_size,
        config,
        kind,
    })
}

/// Resolve `active`, `#id`/`id`, or an entry name to an entry id.
fn resolve_entry_id(state: &AppState, reference: &str) -> Result<u64> {
    if reference.eq_ignore_ascii_case("active") {
        return state
            .entries
            .active_entry_id()
            .ok_or_else(|| anyhow!("no active entry to use for `{reference}`"));
    }
    if let Some(id) = reference
        .strip_prefix('#')
        .unwrap_or(reference)
        .parse::<u64>()
        .ok()
        .filter(|id| state.entries.entry(*id).is_some())
    {
        return Ok(id);
    }
    state
        .entries
        .records
        .iter()
        .find(|record| record.name.eq_ignore_ascii_case(reference))
        .map(|record| record.id)
        .ok_or_else(|| {
            anyhow!(
                "no open entry matches `{reference}` (use active, an id like #2, or an entry name)"
            )
        })
}

fn entry_structure(state: &AppState, entry_id: u64, role: &str) -> Result<Structure> {
    let entry = state
        .entries
        .entry(entry_id)
        .ok_or_else(|| anyhow!("{role} entry #{entry_id} not found"))?;
    if entry.structure.atoms.is_empty() {
        bail!("{role} entry #{entry_id} has no atoms");
    }
    Ok(entry.structure.clone())
}

/// Parse `--center`/`--size`: three numbers separated by `,` or `x`.
fn parse_triple(spec: &str, what: &str) -> Result<[f64; 3]> {
    let nums = spec
        .split([',', 'x', 'X'])
        .map(|part| {
            part.trim()
                .parse::<f64>()
                .map_err(|_| anyhow!("--{what} expects three numbers like 10,10,10, got `{spec}`"))
        })
        .collect::<Result<Vec<f64>>>()?;
    match nums.as_slice() {
        [a, b, c] => Ok([*a, *b, *c]),
        _ => bail!("--{what} expects three numbers (e.g. 10,10,10), got `{spec}`"),
    }
}

/// Create one entry per pose under a "Docking poses" group, activating the best.
/// Shared by the synchronous command and the agent's async docking tool.
pub(crate) fn add_pose_entries(state: &mut AppState, outcome: &DockingOutcome) {
    if outcome.poses.is_empty() {
        return;
    }
    let group_id = state
        .entries
        .create_group("Docking poses")
        .unwrap_or_default();
    let mut best = None;
    for (rank, pose) in outcome.poses.iter().enumerate() {
        let structure = pose.structure.clone();
        let name = structure.title.clone();
        let save_path = default_structure_save_path(&structure, None);
        let entry_id = state.entries.add_entry_to_group(
            structure,
            None,
            save_path,
            group_id.clone(),
            Some(name),
            false,
        );
        if rank == 0 {
            best = Some(entry_id);
        }
    }
    if let Some(best_id) = best {
        state.show_entry(best_id);
    }
}

/// Minimal `--key value` parser (the docking commands take no boolean flags),
/// matching the `qm`/`md` command scanners.
struct DockFlags {
    values: BTreeMap<String, String>,
}

impl DockFlags {
    fn parse(args: &[String]) -> Result<Self> {
        let mut values = BTreeMap::new();
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
                bail!("--{key} expects a value");
            }
        }
        Ok(Self { values })
    }

    fn str(&self, key: &str) -> Option<&str> {
        self.values.get(key).map(String::as_str)
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

    use super::{dock_command, score_command};
    use crate::{
        domain::{Atom, Bond, BondType, Structure},
        frontend::state::AppState,
        io::structure_paths::default_structure_save_path,
    };

    fn butane() -> Structure {
        let atom = |x: f32, y: f32, z: f32| Atom {
            element: "C".to_string(),
            position: Point3::new(x, y, z),
            charge: 0.0,
        };
        Structure::with_bonds(
            "butane",
            vec![
                atom(0.0, 0.0, 0.0),
                atom(1.5, 0.0, 0.0),
                atom(2.2, 1.3, 0.0),
                atom(3.7, 1.3, 0.0),
            ],
            vec![
                Bond::with_type(0, 1, BondType::Single),
                Bond::with_type(1, 2, BondType::Single),
                Bond::with_type(2, 3, BondType::Single),
            ],
        )
    }

    fn add(state: &mut AppState) -> u64 {
        let structure = butane();
        let save_path = default_structure_save_path(&structure, None);
        state.entries.add_entry(structure, None, save_path)
    }

    #[test]
    fn score_command_reports_an_affinity() {
        let mut state = AppState::scratch(Default::default(), Vec::new());
        let receptor = add(&mut state);
        let ligand = add(&mut state);
        let out = score_command(
            &mut state,
            &[
                "--receptor".to_string(),
                receptor.to_string(),
                "--ligand".to_string(),
                ligand.to_string(),
                "--center".to_string(),
                "1.8,0.6,0.0".to_string(),
                "--size".to_string(),
                "20,20,20".to_string(),
            ],
        )
        .expect("score should succeed");
        assert!(out.contains("free energy"), "got: {out}");
    }

    #[test]
    fn dock_requires_distinct_entries() {
        let mut state = AppState::scratch(Default::default(), Vec::new());
        let entry = add(&mut state);
        let err = dock_command(
            &mut state,
            &[
                "--receptor".to_string(),
                entry.to_string(),
                "--ligand".to_string(),
                entry.to_string(),
            ],
        )
        .expect_err("same entry for both should be rejected");
        assert!(err.to_string().contains("different entries"), "got: {err}");
    }

    #[test]
    fn dock_creates_pose_entries() {
        let mut state = AppState::scratch(Default::default(), Vec::new());
        let receptor = add(&mut state);
        let ligand = add(&mut state);
        let out = dock_command(
            &mut state,
            &[
                "--receptor".to_string(),
                receptor.to_string(),
                "--ligand".to_string(),
                ligand.to_string(),
                "--center".to_string(),
                "1.8,0.6,0.0".to_string(),
                "--size".to_string(),
                "20,20,20".to_string(),
                // Keep the search tiny so the test stays fast.
                "--exhaustiveness".to_string(),
                "1".to_string(),
                "--modes".to_string(),
                "3".to_string(),
            ],
        )
        .expect("dock should succeed");
        assert!(out.contains("Docking complete"), "got: {out}");
        // The best pose is activated as a new entry, distinct from both inputs.
        let active = state
            .entries
            .active_entry_id()
            .expect("an active pose entry");
        assert_ne!(active, receptor);
        assert_ne!(active, ligand);
    }

    #[test]
    fn missing_receptor_is_reported() {
        let mut state = AppState::scratch(Default::default(), Vec::new());
        let err = dock_command(&mut state, &["--ligand".to_string(), "active".to_string()])
            .expect_err("missing --receptor should error");
        assert!(err.to_string().contains("--receptor"), "got: {err}");
    }
}
