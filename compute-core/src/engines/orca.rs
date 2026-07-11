//! ORCA molecular quantum-chemistry subprocess adapter.

use std::{
    fs,
    path::Path,
    sync::{Arc, atomic::AtomicBool},
};

use anyhow::{Context, Result, anyhow, bail};

use crate::{
    domain::{Atom, Structure},
    engines::{
        process,
        qm::{CpcmDielectric, QmDispersion, QmKind, QmMethod, QmOutcome, QmRequest, QmSolvation},
        registry::EngineLaunch,
    },
};

const INPUT_FILE: &str = "silicolab_orca.inp";
const XYZ_FILE: &str = "silicolab_orca.xyz";

pub fn run_orca(
    request: QmRequest,
    launch: EngineLaunch,
    cores: Option<usize>,
    cancel: Arc<AtomicBool>,
    mut report: impl FnMut(&str),
) -> Result<QmOutcome> {
    validate_request(&request)?;
    let run_dir = std::env::temp_dir().join(format!("silicolab-orca-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&run_dir)
        .with_context(|| format!("create ORCA working directory {}", run_dir.display()))?;
    let result = (|| {
        let input = render_input(&request, cores)?;
        fs::write(run_dir.join(INPUT_FILE), input).context("write ORCA input")?;
        report("running ORCA");
        let config = launch.to_process_config(&run_dir, [INPUT_FILE.to_string()], None);
        let output = process::spawn_with_cancel(config, cancel)?.join()?;
        if output.cancelled {
            bail!("ORCA calculation cancelled");
        }
        if !output.success() {
            let combined = format!("{}\n{}", output.stdout, output.stderr);
            if let Some(reason) = parallel_startup_failure(cores, &combined) {
                bail!("{reason}");
            }
            bail!(
                "ORCA failed with exit code {}: {}",
                output.exit_code,
                output_tail(&combined, 20)
            );
        }
        if !output.stdout.contains("ORCA TERMINATED NORMALLY") {
            bail!(
                "ORCA did not terminate normally: {}",
                output_tail(&format!("{}\n{}", output.stdout, output.stderr), 20)
            );
        }
        report("collecting ORCA results");
        parse_outcome(&request, &run_dir, &output.stdout)
    })();
    let _ = fs::remove_dir_all(&run_dir);
    result
}

pub fn render_input(request: &QmRequest, cores: Option<usize>) -> Result<String> {
    validate_request(request)?;
    let method = method_keyword(&request.method)?;
    let basis = (!matches!(request.method, QmMethod::Composite(_)))
        .then(|| safe_keyword(&request.basis, "basis set"))
        .transpose()?;
    let mut keywords = vec![method];
    if let Some(basis) = basis {
        keywords.push(basis);
    }
    keywords.push("TightSCF".to_string());
    match request.kind {
        QmKind::SinglePoint => {}
        QmKind::Optimize => keywords.push("Opt".to_string()),
        QmKind::Frequencies => keywords.push("Freq".to_string()),
        QmKind::TransitionState => unreachable!("validated above"),
    }
    if let Some(dispersion) = request.options.dispersion {
        keywords.push(
            match dispersion {
                QmDispersion::D3Bj => "D3BJ",
                QmDispersion::D4 => "D4",
            }
            .to_string(),
        );
    }
    let mut blocks = Vec::new();
    if let Some(solvation) = &request.options.solvation {
        match solvation {
            QmSolvation::Cpcm(CpcmDielectric::Named(name)) => {
                keywords.push(format!("CPCM({})", safe_keyword(name, "solvent")?));
            }
            QmSolvation::Cpcm(CpcmDielectric::Epsilon(epsilon)) => {
                if !epsilon.is_finite() || *epsilon <= 1.0 {
                    bail!("ORCA C-PCM dielectric must be finite and greater than 1");
                }
                blocks.push(format!("%cpcm\n  epsilon {epsilon}\nend"));
            }
            QmSolvation::Smd(name) => {
                let name = safe_keyword(name, "SMD solvent")?;
                blocks.push(format!("%cpcm\n  smd true\n  SMDsolvent \"{name}\"\nend"));
            }
            QmSolvation::Alpb(_) | QmSolvation::Gbsa(_) => {
                bail!("ORCA support does not include ALPB or GBSA solvation")
            }
        }
    }
    if request.options.compute_properties {
        keywords.push("Mayer".to_string());
    }
    if let Some(cores) = cores.filter(|cores| *cores > 1) {
        blocks.push(format!("%pal\n  nprocs {cores}\nend"));
    }

    let mut input = format!("! {}\n", keywords.join(" "));
    for block in blocks {
        input.push_str(&block);
        input.push('\n');
    }
    input.push_str(&format!(
        "* xyz {} {}\n",
        request.charge, request.multiplicity
    ));
    for atom in &request.structure.atoms {
        input.push_str(&format!(
            "{} {:.12} {:.12} {:.12}\n",
            atom.element, atom.position.x, atom.position.y, atom.position.z
        ));
    }
    input.push_str("*\n");
    Ok(input)
}

fn validate_request(request: &QmRequest) -> Result<()> {
    if request.structure.atoms.is_empty() {
        bail!("ORCA request has no atoms");
    }
    if request.kind == QmKind::TransitionState {
        bail!("ORCA support currently covers single-point, optimization, and frequencies only");
    }
    if request.multiplicity == 0 {
        bail!("spin multiplicity must be at least 1");
    }
    Ok(())
}

fn method_keyword(method: &QmMethod) -> Result<String> {
    match method {
        QmMethod::Hf => Ok("HF".to_string()),
        QmMethod::Rhf => Ok("RHF".to_string()),
        QmMethod::Uhf => Ok("UHF".to_string()),
        QmMethod::Rohf => Ok("ROHF".to_string()),
        QmMethod::Mp2 => Ok("MP2".to_string()),
        QmMethod::Ccsd => Ok("CCSD".to_string()),
        QmMethod::CcsdT => Ok("CCSD(T)".to_string()),
        QmMethod::Dft(name) | QmMethod::Composite(name) => safe_keyword(name, "method"),
    }
}

fn safe_keyword(value: &str, field: &str) -> Result<String> {
    let value = value.trim();
    if value.is_empty()
        || !value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '+' | '(' | ')' | ',' | '.'))
    {
        bail!("invalid ORCA {field} `{value}`");
    }
    Ok(value.to_string())
}

