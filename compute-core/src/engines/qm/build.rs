use super::*;

use anyhow::{Context, Result, anyhow, bail};
use hartree::composite::Composite;
use hartree::composite::composite;
use hartree::dft::FunctionalSpec;
use hartree::disp::Dispersion;
use hartree::opt::internals::Internal;
use hartree::opt::ts::{Coordinates, NebOptions, TsAlgorithm, TsOptions};
use hartree::scf::Smearing;
use hartree::{CoordScanSpec, Element, Job, JobOptions, Method, Molecule, TsGuessInput};

use crate::domain::Structure;
use crate::io::structure_text::to_xyz;

/// Validate that every atom carries a hartree-recognized element symbol, naming
/// the offending atom (hartree's own parse error does not). The structure editor
/// accepts free-text symbols, so an atom may carry a typo, a stray character, or
/// a blank. Shared by the molecular and periodic engine paths.
pub(crate) fn ensure_known_elements(structure: &Structure) -> Result<()> {
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
    Ok(())
}

/// Whether hartree will accept `symbol` as an element. Mirrors `Molecule::from_xyz`,
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

/// Build a hartree [`Molecule`] from one of our [`Structure`]s.
///
/// We round-trip through an XYZ string (Ångström) so hartree owns element-symbol
/// parsing and the Å→bohr conversion, then apply the net charge and spin.
pub(crate) fn molecule_from_structure(
    structure: &Structure,
    charge: i32,
    multiplicity: u32,
) -> Result<Molecule> {
    ensure_known_elements(structure)?;

    let molecule = Molecule::from_xyz(&to_xyz(structure))
        .context("hartree could not parse the molecule geometry")?
        .with_charge(charge)
        .with_multiplicity(multiplicity);
    molecule
        .validate()
        .context("invalid charge / spin multiplicity for this molecule")?;
    Ok(molecule)
}

