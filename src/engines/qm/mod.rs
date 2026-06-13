//! In-process quantum-chemistry engine.
//!
//! Wraps the `chemx` crate (pure-Rust HF/DFT/MP2/CC) so the rest of the app can
//! request single-point energies, geometry optimization, and properties or
//! vibrational frequencies from a [`Structure`] without knowing chemx's types.
//! Unlike the GROMACS engine this is a library call — it runs in-process on a
//! worker thread, not as an external subprocess.
//!
//! The request/outcome edge ([`QmRequest`], [`QmOutcome`]) deliberately keeps
//! chemx's types off the public API: every chemx option silicolab exposes is
//! mirrored by a plain enum/struct here, and every chemx result field we report
//! is folded into the formatted [`QmOutcome::summary`]. That boundary is what
//! would let a future build run chemx as an out-of-process engine (see the
//! `chemx` binary) without touching any caller.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result, anyhow, bail};
use chemx::composite::{Composite, composite};
use chemx::dft::FunctionalSpec;
use chemx::disp::Dispersion;
use chemx::props::thermo::QRRHO_W0_DEFAULT_CM1;
use chemx::scf::{Reference, Smearing};
use chemx::{
    BasisSet, Element, Job, JobOptions, JobResult, Method, Molecule, PostHfResult, ecp_summary,
};

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
///
/// [`QmMethod::Composite`] additionally covers chemx's "3c" composites
/// (r2scan-3c, b3lyp-3c, b97-3c, pbeh-3c), which bundle a functional, an implied
/// basis, dispersion, and short-range corrections under one keyword.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QmMethod {
    /// Hartree–Fock; picks RHF or UHF from the spin multiplicity.
    Hf,
    Rhf,
    Uhf,
    Rohf,
    Mp2,
    Ccsd,
    CcsdT,
    /// Kohn–Sham DFT with the named functional (e.g. `b3lyp`, `pbe`, `r2scan`,
    /// `m06-2x`, `wb97m-v`, or a double hybrid like `b2plyp`).
    Dft(String),
    /// A composite ("3c") method, by keyword (`r2scan-3c`, `b3lyp-3c`,
    /// `b97-3c`, `pbeh-3c`). Carries its own basis and corrections.
    Composite(String),
}

impl QmMethod {
    /// Methods offered in the GUI dropdown, in display order. A free-text
    /// functional field in the panel covers anything not listed here.
    pub fn presets() -> Vec<QmMethod> {
        vec![
            // Composites first: robust, batteries-included production defaults.
            QmMethod::Composite("r2scan-3c".to_string()),
            QmMethod::Composite("b97-3c".to_string()),
            QmMethod::Composite("pbeh-3c".to_string()),
            QmMethod::Composite("b3lyp-3c".to_string()),
            // Wavefunction methods.
            QmMethod::Hf,
            QmMethod::Rhf,
            QmMethod::Uhf,
            QmMethod::Rohf,
            QmMethod::Mp2,
            QmMethod::Ccsd,
            QmMethod::CcsdT,
            // Common functionals.
            QmMethod::Dft("b3lyp".to_string()),
            QmMethod::Dft("pbe0".to_string()),
            QmMethod::Dft("pbe".to_string()),
            QmMethod::Dft("blyp".to_string()),
            QmMethod::Dft("tpss".to_string()),
            QmMethod::Dft("r2scan".to_string()),
            QmMethod::Dft("m06-2x".to_string()),
            QmMethod::Dft("wb97x-v".to_string()),
            QmMethod::Dft("wb97m-v".to_string()),
            QmMethod::Dft("b2plyp".to_string()),
            QmMethod::Dft("svwn".to_string()),
        ]
    }

    /// Parse a method keyword, honoring composite keywords and a trailing
    /// `-d3`/`-d4` dispersion suffix (returned separately, since dispersion is
    /// an independent option). Anything that is not a known wavefunction method
    /// or composite is treated as a DFT functional name and validated when the
    /// job runs. Mirrors chemx-cli's method resolution.
    pub fn parse(input: &str) -> (QmMethod, Option<QmDispersion>) {
        let lower = input.trim().to_ascii_lowercase();
        // Split off a dispersion suffix first. Composites define their own
        // dispersion, so a suffix on a composite is left for the job layer to
        // reject (the base keyword still resolves to the composite).
        let (base, dispersion) = if let Some(b) = lower.strip_suffix("-d3") {
            (b.to_string(), Some(QmDispersion::D3Bj))
        } else if let Some(b) = lower.strip_suffix("-d4") {
            (b.to_string(), Some(QmDispersion::D4))
        } else {
            (lower.clone(), None)
        };
        // A composite keyword (e.g. `r2scan-3c`) must be checked against the
        // *unstripped* name — `-d3`/`-d4` are not composite suffixes.
        if composite(&lower).is_some() {
            return (QmMethod::Composite(lower), None);
        }
        let method = match base.as_str() {
            "hf" => QmMethod::Hf,
            "rhf" => QmMethod::Rhf,
            "uhf" => QmMethod::Uhf,
            "rohf" => QmMethod::Rohf,
            "mp2" => QmMethod::Mp2,
            "ccsd" => QmMethod::Ccsd,
            "ccsd(t)" | "ccsdt" => QmMethod::CcsdT,
            other => QmMethod::Dft(other.to_string()),
        };
        (method, dispersion)
    }

