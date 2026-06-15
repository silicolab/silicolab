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
        CpcmDielectric, KMesh, PeriodicFunctional, PeriodicQmRequest, QmDispersion, QmJob, QmKind,
        QmMethod, QmOptions, QmRequest, QmScfBackend, QmSolvation, periodic,
    },
    frontend::state::AppState,
    io::structure_paths::default_structure_save_path,
    workflows::qm::run_qm_calculation,
};

/// Dispatch `qm <subcommand> ...`.
pub fn qm_command(state: &mut AppState, args: &[String]) -> Result<String> {
    let Some(sub) = args.first().map(String::as_str) else {
        bail!(
            "usage: qm <energy|optimize|freq|periodic> [options]\n\
             \n\
             core:     --method <m>   hf|rhf|uhf|rohf|mp2|ccsd|ccsd(t), a functional \
             (pbe, b3lyp, r2scan, wb97m-v, b2plyp, …) with an optional -d3/-d4 suffix, \
             or a composite (r2scan-3c, b97-3c, pbeh-3c, b3lyp-3c)\n\
             \x20         --basis <name>   e.g. def2-svp, cc-pvtz (ignored for composites)\n\
             \x20         --charge <int>   --spin <2S+1>   --properties\n\
             dispersion: --dispersion d3|d4   (or use a -d3/-d4 method suffix)\n\
             solvation:  --solvent <name> | --eps <ε> | --smd <name> | --alpb <name> | --gbsa <name>\n\
             backend:    --direct | --ri | --cosx   --ri-mp2   --x2c   --all-electron   --grid <0..4>\n\
             advanced:   --smear <K>   --fod   --sph   --symmetry-number <int>   --qrrho-w0 <cm-1>\n\
             periodic:   qm periodic (needs a unit cell)  --functional pade|lda  --basis <gth-set>\n\
             \x20         --kmesh <n|nxnxn>   --cutoff <Ry>   --max-iter <n>   --forces   --stress"
        );
    };
    // `qm recommend <task>` prints chemx's recommended level of theory for a
    // task and needs no active structure.
    if sub == "recommend" {
        return qm_recommend(&args[1..]);
    }
    // `qm periodic` runs a periodic (crystalline) single point on the active
    // unit cell — a distinct option set from the molecular subcommands.
    if sub == "periodic" {
        return run_periodic_command(state, &args[1..]);
    }
    let kind = match sub {
        "energy" | "sp" | "single-point" => QmKind::SinglePoint,
        "optimize" | "opt" => QmKind::Optimize,
        "freq" | "frequencies" => QmKind::Frequencies,
        other => {
            bail!(
                "unknown qm subcommand `{other}` \
                 (expected energy, optimize, freq, periodic, or recommend)"
            )
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

/// Assemble a [`QmRequest`] from the active structure and parsed `--flags`.
/// Shared by the synchronous `run` and the agent's async `run_qm` tool so the two
/// build the exact same request.
fn assemble_qm_request(state: &AppState, kind: QmKind, args: &[String]) -> Result<QmRequest> {
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
    Ok(QmRequest {
        structure: state.structure().clone(),
        method,
        basis,
        charge,
        multiplicity,
        kind,
        options,
    })
}

/// Map a `qm <subcommand>` line (subcommand + flags) to a [`QmRequest`] for the
/// agent's async tool. Mirrors [`qm_command`]'s subcommand dispatch.
pub fn build_agent_qm_request(state: &AppState, args: &[String]) -> Result<QmRequest> {
    let Some(sub) = args.first().map(String::as_str) else {
        bail!("usage: qm <energy|optimize|freq> [options]");
    };
    let kind = match sub {
        "energy" | "sp" | "single-point" => QmKind::SinglePoint,
        "optimize" | "opt" => QmKind::Optimize,
        "freq" | "frequencies" => QmKind::Frequencies,
        other => bail!("unknown qm subcommand `{other}` (expected energy, optimize, or freq)"),
    };
    assemble_qm_request(state, kind, &args[1..])
}

fn run(state: &mut AppState, kind: QmKind, args: &[String]) -> Result<String> {
    let request = assemble_qm_request(state, kind, args)?;

    // Synchronous: a throwaway cancel flag and a no-op progress sink.
    let cancel = Arc::new(AtomicBool::new(false));
    let result = run_qm_calculation(QmJob::Molecular(request), cancel, |_| {})?;
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

/// `qm periodic [options]`: a periodic (crystalline) single point on the active
/// unit cell. Runs synchronously, like the molecular subcommands; periodic v1
/// has no geometry relaxation, so it never creates a new entry.
fn run_periodic_command(state: &mut AppState, args: &[String]) -> Result<String> {
    let request = assemble_periodic_request(state, args)?;
    let cancel = Arc::new(AtomicBool::new(false));
    let result = run_qm_calculation(QmJob::Periodic(request), cancel, |_| {})?;
    Ok(result.outcome.summary)
}

/// Assemble a [`PeriodicQmRequest`] from the active structure and `--flags`.
fn assemble_periodic_request(state: &AppState, args: &[String]) -> Result<PeriodicQmRequest> {
    if state.structure().atoms.is_empty() {
        bail!("no active structure; open a crystal before `qm periodic`");
    }
    let flags = QmFlags::parse(args)?;
    let functional = flags
        .str("functional")
        .map(PeriodicFunctional::parse)
        .transpose()?
        .unwrap_or_default();
    let basis = flags
        .str("basis")
        .unwrap_or(periodic::DEFAULT_PERIODIC_BASIS)
        .to_string();
    let kmesh = parse_kmesh(&flags)?;
    let e_cut_ry = flags.float("cutoff")?.unwrap_or(periodic::DEFAULT_E_CUT_RY);
    let max_iter = flags
        .uint("max-iter")?
        .unwrap_or(periodic::DEFAULT_MAX_ITER);
    Ok(PeriodicQmRequest {
        structure: state.structure().clone(),
        functional,
        basis,
        kmesh,
        e_cut_ry,
        max_iter,
        forces: flags.flag("forces"),
        stress: flags.flag("stress"),
    })
}

/// Parse `--kmesh`: a single integer `n` (uniform `n×n×n`) or three separated by
/// `x` or `,` (e.g. `4x4x2`). Absent means the Γ point.
fn parse_kmesh(flags: &QmFlags) -> Result<KMesh> {
    let Some(spec) = flags.str("kmesh") else {
        return Ok(KMesh::gamma());
    };
    let nums = spec
        .split(['x', 'X', ','])
        .map(|p| {
            p.trim()
                .parse::<u32>()
                .map_err(|_| anyhow!("--kmesh expects integers like 4 or 4x4x4, got `{spec}`"))
        })
        .collect::<Result<Vec<u32>>>()?;
    let divisions = match nums.as_slice() {
        [n] => [*n, *n, *n],
        [a, b, c] => [*a, *b, *c],
        _ => bail!("--kmesh expects one or three integers (e.g. 4 or 4x4x4), got `{spec}`"),
    };
    if divisions.contains(&0) {
        bail!("--kmesh divisions must each be ≥ 1");
    }
    Ok(KMesh { divisions })
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
    fn build_agent_qm_request_maps_subcommands() {
        let mut state = AppState::scratch(Default::default(), Vec::new());
        let save_path = default_structure_save_path(&water(), None);
        state.entries.add_entry(water(), None, save_path);

        let request = super::build_agent_qm_request(
            &state,
            &[
                "optimize".to_string(),
                "--basis".to_string(),
                "sto-3g".to_string(),
            ],
        )
        .expect("agent qm request should build");
        assert!(matches!(request.kind, super::QmKind::Optimize));
        assert_eq!(request.basis, "sto-3g");
        // Unknown subcommand is rejected.
        assert!(super::build_agent_qm_request(&state, &["bogus".to_string()]).is_err());
    }

    #[test]
    fn agent_qm_request_runs_off_thread() {
        use crate::frontend::jobs::{QmWorkerMessage, spawn_qm_job};
        use std::time::{Duration, Instant};

        let mut state = AppState::scratch(Default::default(), Vec::new());
        let save_path = default_structure_save_path(&water(), None);
        state.entries.add_entry(water(), None, save_path);

        let request = super::build_agent_qm_request(
            &state,
            &[
                "energy".to_string(),
                "--method".to_string(),
                "rhf".to_string(),
                "--basis".to_string(),
                "sto-3g".to_string(),
            ],
        )
        .expect("request builds");

        // Spawn the same job the agent's heavy path uses and poll it to
        // completion, exactly as `poll_heavy_qm` does (minus the agent loop).
        let job = spawn_qm_job(crate::engines::qm::QmJob::Molecular(request));
        let deadline = Instant::now() + Duration::from_secs(120);
        let mut summary = None;
        while Instant::now() < deadline {
            match job.receiver.try_recv() {
                Ok(QmWorkerMessage::Finished(outcome)) => {
                    summary = Some(outcome.summary);
                    break;
                }
                Ok(QmWorkerMessage::Failed(error)) => panic!("qm job failed: {error}"),
                Ok(QmWorkerMessage::Progress { .. }) => {}
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => break,
            }
        }
        assert!(
            summary.is_some(),
            "async qm job should finish with a summary"
        );
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