fn parse_outcome(request: &QmRequest, run_dir: &Path, output: &str) -> Result<QmOutcome> {
    let energy_hartree = output
        .lines()
        .filter_map(|line| line.trim().strip_prefix("FINAL SINGLE POINT ENERGY"))
        .filter_map(|value| value.trim().parse::<f64>().ok())
        .next_back()
        .ok_or_else(|| anyhow!("ORCA output contains no final single-point energy"))?;
    let optimized_structure = if request.kind == QmKind::Optimize {
        let path = run_dir.join(XYZ_FILE);
        Some(
            parse_xyz(&path, &request.structure.title)
                .with_context(|| format!("read ORCA optimized geometry from {}", path.display()))?,
        )
    } else {
        None
    };
    let frequencies = if request.kind == QmKind::Frequencies {
        parse_frequencies(output)
    } else {
        Vec::new()
    };
    let converged = match request.kind {
        QmKind::Optimize => output.contains("THE OPTIMIZATION HAS CONVERGED"),
        _ => true,
    };
    let mut summary = format!(
        "ORCA {} {} / {}\n  final energy: {:.12} Eh\n  converged: {}",
        request.kind.label(),
        request.method.label(),
        request.basis,
        energy_hartree,
        if converged { "yes" } else { "no" }
    );
    if !frequencies.is_empty() {
        summary.push_str("\n  frequencies (cm^-1):");
        for chunk in frequencies.chunks(6) {
            summary.push_str("\n   ");
            for frequency in chunk {
                summary.push_str(&format!(" {frequency:.2}"));
            }
        }
    }
    Ok(QmOutcome {
        energy_hartree,
        converged,
        optimized_structure,
        summary,
        scf_trace: Vec::new(),
        opt_trace: Vec::new(),
        frequencies,
    })
}

fn parse_xyz(path: &Path, name: &str) -> Result<Structure> {
    let text = fs::read_to_string(path)?;
    let mut lines = text.lines();
    let count = lines
        .next()
        .ok_or_else(|| anyhow!("empty XYZ file"))?
        .trim()
        .parse::<usize>()
        .context("parse XYZ atom count")?;
    let _ = lines.next();
    let mut atoms = Vec::with_capacity(count);
    for (index, line) in lines.take(count).enumerate() {
        let fields: Vec<_> = line.split_whitespace().collect();
        if fields.len() < 4 {
            bail!("invalid XYZ atom line {}", index + 3);
        }
        atoms.push(Atom {
            element: fields[0].to_string(),
            position: nalgebra::Point3::new(
                fields[1].parse().context("parse XYZ x coordinate")?,
                fields[2].parse().context("parse XYZ y coordinate")?,
                fields[3].parse().context("parse XYZ z coordinate")?,
            ),
            charge: 0.0,
        });
    }
    if atoms.len() != count {
        bail!("XYZ declared {count} atoms but contained {}", atoms.len());
    }
    Ok(Structure::new(format!("{name} (ORCA optimized)"), atoms))
}

fn parse_frequencies(output: &str) -> Vec<f64> {
    let mut in_section = false;
    let mut frequencies = Vec::new();
    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed == "VIBRATIONAL FREQUENCIES" {
            in_section = true;
            continue;
        }
        if !in_section {
            continue;
        }
        if trimmed.starts_with("NORMAL MODES") || trimmed.starts_with("IR SPECTRUM") {
            break;
        }
        let Some((_, rest)) = trimmed.split_once(':') else {
            continue;
        };
        let Some(value) = rest.split_whitespace().next() else {
            continue;
        };
        if rest.contains("cm**-1")
            && let Ok(value) = value.parse::<f64>()
        {
            frequencies.push(value);
        }
    }
    frequencies
}

fn output_tail(output: &str, lines: usize) -> String {
    let mut tail: Vec<_> = output.lines().rev().take(lines).collect();
    tail.reverse();
    tail.join("\n")
}