    /// Human-readable label, e.g. `RHF`, `B3LYP`, or `r2scan-3c`.
    pub fn label(&self) -> String {
        match self {
            QmMethod::Hf => "HF".to_string(),
            QmMethod::Rhf => "RHF".to_string(),
            QmMethod::Uhf => "UHF".to_string(),
            QmMethod::Rohf => "ROHF".to_string(),
            QmMethod::Mp2 => "MP2".to_string(),
            QmMethod::Ccsd => "CCSD".to_string(),
            QmMethod::CcsdT => "CCSD(T)".to_string(),
            QmMethod::Dft(name) => name.to_ascii_uppercase(),
            // Composites are conventionally written lower-case (r2scan-3c).
            QmMethod::Composite(kw) => kw.clone(),
        }
    }

    /// True for the post-Hartree–Fock correlated methods, which reject
    /// dispersion, most SCF backends, and the SCF-only options.
    fn is_post_hf(&self) -> bool {
        matches!(self, QmMethod::Mp2 | QmMethod::Ccsd | QmMethod::CcsdT)
    }
}

/// A `-d3`/`-d4` dispersion correction added on top of an SCF-level method.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QmDispersion {
    /// Grimme D3 with Becke–Johnson damping.
    D3Bj,
    /// DFT-D4 (EEQ charges + ATM three-body term).
    D4,
}

impl QmDispersion {
    pub fn label(self) -> &'static str {
        match self {
            QmDispersion::D3Bj => "D3(BJ)",
            QmDispersion::D4 => "D4",
        }
    }
}

/// Which SCF integral backend to use. `InCore` stores the full ERI tensor (the
/// default; required for properties, frequencies, optimization, and post-HF).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum QmScfBackend {
    /// Conventional in-core SCF (stores the nao⁴ ERI tensor).
    #[default]
    InCore,
    /// Integral-direct SCF: recompute ERIs each iteration (reaches larger
    /// systems, slower; single points only).
    Direct,
    /// Density-fitted RI-JK SCF over the def2-universal-jkfit auxiliary set
    /// (single points only; small fitting error).
    RiJk,
    /// COSX semi-numerical exchange (HF and hybrid DFT single points).
    Cosx,
}

impl QmScfBackend {
    pub fn label(self) -> &'static str {
        match self {
            QmScfBackend::InCore => "conventional in-core",
            QmScfBackend::Direct => "integral-direct",
            QmScfBackend::RiJk => "RI-JK density fitting",
            QmScfBackend::Cosx => "COSX semi-numerical exchange",
        }
    }
}

/// An implicit-solvation model. The continuum models (`Cpcm`, `Smd`) enter the
/// SCF; `Alpb`/`Gbsa` are post-SCF corrections on the converged Mulliken
/// charges. At most one applies to a calculation.
#[derive(Debug, Clone, PartialEq)]
pub enum QmSolvation {
    /// C-PCM electrostatics: a named solvent (resolved to its dielectric) or an
    /// explicit dielectric constant ε.
    Cpcm(CpcmDielectric),
    /// SMD universal solvation (named solvent from chemx's bundled library).
    Smd(String),
    /// ALPB implicit solvation (xtb GFN2 parameters).
    Alpb(String),
    /// GBSA implicit solvation (xtb GFN2 parameters).
    Gbsa(String),
}

/// How a C-PCM run fixes its dielectric constant.
#[derive(Debug, Clone, PartialEq)]
pub enum CpcmDielectric {
    /// A named solvent (e.g. `water`), resolved to ε from chemx's table.
    Named(String),
    /// An explicit dielectric constant.
    Epsilon(f64),
}

/// Advanced options for a [`QmRequest`], mirroring `chemx::JobOptions`. Every
/// field defaults to chemx's own default, so `QmOptions::default()` reproduces a
/// plain SCF single point.
#[derive(Debug, Clone)]
pub struct QmOptions {
    /// Also compute dipole moment, Mulliken/Löwdin charges, and Mayer bond
    /// orders after the SCF.
    pub compute_properties: bool,
    /// Add a D3(BJ)/D4 dispersion correction (SCF-level methods only).
    pub dispersion: Option<QmDispersion>,
    /// Implicit-solvation model.
    pub solvation: Option<QmSolvation>,
    /// SCF integral backend.
    pub scf_backend: QmScfBackend,
    /// Density-fit the MP2 step (RI-MP2; `--method mp2` only).
    pub ri_mp2: bool,
    /// Scalar-relativistic X2C-1e one-electron Hamiltonian.
    pub x2c: bool,
    /// Correlate all orbitals (disable the noble-gas frozen core) for post-HF.
    pub all_electron: bool,
    /// DFT integration grid level 0–4. `None` uses chemx's per-method default.
    pub grid_level: Option<usize>,
    /// Fermi-Dirac fractional-occupation smearing at this electronic
    /// temperature (kelvin). Energy-only.
    pub smearing_temperature_k: Option<f64>,
    /// Grimme FOD multireference diagnostic (implies a TPSS/def2-TZVP default).
    pub fod: bool,
    /// Single-point Hessian treatment of a frequency run (geometry taken
    /// as-is; gradient direction projected out). Approximate.
    pub single_point_hessian: bool,
    /// Rotational symmetry number σ for RRHO entropy.
    pub symmetry_number: u32,
    /// Quasi-RRHO interpolation frequency ω₀ (cm⁻¹) for the mRRHO entropy.
    pub qrrho_w0_cm1: f64,
}

