//! Grimme's xtb (GFN-xTB semi-empirical tight binding) subprocess adapter.
//!
//! xtb covers the whole periodic table up to radon at a cost of a fraction of a
//! second per gradient for a few dozen atoms, which makes it the engine for
//! things the built-in DFT engine cannot afford: dynamics, and metal clusters.
//! Its accuracy is semi-empirical, and the summary says so.

use std::{
    fs,
    path::Path,
    sync::{Arc, atomic::AtomicBool},
};

use anyhow::{Context, Result, anyhow, bail};

use crate::{
    domain::Structure,
    engines::{
        process,
        qm::{QmKind, QmMdConfig, QmMethod, QmOutcome, QmRequest, QmSolvation, QmTrajectory},
        registry::EngineLaunch,
    },
};

const INPUT_FILE: &str = "silicolab_xtb.xyz";
const CONTROL_FILE: &str = "silicolab_xtb.inp";
/// The trajectory file a dynamics run hands back (multi-frame XYZ, one frame per
/// dump, with `t=`, `E=`, and `method=` in each comment line).
pub const TRAJECTORY_FILE: &str = "md.xyz";

pub fn run_xtb(
    request: QmRequest,
    launch: EngineLaunch,
    cores: Option<usize>,
    cancel: Arc<AtomicBool>,
    mut report: impl FnMut(&str),
) -> Result<QmOutcome> {
    let gfn = gfn_level(&request.method)?;
    validate_request(&request)?;
    let run_dir = std::env::temp_dir().join(format!("silicolab-xtb-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&run_dir)
        .with_context(|| format!("create xtb working directory {}", run_dir.display()))?;
    let result = (|| {
        fs::write(run_dir.join(INPUT_FILE), render_xyz(&request.structure))
            .context("write xtb input geometry")?;
        let md = request.options.md.unwrap_or_default();
        if request.kind == QmKind::Dynamics {
            fs::write(run_dir.join(CONTROL_FILE), render_md_control(&md))
                .context("write xtb MD settings")?;
        }
        let args = render_args(&request, gfn)?;
        report(match request.kind {
            QmKind::Dynamics => "running xtb molecular dynamics",
            QmKind::Optimize => "running xtb optimization",
            _ => "running xtb",
        });
        let threads = cores.unwrap_or(1).max(1).to_string();
        let config = launch
            .to_process_config(&run_dir, args, None)
            .env("OMP_NUM_THREADS", threads.clone())
            .env("MKL_NUM_THREADS", threads)
            .env("OMP_STACKSIZE", "1G");
        let output = process::spawn_with_cancel(config, cancel)?.join()?;
        if output.cancelled {
            bail!("xtb calculation cancelled");
        }
        let combined = format!("{}\n{}", output.stdout, output.stderr);
        if !output.success() || !combined.contains("normal termination of xtb") {
            bail!(
                "xtb failed with exit code {}: {}",
                output.exit_code,
                output_tail(&combined, 20)
            );
        }
        report("collecting xtb results");
        parse_outcome(&request, gfn, &md, &run_dir, &output.stdout)
    })();
    let _ = fs::remove_dir_all(&run_dir);
    result
}

fn gfn_level(method: &QmMethod) -> Result<&'static str> {
    let name = match method {
        QmMethod::Dft(name) | QmMethod::Composite(name) => name.to_ascii_lowercase(),
        other => bail!("the xtb engine runs GFN-xTB methods, not {}", other.label()),
    };
    Ok(match name.as_str() {
        "gfn2" | "gfn2-xtb" | "gfn2xtb" | "xtb" => "2",
        "gfn1" | "gfn1-xtb" | "gfn1xtb" => "1",
        "gfn0" | "gfn0-xtb" | "gfn0xtb" => "0",
        "gfnff" | "gfn-ff" => "ff",
        other => bail!("the xtb engine runs gfn2-xtb, gfn1-xtb, gfn0-xtb, or gfnff, not `{other}`"),
    })
}

fn method_label(gfn: &str) -> &'static str {
    match gfn {
        "1" => "GFN1-xTB",
        "0" => "GFN0-xTB",
        "ff" => "GFN-FF",
        _ => "GFN2-xTB",
    }
}

