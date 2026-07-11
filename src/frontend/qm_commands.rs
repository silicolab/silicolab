//! High-level `qm` console commands: the scriptable mirror of the GUI QM panel.
//!
//! * `qm energy`   — single-point energy at the current geometry.
//! * `qm optimize` — relax the geometry on the QM surface; the relaxed structure
//!   is added as a new entry (the original is preserved).
//! * `qm freq`     — harmonic vibrational frequencies at the current geometry.
//! * `qm ts`       — climb to a first-order saddle point (transition state); the
//!   saddle structure is added as a new entry. The guess starts from the current
//!   geometry, a reactant→product pair (`--product`), or a driven coordinate
//!   (`--scan-bond`/`--scan-angle`/`--scan-dihedral`).
//!
//! Commands run synchronously for headless `.sls`/CLI use; the GUI drives the
//! same selected backend on a worker thread so long calculations do not block
//! rendering.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, atomic::AtomicBool};

use anyhow::{Result, anyhow, bail};

use crate::{
    engines::qm::{
        CpcmDielectric, KMesh, PeriodicFunctional, PeriodicQmRequest, QmCalculation, QmDispersion,
        QmEngine, QmInternalCoordinate, QmJob, QmKind, QmMethod, QmOptions, QmRequest,
        QmScfBackend, QmSolvation, QmTsAlgorithm, QmTsConfig, QmTsCoordinateScan, QmTsCoordinates,
        QmTsEndpoints, QmTsGuess, periodic,
    },
    frontend::state::AppState,
    io::structure_paths::default_structure_save_path,
    workflows::qm::run_qm_calculation,
};

/// Dispatch `qm <subcommand> ...`.
pub fn qm_command(state: &mut AppState, args: &[String]) -> Result<String> {
    let Some(sub) = args.first().map(String::as_str) else {
        bail!(
            "usage: qm <energy|optimize|freq|ts|periodic> [options]\n\
             \n\
             engine:   --engine hartree|orca   (default: hartree; ORCA path must be configured)\n\
             core:     --method <m>   hf|rhf|uhf|rohf|mp2|ccsd|ccsd(t), a functional \
             (pbe, b3lyp, r2scan, wb97m-v, b2plyp, …) with an optional -d3/-d4 suffix, \
             or a composite (r2scan-3c, b97-3c, pbeh-3c, b3lyp-3c)\n\
             \x20         --basis <name>   e.g. def2-svp, cc-pvtz (ignored for composites)\n\
             \x20         --charge <int>   --spin <2S+1>   --properties\n\
             dispersion: --dispersion d3|d4   (or use a -d3/-d4 method suffix)\n\
             solvation:  --solvent <name> | --eps <ε> | --smd <name> | --alpb <name> | --gbsa <name>\n\
             backend:    --direct | --ri | --cosx   --ri-mp2   --x2c   --all-electron   --grid <0..4>\n\
             advanced:   --smear <K>   --fod   --sph   --symmetry-number <int>   --qrrho-w0 <cm-1>\n\
             transition state (qm ts): --ts-algo prfo|dimer   --ts-coords mass-weighted|internal   --irc\n\
             \x20         guess: (default) climb from the current geometry;\n\
             \x20                --product <entry> [--neb --neb-images <n> | --idpp-scan <n>] [--no-map-atoms];\n\
             \x20                --scan-bond i,j | --scan-angle i,j,k | --scan-dihedral i,j,k,l --scan-from <v> --scan-to <v> [--scan-steps <n>]\n\
             periodic:   qm periodic (needs a unit cell)  --functional pade|lda  --basis <gth-set>\n\
             \x20         --kmesh <n|nxnxn>   --cutoff <Ry>   --max-iter <n>   --forces   --stress"
        );
    };
    // `qm recommend <task>` prints hartree's recommended level of theory for a
    // task and needs no active structure.
    if sub == "recommend" {
        return qm_recommend(&args[1..]);
    }
    if sub == "status" {
        return Ok(crate::frontend::jobs::qm_jobs_status(state));
    }
    if sub == "cancel" {
        return crate::frontend::jobs::cancel_qm_job_alias(state);
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
        "ts" | "saddle" | "transition-state" => QmKind::TransitionState,
        other => {
            bail!(
                "unknown qm subcommand `{other}` \
                 (expected energy, optimize, freq, ts, periodic, recommend, status, or cancel)"
            )
        }
    };
    run(state, kind, &args[1..])
}

