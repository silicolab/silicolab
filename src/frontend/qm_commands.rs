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
    engines::qm::{
        CpcmDielectric, QmDispersion, QmKind, QmMethod, QmOptions, QmRequest, QmScfBackend,
        QmSolvation,
    },
    frontend::state::AppState,
    io::structure_paths::default_structure_save_path,
    workflows::qm::run_qm_calculation,
};

/// Dispatch `qm <subcommand> ...`.
pub fn qm_command(state: &mut AppState, args: &[String]) -> Result<String> {
    let Some(sub) = args.first().map(String::as_str) else {
        bail!(
            "usage: qm <energy|optimize|freq> [options]\n\
             \n\
             core:     --method <m>   hf|rhf|uhf|rohf|mp2|ccsd|ccsd(t), a functional \
             (pbe, b3lyp, r2scan, wb97m-v, b2plyp, …) with an optional -d3/-d4 suffix, \
             or a composite (r2scan-3c, b97-3c, pbeh-3c, b3lyp-3c)\n\
             \x20         --basis <name>   e.g. def2-svp, cc-pvtz (ignored for composites)\n\
             \x20         --charge <int>   --spin <2S+1>   --properties\n\
             dispersion: --dispersion d3|d4   (or use a -d3/-d4 method suffix)\n\
             solvation:  --solvent <name> | --eps <ε> | --smd <name> | --alpb <name> | --gbsa <name>\n\
             backend:    --direct | --ri | --cosx   --ri-mp2   --x2c   --all-electron   --grid <0..4>\n\
             advanced:   --smear <K>   --fod   --sph   --symmetry-number <int>   --qrrho-w0 <cm-1>"
        );
    };
    // `qm recommend <task>` prints chemx's recommended level of theory for a
    // task and needs no active structure.
    if sub == "recommend" {
        return qm_recommend(&args[1..]);
    }
    let kind = match sub {
        "energy" | "sp" | "single-point" => QmKind::SinglePoint,
        "optimize" | "opt" => QmKind::Optimize,
        "freq" | "frequencies" => QmKind::Frequencies,
        other => {
            bail!("unknown qm subcommand `{other}` (expected energy, optimize, freq, or recommend)")
        }
    };
    run(state, kind, &args[1..])
}

/// `qm recommend <task>`: chemx's data-driven level-of-theory guidance.
fn qm_recommend(args: &[String]) -> Result<String> {
    let tasks = chemx::guardrails::recommendation_tasks().join(", ");
    let Some(task) = args.first() else {
        bail!("usage: qm recommend <task>  (available: {tasks})");
    };
    let rec = chemx::guardrails::recommend(task)
        .ok_or_else(|| anyhow!("unknown task `{task}` (available: {tasks})"))?;
    let mut out = format!("recommended level of theory for {}:\n", rec.task);
    out.push_str(&format!("  level:     {}\n", rec.level));
    out.push_str(&format!("  rationale: {}\n", rec.rationale));
    if !rec.invocation.is_empty() {
        out.push_str("  run:\n");
        for inv in rec.invocation {
            out.push_str(&format!("    {inv}\n"));
        }
    }
    for note in rec.notes {
        out.push_str(&format!("  note: {note}\n"));
    }
    Ok(out.trim_end().to_string())
}