fn validate_request(request: &QmRequest) -> Result<()> {
    if request.structure.atoms.is_empty() {
        bail!("xtb request has no atoms");
    }
    if request.multiplicity == 0 {
        bail!("spin multiplicity must be at least 1");
    }
    if request.kind == QmKind::TransitionState {
        bail!(
            "the xtb engine covers energy, optimize, freq, and md, not transition-state searches"
        );
    }
    if let Some(md) = &request.options.md {
        let positive = |value: f64| value.is_finite() && value > 0.0;
        if !(positive(md.temperature_k)
            && positive(md.time_ps)
            && positive(md.step_fs)
            && positive(md.dump_fs))
        {
            bail!("MD temperature, time, step, and dump interval must all be positive");
        }
        if md.step_fs > 4.0 {
            bail!("an MD step above 4 fs is unstable without bond constraints");
        }
        if md.time_ps * 1000.0 / md.dump_fs > 50_000.0 {
            bail!("that MD run would save more than 50 000 frames; raise the dump interval");
        }
    }
    Ok(())
}

fn render_xyz(structure: &Structure) -> String {
    let mut text = format!(
        "{}\n{}\n",
        structure.atoms.len(),
        structure.title.replace('\n', " ")
    );
    for atom in &structure.atoms {
        text.push_str(&format!(
            "{} {:.10} {:.10} {:.10}\n",
            atom.element, atom.position.x, atom.position.y, atom.position.z
        ));
    }
    text
}

/// xtb's `$md` block. `shake=0` and `hmass=1` keep every bond — including X–H —
/// free to break, which is the point of reactive dynamics; the cost is the short
/// step that requires.
fn render_md_control(md: &QmMdConfig) -> String {
    format!(
        "$md\n   temp={}\n   time={}\n   dump={}\n   step={}\n   hmass=1\n   shake=0\n   velo=false\n   nvt=true\n$end\n",
        md.temperature_k, md.time_ps, md.dump_fs, md.step_fs
    )
}

pub fn render_args(request: &QmRequest, gfn: &str) -> Result<Vec<String>> {
    let mut args = vec![INPUT_FILE.to_string()];
    if gfn == "ff" {
        args.push("--gfnff".to_string());
    } else {
        args.extend(["--gfn".to_string(), gfn.to_string()]);
    }
    args.extend([
        "--chrg".to_string(),
        request.charge.to_string(),
        "--uhf".to_string(),
        (request.multiplicity - 1).to_string(),
    ]);
    match &request.options.solvation {
        None => {}
        Some(QmSolvation::Alpb(solvent)) => {
            args.extend(["--alpb".to_string(), safe_solvent(solvent)?])
        }
        Some(QmSolvation::Gbsa(solvent)) => {
            args.extend(["--gbsa".to_string(), safe_solvent(solvent)?])
        }
        Some(_) => bail!("the xtb engine supports ALPB and GBSA solvation only"),
    }
    if let Some(temperature) = request.options.smearing_temperature_k {
        args.extend(["--etemp".to_string(), temperature.to_string()]);
    }
    match request.kind {
        QmKind::SinglePoint => {}
        QmKind::Optimize => args.push("--opt".to_string()),
        QmKind::Frequencies => args.push("--hess".to_string()),
        QmKind::Dynamics => args.extend([
            "--md".to_string(),
            "--input".to_string(),
            CONTROL_FILE.to_string(),
        ]),
        QmKind::TransitionState => unreachable!("validated above"),
    }
    Ok(args)
}

fn safe_solvent(name: &str) -> Result<String> {
    let name = name.trim();
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        bail!("invalid solvent name `{name}`");
    }
    Ok(name.to_string())
}