/// Graft optimized bohr coordinates back onto a copy of `original` (Å).
pub(crate) fn structure_with_positions(
    original: &Structure,
    positions: &[[f64; 3]],
) -> Result<Structure> {
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

/// The resolved hartree [`Job`] plus the bits of context the summary needs that
/// are not recoverable from [`JobResult`] alone (the displayed basis, the
/// composite registry entry, and whether scalar relativity was on).
pub(crate) struct ResolvedJob {
    pub(crate) job: Job,
    /// The basis actually used (a composite overrides the request's basis).
    pub(crate) basis: String,
    /// The composite registry entry, when the method is a composite.
    pub(crate) composite: Option<&'static Composite>,
    /// Whether the X2C-1e Hamiltonian is active (for the report caveat).
    pub(crate) x2c: bool,
    /// The named solvent, for the C-PCM report line (`None` for a bare ε).
    pub(crate) cpcm_solvent: Option<String>,
}

/// Resolve `request` into a hartree [`Job`], mapping every silicolab option onto
/// `JobOptions`/`Method` exactly as hartree-cli does. CLI-level incompatibilities
/// that hartree's `Job::run` does not itself catch are rejected here with a
/// pointed message; the deeper physics guards are left to hartree.
#[allow(clippy::too_many_arguments)]
pub(crate) fn build_job(
    structure: &Structure,
    method: &QmMethod,
    basis: &str,
    charge: i32,
    multiplicity: u32,
    kind: QmKind,
    options: &QmOptions,
    ts: Option<&QmTsConfig>,
) -> Result<ResolvedJob> {
    let molecule = molecule_from_structure(structure, charge, multiplicity)?;

    // Transition-state search is gated to the same gradient-capable, in-core,
    // gas-phase combinations hartree allows; reject the incompatible options here
    // with a pointed message rather than letting the deeper engine error surface.
    if kind == QmKind::TransitionState {
        ensure_ts_compatible(method, options)?;
    }

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

    let hartree_method = resolve_hartree_method(method, multiplicity, comp)?;
    let resolved_basis = match comp {
        Some(c) => c.basis.to_string(),
        None => basis.to_string(),
    };

    // Dispersion: a composite's own parametrization, or a `-d3`/`-d4` request
    // keyed by the method (mirrors hartree-cli's param-key derivation).
    let dispersion = match (comp, options.dispersion) {
        (Some(c), _) => Some(c.dispersion),
        (None, Some(disp)) => Some(resolve_dispersion(method, &hartree_method, disp)?),
        (None, None) => None,
    };

    // SCF backend.
    let direct = options.scf_backend == QmScfBackend::Direct;
    let ri = options.scf_backend == QmScfBackend::RiJk;
    let cosx = options.scf_backend == QmScfBackend::Cosx;

    // Grid level: explicit override, else the composite's recommended grid,
    // else a grid-sensitive functional's recommended level, else hartree's 3.
    let grid_level = options.grid_level.unwrap_or_else(|| {
        comp.map(|c| c.grid_level)
            .unwrap_or_else(|| match &hartree_method {
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
                let eps = hartree::solv::solvent_epsilon(name).ok_or_else(|| {
                    let names: Vec<&str> =
                        hartree::solv::SOLVENTS.iter().map(|(n, _)| *n).collect();
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

    let (ts_options, ts_guess, ts_coord_scan) = if kind == QmKind::TransitionState {
        build_ts_inputs(structure, ts, charge, multiplicity)?
    } else {
        (TsOptions::default(), None, None)
    };

    let job_options = JobOptions {
        all_electron: options.all_electron,
        direct,
        ri,
        compute_properties: options.compute_properties,
        compute_frequencies: kind == QmKind::Frequencies,
        single_point_hessian: options.single_point_hessian,
        optimize_geometry: kind == QmKind::Optimize,
        transition_state: kind == QmKind::TransitionState,
        ts_options,
        ts_guess,
        ts_coord_scan,
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
        // hartree's n_threads / mem_budget knobs default off — thread capping is
        // done by the workflow's rayon pool, not here.
        ..Default::default()
    };

    Ok(ResolvedJob {
        job: Job {
            molecule,
            basis: resolved_basis.clone(),
            method: hartree_method,
            options: job_options,
        },
        basis: resolved_basis,
        composite: comp,
        x2c: options.x2c,
        cpcm_solvent,
    })
}

/// Reject the options a transition-state search cannot run with, before the job
/// is assembled. hartree gates the saddle search to the gradient-capable, in-core,
/// gas-phase combinations; mirror that here with caller-facing messages. The
/// functional-specific rejections (double hybrids, VV10) are left to hartree,
/// which knows the functional internals.
fn ensure_ts_compatible(method: &QmMethod, options: &QmOptions) -> Result<()> {
    if method.is_post_hf() {
        bail!(
            "transition-state search needs an analytic gradient: it supports HF and DFT, \
             not MP2/CCSD/CCSD(T)"
        );
    }
    if options.scf_backend != QmScfBackend::InCore {
        bail!(
            "transition-state search requires the in-core SCF backend; \
             remove the integral-direct / RI-JK / COSX option"
        );
    }
    if options.x2c {
        bail!("transition-state search does not support the X2C Hamiltonian (energy-only)");
    }
    if options.smearing_temperature_k.is_some() {
        bail!("transition-state search does not support Fermi smearing (energy-only)");
    }
    if options.solvation.is_some() {
        bail!(
            "transition-state search does not support implicit solvation \
             (no analytic gradient on the solvated surface)"
        );
    }
    Ok(())
}

/// Resolve a [`QmTsConfig`] into hartree's `(TsOptions, ts_guess, ts_coord_scan)`.
/// `None` (no config on a TS request) is a single-guess P-RFO climb from the
/// current geometry. `charge`/`multiplicity` are applied to the product endpoint
/// of a two-ended search so it matches the reactant's electronic state.
fn build_ts_inputs(
    reactant: &Structure,
    config: Option<&QmTsConfig>,
    charge: i32,
    multiplicity: u32,
) -> Result<(TsOptions, Option<TsGuessInput>, Option<CoordScanSpec>)> {
    let config = match config {
        Some(c) => c,
        None => return Ok((TsOptions::default(), None, None)),
    };

    let mut ts_options = TsOptions::default();
    ts_options.algorithm = match config.algorithm {
        QmTsAlgorithm::Prfo => TsAlgorithm::Prfo,
        QmTsAlgorithm::Dimer => TsAlgorithm::Dimer,
    };
    ts_options.coordinates = match config.coordinates {
        QmTsCoordinates::MassWeighted => Coordinates::MassWeighted,
        QmTsCoordinates::Internal => Coordinates::Internal,
    };
    ts_options.confirm_irc = config.confirm_irc;

    let (ts_guess, ts_coord_scan) = match &config.guess {
        QmTsGuess::Single => (None, None),
        QmTsGuess::TwoEndpoint(endpoints) => {
            let product = molecule_from_structure(&endpoints.product, charge, multiplicity)
                .context("invalid product geometry for the transition-state search")?;
            let mut guess = TsGuessInput::new(product);
            guess.use_neb = endpoints.use_neb;
            guess.scan_points = endpoints.scan_points;
            let mut neb = NebOptions::default();
            neb.n_images = endpoints.neb_images.max(1);
            neb.climbing = endpoints.neb_climbing;
            neb.map_atoms = endpoints.map_atoms;
            // Two separately optimized minima rarely share a frame; aligning the
            // product onto the reactant removes an arbitrary relative orientation.
            neb.align = true;
            guess.neb_options = neb;
            (Some(guess), None)
        }
        QmTsGuess::CoordinateScan(scan) => {
            let spec = build_coord_scan(reactant, scan)?;
            (None, Some(spec))
        }
    };

    Ok((ts_options, ts_guess, ts_coord_scan))
}

/// Build a hartree [`CoordScanSpec`] from a UI-level [`QmTsCoordinateScan`],
/// converting 1-based atom indices to hartree's 0-based [`Internal`] and the
/// range from display units (Ångström / degrees) to hartree's (Bohr / radians).
fn build_coord_scan(reactant: &Structure, scan: &QmTsCoordinateScan) -> Result<CoordScanSpec> {
    if scan.n_points < 3 {
        bail!(
            "a coordinate scan needs at least 3 grid points, got {}",
            scan.n_points
        );
    }
    let natoms = reactant.atoms.len();
    let atoms = scan.coordinate.atoms();
    for &atom in &atoms {
        if atom < 1 || atom > natoms {
            bail!(
                "coordinate-scan atom index {atom} is out of range (the structure has {natoms} atoms, \
                 numbered 1..={natoms})"
            );
        }
    }
    // A coordinate over a repeated atom (e.g. a bond from an atom to itself) is
    // degenerate — its direction is undefined, so reject it rather than let the
    // B-matrix produce a NaN.
    for i in 0..atoms.len() {
        for j in (i + 1)..atoms.len() {
            if atoms[i] == atoms[j] {
                bail!(
                    "coordinate-scan atoms must be distinct (atom {} is repeated)",
                    atoms[i]
                );
            }
        }
    }
    // 1-based (UI) → 0-based (hartree).
    let internal = match scan.coordinate {
        QmInternalCoordinate::Bond(i, j) => Internal::Bond(i - 1, j - 1),
        QmInternalCoordinate::Angle(i, j, k) => Internal::Angle(i - 1, j - 1, k - 1),
        QmInternalCoordinate::Dihedral(i, j, k, l) => {
            Internal::Dihedral(i - 1, j - 1, k - 1, l - 1)
        }
    };
    let (start, end) = if scan.coordinate.is_distance() {
        let to_bohr = 1.0 / BOHR_TO_ANGSTROM;
        (scan.start * to_bohr, scan.end * to_bohr)
    } else {
        (scan.start.to_radians(), scan.end.to_radians())
    };
    Ok(CoordScanSpec::new(internal, start, end, scan.n_points))
}

/// Map a [`QmMethod`] to a hartree [`Method`]. A composite runs its plain
/// functional (the corrections are added at the job layer).
fn resolve_hartree_method(
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

/// Whether hartree carries a dispersion parametrization for `method` + `disp` —
/// i.e. whether [`resolve_dispersion`] would resolve it rather than bail. The
/// panel uses this to offer only the dispersion variants the chosen functional
/// supports (D3(BJ) covers a small set; D4 additionally covers the double
/// hybrids). Composites carry their own and post-HF has none, so both return
/// false. Mirrors [`resolve_dispersion`]'s key derivation exactly.
pub fn supports_dispersion(method: &QmMethod, disp: QmDispersion) -> bool {
    if method.is_post_hf() {
        return false;
    }
    let param_key = match method {
        QmMethod::Hf | QmMethod::Rhf | QmMethod::Uhf | QmMethod::Rohf => "hf".to_string(),
        QmMethod::Dft(name) => match FunctionalSpec::parse(name) {
            Ok(spec) => spec
                .d4_param_set()
                .map(str::to_string)
                .unwrap_or_else(|| spec.name().to_string()),
            Err(_) => return false,
        },
        // Composites define their own dispersion; post-HF is rejected above.
        QmMethod::Composite(_) | QmMethod::Mp2 | QmMethod::Ccsd | QmMethod::CcsdT => return false,
    };
    Dispersion::for_method(disp == QmDispersion::D4, &param_key).is_some()
}

/// Resolve a `-d3`/`-d4` request for a non-composite method into a hartree
/// [`Dispersion`], keyed by the method (mirrors hartree-cli lines 613–646).
fn resolve_dispersion(
    method: &QmMethod,
    hartree_method: &Method,
    disp: QmDispersion,
) -> Result<Dispersion> {
    if method.is_post_hf() {
        bail!(
            "{} dispersion is not supported for post-HF methods; it applies to HF and DFT",
            disp.label()
        );
    }
    let d4 = disp == QmDispersion::D4;
    let param_key = match hartree_method {
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
