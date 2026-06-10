//! In-process quantum-chemistry engine.
//!
//! Wraps the `chemx` crate (pure-Rust HF/DFT/MP2/CC) so the rest of the app can
//! request single-point energies, geometry optimization, and properties or
//! vibrational frequencies from a [`Structure`] without knowing chemx's types.
//! Unlike the GROMACS engine this is a library call — it runs in-process on a
//! worker thread, not as an external subprocess.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result, anyhow, bail};
use chemx::dft::FunctionalSpec;
use chemx::{Element, Job, JobOptions, JobResult, Method, Molecule};

use crate::domain::Structure;
use crate::io::structure_text::to_xyz;

/// Bohr → Ångström. chemx stores nuclear positions in bohr.
const BOHR_TO_ANGSTROM: f64 = 0.529_177_210_903;
/// Hartree → electron-volt.
const HARTREE_TO_EV: f64 = 27.211_386_245_988;
/// Hartree → kcal/mol.
const HARTREE_TO_KCAL: f64 = 627.509_474_063;
/// Atomic-unit dipole (e·a₀) → Debye.
const AU_DIPOLE_TO_DEBYE: f64 = 2.541_746_473;

/// Electronic-structure method. Mirrors `chemx::Method` but is parseable from a
/// console argument or UI dropdown and keeps the external type off our API edge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QmMethod {
    Rhf,
    Uhf,
    Rohf,
    Mp2,
    Ccsd,
    CcsdT,
    /// Kohn–Sham DFT with the named functional (e.g. `b3lyp`, `pbe`).
    Dft(String),
}

impl QmMethod {
    /// Methods offered in the GUI dropdown, in display order.
    pub fn presets() -> Vec<QmMethod> {
        vec![
            QmMethod::Rhf,
            QmMethod::Uhf,
            QmMethod::Rohf,
            QmMethod::Dft("b3lyp".to_string()),
            QmMethod::Dft("pbe".to_string()),
            QmMethod::Dft("pbe0".to_string()),
            QmMethod::Dft("blyp".to_string()),
            QmMethod::Dft("svwn".to_string()),
            QmMethod::Mp2,
            QmMethod::Ccsd,
            QmMethod::CcsdT,
        ]
    }

    /// Parse a method keyword. Anything that is not a known wavefunction method
    /// is treated as a DFT functional name and validated when the job runs.
    pub fn parse(input: &str) -> QmMethod {
        match input.to_ascii_lowercase().as_str() {
            "rhf" => QmMethod::Rhf,
            "uhf" => QmMethod::Uhf,
            "rohf" => QmMethod::Rohf,
            "mp2" => QmMethod::Mp2,
            "ccsd" => QmMethod::Ccsd,
            "ccsd(t)" | "ccsdt" => QmMethod::CcsdT,
            other => QmMethod::Dft(other.to_string()),
        }
    }

    /// Human-readable label, e.g. `RHF` or `B3LYP`.
    pub fn label(&self) -> String {
        match self {
            QmMethod::Rhf => "RHF".to_string(),
            QmMethod::Uhf => "UHF".to_string(),
            QmMethod::Rohf => "ROHF".to_string(),
            QmMethod::Mp2 => "MP2".to_string(),
            QmMethod::Ccsd => "CCSD".to_string(),
            QmMethod::CcsdT => "CCSD(T)".to_string(),
            QmMethod::Dft(name) => name.to_ascii_uppercase(),
        }
    }

    fn to_chemx(&self) -> Result<Method> {
        Ok(match self {
            QmMethod::Rhf => Method::Rhf,
            QmMethod::Uhf => Method::Uhf,
            QmMethod::Rohf => Method::Rohf,
            QmMethod::Mp2 => Method::Mp2,
            QmMethod::Ccsd => Method::Ccsd,
            QmMethod::CcsdT => Method::CcsdT,
            QmMethod::Dft(name) => {
                let spec = FunctionalSpec::parse(name).map_err(|_| {
                    anyhow!(
                        "unknown method or functional `{name}` \
                         (try rhf, uhf, rohf, mp2, ccsd, ccsd(t), \
                         or a functional like svwn/pbe/blyp/b3lyp/pbe0)"
                    )
                })?;
                Method::Dft(spec)
            }
        })
    }
}