fn parse_outcome(
    request: &QmRequest,
    gfn: &str,
    md: &QmMdConfig,
    run_dir: &Path,
    stdout: &str,
) -> Result<QmOutcome> {
    let method = method_label(gfn);
    let final_energy = stdout
        .lines()
        .filter_map(|line| {
            let line = line.trim().trim_matches('|').trim();
            line.strip_prefix("TOTAL ENERGY")
        })
        .filter_map(|rest| rest.split_whitespace().next()?.parse::<f64>().ok())
        .next_back();

    let mut optimized_structure = None;
    let mut opt_trace = Vec::new();
    let mut frequencies = Vec::new();
    let mut trajectory = None;
    let mut converged = true;
    let mut energy_hartree = final_energy;
    let mut details = String::new();

    match request.kind {
        QmKind::Optimize => {
            optimized_structure = Some(
                read_final_frame(&run_dir.join("xtbopt.xyz"), &request.structure)
                    .context("read xtb optimized geometry")?,
            );
            opt_trace = frame_energies(&run_dir.join("xtbopt.log")).unwrap_or_default();
            converged = stdout.contains("GEOMETRY OPTIMIZATION CONVERGED");
        }
        QmKind::Frequencies => {
            frequencies = parse_frequencies(&run_dir.join("vibspectrum")).unwrap_or_default();
        }
        QmKind::Dynamics => {
            let raw =
                fs::read_to_string(run_dir.join("xtb.trj")).context("read xtb MD trajectory")?;
            let (xyz, energies) = annotate_trajectory(&raw, md.dump_fs / 1000.0, method)?;
            let last = read_last_frame_text(&xyz, &request.structure)?;
            energy_hartree = energies.last().copied().or(energy_hartree);
            details = format!(
                "\n  NVT {:.0} K, {:.2} ps, step {} fs, {} frames every {} fs\n  \
                 no bond constraints (SHAKE off, true H mass): bonds may form and break\n  \
                 note: {method} is semi-empirical; read the trajectory as qualitative mechanism, \
                 not as quantitative barriers or rates",
                md.temperature_k,
                md.time_ps,
                md.step_fs,
                energies.len(),
                md.dump_fs
            );
            opt_trace = energies;
            optimized_structure = Some(last);
            trajectory = Some(QmTrajectory {
                file_name: TRAJECTORY_FILE.to_string(),
                bytes: xyz.into_bytes(),
            });
        }
        QmKind::SinglePoint | QmKind::TransitionState => {}
    }

    let energy_hartree =
        energy_hartree.ok_or_else(|| anyhow!("xtb output contains no total energy"))?;
    let mut summary = format!(
        "xtb {} {method}\n  final energy: {:.12} Eh\n  converged: {}",
        request.kind.label(),
        energy_hartree,
        if converged { "yes" } else { "no" }
    );
    summary.push_str(&details);
    if !frequencies.is_empty() {
        summary.push_str("\n  frequencies (cm^-1):");
        for chunk in frequencies.chunks(6) {
            summary.push_str("\n   ");
            for frequency in chunk {
                summary.push_str(&format!(" {frequency:.2}"));
            }
        }
    }
    if let Some(structure) = &mut optimized_structure {
        structure.title = format!(
            "{} ({method} {})",
            request.structure.title,
            if request.kind == QmKind::Dynamics {
                "MD final frame"
            } else {
                "opt"
            }
        );
    }
    Ok(QmOutcome {
        energy_hartree,
        converged,
        optimized_structure,
        summary,
        scf_trace: Vec::new(),
        opt_trace,
        frequencies,
        trajectory,
    })
}

/// Rewrite xtb's `xtb.trj` so every comment line carries the frame's time (ps),
/// energy (Eh), and the method, and return the per-frame energies.
pub fn annotate_trajectory(raw: &str, dt_ps: f64, method: &str) -> Result<(String, Vec<f64>)> {
    let lines: Vec<&str> = raw.lines().collect();
    let mut out = String::with_capacity(raw.len() + raw.len() / 8);
    let mut energies = Vec::new();
    let mut cursor = 0;
    while cursor < lines.len() {
        if lines[cursor].trim().is_empty() {
            cursor += 1;
            continue;
        }
        let count: usize = lines[cursor]
            .trim()
            .parse()
            .context("xtb trajectory frame header")?;
        if cursor + 2 + count > lines.len() {
            break; // a frame cut short by cancellation; keep the complete ones
        }
        let comment = lines[cursor + 1];
        let energy = comment
            .split_whitespace()
            .skip_while(|token| *token != "energy:")
            .nth(1)
            .and_then(|value| value.parse::<f64>().ok())
            .ok_or_else(|| anyhow!("xtb trajectory frame without an energy"))?;
        let frame = energies.len();
        energies.push(energy);
        out.push_str(&format!(
            "{count}\nt= {:.4} E= {energy:.10} method={method}\n",
            frame as f64 * dt_ps
        ));
        for line in &lines[cursor + 2..cursor + 2 + count] {
            out.push_str(line.trim());
            out.push('\n');
        }
        cursor += 2 + count;
    }
    if energies.is_empty() {
        bail!("xtb wrote no trajectory frames");
    }
    Ok((out, energies))
}