fn parallel_startup_failure(cores: Option<usize>, output: &str) -> Option<String> {
    let cores = cores.filter(|cores| *cores > 1)?;
    let lower = output.to_ascii_lowercase();
    let missing_mpirun = lower.contains("mpirun: not found")
        || lower.contains("mpirun: command not found")
        || lower.contains("no such file or directory") && lower.contains("mpirun");
    missing_mpirun.then(|| {
        format!(
            "ORCA parallel execution requested {cores} CPU cores, but `mpirun` is not available \
             in the target environment. Install a compatible MPI runtime there, or set CPU cores \
             to 1 to run ORCA serially."
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engines::qm::QmOptions;
    use nalgebra::Point3;

    fn request(kind: QmKind) -> QmRequest {
        QmRequest {
            structure: Structure::new(
                "water",
                vec![Atom {
                    element: "O".into(),
                    position: Point3::new(0.0, 0.0, 0.0),
                    charge: 0.0,
                }],
            ),
            method: QmMethod::Dft("b3lyp".into()),
            basis: "def2-svp".into(),
            charge: 0,
            multiplicity: 1,
            kind,
            options: QmOptions {
                dispersion: Some(QmDispersion::D3Bj),
                ..Default::default()
            },
            ts: None,
        }
    }

    #[test]
    fn renders_single_point_input() {
        let input = render_input(&request(QmKind::SinglePoint), Some(4)).unwrap();
        assert!(input.contains("! b3lyp def2-svp TightSCF D3BJ"));
        assert!(input.contains("nprocs 4"));
        assert!(input.contains("* xyz 0 1"));
    }

    #[test]
    fn parses_final_energy_and_frequencies() {
        let output = "\nFINAL SINGLE POINT ENERGY     -76.123456789\n\nVIBRATIONAL FREQUENCIES\n-----------------------\n  0:       0.00 cm**-1\n  6:    1595.12 cm**-1\n  7:    3657.05 cm**-1\nNORMAL MODES\nORCA TERMINATED NORMALLY\n";
        let parsed = parse_outcome(&request(QmKind::Frequencies), Path::new("."), output).unwrap();
        assert_eq!(parsed.energy_hartree, -76.123456789);
        assert_eq!(parsed.frequencies, vec![0.0, 1595.12, 3657.05]);
    }

    #[test]
    fn rejects_transition_state() {
        let error = render_input(&request(QmKind::TransitionState), None).unwrap_err();
        assert!(error.to_string().contains("currently covers"));
    }

    #[test]
    fn missing_mpirun_has_an_actionable_parallel_error() {
        let output = "Calling Command: mpirun -np 8 orca_startup_mpi\nsh: 1: mpirun: not found";
        let error = parallel_startup_failure(Some(8), output).expect("recognized MPI failure");
        assert!(error.contains("requested 8 CPU cores"));
        assert!(error.contains("Install a compatible MPI runtime"));
        assert!(error.contains("set CPU cores to 1"));
        assert!(parallel_startup_failure(Some(1), output).is_none());
    }

    #[test]
    #[ignore = "requires SILICOLAB_TEST_ORCA_PROGRAM to name an ORCA executable"]
    fn configured_orca_runs_supported_calculations() {
        let program =
            std::env::var("SILICOLAB_TEST_ORCA_PROGRAM").expect("set SILICOLAB_TEST_ORCA_PROGRAM");
        let command_prefix: Vec<String> = std::env::var("SILICOLAB_TEST_ORCA_PREFIX")
            .ok()
            .map(|prefix| prefix.split_whitespace().map(str::to_string).collect())
            .unwrap_or_default();
        let launch = EngineLaunch {
            command_prefix,
            program,
        };
        let spec = crate::engines::registry::engine_spec(crate::launch::EngineId::ORCA)
            .expect("ORCA engine spec");
        let version = crate::engines::registry::verify_launch(&launch, spec)
            .expect("configured ORCA should verify");
        assert!(!version.is_empty());
        for kind in [QmKind::SinglePoint, QmKind::Optimize, QmKind::Frequencies] {
            let mut request = request(kind);
            request.structure = Structure::new(
                "h2",
                vec![
                    Atom {
                        element: "H".into(),
                        position: Point3::new(0.0, 0.0, 0.0),
                        charge: 0.0,
                    },
                    Atom {
                        element: "H".into(),
                        position: Point3::new(0.0, 0.0, 0.9),
                        charge: 0.0,
                    },
                ],
            );
            request.method = QmMethod::Rhf;
            request.basis = "sto-3g".to_string();
            request.options.dispersion = None;
            let outcome = run_orca(request, launch.clone(), Some(1), Default::default(), |_| {})
                .unwrap_or_else(|error| {
                    panic!("configured ORCA should complete {kind:?}: {error}")
                });
            assert!(outcome.converged);
            assert!(outcome.energy_hartree.is_finite());
            if kind == QmKind::Optimize {
                assert!(outcome.optimized_structure.is_some());
            }
            if kind == QmKind::Frequencies {
                assert!(!outcome.frequencies.is_empty());
            }
        }
    }
}