impl Default for QmOptions {
    fn default() -> Self {
        Self {
            compute_properties: false,
            dispersion: None,
            solvation: None,
            scf_backend: QmScfBackend::default(),
            ri_mp2: false,
            x2c: false,
            all_electron: false,
            grid_level: None,
            smearing_temperature_k: None,
            fod: false,
            single_point_hessian: false,
            symmetry_number: 1,
            qrrho_w0_cm1: QRRHO_W0_DEFAULT_CM1,
        }
    }
}

/// Which calculation to run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QmKind {
    /// Energy at the current geometry. Does not move atoms.
    SinglePoint,
    /// Relax the geometry; the optimized coordinates are returned.
    Optimize,
    /// Harmonic vibrational frequencies and thermochemistry at the current
    /// geometry.
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
    /// Basis-set name (e.g. `sto-3g`, `6-31g`, `cc-pvdz`, `def2-svp`). Ignored
    /// for [`QmMethod::Composite`], which carries its own implied basis.
    pub basis: String,
    /// Net molecular charge.
    pub charge: i32,
    /// Spin multiplicity, `2S + 1` (1 = singlet).
    pub multiplicity: u32,
    pub kind: QmKind,
    /// Advanced chemx options (dispersion, solvation, SCF backend, …).
    pub options: QmOptions,
}

/// The result of a quantum-chemistry calculation.
///
/// Structured fields cover what callers read programmatically (energy, the
/// optimized geometry); everything chemx reports — properties, frequencies,
/// thermochemistry, dispersion/solvation breakdowns, diagnostics, and the
/// method-quality warnings — is folded into the formatted [`Self::summary`].
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

/// The resolved chemx [`Job`] plus the bits of context the summary needs that
/// are not recoverable from [`JobResult`] alone (the displayed basis, the
/// composite registry entry, and whether scalar relativity was on).
struct ResolvedJob {
    job: Job,
    /// The basis actually used (a composite overrides the request's basis).
    basis: String,
    /// The composite registry entry, when the method is a composite.
    composite: Option<&'static Composite>,
    /// Whether the X2C-1e Hamiltonian is active (for the report caveat).
    x2c: bool,
    /// The named solvent, for the C-PCM report line (`None` for a bare ε).
    cpcm_solvent: Option<String>,
}

