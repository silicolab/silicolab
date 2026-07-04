use super::*;

use hartree::composite::composite;
use hartree::props::thermo::QRRHO_W0_DEFAULT_CM1;
use serde::{Deserialize, Serialize};

use crate::domain::Structure;

/// Bohr → Ångström. hartree stores nuclear positions in bohr.
pub(crate) const BOHR_TO_ANGSTROM: f64 = 0.529_177_210_903;
/// Hartree → electron-volt.
pub(crate) const HARTREE_TO_EV: f64 = 27.211_386_245_988;
/// Hartree → kcal/mol.
pub(crate) const HARTREE_TO_KCAL: f64 = 627.509_474_063;
/// Atomic-unit dipole (e·a₀) → Debye.
pub(crate) const AU_DIPOLE_TO_DEBYE: f64 = 2.541_746_473;

/// Electronic-structure method. Mirrors `hartree::Method` but is parseable from a
/// console argument or UI dropdown and keeps the external type off our API edge.
///
/// [`QmMethod::Composite`] additionally covers hartree's "3c" composites
/// (r2scan-3c, b3lyp-3c, b97-3c, pbeh-3c), which bundle a functional, an implied
/// basis, dispersion, and short-range corrections under one keyword.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
            // DFT functionals by Jacob's-ladder rung. Mirrors the keywords
            // hartree's `FunctionalSpec::parse` accepts; the drift-guard test
            // `panel_functionals_are_all_recognized` asserts each still parses.
            QmMethod::Dft("b3lyp".to_string()),
            // b3lyp5 is the VWN5-correlation variant of b3lyp (vs the common
            // VWN3); surfaced for completeness, the trailing 5 marks it niche.
            QmMethod::Dft("b3lyp5".to_string()),
            QmMethod::Dft("pbe0".to_string()),
            QmMethod::Dft("pbe".to_string()),
            QmMethod::Dft("blyp".to_string()),
            QmMethod::Dft("tpss".to_string()),
            QmMethod::Dft("r2scan".to_string()),
            QmMethod::Dft("m06-2x".to_string()),
            QmMethod::Dft("pw6b95".to_string()),
            QmMethod::Dft("wb97x-v".to_string()),
            QmMethod::Dft("b97m-v".to_string()),
            QmMethod::Dft("wb97m-v".to_string()),
            QmMethod::Dft("b2plyp".to_string()),
            QmMethod::Dft("pwpb95".to_string()),
            QmMethod::Dft("revdsd-pbep86".to_string()),
            QmMethod::Dft("wb97m(2)".to_string()),
            QmMethod::Dft("svwn".to_string()),
        ]
    }

    /// Parse a method keyword, honoring composite keywords and a trailing
    /// `-d3`/`-d4` dispersion suffix (returned separately, since dispersion is
    /// an independent option). Anything that is not a known wavefunction method
    /// or composite is treated as a DFT functional name and validated when the
    /// job runs. Mirrors hartree-cli's method resolution.
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
    pub(crate) fn is_post_hf(&self) -> bool {
        matches!(self, QmMethod::Mp2 | QmMethod::Ccsd | QmMethod::CcsdT)
    }
}

/// Orbital basis sets the molecular QM panel offers, in the order they appear in
/// its dropdown: by family (Pople, Dunning, Karlsruhe), then ζ-level. Mirrors the
/// names `hartree::BasisSet::load` accepts as orbital sets — auxiliary/fitting
/// sets and the composite-only def2-m* bases are deliberately excluded. The
/// drift-guard test `panel_bases_are_all_loadable` asserts each still loads, so a
/// hartree bump that drops one is caught at test time rather than at run time.
pub const QM_BASIS_SETS: &[&str] = &[
    // Pople.
    "sto-3g",
    "6-31g",
    "6-311g",
    "6-311g(d,p)",
    "6-311+g(d,p)",
    "6-311++g(d,p)",
    // Dunning correlation-consistent.
    "cc-pvdz",
    "cc-pvtz",
    "cc-pvqz",
    "aug-cc-pvtz",
    // Karlsruhe def2.
    "def2-svp",
    "def2-svpd",
    "def2-tzvp",
    "def2-tzvpp",
    "def2-tzvpd",
    "def2-tzvppd",
    "def2-qzvp",
    "def2-qzvpp",
    // Minimally augmented Karlsruhe.
    "ma-def2-svp",
    "ma-def2-tzvp",
];