fn read_final_frame(path: &Path, template: &Structure) -> Result<Structure> {
    let text = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    read_last_frame_text(&text, template)
}

/// The last XYZ record of `text`, applied to `template`'s atoms (so bonds,
/// charges, and labels survive) after checking the element order matches.
fn read_last_frame_text(text: &str, template: &Structure) -> Result<Structure> {
    let records = crate::io::formats::xyz::parse_xyz_records(text)?;
    let last = records.last().ok_or_else(|| anyhow!("no XYZ frames"))?;
    if last.atoms.len() != template.atoms.len() {
        bail!(
            "xtb returned {} atoms for a {}-atom input",
            last.atoms.len(),
            template.atoms.len()
        );
    }
    let mut structure = template.clone();
    for (atom, frame_atom) in structure.atoms.iter_mut().zip(&last.atoms) {
        if !atom.element.eq_ignore_ascii_case(&frame_atom.element) {
            bail!(
                "xtb reordered atoms ({} vs {})",
                atom.element,
                frame_atom.element
            );
        }
        atom.position = frame_atom.position;
    }
    structure.recompute_bonds();
    Ok(structure)
}

fn frame_energies(path: &Path) -> Result<Vec<f64>> {
    let text = fs::read_to_string(path)?;
    let lines: Vec<&str> = text.lines().collect();
    let mut energies = Vec::new();
    let mut cursor = 0;
    while cursor + 1 < lines.len() {
        let Ok(count) = lines[cursor].trim().parse::<usize>() else {
            break;
        };
        if let Some(energy) = lines[cursor + 1]
            .split_whitespace()
            .skip_while(|token| *token != "energy:")
            .nth(1)
            .and_then(|value| value.parse().ok())
        {
            energies.push(energy);
        }
        cursor += 2 + count;
    }
    Ok(energies)
}

/// Wavenumbers from xtb's Turbomole-style `vibspectrum`, skipping the zero
/// translation/rotation modes.
fn parse_frequencies(path: &Path) -> Result<Vec<f64>> {
    let text = fs::read_to_string(path)?;
    Ok(text
        .lines()
        .filter(|line| !line.trim_start().starts_with('$') && !line.trim_start().starts_with('#'))
        .filter_map(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            // "mode  symmetry  wave number  IR intensity  selection rules"; the
            // symmetry column is blank for the zero modes.
            match fields.len() {
                n if n >= 5 => fields[2].parse::<f64>().ok(),
                _ => None,
            }
        })
        .filter(|frequency| frequency.abs() > 1e-3)
        .collect())
}