/// Resolve `request` into a chemx [`Job`], mapping every silicolab option onto
/// `JobOptions`/`Method` exactly as chemx-cli does. CLI-level incompatibilities
/// that chemx's `Job::run` does not itself catch are rejected here with a
/// pointed message; the deeper physics guards are left to chemx.
fn build_job(
    structure: &Structure,
    method: &QmMethod,
    basis: &str,
    charge: i32,
    multiplicity: u32,
    kind: QmKind,
    options: &QmOptions,
) -> Result<ResolvedJob> {
    let molecule = molecule_from_structure(structure, charge, multiplicity)?;

    // Composite resolution: a composite fixes the functional, basis, grid,
    // dispersion, and gCP/SRB corrections, and forbids a conflicting basis or
    // an extra dispersion suffix.
    let comp = match method {
        QmMethod::Composite(kw) => Some(composite(kw).ok_or_else(|| {
            anyhow!(
                "unknown composite method `{kw}` (expected r2scan-3c, b97-3c, pbeh-3c, b3lyp-3c)"
            )
        })?),
        _ => None,
    };
    if comp.is_some() && options.dispersion.is_some() {
        bail!(
            "a composite method defines its own dispersion; remove the -d3/-d4 dispersion option"
        );
    }

    let chemx_method = resolve_chemx_method(method, multiplicity, comp)?;
    let resolved_basis = match comp {
        Some(c) => c.basis.to_string(),
        None => basis.to_string(),
    };

    // Dispersion: a composite's own parametrization, or a `-d3`/`-d4` request
    // keyed by the method (mirrors chemx-cli's param-key derivation).
    let dispersion = match (comp, options.dispersion) {
        (Some(c), _) => Some(c.dispersion),
        (None, Some(disp)) => Some(resolve_dispersion(method, &chemx_method, disp)?),
        (None, None) => None,
    };

    // SCF backend.
    let direct = options.scf_backend == QmScfBackend::Direct;
    let ri = options.scf_backend == QmScfBackend::RiJk;
    let cosx = options.scf_backend == QmScfBackend::Cosx;

    // Grid level: explicit override, else the composite's recommended grid,
    // else a grid-sensitive functional's recommended level, else chemx's 3.
    let grid_level = options.grid_level.unwrap_or_else(|| {
        comp.map(|c| c.grid_level)
            .unwrap_or_else(|| match &chemx_method {
                Method::Dft(spec) if spec.grid_sensitive() => spec.recommended_grid_level(),
                _ => 3,
            })
    });

    // Solvation → the matching JobOptions fields (at most one is set).
    let mut solvent_eps = None;
    let mut smd = None;
    let mut alpb = None;
    let mut gbsa = None;
    let mut cpcm_solvent = None;
    if let Some(solv) = &options.solvation {
        match solv {
            QmSolvation::Cpcm(CpcmDielectric::Named(name)) => {
                let eps = chemx::solv::solvent_epsilon(name).ok_or_else(|| {
                    let names: Vec<&str> = chemx::solv::SOLVENTS.iter().map(|(n, _)| *n).collect();
                    anyhow!(
                        "unknown C-PCM solvent `{name}` (available: {}; or give an explicit ε)",
                        names.join(", ")
                    )
                })?;
                solvent_eps = Some(eps);
                cpcm_solvent = Some(name.clone());
            }
            QmSolvation::Cpcm(CpcmDielectric::Epsilon(eps)) => solvent_eps = Some(*eps),
            QmSolvation::Smd(name) => smd = Some(name.clone()),
            QmSolvation::Alpb(name) => alpb = Some(name.clone()),
            QmSolvation::Gbsa(name) => gbsa = Some(name.clone()),
        }
    }

    let job_options = JobOptions {
        all_electron: options.all_electron,
        direct,
        ri,
        compute_properties: options.compute_properties,
        compute_frequencies: kind == QmKind::Frequencies,
        single_point_hessian: options.single_point_hessian,
        optimize_geometry: kind == QmKind::Optimize,
        symmetry_number: options.symmetry_number,
        qrrho_w0_cm1: options.qrrho_w0_cm1,
        grid_level,
        dispersion,
        solvent_eps,
        smd,
        alpb,
        gbsa,
        cosmo_file: None,
        gcp: comp.and_then(|c| c.gcp),
        srb: comp.and_then(|c| c.srb),
        smearing: options
            .smearing_temperature_k
            .map(|temperature_k| Smearing::Fermi { temperature_k }),
        fod: options.fod,
        fod_cube: None,
        ri_mp2: options.ri_mp2,
        cosx,
        x2c: options.x2c,
    };

    Ok(ResolvedJob {
        job: Job {
            molecule,
            basis: resolved_basis.clone(),
            method: chemx_method,
            options: job_options,
        },
        basis: resolved_basis,
        composite: comp,
        x2c: options.x2c,
        cpcm_solvent,
    })
}

/// Map a [`QmMethod`] to a chemx [`Method`]. A composite runs its plain
/// functional (the corrections are added at the job layer).
fn resolve_chemx_method(
    method: &QmMethod,
    multiplicity: u32,
    comp: Option<&'static Composite>,
) -> Result<Method> {
    if let Some(c) = comp {
        let spec = FunctionalSpec::parse(c.functional)
            .map_err(|e| anyhow!("composite functional `{}`: {e}", c.functional))?;
        return Ok(Method::Dft(spec));
    }
    Ok(match method {
        // `hf` picks RHF/UHF from the multiplicity, like the DFT methods do.
        QmMethod::Hf => {
            if multiplicity > 1 {
                Method::Uhf
            } else {
                Method::Rhf
            }
        }
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
                     (try hf, rhf, uhf, rohf, mp2, ccsd, ccsd(t), a composite like \
                     r2scan-3c, or a functional like pbe/b3lyp/r2scan/wb97m-v)"
                )
            })?;
            Method::Dft(spec)
        }
        QmMethod::Composite(_) => unreachable!("composites resolved above"),
    })
}

