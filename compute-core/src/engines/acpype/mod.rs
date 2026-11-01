//! acpype subprocess adapter: GAFF/GAFF2 atom typing and partial charges for
//! one organic molecule through AmberTools (antechamber, parmchk2) driven by
//! acpype, which also writes the GROMACS include topology this reads back.
//!
//! The adapter runs on a small molecule only — a guest, a C60, a capped
//! oligomer whose middle unit becomes a repeat-unit template. A whole polymer
//! box is never typed here; it is tiled from templates by the MD layer.

mod parse;

use std::{
    fs,
    path::Path,
    sync::{Arc, atomic::AtomicBool},
    time::Duration,
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::{
    domain::Structure,
    engines::{process, registry::EngineLaunch},
    io::formats::mol2::to_mol2,
    md::TypedMolecule,
};

pub use parse::parse_itp;

const INPUT_FILE: &str = "silicolab_input.mol2";
/// acpype's `-b` base name: it names the output directory and files, and
/// becomes the GROMACS molecule type.
const BASE_NAME: &str = "MOL";
/// Elements GAFF parameterizes; antechamber refuses anything else.
const GAFF_ELEMENTS: &[&str] = &["H", "C", "N", "O", "F", "P", "S", "Cl", "Br", "I"];
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChargeMethod {
    /// AM1-BCC through sqm: the GAFF reference charges; seconds to minutes.
    AmBcc,
    /// Gasteiger: instant, rough; fine for a pipeline test, not for production.
    Gasteiger,
}

impl ChargeMethod {
    pub fn acpype_token(self) -> &'static str {
        match self {
            Self::AmBcc => "bcc",
            Self::Gasteiger => "gas",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::AmBcc => "AM1-BCC",
            Self::Gasteiger => "Gasteiger",
        }
    }

    pub fn parse(token: &str) -> Result<Self> {
        Ok(match token.to_ascii_lowercase().as_str() {
            "bcc" | "am1-bcc" | "am1bcc" => Self::AmBcc,
            "gas" | "gasteiger" => Self::Gasteiger,
            other => bail!("charge method must be `bcc` or `gasteiger`, not `{other}`"),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AtomTypeSet {
    Gaff2,
    Gaff,
}

impl AtomTypeSet {
    pub fn acpype_token(self) -> &'static str {
        match self {
            Self::Gaff2 => "gaff2",
            Self::Gaff => "gaff",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Gaff2 => "GAFF2",
            Self::Gaff => "GAFF",
        }
    }

    pub fn parse(token: &str) -> Result<Self> {
        Ok(match token.to_ascii_lowercase().as_str() {
            "gaff2" => Self::Gaff2,
            "gaff" => Self::Gaff,
            other => bail!("atom types must be `gaff2` or `gaff`, not `{other}`"),
        })
    }
}

#[derive(Debug, Clone)]
pub struct AcpypeRequest {
    pub structure: Structure,
    pub net_charge: i32,
    pub charge_method: ChargeMethod,
    pub atom_types: AtomTypeSet,
    pub max_duration: Duration,
}

#[derive(Debug, Clone)]
pub struct AcpypeOutcome {
    /// The GROMACS include topology exactly as acpype wrote it.
    pub itp: String,
    pub molecule: TypedMolecule,
    pub summary: String,
    pub wall_time: Duration,
}

pub fn run_acpype(
    request: AcpypeRequest,
    launch: EngineLaunch,
    cancel: Arc<AtomicBool>,
    mut report: impl FnMut(&str),
) -> Result<AcpypeOutcome> {
    validate_request(&request)?;
    let run_dir = std::env::temp_dir().join(format!("silicolab-acpype-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&run_dir)
        .with_context(|| format!("create acpype working directory {}", run_dir.display()))?;
    let result = (|| {
        fs::write(run_dir.join(INPUT_FILE), to_mol2(&request.structure))
            .context("write acpype input geometry")?;
        report(&format!(
            "running acpype ({} types, {} charges)",
            request.atom_types.label(),
            request.charge_method.label()
        ));
        let config =
            launch.to_process_config(&run_dir, render_args(&request), Some(request.max_duration));
        let output = process::spawn_with_cancel(config, cancel)?.join()?;
        if output.cancelled {
            bail!("acpype typing cancelled");
        }
        if output.timed_out {
            bail!(
                "acpype did not finish within {} s; AM1-BCC on a large molecule is slow, try \
                 a smaller molecule or `--method gasteiger` for a pipeline test",
                request.max_duration.as_secs()
            );
        }
        let combined = format!("{}\n{}", output.stdout, output.stderr);
        if !output.success() {
            bail!(
                "acpype failed with exit code {}: {}",
                output.exit_code,
                output_tail(&combined, 20)
            );
        }
        report("reading the typed topology");
        let itp_path = itp_path(&run_dir);
        let itp = fs::read_to_string(&itp_path).with_context(|| {
            format!(
                "acpype finished but wrote no {}: {}",
                itp_path.display(),
                output_tail(&combined, 10)
            )
        })?;
        let molecule = parse_itp(&itp, &source_label(&request))?;
        let summary = summarize(&request, &molecule);
        Ok(AcpypeOutcome {
            itp,
            molecule,
            summary,
            wall_time: output.wall_time,
        })
    })();
    let _ = fs::remove_dir_all(&run_dir);
    result
}

fn validate_request(request: &AcpypeRequest) -> Result<()> {
    if request.structure.atoms.is_empty() {
        bail!("acpype request has no atoms");
    }
    if request.structure.bonds.is_empty() && request.structure.atoms.len() > 1 {
        bail!("acpype needs the molecule's bonds; recompute bonds before typing");
    }
    let unsupported: Vec<&str> = request
        .structure
        .atoms
        .iter()
        .map(|atom| atom.element.as_str())
        .filter(|element| !GAFF_ELEMENTS.contains(element))
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    if !unsupported.is_empty() {
        bail!(
            "GAFF has no parameters for {}; it covers {}",
            unsupported.join(", "),
            GAFF_ELEMENTS.join(", ")
        );
    }
    let formal: f32 = request.structure.atoms.iter().map(|atom| atom.charge).sum();
    if (formal - request.net_charge as f32).abs() > 0.5 {
        bail!(
            "net charge {} disagrees with the structure's formal charges ({formal:+.1}); pass \
             `--charge` explicitly if the formal charges are wrong",
            request.net_charge
        );
    }
    Ok(())
}

/// `acpype -i <mol2> -b MOL -c <bcc|gas> -n <charge> -a <gaff2|gaff> -o gmx -f`.
pub fn render_args(request: &AcpypeRequest) -> Vec<String> {
    [
        "-i",
        INPUT_FILE,
        "-b",
        BASE_NAME,
        "-c",
        request.charge_method.acpype_token(),
        "-n",
        &request.net_charge.to_string(),
        "-a",
        request.atom_types.acpype_token(),
        "-o",
        "gmx",
        "-f",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

fn itp_path(run_dir: &Path) -> std::path::PathBuf {
    run_dir
        .join(format!("{BASE_NAME}.acpype"))
        .join(format!("{BASE_NAME}_GMX.itp"))
}

fn source_label(request: &AcpypeRequest) -> String {
    format!(
        "acpype {} {}",
        request.atom_types.acpype_token(),
        request.charge_method.acpype_token()
    )
}

fn summarize(request: &AcpypeRequest, molecule: &TypedMolecule) -> String {
    let counts = molecule.type_counts();
    let types: Vec<String> = counts
        .iter()
        .map(|(name, count)| format!("{name}×{count}"))
        .collect();
    let mut summary = format!(
        "{} typing with {} charges: {} atoms, {} atom types ({}), net charge {:+.3}",
        request.atom_types.label(),
        request.charge_method.label(),
        molecule.atoms.len(),
        counts.len(),
        types.join(", "),
        molecule.net_charge()
    );
    if (molecule.net_charge() - request.net_charge as f64).abs() > 0.01 {
        summary.push_str(&format!(
            "; the charges do not sum to the requested {:+}",
            request.net_charge
        ));
    }
    summary
}

fn output_tail(text: &str, lines: usize) -> String {
    let all: Vec<&str> = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    let start = all.len().saturating_sub(lines);
    all[start..].join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Atom, Bond, BondType};
    use nalgebra::Point3;

    fn methane() -> Structure {
        let atoms = vec![
            Atom {
                element: "C".into(),
                position: Point3::origin(),
                charge: 0.0,
            },
            Atom {
                element: "H".into(),
                position: Point3::new(1.09, 0.0, 0.0),
                charge: 0.0,
            },
        ];
        Structure::with_bonds(
            "methane",
            atoms,
            vec![Bond::with_type(0, 1, BondType::Single)],
        )
    }

    fn request(structure: Structure) -> AcpypeRequest {
        AcpypeRequest {
            structure,
            net_charge: 0,
            charge_method: ChargeMethod::AmBcc,
            atom_types: AtomTypeSet::Gaff2,
            max_duration: DEFAULT_TIMEOUT,
        }
    }

    #[test]
    fn renders_the_acpype_command_line() {
        let args = render_args(&request(methane()));
        assert_eq!(
            args,
            [
                "-i",
                "silicolab_input.mol2",
                "-b",
                "MOL",
                "-c",
                "bcc",
                "-n",
                "0",
                "-a",
                "gaff2",
                "-o",
                "gmx",
                "-f"
            ]
        );
        let mut gasteiger = request(methane());
        gasteiger.charge_method = ChargeMethod::Gasteiger;
        gasteiger.net_charge = -1;
        gasteiger.structure.atoms[0].charge = -1.0;
        let args = render_args(&gasteiger);
        assert!(args.windows(2).any(|w| w == ["-c", "gas"]));
        assert!(args.windows(2).any(|w| w == ["-n", "-1"]));
    }

    #[test]
    fn rejects_elements_gaff_cannot_type() {
        let mut structure = methane();
        structure.atoms[0].element = "Pt".into();
        let err = validate_request(&request(structure))
            .unwrap_err()
            .to_string();
        assert!(err.contains("Pt"), "{err}");
    }

    #[test]
    fn rejects_a_net_charge_that_contradicts_formal_charges() {
        let mut request = request(methane());
        request.net_charge = 2;
        let err = validate_request(&request).unwrap_err().to_string();
        assert!(err.contains("formal charges"), "{err}");
    }

    #[test]
    fn rejects_a_bond_free_molecule() {
        let mut structure = methane();
        structure.bonds.clear();
        assert!(validate_request(&request(structure)).is_err());
    }

    #[test]
    fn summary_reports_types_and_charge() {
        let molecule = parse_itp(parse::METHANOL_ITP, "acpype gaff2 bcc").unwrap();
        let summary = summarize(&request(methane()), &molecule);
        assert!(summary.contains("6 atoms"), "{summary}");
        assert!(summary.contains("4 atom types"), "{summary}");
        assert!(summary.contains("h1×3"), "{summary}");
        assert!(!summary.contains("do not sum"), "{summary}");
    }

    #[test]
    fn charge_method_and_type_set_parse_their_spellings() {
        assert_eq!(ChargeMethod::parse("AM1-BCC").unwrap(), ChargeMethod::AmBcc);
        assert_eq!(
            ChargeMethod::parse("gasteiger").unwrap(),
            ChargeMethod::Gasteiger
        );
        assert!(ChargeMethod::parse("resp").is_err());
        assert_eq!(AtomTypeSet::parse("gaff").unwrap(), AtomTypeSet::Gaff);
        assert!(AtomTypeSet::parse("opls").is_err());
    }

    #[test]
    fn acpype_spec_is_registered() {
        let spec = crate::engines::registry::engine_spec(crate::launch::EngineId::ACPYPE)
            .expect("acpype engine spec");
        assert_eq!(spec.identity_marker, "acpype");
        assert!(spec.candidate_executables.contains(&"acpype"));
    }

    /// Opt-in end-to-end run against a real acpype install.
    #[test]
    #[ignore = "requires SILICOLAB_TEST_ACPYPE_PROGRAM to name an acpype executable"]
    fn configured_acpype_types_methanol() {
        let program = std::env::var("SILICOLAB_TEST_ACPYPE_PROGRAM")
            .expect("set SILICOLAB_TEST_ACPYPE_PROGRAM");
        let launch = EngineLaunch {
            command_prefix: Vec::new(),
            program,
        };
        let structure =
            crate::workflows::sketch_to_structure::smiles_to_structure("CO", Some("methanol"))
                .unwrap();
        let mut request = request(structure);
        request.charge_method = ChargeMethod::Gasteiger;
        let outcome = run_acpype(request, launch, Default::default(), |_| {}).unwrap();
        assert_eq!(outcome.molecule.atoms.len(), 6);
        assert!(outcome.itp.contains("[ atomtypes ]"));
    }
}