fn run(state: &mut AppState, kind: QmKind, args: &[String]) -> Result<String> {
    if state.structure().atoms.is_empty() {
        bail!("no active structure; open one before `qm`");
    }
    let flags = QmFlags::parse(args)?;
    let (method, suffix_dispersion) = flags
        .str("method")
        .map(QmMethod::parse)
        .unwrap_or_else(|| (QmMethod::Dft("b3lyp".to_string()), None));
    let basis = flags.str("basis").unwrap_or("def2-svp").to_string();
    let charge = flags.int("charge")?.unwrap_or(0);
    let multiplicity = flags.uint("spin")?.or(flags.uint("mult")?).unwrap_or(1);

    let options = build_options(&flags, suffix_dispersion)?;

    let request = QmRequest {
        structure: state.structure().clone(),
        method,
        basis,
        charge,
        multiplicity,
        kind,
        options,
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

    fn float(&self, key: &str) -> Result<Option<f64>> {
        self.values
            .get(key)
            .map(|v| {
                v.parse::<f64>()
                    .map_err(|_| anyhow!("--{key} must be a number, got `{v}`"))
            })
            .transpose()
    }
}

/// Assemble the advanced [`QmOptions`] from parsed flags. `suffix_dispersion`
/// is the `-d3`/`-d4` correction split off the `--method` keyword, if any.
fn build_options(flags: &QmFlags, suffix_dispersion: Option<QmDispersion>) -> Result<QmOptions> {
    // Dispersion: a `-d3`/`-d4` method suffix or an explicit `--dispersion`,
    // not both.
    let flag_dispersion = flags
        .str("dispersion")
        .map(|v| match v.to_ascii_lowercase().as_str() {
            "d3" | "d3bj" | "d3(bj)" => Ok(QmDispersion::D3Bj),
            "d4" => Ok(QmDispersion::D4),
            other => Err(anyhow!("--dispersion must be d3 or d4, got `{other}`")),
        })
        .transpose()?;
    let dispersion = match (suffix_dispersion, flag_dispersion) {
        (Some(_), Some(_)) => {
            bail!("specify dispersion once: a -d3/-d4 method suffix or --dispersion, not both")
        }
        (a, b) => a.or(b),
    };

    // Solvation: at most one model.
    let solvation = build_solvation(flags)?;

    // SCF backend: at most one of --direct / --ri / --cosx.
    let backend = [
        (flags.flag("direct"), QmScfBackend::Direct),
        (flags.flag("ri"), QmScfBackend::RiJk),
        (flags.flag("cosx"), QmScfBackend::Cosx),
    ];
    let chosen: Vec<QmScfBackend> = backend
        .iter()
        .filter(|(on, _)| *on)
        .map(|(_, b)| *b)
        .collect();
    if chosen.len() > 1 {
        bail!("choose at most one SCF backend: --direct, --ri, or --cosx");
    }
    let scf_backend = chosen.first().copied().unwrap_or_default();

    let grid_level = flags
        .uint("grid")?
        .map(|g| {
            if g > 4 {
                Err(anyhow!("--grid must be 0..=4, got {g}"))
            } else {
                Ok(g as usize)
            }
        })
        .transpose()?;

    let mut options = QmOptions {
        compute_properties: flags.flag("properties") || flags.flag("props"),
        dispersion,
        solvation,
        scf_backend,
        ri_mp2: flags.flag("ri-mp2"),
        x2c: flags.flag("x2c"),
        all_electron: flags.flag("all-electron"),
        grid_level,
        smearing_temperature_k: flags.float("smear")?,
        fod: flags.flag("fod"),
        single_point_hessian: flags.flag("sph"),
        symmetry_number: flags.uint("symmetry-number")?.unwrap_or(1),
        ..QmOptions::default()
    };
    if let Some(w0) = flags.float("qrrho-w0")? {
        options.qrrho_w0_cm1 = w0;
    }
    Ok(options)
}

/// Resolve the (at most one) solvation flag into a [`QmSolvation`].
fn build_solvation(flags: &QmFlags) -> Result<Option<QmSolvation>> {
    let candidates = [
        flags
            .str("solvent")
            .map(|n| QmSolvation::Cpcm(CpcmDielectric::Named(n.to_string()))),
        flags
            .float("eps")?
            .map(|e| QmSolvation::Cpcm(CpcmDielectric::Epsilon(e))),
        flags.str("smd").map(|n| QmSolvation::Smd(n.to_string())),
        flags.str("alpb").map(|n| QmSolvation::Alpb(n.to_string())),
        flags.str("gbsa").map(|n| QmSolvation::Gbsa(n.to_string())),
    ];
    let mut chosen = candidates.into_iter().flatten();
    let first = chosen.next();
    if chosen.next().is_some() {
        bail!("choose at most one solvation model: --solvent, --eps, --smd, --alpb, or --gbsa");
    }
    Ok(first)
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
    fn qm_recommend_reports_a_level_of_theory() {
        let mut state = AppState::scratch(Default::default(), Vec::new());
        // `recommend` needs no structure and should name a level and a run line.
        let out = qm_command(
            &mut state,
            &["recommend".to_string(), "general".to_string()],
        )
        .expect("qm recommend general should succeed");
        assert!(
            out.contains("level:"),
            "recommendation should name a level: {out}"
        );
        // An unknown task lists the available ones rather than panicking.
        assert!(
            qm_command(
                &mut state,
                &["recommend".to_string(), "nonsense".to_string()]
            )
            .is_err()
        );
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