/// Resolve a `-d3`/`-d4` request for a non-composite method into a chemx
/// [`Dispersion`], keyed by the method (mirrors chemx-cli lines 613–646).
fn resolve_dispersion(
    method: &QmMethod,
    chemx_method: &Method,
    disp: QmDispersion,
) -> Result<Dispersion> {
    if method.is_post_hf() {
        bail!(
            "{} dispersion is not supported for post-HF methods; it applies to HF and DFT",
            disp.label()
        );
    }
    let d4 = disp == QmDispersion::D4;
    let param_key = match chemx_method {
        Method::Rhf | Method::Uhf | Method::Rohf => "hf".to_string(),
        Method::Dft(spec) => spec
            .d4_param_set()
            .map(str::to_string)
            .unwrap_or_else(|| spec.name().to_string()),
        // Post-HF was rejected above; nothing else reaches here.
        _ => "hf".to_string(),
    };
    Dispersion::for_method(d4, &param_key).ok_or_else(|| {
        anyhow!(
            "no {} parametrization for `{param_key}` (supported: pbe, blyp, b3lyp, b3lyp5, \
             pbe0, tpss, r2scan, hf; D4 additionally: b2plyp, revdsd-pbep86, pwpb95)",
            disp.label()
        )
    })
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
        options,
    } = request;

    if structure.atoms.is_empty() {
        bail!("the structure has no atoms to compute");
    }
    if cancel.load(Ordering::Relaxed) {
        bail!("cancelled before the calculation started");
    }

    report("preparing molecule");
    let resolved = build_job(
        &structure,
        &method,
        &basis,
        charge,
        multiplicity,
        kind,
        &options,
    )?;

    if cancel.load(Ordering::Relaxed) {
        bail!("cancelled before the calculation started");
    }
    report(match kind {
        QmKind::SinglePoint => "running scf",
        QmKind::Optimize => "optimizing geometry",
        QmKind::Frequencies => "running scf and hessian",
    });

    let result = resolved
        .job
        .run()
        .map_err(|e| anyhow!("chemx calculation failed: {e}"))?;

    report("collecting results");

    let energy_hartree = result.best_energy();
    let converged = result.converged();

    let optimized_structure = match (kind, &result.optimized_geometry) {
        (QmKind::Optimize, Some(opt)) => {
            let mut relaxed = structure_with_positions(&structure, &opt.positions)?;
            // Distinguish the relaxed copy from the original in the entry list.
            relaxed.title = format!(
                "{} ({}/{} opt)",
                structure.title,
                method.label(),
                resolved.basis
            );
            Some(relaxed)
        }
        _ => None,
    };

    let summary = format_summary(&method, &resolved, kind, &structure, &result);

    Ok(QmOutcome {
        energy_hartree,
        converged,
        optimized_structure,
        summary,
    })
}

/// Render a human-readable report of `result` covering every section chemx
/// populates. Mirrors chemx-cli's `report*` formatters (trimming only the
/// per-iteration SCF/optimizer history tables, which are noise in a panel).
fn format_summary(
    method: &QmMethod,
    resolved: &ResolvedJob,
    kind: QmKind,
    structure: &Structure,
    result: &JobResult,
) -> String {
    let mut out = String::new();
    let basis = &resolved.basis;

    // Header.
    out.push_str(&format!("{}/{} {}\n", method.label(), basis, kind.label()));

    // Effective core potentials in use, when any.
    if let Ok(set) = BasisSet::load(basis) {
        let ecp = ecp_summary(&resolved.job.molecule, &set);
        if !ecp.is_empty() {
            let list = ecp
                .iter()
                .map(|(sym, z, n_core)| format!("{sym} (Z={z}, {n_core} core e⁻ replaced)"))
                .collect::<Vec<_>>()
                .join(", ");
            out.push_str(&format!("  effective core potentials: {list}\n"));
        }
    }

    // Geometry-optimization summary.
    if let Some(opt) = &result.optimized_geometry {
        out.push_str(&format!(
            "  optimization: {} in {} steps\n",
            if opt.converged {
                "converged"
            } else {
                "NOT converged"
            },
            opt.iterations
        ));
    }

    // SCF backend / density-fitting / COSX diagnostics.
    if let Some(ri) = &result.ri {
        out.push_str(&format!(
            "  RI-JK density fitting: aux {} ({} aux fns)\n",
            ri.aux_basis, ri.naux
        ));
    }
    if let Some(cosx) = &result.cosx {
        out.push_str(&format!(
            "  COSX exchange: grid {} ({} points{})\n",
            cosx.grid,
            cosx.n_points,
            if cosx.overlap_fitted {
                ", overlap-fitted"
            } else {
                ""
            }
        ));
    }

    // Energies and convergence.
    let energy = result.best_energy();
    out.push_str(&format!(
        "  total energy: {energy:.8} Eh  ({:.4} eV, {:.3} kcal/mol)\n",
        energy * HARTREE_TO_EV,
        energy * HARTREE_TO_KCAL
    ));
    out.push_str(&format!("  SCF energy:   {:.8} Eh\n", result.scf.energy));
    out.push_str(&format!(
        "  converged: {}\n",
        if result.converged() { "yes" } else { "no" }
    ));

    // HOMO–LUMO gap and spin contamination.
    let (gap_a, gap_b) = result.scf.homo_lumo_gap();
    if result.scf.reference == Reference::Uhf {
        if let Some(g) = gap_a {
            out.push_str(&format!("  HOMO-LUMO gap (α): {g:.4} Eh\n"));
        }
        if let Some(g) = gap_b {
            out.push_str(&format!("  HOMO-LUMO gap (β): {g:.4} Eh\n"));
        }
        out.push_str(&format!("  <S^2>: {:.4}\n", result.scf.spin_squared));
    } else if let Some(g) = gap_a {
        out.push_str(&format!("  HOMO-LUMO gap: {g:.4} Eh\n"));
    }

    // Kohn–Sham DFT diagnostics.
    if let Some(dft) = &result.dft {
        out.push_str(&format!(
            "  DFT: {} (grid level {}, {} points",
            dft.functional_name, dft.grid_level, dft.n_grid_points
        ));
        if dft.exx_fraction > 0.0 {
            out.push_str(&format!(
                ", {:.0}% exact exchange",
                dft.exx_fraction * 100.0
            ));
        }
        out.push_str(")\n");
        if let Some(exc) = result.scf.xc_energy {
            out.push_str(&format!("  E_xc: {exc:.8} Eh\n"));
        }
    }

    // X2C scalar-relativistic note.
    if resolved.x2c {
        out.push_str(
            "  X2C-1e scalar-relativistic Hamiltonian active (2e integrals nonrelativistic)\n",
        );
    }

    push_composite_or_dispersion(&mut out, resolved, result);
    push_double_hybrid(&mut out, result);
    push_solvation(&mut out, resolved.cpcm_solvent.as_deref(), result);
    push_smearing(&mut out, result);
    push_fod(&mut out, result);
    push_post_hf(&mut out, result);
    push_properties(&mut out, structure, result);
    push_frequencies(&mut out, result);
    push_method_warnings(&mut out, result);

    out.trim_end().to_string()
}