/// A `-d3`/`-d4` dispersion correction added on top of an SCF-level method.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum QmSolvation {
    /// C-PCM electrostatics: a named solvent (resolved to its dielectric) or an
    /// explicit dielectric constant ε.
    Cpcm(CpcmDielectric),
    /// SMD universal solvation (named solvent from hartree's bundled library).
    Smd(String),
    /// ALPB implicit solvation (xtb GFN2 parameters).
    Alpb(String),
    /// GBSA implicit solvation (xtb GFN2 parameters).
    Gbsa(String),
}

/// How a C-PCM run fixes its dielectric constant.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum CpcmDielectric {
    /// A named solvent (e.g. `water`), resolved to ε from hartree's table.
    Named(String),
    /// An explicit dielectric constant.
    Epsilon(f64),
}

/// Advanced options for a [`QmRequest`], mirroring `hartree::JobOptions`. Every
/// field defaults to hartree's own default, so `QmOptions::default()` reproduces a
/// plain SCF single point.
#[derive(Debug, Clone, Serialize, Deserialize)]
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
    /// DFT integration grid level 0–4. `None` uses hartree's per-method default.
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum QmKind {
    /// Energy at the current geometry. Does not move atoms.
    SinglePoint,
    /// Relax the geometry; the optimized coordinates are returned.
    Optimize,
    /// Harmonic vibrational frequencies and thermochemistry at the current
    /// geometry.
    Frequencies,
    /// Climb to a first-order saddle point (transition state). The TS-specific
    /// inputs (search algorithm, guess route, IRC) live in [`QmRequest::ts`].
    TransitionState,
}

impl QmKind {
    pub fn label(self) -> &'static str {
        match self {
            QmKind::SinglePoint => "single point",
            QmKind::Optimize => "geometry optimization",
            QmKind::Frequencies => "frequencies",
            QmKind::TransitionState => "transition-state search",
        }
    }
}

/// Saddle-point search algorithm. Mirrors `hartree::opt::ts::TsAlgorithm`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum QmTsAlgorithm {
    /// Partitioned rational-function optimization (eigenvector following). The
    /// Newton-type local method; needs a guess inside the saddle's basin.
    #[default]
    Prfo,
    /// Hessian-free dimer method; cheaper per step and more forgiving of the
    /// initial guess.
    Dimer,
}

impl QmTsAlgorithm {
    pub fn label(self) -> &'static str {
        match self {
            QmTsAlgorithm::Prfo => "P-RFO (eigenvector following)",
            QmTsAlgorithm::Dimer => "dimer (Hessian-free)",
        }
    }
}

/// Coordinate frame the P-RFO climb steps in. Mirrors `hartree::opt::ts::Coordinates`;
/// the dimer method discovers its own direction and ignores this.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum QmTsCoordinates {
    /// Mass-weighted Cartesian coordinates (the well-conditioned default).
    #[default]
    MassWeighted,
    /// Redundant internal coordinates — better for soft reaction coordinates
    /// (long symmetric stretches, floppy angles); falls back to Cartesian when
    /// the internal set is incomplete.
    Internal,
}

impl QmTsCoordinates {
    pub fn label(self) -> &'static str {
        match self {
            QmTsCoordinates::MassWeighted => "mass-weighted Cartesian",
            QmTsCoordinates::Internal => "redundant internal",
        }
    }
}