/// Which calculation to run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QmKind {
    /// Energy at the current geometry. Does not move atoms.
    SinglePoint,
    /// Relax the geometry; the optimized coordinates are returned.
    Optimize,
    /// Harmonic vibrational frequencies at the current geometry.
    Frequencies,
}

impl QmKind {
    pub fn label(self) -> &'static str {
        match self {
            QmKind::SinglePoint => "single point",
            QmKind::Optimize => "geometry optimization",
            QmKind::Frequencies => "frequencies",
        }
    }
}

/// A request to run a quantum-chemistry calculation on `structure`.
#[derive(Debug, Clone)]
pub struct QmRequest {
    pub structure: Structure,
    pub method: QmMethod,
    /// Basis-set name (e.g. `sto-3g`, `6-31g`, `cc-pvdz`, `def2-svp`).
    pub basis: String,
    /// Net molecular charge.
    pub charge: i32,
    /// Spin multiplicity, `2S + 1` (1 = singlet).
    pub multiplicity: u32,
    pub kind: QmKind,
    /// Also compute dipole moment and Mulliken charges.
    pub compute_properties: bool,
}

/// The result of a quantum-chemistry calculation.
///
/// Dipole, charges, and frequencies are not exposed as structured fields: the
/// caller (console output and the GUI Output panel) consumes the formatted
/// [`Self::summary`]. Add structured fields here when a caller needs to read
/// them programmatically.
#[derive(Debug, Clone)]
pub struct QmOutcome {
    pub energy_hartree: f64,
    pub converged: bool,
    /// Present only for [`QmKind::Optimize`]: the relaxed structure (Å).
    pub optimized_structure: Option<Structure>,
    /// Pre-formatted, human-readable report of every result.
    pub summary: String,
}

/// Whether chemx will accept `symbol` as an element. Mirrors `Molecule::from_xyz`,
/// which takes either a chemical symbol (`"O"`) or an atomic number (`"8"`).
fn is_known_element(symbol: &str) -> bool {
    let symbol = symbol.trim();
    if symbol.is_empty() {
        return false;
    }
    match symbol.parse::<u32>() {
        Ok(z) => Element::from_z(z).is_ok(),
        Err(_) => Element::from_symbol(symbol).is_ok(),
    }
}

/// Build a chemx [`Molecule`] from one of our [`Structure`]s.
///
/// We round-trip through an XYZ string (Ångström) so chemx owns element-symbol
/// parsing and the Å→bohr conversion, then apply the net charge and spin.
fn molecule_from_structure(
    structure: &Structure,
    charge: i32,
    multiplicity: u32,
) -> Result<Molecule> {
    // The structure editor accepts free-text element symbols, so an atom may
    // carry an invalid entry (a typo, a stray character, or a blank). Validate
    // up front to name the offending atom — chemx's own parse error does not.
    for (index, atom) in structure.atoms.iter().enumerate() {
        if !is_known_element(&atom.element) {
            bail!(
                "atom {} has an invalid element symbol `{}`; set a real element \
                 (e.g. C, N, O) in the structure editor before running a QM calculation",
                index + 1,
                atom.element
            );
        }
    }

    let molecule = Molecule::from_xyz(&to_xyz(structure))
        .context("chemx could not parse the molecule geometry")?
        .with_charge(charge)
        .with_multiplicity(multiplicity);
    molecule
        .validate()
        .context("invalid charge / spin multiplicity for this molecule")?;
    Ok(molecule)
}

