use super::*;

use anyhow::{Context, Result, anyhow, bail};
use hartree::composite::Composite;
use hartree::composite::composite;
use hartree::dft::FunctionalSpec;
use hartree::disp::Dispersion;
use hartree::scf::Smearing;
use hartree::{Element, Job, JobOptions, Method, Molecule};

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
pub(crate) fn build_job(
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
            method: hartree_method,
            options: job_options,
        },
        basis: resolved_basis,
        composite: comp,
        x2c: options.x2c,
        cpcm_solvent,
    })
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