/// One internal coordinate to drive in a distinguished-coordinate scan. Atom
/// indices are **1-based** (as shown in the structure editor and reports); they
/// are converted to hartree's 0-based [`hartree::opt::internals::Internal`] when
/// the job is built.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum QmInternalCoordinate {
    /// Bond `i–j`. The scan range is given in Ångström.
    Bond(usize, usize),
    /// Valence angle `i–j–k` about the central atom `j`. The range is in degrees.
    Angle(usize, usize, usize),
    /// Dihedral `i–j–k–l` about the central `j–k` bond. The range is in degrees.
    Dihedral(usize, usize, usize, usize),
}

impl QmInternalCoordinate {
    /// The 1-based atom indices this coordinate references, in definition order.
    pub fn atoms(&self) -> Vec<usize> {
        match *self {
            QmInternalCoordinate::Bond(i, j) => vec![i, j],
            QmInternalCoordinate::Angle(i, j, k) => vec![i, j, k],
            QmInternalCoordinate::Dihedral(i, j, k, l) => vec![i, j, k, l],
        }
    }

    /// `true` for a bond (range in Ångström); `false` for an angle/dihedral
    /// (range in degrees). Drives the unit conversion and the displayed unit.
    pub fn is_distance(&self) -> bool {
        matches!(self, QmInternalCoordinate::Bond(_, _))
    }
}

/// Reactant→product endpoints for a two-ended transition-state search. The
/// reactant is [`QmRequest::structure`]; this carries the product and the
/// guess-builder controls. Mirrors `hartree::TsGuessInput` + the bits of
/// `NebOptions` worth exposing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QmTsEndpoints {
    /// The product minimum. Must hold the same element multiset as the reactant.
    #[serde(with = "crate::payload::structure_serde")]
    pub product: Structure,
    /// Relax a climbing-image NEB band between the endpoints instead of building a
    /// single IDPP guess. More robust for floppy or bimolecular reactions; more
    /// expensive (one gradient per interior image per band iteration).
    pub use_neb: bool,
    /// IDPP route only (`use_neb = false`): scan this many path points for the
    /// energy peak (must be ≥ 3) instead of the single geometric image. `None`
    /// uses the single IDPP image (cheapest).
    pub scan_points: Option<usize>,
    /// NEB route only: number of interior band images.
    pub neb_images: usize,
    /// NEB route only: enable the climbing image (rides the highest image up to
    /// the saddle).
    pub neb_climbing: bool,
    /// NEB route only: reorder the product's atoms onto the reactant's before
    /// building the band, so the two endpoints need not share atom ordering.
    /// (Product orientation is always Kabsch-aligned regardless; and the IDPP
    /// route always maps atoms by connectivity, so this flag is a no-op there.)
    pub map_atoms: bool,
}

impl QmTsEndpoints {
    /// Two-endpoint input for `product`, defaulting to the single-IDPP-guess
    /// route with atom mapping on.
    pub fn new(product: Structure) -> Self {
        Self {
            product,
            use_neb: false,
            scan_points: None,
            neb_images: 8,
            neb_climbing: true,
            map_atoms: true,
        }
    }
}

/// A distinguished-coordinate (relaxed) scan: drive one internal coordinate over
/// a range, relaxing the rest at each grid point, and refine the saddle from the
/// energy peak. Single-ended (no product needed). Mirrors `hartree::CoordScanSpec`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QmTsCoordinateScan {
    /// The coordinate to drive (atom indices 1-based).
    pub coordinate: QmInternalCoordinate,
    /// Start of the range. Ångström for a bond, degrees for an angle/dihedral.
    pub start: f64,
    /// End of the range (same units as [`Self::start`]).
    pub end: f64,
    /// Number of grid points across `[start, end]` inclusive (must be ≥ 3).
    pub n_points: usize,
}