fn output_tail(text: &str, lines: usize) -> String {
    let all: Vec<&str> = text.lines().collect();
    all[all.len().saturating_sub(lines)..].join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Atom, Structure};
    use crate::engines::qm::QmOptions;
    use nalgebra::Point3;

    fn h2() -> Structure {
        Structure::new(
            "h2".to_string(),
            vec![
                Atom {
                    element: "H".to_string(),
                    position: Point3::new(0.0, 0.0, 0.0),
                    charge: 0.0,
                },
                Atom {
                    element: "H".to_string(),
                    position: Point3::new(0.0, 0.0, 0.74),
                    charge: 0.0,
                },
            ],
        )
    }

    fn request(kind: QmKind) -> QmRequest {
        QmRequest {
            structure: h2(),
            method: QmMethod::Dft("gfn2-xtb".to_string()),
            basis: String::new(),
            charge: 0,
            multiplicity: 1,
            kind,
            options: QmOptions {
                md: Some(QmMdConfig::default()),
                ..QmOptions::default()
            },
            ts: None,
        }
    }

    #[test]
    fn dynamics_args_turn_md_on_with_the_control_file() {
        let args = render_args(&request(QmKind::Dynamics), "2").unwrap();
        assert_eq!(
            args,
            [
                "silicolab_xtb.xyz",
                "--gfn",
                "2",
                "--chrg",
                "0",
                "--uhf",
                "0",
                "--md",
                "--input",
                "silicolab_xtb.inp"
            ]
        );
        let control = render_md_control(&QmMdConfig::default());
        assert!(
            control.contains("shake=0")
                && control.contains("hmass=1")
                && control.contains("temp=300")
        );
    }

    #[test]
    fn methods_map_to_gfn_levels() {
        assert_eq!(gfn_level(&QmMethod::Dft("GFN2-xTB".into())).unwrap(), "2");
        assert_eq!(gfn_level(&QmMethod::Dft("gfnff".into())).unwrap(), "ff");
        assert!(gfn_level(&QmMethod::Dft("b3lyp".into())).is_err());
        assert!(gfn_level(&QmMethod::Hf).is_err());
    }

    #[test]
    fn trajectory_comments_gain_time_energy_and_method() {
        let raw = "2\n energy: -1.0 gnorm: 0.1 xtb: 6.7.1 (edcfbbe)\nH 0 0 0\nH 0 0 0.74\n\
                   2\n energy: -0.9 gnorm: 0.1 xtb: 6.7.1 (edcfbbe)\nH 0 0 0\nH 0 0 0.80\n";
        let (xyz, energies) = annotate_trajectory(raw, 0.002, "GFN2-xTB").unwrap();
        assert_eq!(energies, [-1.0, -0.9]);
        assert!(xyz.contains("t= 0.0020 E= -0.9000000000 method=GFN2-xTB"));
        let trajectory = crate::io::trajectory::xyz::parse_xyz_trajectory(&xyz, 1.0).unwrap();
        assert_eq!(trajectory.method(), Some("GFN2-xTB"));
        assert!((trajectory.time(1) - 0.002).abs() < 1e-6);
        let last = read_last_frame_text(&xyz, &h2()).unwrap();
        assert!((last.atoms[1].position.z - 0.80).abs() < 1e-6);
    }

    #[test]
    fn transition_states_are_rejected() {
        assert!(validate_request(&request(QmKind::TransitionState)).is_err());
    }

    #[test]
    #[ignore = "requires SILICOLAB_TEST_XTB_PROGRAM to name an xtb executable"]
    fn configured_xtb_runs_supported_calculations() {
        let program =
            std::env::var("SILICOLAB_TEST_XTB_PROGRAM").expect("set SILICOLAB_TEST_XTB_PROGRAM");
        let launch = EngineLaunch {
            command_prefix: Vec::new(),
            program,
        };
        let spec = crate::engines::registry::engine_spec(crate::launch::EngineId::XTB)
            .expect("xtb engine spec");
        let version = crate::engines::registry::verify_launch(&launch, spec)
            .expect("configured xtb should verify");
        assert!(!version.is_empty());
        for kind in [
            QmKind::SinglePoint,
            QmKind::Optimize,
            QmKind::Frequencies,
            QmKind::Dynamics,
        ] {
            let mut request = request(kind);
            request.options.md = Some(QmMdConfig {
                time_ps: 0.05,
                ..QmMdConfig::default()
            });
            let outcome = run_xtb(request, launch.clone(), Some(1), Default::default(), |_| {})
                .unwrap_or_else(|error| panic!("configured xtb should complete {kind:?}: {error}"));
            assert!(outcome.converged, "{kind:?} did not converge");
            assert!(outcome.energy_hartree.is_finite());
            match kind {
                QmKind::Frequencies => assert!(!outcome.frequencies.is_empty()),
                QmKind::Dynamics => {
                    let trajectory = outcome.trajectory.expect("dynamics returns a trajectory");
                    let text = String::from_utf8(trajectory.bytes).unwrap();
                    let parsed =
                        crate::io::trajectory::xyz::parse_xyz_trajectory(&text, 1.0).unwrap();
                    assert!(parsed.frame_count() > 1);
                    assert_eq!(parsed.energies().len(), parsed.frame_count());
                }
                _ => {}
            }
        }
    }
}