/// Composite ("3c") breakdown, or a plain dispersion correction.
fn push_composite_or_dispersion(out: &mut String, resolved: &ResolvedJob, result: &JobResult) {
    if let Some(c) = resolved.composite {
        out.push_str(&format!("  {} composite:\n", c.keyword));
        out.push_str(&format!(
            "    E_SCF ({}): {:.8} Eh\n",
            c.functional, result.scf.energy
        ));
        if let Some(e) = result.dispersion_energy {
            out.push_str(&format!("    E_{}: {e:.8} Eh\n", c.disp_label));
        }
        if let Some(e) = result.gcp_energy {
            out.push_str(&format!("    E_gCP: {e:.8} Eh\n"));
        }
        if let Some(e) = result.srb_energy {
            out.push_str(&format!("    E_SRB: {e:.8} Eh\n"));
        }
    } else if let (Some(e), None) = (result.dispersion_energy, &result.double_hybrid) {
        out.push_str(&format!("  dispersion: {e:.8} Eh\n"));
    }
}

/// Double-hybrid SCF + PT2 breakdown.
fn push_double_hybrid(out: &mut String, result: &JobResult) {
    let Some(dh) = &result.double_hybrid else {
        return;
    };
    out.push_str(&format!(
        "  double hybrid {} (PT2 on {} orbitals):\n",
        dh.functional_name, dh.scf_functional_name
    ));
    out.push_str(&format!("    E_SCF (no PT2): {:.8} Eh\n", dh.e_scf));
    out.push_str(&format!(
        "    E_PT2 (os+ss): {:.8} Eh  (c_os={:.4}, c_ss={:.4})\n",
        dh.pt2_energy(),
        dh.c_os,
        dh.c_ss
    ));
    if let Some(e) = result.vv10_energy {
        out.push_str(&format!(
            "    E_nl (VV10 ×{:.4}): {e:.8} Eh\n",
            dh.vv10_scale
        ));
    }
}

/// Implicit-solvation breakdown (SMD, ALPB/GBSA, or bare C-PCM). `cpcm_solvent`
/// is the named C-PCM solvent, when the run used one (rather than a bare ε).
fn push_solvation(out: &mut String, cpcm_solvent: Option<&str>, result: &JobResult) {
    const KCAL: f64 = HARTREE_TO_KCAL;
    if let Some(smd) = &result.smd {
        out.push_str(&format!(
            "  SMD solvation ({}, ε={:.4}):\n",
            smd.solvent, smd.epsilon
        ));
        out.push_str(&format!(
            "    ΔG_EP: {:.8} Eh ({:.3} kcal/mol)\n",
            smd.g_ep,
            smd.g_ep * KCAL
        ));
        out.push_str(&format!(
            "    G_CDS: {:.8} Eh ({:.3} kcal/mol)\n",
            smd.g_cds,
            smd.g_cds * KCAL
        ));
        out.push_str(&format!(
            "    ΔG_solv: {:.8} Eh ({:.3} kcal/mol)\n",
            smd.dg_solv,
            smd.dg_solv * KCAL
        ));
    } else if let Some(g) = &result.gbsa {
        out.push_str(&format!(
            "  {} solvation ({}, ε={:.4}):\n",
            g.model, g.solvent, g.epsilon
        ));
        out.push_str(&format!(
            "    ΔG_solv: {:.8} Eh ({:.3} kcal/mol)\n",
            g.g_solv,
            g.g_solv * KCAL
        ));
    } else if let Some(e) = result.scf.solvation_energy {
        match cpcm_solvent {
            Some(name) => out.push_str(&format!(
                "  C-PCM solvation ({name}): E_solv {e:.8} Eh (included in total)\n"
            )),
            None => out.push_str(&format!(
                "  C-PCM solvation: E_solv {e:.8} Eh (included in total)\n"
            )),
        }
    }
}