/// `qm recommend <task>`: hartree's data-driven level-of-theory guidance.
fn qm_recommend(args: &[String]) -> Result<String> {
    let tasks = hartree::guardrails::recommendation_tasks().join(", ");
    let Some(task) = args.first() else {
        bail!("usage: qm recommend <task>  (available: {tasks})");
    };
    let rec = hartree::guardrails::recommend(task)
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
fn assemble_qm_job(state: &AppState, kind: QmKind, args: &[String]) -> Result<QmJob> {
    if state.structure().atoms.is_empty() {
        bail!("no active structure; open one before `qm`");
    }
    let flags = QmFlags::parse(args)?;
    let engine = match flags.str("engine") {
        None | Some("hartree") => QmEngine::Hartree,
        Some("orca") => QmEngine::Orca,
        Some(other) => bail!("--engine must be hartree or orca, got `{other}`"),
    };
    if engine == QmEngine::Orca && kind == QmKind::TransitionState {
        bail!("ORCA support currently covers energy, optimize, and freq only");
    }
    if engine == QmEngine::Orca {
        for option in [
            "direct",
            "ri",
            "cosx",
            "ri-mp2",
            "x2c",
            "all-electron",
            "grid",
            "smear",
            "fod",
            "sph",
            "symmetry-number",
            "qrrho-w0",
            "alpb",
            "gbsa",
            "properties",
        ] {
            if flags.flag(option) || flags.str(option).is_some() {
                bail!("--{option} is available only with the Hartree engine");
            }
        }
    }
    let (method, suffix_dispersion) = flags
        .str("method")
        .map(QmMethod::parse)
        .unwrap_or_else(|| (QmMethod::Dft("b3lyp".to_string()), None));
    let basis = flags.str("basis").unwrap_or("def2-svp").to_string();
    let charge = flags.int("charge")?.unwrap_or(0);
    let multiplicity = flags.uint("spin")?.or(flags.uint("mult")?).unwrap_or(1);
    let options = build_options(&flags, suffix_dispersion)?;
    let ts = if kind == QmKind::TransitionState {
        Some(build_ts_config(state, &flags)?)
    } else {
        None
    };
    Ok(QmJob::molecular(
        engine,
        QmRequest {
            structure: state.structure().clone(),
            method,
            basis,
            charge,
            multiplicity,
            kind,
            options,
            ts,
        },
    ))
}

/// Assemble the [`QmTsConfig`] for a `qm ts` run from parsed flags. The guess
/// route is chosen by which flags are present: `--product` (two-endpoint),
/// `--scan-bond`/`--scan-angle`/`--scan-dihedral` (coordinate scan), or neither
/// (single guess from the current geometry).
fn build_ts_config(state: &AppState, flags: &QmFlags) -> Result<QmTsConfig> {
    let algorithm = match flags.str("ts-algo") {
        None => QmTsAlgorithm::default(),
        Some("prfo") => QmTsAlgorithm::Prfo,
        Some("dimer") => QmTsAlgorithm::Dimer,
        Some(other) => bail!("--ts-algo must be prfo or dimer, got `{other}`"),
    };
    let coordinates = match flags.str("ts-coords") {
        None => QmTsCoordinates::default(),
        Some("mass-weighted") | Some("cartesian") => QmTsCoordinates::MassWeighted,
        Some("internal") => QmTsCoordinates::Internal,
        Some(other) => bail!("--ts-coords must be mass-weighted or internal, got `{other}`"),
    };
    let confirm_irc = flags.flag("irc");

    // Exactly one guess route. `--product` and a `--scan-*` coordinate are
    // mutually exclusive.
    let scan_coordinate = parse_scan_coordinate(flags)?;
    let guess = match (flags.str("product"), scan_coordinate) {
        (Some(_), Some(_)) => bail!(
            "choose one transition-state guess: --product (two-endpoint) or a \
             --scan-bond/--scan-angle/--scan-dihedral (coordinate scan), not both"
        ),
        (Some(reference), None) => {
            let product_id = crate::frontend::entry_ref::resolve_entry_id(state, reference)?;
            // The reactant is the active structure; identical endpoints give a
            // zero-displacement, direction-less guess, so reject them.
            if state.entries.active_entry_id() == Some(product_id) {
                bail!(
                    "the --product must be a different structure than the reactant (the active entry)"
                );
            }
            let product =
                crate::frontend::entry_ref::entry_structure(state, product_id, "product")?;
            let mut endpoints = QmTsEndpoints::new(product);
            endpoints.use_neb = flags.flag("neb");
            // hartree's energy-peaked IDPP scan needs ≥3 points; below that, fall
            // back to the single geometric image (matching the GUI form) rather
            // than failing deep in the engine.
            endpoints.scan_points = flags
                .uint("idpp-scan")?
                .filter(|&n| n >= 3)
                .map(|n| n as usize);
            if let Some(images) = flags.uint("neb-images")? {
                endpoints.neb_images = images.max(1) as usize;
            }
            endpoints.map_atoms = !flags.flag("no-map-atoms");
            QmTsGuess::TwoEndpoint(Box::new(endpoints))
        }
        (None, Some(coordinate)) => {
            let start = flags
                .float("scan-from")?
                .ok_or_else(|| anyhow!("a coordinate scan needs --scan-from <value>"))?;
            let end = flags
                .float("scan-to")?
                .ok_or_else(|| anyhow!("a coordinate scan needs --scan-to <value>"))?;
            let n_points = flags.uint("scan-steps")?.unwrap_or(7) as usize;
            QmTsGuess::CoordinateScan(QmTsCoordinateScan {
                coordinate,
                start,
                end,
                n_points,
            })
        }
        (None, None) => QmTsGuess::Single,
    };

    Ok(QmTsConfig {
        guess,
        algorithm,
        coordinates,
        confirm_irc,
    })
}

/// Parse the `--scan-bond`/`--scan-angle`/`--scan-dihedral` coordinate (1-based
/// atom indices, comma-separated), if any. At most one may be given.
fn parse_scan_coordinate(flags: &QmFlags) -> Result<Option<QmInternalCoordinate>> {
    let specs = [
        ("scan-bond", 2usize),
        ("scan-angle", 3),
        ("scan-dihedral", 4),
    ];
    let mut found: Option<QmInternalCoordinate> = None;
    for (key, arity) in specs {
        let Some(value) = flags.str(key) else {
            continue;
        };
        if found.is_some() {
            bail!("specify at most one of --scan-bond, --scan-angle, --scan-dihedral");
        }
        let atoms = value
            .split([',', ':'])
            .map(|p| {
                p.trim()
                    .parse::<usize>()
                    .map_err(|_| anyhow!("--{key} expects atom indices like 1,2, got `{value}`"))
            })
            .collect::<Result<Vec<usize>>>()?;
        if atoms.len() != arity {
            bail!("--{key} expects {arity} atom indices, got {}", atoms.len());
        }
        found = Some(match arity {
            2 => QmInternalCoordinate::Bond(atoms[0], atoms[1]),
            3 => QmInternalCoordinate::Angle(atoms[0], atoms[1], atoms[2]),
            _ => QmInternalCoordinate::Dihedral(atoms[0], atoms[1], atoms[2], atoms[3]),
        });
    }
    Ok(found)
}

/// Map a `qm <subcommand>` line (subcommand + flags) to a [`QmRequest`] for the
/// agent's async tool. Mirrors [`qm_command`]'s subcommand dispatch.
pub fn build_agent_qm_request(state: &AppState, args: &[String]) -> Result<QmJob> {
    let Some(sub) = args.first().map(String::as_str) else {
        bail!("usage: qm <energy|optimize|freq> [options]");
    };
    let kind = match sub {
        "energy" | "sp" | "single-point" => QmKind::SinglePoint,
        "optimize" | "opt" => QmKind::Optimize,
        "freq" | "frequencies" => QmKind::Frequencies,
        "ts" | "saddle" | "transition-state" => QmKind::TransitionState,
        other => {
            bail!("unknown qm subcommand `{other}` (expected energy, optimize, freq, or ts)")
        }
    };
    assemble_qm_job(state, kind, &args[1..])
}

fn kind_keyword(kind: QmKind) -> &'static str {
    match kind {
        QmKind::SinglePoint => "energy",
        QmKind::Optimize => "optimize",
        QmKind::Frequencies => "freq",
        QmKind::TransitionState => "ts",
    }
}