/// Graft optimized bohr coordinates back onto a copy of `original` (Å).
fn structure_with_positions(original: &Structure, positions: &[[f64; 3]]) -> Result<Structure> {
    if positions.len() != original.atoms.len() {
        bail!(
            "optimizer returned {} atoms but the structure has {}",
            positions.len(),
            original.atoms.len()
        );
    }
    let mut relaxed = original.clone();
    for (atom, p) in relaxed.atoms.iter_mut().zip(positions) {
        atom.position.x = (p[0] * BOHR_TO_ANGSTROM) as f32;
        atom.position.y = (p[1] * BOHR_TO_ANGSTROM) as f32;
        atom.position.z = (p[2] * BOHR_TO_ANGSTROM) as f32;
    }
    Ok(relaxed)
}

/// Run a quantum-chemistry calculation.
///
/// `report` receives coarse stage strings (`"running scf"`, …). `cancel` is
/// **best-effort**: it is honored before the calculation starts, but chemx's
/// `Job::run` is a single opaque call with no preemption hook, so an in-flight
/// SCF cannot be interrupted — the worker runs to completion and the caller
/// discards the result.
pub fn run_qm(
    request: QmRequest,
    cancel: Arc<AtomicBool>,
    mut report: impl FnMut(&str),
) -> Result<QmOutcome> {
    let QmRequest {
        structure,
        method,
        basis,
        charge,
        multiplicity,
        kind,
        compute_properties,
    } = request;

    if structure.atoms.is_empty() {
        bail!("the structure has no atoms to compute");
    }
    if cancel.load(Ordering::Relaxed) {
        bail!("cancelled before the calculation started");
    }

    report("preparing molecule");
    let molecule = molecule_from_structure(&structure, charge, multiplicity)?;
    let chemx_method = method.to_chemx()?;

    let options = JobOptions {
        optimize_geometry: kind == QmKind::Optimize,
        compute_frequencies: kind == QmKind::Frequencies,
        compute_properties: compute_properties || kind == QmKind::Frequencies,
        ..JobOptions::default()
    };

    if cancel.load(Ordering::Relaxed) {
        bail!("cancelled before the calculation started");
    }
    report(match kind {
        QmKind::SinglePoint => "running scf",
        QmKind::Optimize => "optimizing geometry",
        QmKind::Frequencies => "running scf and hessian",
    });

    let job = Job {
        molecule,
        basis: basis.clone(),
        method: chemx_method,
        options,
    };
    let result = job
        .run()
        .map_err(|e| anyhow!("chemx calculation failed: {e}"))?;

    report("collecting results");

    let energy_hartree = result.best_energy();
    let converged = result.converged();

    let optimized_structure = match (kind, &result.optimized_geometry) {
        (QmKind::Optimize, Some(opt)) => {
            let mut relaxed = structure_with_positions(&structure, &opt.positions)?;
            // Distinguish the relaxed copy from the original in the entry list.
            relaxed.title = format!("{} ({}/{} opt)", structure.title, method.label(), basis);
            Some(relaxed)
        }
        _ => None,
    };

    let summary = format_summary(&method, &basis, kind, &structure, &result);

    Ok(QmOutcome {
        energy_hartree,
        converged,
        optimized_structure,
        summary,
    })
}