/// Fermi-smearing fractional-occupation summary.
fn push_smearing(out: &mut String, result: &JobResult) {
    let Some((occ_a, _occ_b)) = &result.scf.occupations else {
        return;
    };
    // Only report the dedicated smearing block when it is not the FOD run
    // (which has its own section below).
    if result.fod.is_some() {
        return;
    }
    let ts = result.scf.electronic_entropy.unwrap_or(0.0);
    let free = result.scf.free_energy.unwrap_or(result.scf.energy);
    let frac = occ_a
        .iter()
        .filter(|&&f| f > 1e-6 && f < 1.0 - 1e-6)
        .count();
    out.push_str("  Fermi smearing:\n");
    out.push_str(&format!("    fractionally occupied (α): {frac}\n"));
    out.push_str(&format!("    T·S_el: {ts:.8} Eh\n"));
    out.push_str(&format!("    free energy F = E − T·S_el: {free:.8} Eh\n"));
}

/// Grimme FOD multireference diagnostic.
fn push_fod(out: &mut String, result: &JobResult) {
    let Some(f) = &result.fod else {
        return;
    };
    out.push_str("  FOD diagnostic (Grimme):\n");
    out.push_str(&format!("    T_el: {:.0} K\n", f.temperature_k));
    out.push_str(&format!(
        "    N_FOD: {:.4}  ({:.4} α, {:.4} β)\n",
        f.n_fod, f.n_fod_alpha, f.n_fod_beta
    ));
    if f.n_fod >= 1.0 {
        out.push_str("    WARNING: N_FOD ≥ 1.0 — strong static correlation (multireference)\n");
    } else if f.n_fod >= 0.5 {
        out.push_str("    note: N_FOD in 0.5–1.0 — mild static correlation\n");
    }
}

/// Post-Hartree–Fock (MP2/CCSD/CCSD(T)) correlation breakdown.
fn push_post_hf(out: &mut String, result: &JobResult) {
    let Some(post) = &result.post_hf else {
        return;
    };
    match post {
        PostHfResult::Mp2 {
            result: r,
            n_frozen,
        } => {
            out.push_str(&format!("  MP2 (frozen core: {n_frozen}):\n"));
            out.push_str(&format!("    opposite-spin: {:.8} Eh\n", r.opposite_spin));
            out.push_str(&format!("    same-spin:     {:.8} Eh\n", r.same_spin));
            out.push_str(&format!(
                "    correlation:   {:.8} Eh\n",
                r.correlation_energy
            ));
            out.push_str(&format!("    total:         {:.8} Eh\n", r.total_energy));
        }
        PostHfResult::RiMp2 {
            result: r,
            n_frozen,
            aux_basis,
        } => {
            out.push_str(&format!(
                "  RI-MP2 (frozen core: {n_frozen}, aux {aux_basis}, {} aux fns):\n",
                r.naux
            ));
            out.push_str(&format!(
                "    correlation:   {:.8} Eh\n",
                r.correlation_energy
            ));
            out.push_str(&format!("    total:         {:.8} Eh\n", r.total_energy));
        }
        PostHfResult::Ccsd {
            result: r,
            n_frozen,
        } => push_ccsd(out, *n_frozen, r),
        PostHfResult::CcsdT {
            result: r,
            n_frozen,
        } => {
            push_ccsd(out, *n_frozen, &r.ccsd);
            out.push_str(&format!("  CCSD(T) triples: {:.8} Eh\n", r.triples_energy));
            out.push_str(&format!("  CCSD(T) total:   {:.8} Eh\n", r.total_energy));
        }
    }
}

fn push_ccsd(out: &mut String, n_frozen: usize, cc: &chemx::cc::CcsdResult) {
    out.push_str(&format!(
        "  CCSD (frozen core: {n_frozen}, {} in {} iters):\n",
        if cc.converged {
            "converged"
        } else {
            "NOT converged"
        },
        cc.iterations
    ));
    out.push_str(&format!(
        "    correlation:   {:.8} Eh\n",
        cc.correlation_energy
    ));
    out.push_str(&format!("    total:         {:.8} Eh\n", cc.total_energy));
    out.push_str(&format!("    T1 diagnostic: {:.4}\n", cc.t1_diagnostic));
    if cc.t1_diagnostic > 0.02 {
        out.push_str("    warning: T1 > 0.02 — single-reference CC may be unreliable\n");
    }
}