fn run(state: &mut AppState, kind: QmKind, args: &[String]) -> Result<String> {
    let job = assemble_qm_job(state, kind, args)?;
    let QmCalculation::Molecular(request) = &job.calculation else {
        unreachable!("molecular command built a periodic job");
    };

    // Reject in-core jobs whose ERI tensor would blow the RAM budget before we
    // start allocating. Periodic runs go through run_periodic_command and are
    // exempt (no nao⁴ in-core tensor).
    let budget = crate::backend::hardware::qm_incore_budget_bytes();
    let verdict = crate::engines::qm::memory_verdict(request, budget);
    match (&job.engine, &verdict) {
        (QmEngine::Orca, _) | (_, crate::engines::qm::MemoryVerdict::Ok) => {}
        (_, crate::engines::qm::MemoryVerdict::ExceedsCanDirect { .. }) => {
            bail!(
                "{} Re-run `qm {}` with --direct (integral-direct SCF) or --ri, \
                 or choose a smaller basis.",
                verdict.detail("this machine").unwrap_or_default(),
                kind_keyword(kind),
            );
        }
        (_, crate::engines::qm::MemoryVerdict::ExceedsMustReduce { .. }) => {
            bail!(
                "{} This calculation type needs in-core integrals; choose a smaller \
                 basis set or a smaller system.",
                verdict.detail("this machine").unwrap_or_default(),
            );
        }
    }

    // Synchronous: a throwaway cancel flag and a no-op progress sink.
    let cancel = Arc::new(AtomicBool::new(false));
    let cores = Some(match job.engine {
        QmEngine::Hartree => state.config.compute_core_count.max(1),
        QmEngine::Orca => 1,
    });
    let outcome = match job.engine {
        QmEngine::Hartree => run_qm_calculation(job, cores, cancel, |_| {})?.outcome,
        QmEngine::Orca => {
            let launch = crate::backend::engine_launch::resolve_engine_launch(
                crate::backend::engine_launch::LaunchTarget::Local(&state.config.engine_overrides),
                crate::engines::registry::EngineId::ORCA,
            )?
            .launch;
            let QmCalculation::Molecular(request) = job.calculation else {
                unreachable!("ORCA command built a periodic job");
            };
            crate::engines::orca::run_orca(request, launch, cores, cancel, |_| {})?
        }
    };

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
    let result = run_qm_calculation(
        QmJob::periodic(request),
        Some(state.config.compute_core_count.max(1)),
        cancel,
        |_| {},
    )?;
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
mod tests;