/// Render a human-readable report of `result` for the console and Output panel.
fn format_summary(
    method: &QmMethod,
    basis: &str,
    kind: QmKind,
    structure: &Structure,
    result: &JobResult,
) -> String {
    let energy = result.best_energy();
    let mut out = format!("{}/{} {}\n", method.label(), basis, kind.label());
    if let Some(opt) = &result.optimized_geometry {
        out.push_str(&format!("  optimization steps: {}\n", opt.iterations));
    }
    out.push_str(&format!(
        "  total energy: {energy:.8} Eh  ({:.4} eV, {:.3} kcal/mol)\n",
        energy * HARTREE_TO_EV,
        energy * HARTREE_TO_KCAL
    ));
    out.push_str(&format!(
        "  converged: {}\n",
        if result.converged() { "yes" } else { "no" }
    ));
    if let Some(props) = &result.properties {
        let dipole =
            (props.dipole_au[0].powi(2) + props.dipole_au[1].powi(2) + props.dipole_au[2].powi(2))
                .sqrt()
                * AU_DIPOLE_TO_DEBYE;
        out.push_str(&format!("  dipole: {dipole:.4} D\n"));
        out.push_str("  Mulliken charges:\n");
        for (atom, q) in structure
            .atoms
            .iter()
            .zip(&props.population.mulliken_charges)
        {
            out.push_str(&format!("    {:<2} {q:+.4}\n", atom.element));
        }
    }
    if let Some(freq) = &result.frequencies {
        // Drop the ~zero translational/rotational modes for readability.
        let modes: Vec<String> = freq
            .frequencies
            .frequencies_cm1
            .iter()
            .filter(|f| f.abs() > 1.0)
            .map(|f| format!("{f:.1}"))
            .collect();
        out.push_str(&format!("  frequencies (cm^-1): {}\n", modes.join(", ")));
        out.push_str(&format!(
            "  imaginary modes: {}\n",
            freq.frequencies.n_imaginary
        ));
    }
    out.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Atom, Structure};
    use nalgebra::Point3;

    fn h2() -> Structure {
        // H2 near its equilibrium bond length (~0.74 Å).
        Structure::new(
            "h2",
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

    fn no_cancel() -> Arc<AtomicBool> {
        Arc::new(AtomicBool::new(false))
    }

    #[test]
    fn rhf_sto3g_h2_single_point_energy() {
        let outcome = run_qm(
            QmRequest {
                structure: h2(),
                method: QmMethod::Rhf,
                basis: "sto-3g".to_string(),
                charge: 0,
                multiplicity: 1,
                kind: QmKind::SinglePoint,
                compute_properties: false,
            },
            no_cancel(),
            |_| {},
        )
        .expect("RHF/STO-3G on H2 should succeed");

        assert!(outcome.converged);
        // RHF/STO-3G H2 at 0.74 Å is about -1.117 Eh.
        assert!(
            (outcome.energy_hartree - (-1.117)).abs() < 0.02,
            "unexpected H2 energy: {}",
            outcome.energy_hartree
        );
        assert!(outcome.optimized_structure.is_none());
    }

    #[test]
    fn structure_to_molecule_preserves_atoms() {
        let mol = molecule_from_structure(&h2(), 0, 1).expect("valid molecule");
        assert_eq!(mol.len(), 2);
    }

    #[test]
    fn invalid_spin_is_rejected() {
        // Two electrons (H2, neutral) cannot be a doublet.
        let err = molecule_from_structure(&h2(), 0, 2);
        assert!(err.is_err());
    }

    #[test]
    fn invalid_element_is_rejected_with_atom_index() {
        // A hand-drawn atom whose element was mistyped (e.g. a stray bracket).
        let mut structure = h2();
        structure.atoms[1].element = "（".to_string();
        let message = molecule_from_structure(&structure, 0, 1)
            .expect_err("invalid element should be rejected")
            .to_string();
        assert!(
            message.contains("atom 2") && message.contains("（"),
            "error should name the offending atom and symbol: {message}"
        );
    }

    #[test]
    fn blank_element_is_rejected() {
        let mut structure = h2();
        structure.atoms[0].element = "  ".to_string();
        assert!(molecule_from_structure(&structure, 0, 1).is_err());
    }

    #[test]
    fn optimize_h2_returns_relaxed_structure() {
        let outcome = run_qm(
            QmRequest {
                structure: h2(),
                method: QmMethod::Rhf,
                basis: "sto-3g".to_string(),
                charge: 0,
                multiplicity: 1,
                kind: QmKind::Optimize,
                compute_properties: false,
            },
            no_cancel(),
            |_| {},
        )
        .expect("RHF/STO-3G H2 optimization should succeed");

        let relaxed = outcome
            .optimized_structure
            .expect("optimize should return a structure");
        assert_eq!(relaxed.atoms.len(), 2);
        // Optimized H-H bond length should be a sane, positive value in Å.
        let d = (relaxed.atoms[1].position - relaxed.atoms[0].position).norm();
        assert!(
            (0.5..1.2).contains(&d),
            "optimized H-H distance out of range: {d} Å"
        );
    }
}