/// One-electron properties: dipole, atomic charges, Mayer bond orders.
fn push_properties(out: &mut String, structure: &Structure, result: &JobResult) {
    let Some(props) = &result.properties else {
        return;
    };
    let dipole =
        (props.dipole_au[0].powi(2) + props.dipole_au[1].powi(2) + props.dipole_au[2].powi(2))
            .sqrt()
            * AU_DIPOLE_TO_DEBYE;
    out.push_str(&format!("  dipole: {dipole:.4} D\n"));
    out.push_str("  atomic charges (Mulliken / Löwdin):\n");
    let pop = &props.population;
    for (i, atom) in structure.atoms.iter().enumerate() {
        let mulliken = pop.mulliken_charges.get(i).copied().unwrap_or(0.0);
        let lowdin = pop.lowdin_charges.get(i).copied().unwrap_or(0.0);
        out.push_str(&format!(
            "    {:<2} {mulliken:+.4} / {lowdin:+.4}\n",
            atom.element
        ));
    }
    // Mayer bond orders above a display threshold.
    let n = structure.atoms.len();
    let mut bonds = String::new();
    for i in 0..n {
        for j in (i + 1)..n {
            if let Some(row) = pop.mayer_bond_orders.get(i)
                && let Some(&b) = row.get(j)
                && b > 0.5
            {
                bonds.push_str(&format!(
                    "    {}{}–{}{}: {b:.3}\n",
                    structure.atoms[i].element,
                    i + 1,
                    structure.atoms[j].element,
                    j + 1
                ));
            }
        }
    }
    if !bonds.is_empty() {
        out.push_str("  Mayer bond orders:\n");
        out.push_str(&bonds);
    }
}

/// Harmonic frequencies and RRHO / quasi-RRHO thermochemistry.
fn push_frequencies(out: &mut String, result: &JobResult) {
    let Some(freq) = &result.frequencies else {
        return;
    };
    if freq.is_sph {
        out.push_str("  (single-point Hessian — geometry as-is, approximate frequencies)\n");
    }
    // Drop the ~zero translational/rotational modes for readability.
    let modes: Vec<String> = freq
        .frequencies
        .frequencies_cm1
        .iter()
        .filter(|f| f.abs() > 10.0)
        .map(|f| {
            if *f < 0.0 {
                format!("{:.1}i", -f)
            } else {
                format!("{f:.1}")
            }
        })
        .collect();
    out.push_str(&format!("  frequencies (cm^-1): {}\n", modes.join(", ")));
    out.push_str(&format!(
        "  imaginary modes: {}\n",
        freq.frequencies.n_imaginary
    ));

    let t = &freq.thermochemistry;
    out.push_str(&format!(
        "  thermochemistry at {:.2} K (σ={}, linear={}):\n",
        t.temperature, t.symmetry_number, t.is_linear
    ));
    out.push_str(&format!("    zero-point energy: {:.8} Eh\n", t.zpe));
    out.push_str(&format!("    enthalpy H: {:.8} Eh\n", t.enthalpy));
    out.push_str(&format!("    entropy S: {:.8} Eh/K\n", t.entropy));
    out.push_str(&format!("    Gibbs G (RRHO): {:.8} Eh\n", t.gibbs));
    out.push_str(&format!(
        "    Gibbs G (quasi-RRHO, ω₀={:.0} cm⁻¹): {:.8} Eh\n",
        t.qrrho_w0_cm1, t.gibbs_qrrho
    ));
}

/// chemx's method-quality guardrails (always populated; empty prints nothing).
fn push_method_warnings(out: &mut String, result: &JobResult) {
    if result.method_warnings.is_empty() {
        return;
    }
    out.push_str("  method-quality notes:\n");
    for w in &result.method_warnings {
        out.push_str(&format!("    - {w}\n"));
    }
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

    /// A bare single point, the simplest request.
    fn request(structure: Structure, method: QmMethod, basis: &str, kind: QmKind) -> QmRequest {
        QmRequest {
            structure,
            method,
            basis: basis.to_string(),
            charge: 0,
            multiplicity: 1,
            kind,
            options: QmOptions::default(),
        }
    }

    #[test]
    fn rhf_sto3g_h2_single_point_energy() {
        let outcome = run_qm(
            request(h2(), QmMethod::Rhf, "sto-3g", QmKind::SinglePoint),
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
    fn method_parse_splits_dispersion_suffix() {
        assert_eq!(
            QmMethod::parse("pbe-d4"),
            (QmMethod::Dft("pbe".to_string()), Some(QmDispersion::D4))
        );
        assert_eq!(
            QmMethod::parse("b3lyp-d3"),
            (QmMethod::Dft("b3lyp".to_string()), Some(QmDispersion::D3Bj))
        );
        // A composite keyword is recognized and carries no separate dispersion.
        assert_eq!(
            QmMethod::parse("r2scan-3c"),
            (QmMethod::Composite("r2scan-3c".to_string()), None)
        );
        assert_eq!(QmMethod::parse("ccsd(t)"), (QmMethod::CcsdT, None));
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
            request(h2(), QmMethod::Rhf, "sto-3g", QmKind::Optimize),
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

    #[test]
    fn dispersion_reported_in_summary() {
        let mut req = request(
            h2(),
            QmMethod::Dft("pbe".to_string()),
            "sto-3g",
            QmKind::SinglePoint,
        );
        req.options.dispersion = Some(QmDispersion::D3Bj);
        let outcome = run_qm(req, no_cancel(), |_| {}).expect("PBE-D3/STO-3G H2 should succeed");
        assert!(
            outcome.summary.contains("dispersion"),
            "summary should report the dispersion correction: {}",
            outcome.summary
        );
    }
}