/// Which near-saddle guess the transition-state search starts from.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub enum QmTsGuess {
    /// Climb from the current geometry — it must already sit near the saddle.
    #[default]
    Single,
    /// Build the guess between the reactant and a product (IDPP or NEB). Boxed:
    /// it carries a full product [`Structure`], which would otherwise bloat every
    /// [`QmRequest`] (and so [`QmJob`]) with a second geometry's worth of stack.
    TwoEndpoint(Box<QmTsEndpoints>),
    /// Drive one internal coordinate and refine from the energy peak.
    CoordinateScan(QmTsCoordinateScan),
}

/// Transition-state search configuration, used when
/// [`QmKind::TransitionState`] is requested. `default()` is a single-guess P-RFO
/// climb with no IRC confirmation.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct QmTsConfig {
    /// Which guess the search starts from.
    pub guess: QmTsGuess,
    /// Saddle-point algorithm.
    pub algorithm: QmTsAlgorithm,
    /// Coordinate frame for the P-RFO climb (ignored by the dimer method).
    pub coordinates: QmTsCoordinates,
    /// After convergence, trace the intrinsic reaction coordinate from the saddle
    /// to confirm it connects two distinct minima (extra surface evaluations).
    pub confirm_irc: bool,
}

/// A request to run a quantum-chemistry calculation on `structure`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QmRequest {
    #[serde(with = "crate::payload::structure_serde")]
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
    /// Advanced hartree options (dispersion, solvation, SCF backend, …).
    pub options: QmOptions,
    /// Transition-state search configuration. Consulted only when `kind` is
    /// [`QmKind::TransitionState`]; `None` there means a single-guess P-RFO climb.
    /// `#[serde(default)]` so requests serialized before this field round-trip.
    #[serde(default)]
    pub ts: Option<QmTsConfig>,
}

/// A quantum-chemistry job: a molecular calculation or a periodic (crystalline)
/// one. Both produce a [`QmOutcome`], so the worker thread, the workflow entry
/// point, and the result-polling UI handle the two uniformly — only the input
/// form differs. Built by the panel/console; run by [`crate::workflows::qm`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum QmJob {
    /// A molecular (non-periodic) HF/DFT/post-HF calculation.
    Molecular(QmRequest),
    /// A periodic (PBC) Kohn–Sham calculation over a unit cell.
    Periodic(PeriodicQmRequest),
}

/// The result of a quantum-chemistry calculation.
///
/// Structured fields cover what callers read programmatically (energy, the
/// optimized geometry); everything hartree reports — properties, frequencies,
/// thermochemistry, dispersion/solvation breakdowns, diagnostics, and the
/// method-quality warnings — is folded into the formatted [`Self::summary`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QmOutcome {
    pub energy_hartree: f64,
    pub converged: bool,
    /// The geometry a moving calculation produced (Å): the relaxed structure for
    /// [`QmKind::Optimize`], or the saddle point for [`QmKind::TransitionState`]
    /// (the best point reached even when the search did not converge). `None` for
    /// energy/frequency runs, which leave the atoms in place.
    #[serde(with = "crate::payload::structure_serde_opt")]
    pub optimized_structure: Option<Structure>,
    /// Pre-formatted, human-readable report of every result.
    pub summary: String,
    /// SCF total energy (Eh) per iteration. For moving jobs hartree keeps only
    /// the final geometry's SCF, so this is the final-step convergence history
    /// there. `#[serde(default)]`: outcomes cross process boundaries as
    /// `outcome.json`, and pre-trace workers omit the field (like
    /// [`QmRequest::ts`]).
    #[serde(default)]
    pub scf_trace: Vec<f64>,
    /// Total energy (Eh) per optimizer step for Optimize / TransitionState
    /// runs; empty otherwise.
    #[serde(default)]
    pub opt_trace: Vec<f64>,
    /// Harmonic wavenumbers (cm⁻¹) for Frequencies runs; empty otherwise.
    #[serde(default)]
    pub frequencies: Vec<f64>,
}
