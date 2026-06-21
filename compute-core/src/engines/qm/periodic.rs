//! In-process periodic (crystalline) quantum chemistry.
//!
//! Wraps hartree 0.1's Gaussian-and-plane-waves (GPW) periodic SCF so the rest of
//! the app can compute the energy — and optional forces and stress — of a unit
//! cell without touching hartree's periodic types. It mirrors the molecular engine
//! boundary in the parent module: [`PeriodicQmRequest`] in, [`QmOutcome`] out,
//! with every hartree result folded into [`QmOutcome::summary`].
//!
//! ## What periodic v1 supports
//!
//! hartree's periodic path is deliberately narrower than its molecular one:
//!
//! * **Closed-shell, spin-restricted Kohn–Sham DFT at the LDA level** only
//!   ([`PeriodicFunctional`]). Hybrids, dispersion, and post-HF are
//!   molecular-only.
//! * **GTH pseudopotentials with GTH basis sets** (e.g. `SZV-GTH`, `DZVP-GTH`).
//!   The electron count comes from the GTH valence, so the net molecular charge
//!   is *not* modeled and an odd valence-electron count is rejected by hartree.
//! * **Γ-point or Monkhorst–Pack k-meshes** ([`KMesh`]).
//! * A **real unit cell** is required; a molecular structure has none.
//!
//! Geometry relaxation is not offered (hartree exposes only single-point energy
//! plus optional forces/stress), so [`QmOutcome::optimized_structure`] is always
//! `None` here.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Result, anyhow, bail};
use hartree::periodic::{Cell, KPoint, MonkhorstPack};
use hartree::{
    Molecule, PeriodicFunctional as HartreePeriodicFunctional, PeriodicJob, run_periodic,
};
use nalgebra::Vector3;
use serde::{Deserialize, Serialize};

use super::{BOHR_TO_ANGSTROM, HARTREE_TO_EV, QmOutcome, ensure_known_elements};
use crate::domain::Structure;
use crate::io::structure_text::to_xyz;

/// Ångström → bohr. silicolab stores geometry and lattice vectors in Å; hartree's
/// periodic types work in bohr.
const ANGSTROM_TO_BOHR: f64 = 1.0 / BOHR_TO_ANGSTROM;

/// LDA-level exchange–correlation functional for the periodic GPW path. hartree
/// 0.1's periodic SCF supports only these two; richer functionals remain a
/// molecular-only feature.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum PeriodicFunctional {
    /// GTH-PADE LDA (the Goedecker–Teter–Hutter PADE parametrization), matched
    /// to the GTH-PADE pseudopotentials and basis. The robust default.
    #[default]
    Pade,
    /// Slater exchange + PW92 correlation LDA.
    Lda,
}

impl PeriodicFunctional {
    /// The functionals offered in the GUI dropdown, in display order.
    pub fn all() -> [PeriodicFunctional; 2] {
        [PeriodicFunctional::Pade, PeriodicFunctional::Lda]
    }

    pub fn label(self) -> &'static str {
        match self {
            PeriodicFunctional::Pade => "GTH-PADE (LDA)",
            PeriodicFunctional::Lda => "LDA (Slater+PW92)",
        }
    }

    /// Parse a functional keyword, delegating to hartree's own resolution so the
    /// accepted spellings stay in sync (`pade`/`pz`/`gth-pade`, `lda`/`pw92`/`svwn`).
    pub fn parse(input: &str) -> Result<Self> {
        match HartreePeriodicFunctional::from_name(input.trim()).map_err(|e| anyhow!(e))? {
            HartreePeriodicFunctional::Pade => Ok(PeriodicFunctional::Pade),
            HartreePeriodicFunctional::Lda => Ok(PeriodicFunctional::Lda),
        }
    }

    fn to_hartree(self) -> HartreePeriodicFunctional {
        match self {
            PeriodicFunctional::Pade => HartreePeriodicFunctional::Pade,
            PeriodicFunctional::Lda => HartreePeriodicFunctional::Lda,
        }
    }
}

/// A Monkhorst–Pack k-point mesh given by its divisions along the three
/// reciprocal axes. `[1, 1, 1]` means the single Γ point.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct KMesh {
    pub divisions: [u32; 3],
}

impl Default for KMesh {
    fn default() -> Self {
        Self {
            divisions: [1, 1, 1],
        }
    }
}

impl KMesh {
    pub fn gamma() -> Self {
        Self::default()
    }

    pub fn is_gamma_only(self) -> bool {
        self.divisions == [1, 1, 1]
    }

    pub fn label(self) -> String {
        let [a, b, c] = self.divisions;
        format!("{a}×{b}×{c}")
    }

    /// Resolve to the hartree k-points. `[1, 1, 1]` returns the bare Γ point;
    /// anything else is a regular Monkhorst–Pack mesh.
    fn to_kpoints(self) -> Result<Vec<KPoint>> {
        if self.divisions.contains(&0) {
            bail!(
                "k-point mesh divisions must each be ≥ 1, got {:?}",
                self.divisions
            );
        }
        if self.is_gamma_only() {
            return Ok(vec![KPoint::gamma()]);
        }
        let [a, b, c] = self.divisions;
        MonkhorstPack::regular([a as usize, b as usize, c as usize])
            .map_err(|e| anyhow!("invalid k-point mesh: {e}"))
    }
}

/// A request to run a periodic quantum-chemistry calculation on `structure`,
/// which must carry a real unit cell.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeriodicQmRequest {
    #[serde(with = "crate::payload::structure_serde")]
    pub structure: Structure,
    pub functional: PeriodicFunctional,
    /// GTH basis-set name (e.g. `SZV-GTH`, `DZVP-GTH`). hartree validates it
    /// against the bundled GTH-PADE library and lists the available sets on a
    /// miss.
    pub basis: String,
    pub kmesh: KMesh,
    /// Real-space grid cutoff in **Rydberg** (the CP2K convention; hartree's
    /// default is 280 Ry). Higher is more accurate and slower.
    pub e_cut_ry: f64,
    /// Maximum SCF iterations.
    pub max_iter: u32,
    /// Also compute Hellmann–Feynman forces on the nuclei.
    pub forces: bool,
    /// Also compute the cell stress tensor.
    pub stress: bool,
}

impl PeriodicQmRequest {
    /// A plain Γ-point single point at hartree's default cutoff, for `structure`.
    pub fn new(structure: Structure) -> Self {
        Self {
            structure,
            functional: PeriodicFunctional::default(),
            basis: DEFAULT_PERIODIC_BASIS.to_string(),
            kmesh: KMesh::gamma(),
            e_cut_ry: DEFAULT_E_CUT_RY,
            max_iter: DEFAULT_MAX_ITER,
            forces: false,
            stress: false,
        }
    }
}

/// hartree's default periodic grid cutoff (Rydberg).
pub const DEFAULT_E_CUT_RY: f64 = 280.0;
/// Default SCF iteration cap (hartree's `PeriodicScfOptions` default).
pub const DEFAULT_MAX_ITER: u32 = 100;
/// Default GTH basis. SZV-GTH is minimal but by far the broadest in element
/// coverage among the bundled GTH sets (H, Li, C, O, F, Na, Mg, Si, Cl), so it
/// is the safest "it just runs" default. The bundled `DZVP-GTH` set is the
/// accuracy step but covers only a few elements (Li, F, Na, Mg, Cl) — hartree
/// errors with the list of available sets when an element is missing.
pub const DEFAULT_PERIODIC_BASIS: &str = "SZV-GTH";
/// GTH basis sets offered in the GUI dropdown. hartree accepts more (and the
/// console `qm periodic` command takes any name); see [`DEFAULT_PERIODIC_BASIS`]
/// for the element-coverage caveat on `DZVP-GTH`.
pub const PERIODIC_BASES: &[&str] = &["SZV-GTH", "DZVP-GTH"];

/// Run a periodic quantum-chemistry calculation.
///
/// `report` receives coarse stage strings. `cancel` is **best-effort**: it is
/// honored before the calculation starts, but hartree's `run_periodic` is a single
/// opaque call with no preemption hook, so an in-flight SCF cannot be
/// interrupted — the worker runs to completion and the caller discards the
/// result.
pub fn run_periodic_qm(
    request: PeriodicQmRequest,
    cancel: Arc<AtomicBool>,
    mut report: impl FnMut(&str),
) -> Result<QmOutcome> {
    let PeriodicQmRequest {
        structure,
        functional,
        basis,
        kmesh,
        e_cut_ry,
        max_iter,
        forces,
        stress,
    } = request;

    if structure.atoms.is_empty() {
        bail!("the structure has no atoms to compute");
    }
    // Guard the hartree inputs the GUI clamps but the console (and agent) do not.
    // A non-positive cutoff makes hartree's real-space grid constructor panic, and
    // a zero iteration cap silently returns a bogus zero-energy "result" (the SCF
    // loop `1..=0` never runs), so reject both up front with a clear error.
    if !e_cut_ry.is_finite() || e_cut_ry <= 0.0 {
        bail!("grid cutoff must be a positive number of Rydberg; got {e_cut_ry}");
    }
    if max_iter == 0 {
        bail!("max SCF iterations must be at least 1");
    }
    if cancel.load(Ordering::Relaxed) {
        bail!("cancelled before the calculation started");
    }

    report("preparing periodic cell");
    let cell = periodic_cell_from_structure(&structure)?;
    let molecule = periodic_molecule_from_structure(&structure)?;
    let kpoints = kmesh.to_kpoints()?;

    // `gth_pade` loads the GTH-PADE basis and pseudopotentials and selects the
    // PADE functional; we then override the functional and SCF knobs.
    let mut job = PeriodicJob::gth_pade(molecule, cell, kpoints, &basis)
        .map_err(|e| anyhow!("could not set up the periodic calculation: {e}"))?;
    job.functional = functional.to_hartree();
    job.options.e_cut = e_cut_ry;
    job.options.max_iter = max_iter as usize;
    job.forces = forces;
    job.stress = stress;

    if cancel.load(Ordering::Relaxed) {
        bail!("cancelled before the calculation started");
    }
    report("running periodic scf");
    let result = run_periodic(&job).map_err(|e| anyhow!("periodic calculation failed: {e}"))?;

    report("collecting results");
    let energy_hartree = result.scf.energy;
    let converged = result.scf.converged;
    let summary = format_periodic_summary(&structure, functional, &basis, kmesh, e_cut_ry, &result);

    Ok(QmOutcome {
        energy_hartree,
        converged,
        optimized_structure: None,
        summary,
    })
}

/// Build a hartree [`Cell`] (bohr) from the structure's unit cell (Å). Rejects a
/// missing cell and the `1×1×1`/90° placeholder some tools write for a
/// non-periodic molecule (using it as a lattice is physically meaningless).
fn periodic_cell_from_structure(structure: &Structure) -> Result<Cell> {
    let cell = structure
        .cell
        .as_ref()
        .filter(|cell| !cell.is_placeholder())
        .ok_or_else(|| {
            anyhow!(
                "a periodic QM calculation needs a real unit cell, but this structure has none; \
                 load a crystal (e.g. CIF/POSCAR) or set the lattice before running"
            )
        })?;

    let to_bohr = |v: Vector3<f32>| {
        [
            f64::from(v.x) * ANGSTROM_TO_BOHR,
            f64::from(v.y) * ANGSTROM_TO_BOHR,
            f64::from(v.z) * ANGSTROM_TO_BOHR,
        ]
    };
    Cell::from_vectors(
        to_bohr(cell.vectors[0]),
        to_bohr(cell.vectors[1]),
        to_bohr(cell.vectors[2]),
    )
    .map_err(|e| anyhow!("invalid unit cell: {e}"))
}

/// Build the hartree [`Molecule`] for a periodic job. Like the molecular path it
/// round-trips through an XYZ string (Å → bohr) and validates element symbols,
/// but it does *not* apply charge/spin: the periodic SCF derives its
/// (closed-shell) electron count from the GTH valence, and hartree rejects an odd
/// valence count with a clear message of its own.
fn periodic_molecule_from_structure(structure: &Structure) -> Result<Molecule> {
    ensure_known_elements(structure)?;
    Molecule::from_xyz(&to_xyz(structure))
        .map_err(|e| anyhow!("hartree could not parse the cell geometry: {e}"))
}

/// Render a human-readable report of a periodic result: cell, k-mesh, cutoff,
/// convergence, total and per-atom energy, the GPW energy decomposition, a
/// band-gap estimate, and optional forces/stress.
fn format_periodic_summary(
    structure: &Structure,
    functional: PeriodicFunctional,
    basis: &str,
    kmesh: KMesh,
    e_cut_ry: f64,
    result: &hartree::PeriodicJobResult,
) -> String {
    let mut out = String::new();
    let n_atoms = structure.atoms.len();

    out.push_str(&format!(
        "periodic GPW {} / {basis} single point\n",
        functional.label()
    ));

    if let Some(cell) = &structure.cell {
        out.push_str(&format!(
            "  cell: a={:.4} b={:.4} c={:.4} Å, α={:.2} β={:.2} γ={:.2}°\n",
            cell.a, cell.b, cell.c, cell.alpha, cell.beta, cell.gamma
        ));
    }
    out.push_str(&format!(
        "  k-points: {} mesh ({} point{})\n",
        kmesh.label(),
        result.scf.band_energies.len(),
        if result.scf.band_energies.len() == 1 {
            ""
        } else {
            "s"
        }
    ));
    out.push_str(&format!("  grid cutoff: {e_cut_ry:.0} Ry\n"));

    let scf = &result.scf;
    out.push_str(&format!(
        "  SCF: {} in {} iterations\n",
        if scf.converged {
            "converged"
        } else {
            "NOT converged"
        },
        scf.iterations
    ));
    out.push_str(&format!(
        "  total energy: {:.8} Eh  ({:.4} eV)\n",
        scf.energy,
        scf.energy * HARTREE_TO_EV
    ));
    if n_atoms > 0 {
        out.push_str(&format!(
            "  energy / atom: {:.8} Eh\n",
            scf.energy / n_atoms as f64
        ));
    }
    out.push_str(&format!("  electrons on grid: {:.4}\n", scf.n_elec_grid));

    // GPW energy decomposition.
    let c = &scf.components;
    out.push_str("  energy decomposition (Eh):\n");
    out.push_str(&format!("    E_kinetic:    {:.8}\n", c.e_kin));
    out.push_str(&format!("    E_hartree:    {:.8}\n", c.e_hartree));
    out.push_str(&format!("    E_xc:         {:.8}\n", c.e_xc));
    out.push_str(&format!("    E_local_sr:   {:.8}\n", c.e_local_sr));
    out.push_str(&format!("    E_nonlocal:   {:.8}\n", c.e_nonlocal));
    out.push_str(&format!("    E_self:       {:.8}\n", c.e_self));
    out.push_str(&format!("    E_overlap:    {:.8}\n", c.e_overlap));

    if let Some((gap_ev, kind)) = band_gap_estimate(scf) {
        out.push_str(&format!("  band gap (estimate): {gap_ev:.3} eV ({kind})\n"));
    }

    if let Some(forces) = &result.forces {
        // hartree returns one force per atom, in the molecule's (= structure's)
        // order; assert the invariant so a future desync surfaces instead of the
        // zip silently truncating, mirroring the molecular optimize path.
        debug_assert_eq!(
            forces.len(),
            structure.atoms.len(),
            "periodic force count != atom count"
        );
        out.push_str("  forces (Eh/bohr):\n");
        let mut fmax = 0.0_f64;
        for (atom, f) in structure.atoms.iter().zip(forces) {
            let norm = (f[0] * f[0] + f[1] * f[1] + f[2] * f[2]).sqrt();
            fmax = fmax.max(norm);
            out.push_str(&format!(
                "    {:<2} {:+.6} {:+.6} {:+.6}\n",
                atom.element, f[0], f[1], f[2]
            ));
        }
        out.push_str(&format!("    max |force|: {fmax:.6} Eh/bohr\n"));
    }

    if let Some(stress) = &result.stress {
        out.push_str("  stress tensor (Eh/bohr³):\n");
        for row in stress {
            out.push_str(&format!(
                "    {:+.6} {:+.6} {:+.6}\n",
                row[0], row[1], row[2]
            ));
        }
    }

    out.trim_end().to_string()
}

/// Estimate the band gap from the SCF band energies: the highest occupied and
/// lowest unoccupied levels across the k-mesh, with the occupied count taken
/// from the (closed-shell) grid electron count. Returns the gap in eV and
/// whether the band extrema sit at the same k-point (direct) or not (indirect).
/// `None` when there is no virtual band to compare against (or a metallic
/// crossing makes the gap non-positive).
fn band_gap_estimate(scf: &hartree::periodic::PeriodicScfResult) -> Option<(f64, &'static str)> {
    // The SCF occupied exactly n_elec/2 bands with the exact (even) GTH valence
    // count; recover that integer by rounding the *half* of the grid-integrated
    // count. Rounding `n_elec_grid` first can land on an odd integer (the grid
    // count carries a small integration error), which would miscount by one.
    let n_occ = (scf.n_elec_grid / 2.0).round() as usize;
    if n_occ == 0 {
        return None;
    }
    // Valence-band maximum (and its k-index) and conduction-band minimum.
    let mut vbm = f64::NEG_INFINITY;
    let mut vbm_k = 0usize;
    let mut cbm = f64::INFINITY;
    let mut cbm_k = 0usize;
    for (ik, bands) in scf.band_energies.iter().enumerate() {
        // Need both the highest occupied (index n_occ-1) and the lowest
        // unoccupied (index n_occ) band at this k-point.
        if bands.len() <= n_occ {
            return None;
        }
        let homo = bands[n_occ - 1];
        let lumo = bands[n_occ];
        if homo > vbm {
            vbm = homo;
            vbm_k = ik;
        }
        if lumo < cbm {
            cbm = lumo;
            cbm_k = ik;
        }
    }
    let gap = cbm - vbm;
    if !gap.is_finite() || gap <= 0.0 {
        return None;
    }
    let kind = if vbm_k == cbm_k { "direct" } else { "indirect" };
    Some((gap * HARTREE_TO_EV, kind))
}

#[cfg(test)]
mod tests {
    use nalgebra::{Point3, Vector3};

    use super::*;
    use crate::domain::{Atom, UnitCell};

    fn no_cancel() -> Arc<AtomicBool> {
        Arc::new(AtomicBool::new(false))
    }

    /// Bulk silicon in its 2-atom FCC primitive cell (a = 5.43 Å), the canonical
    /// periodic test case.
    fn silicon_primitive() -> Structure {
        let a = 5.43_f32;
        let vectors = [
            Vector3::new(0.0, a / 2.0, a / 2.0),
            Vector3::new(a / 2.0, 0.0, a / 2.0),
            Vector3::new(a / 2.0, a / 2.0, 0.0),
        ];
        let cell = UnitCell::from_vectors(vectors);
        // Two Si at fractional (0,0,0) and (¼,¼,¼).
        let r1 = cell.fractional_to_cartesian(0.25, 0.25, 0.25);
        let atoms = vec![
            Atom {
                element: "Si".to_string(),
                position: Point3::origin(),
                charge: 0.0,
            },
            Atom {
                element: "Si".to_string(),
                position: r1,
                charge: 0.0,
            },
        ];
        Structure::with_cell("si", atoms, cell)
    }

    #[test]
    fn functional_parse_matches_hartree_spellings() {
        assert_eq!(
            PeriodicFunctional::parse("pade").unwrap(),
            PeriodicFunctional::Pade
        );
        assert_eq!(
            PeriodicFunctional::parse("PW92").unwrap(),
            PeriodicFunctional::Lda
        );
        assert!(PeriodicFunctional::parse("b3lyp").is_err());
    }

    #[test]
    fn gamma_mesh_is_single_point() {
        let kpoints = KMesh::gamma().to_kpoints().expect("gamma");
        assert_eq!(kpoints.len(), 1);
        let mesh = KMesh {
            divisions: [2, 2, 2],
        };
        assert_eq!(mesh.to_kpoints().expect("2x2x2").len(), 8);
        assert!(
            KMesh {
                divisions: [0, 1, 1]
            }
            .to_kpoints()
            .is_err()
        );
    }

    #[test]
    fn missing_cell_is_rejected() {
        let structure = Structure::new(
            "no-cell",
            vec![Atom {
                element: "Si".to_string(),
                position: Point3::origin(),
                charge: 0.0,
            }],
        );
        let err = periodic_cell_from_structure(&structure).expect_err("a molecule has no cell");
        assert!(err.to_string().contains("unit cell"), "{err}");
    }

    #[test]
    fn placeholder_cell_is_rejected() {
        let cell = UnitCell::from_parameters(1.0, 1.0, 1.0, 90.0, 90.0, 90.0);
        let structure = Structure::with_cell(
            "placeholder",
            vec![Atom {
                element: "Si".to_string(),
                position: Point3::origin(),
                charge: 0.0,
            }],
            cell,
        );
        assert!(periodic_cell_from_structure(&structure).is_err());
    }

    #[test]
    fn degenerate_inputs_are_rejected_before_scf() {
        // A non-positive cutoff would otherwise panic inside hartree's grid
        // constructor; a zero iteration cap would silently yield a 0 Eh result.
        let mut zero_cutoff = PeriodicQmRequest::new(silicon_primitive());
        zero_cutoff.e_cut_ry = 0.0;
        let err = run_periodic_qm(zero_cutoff, no_cancel(), |_| {})
            .expect_err("zero cutoff must be rejected");
        assert!(err.to_string().contains("cutoff"), "{err}");

        let mut zero_iters = PeriodicQmRequest::new(silicon_primitive());
        zero_iters.max_iter = 0;
        assert!(run_periodic_qm(zero_iters, no_cancel(), |_| {}).is_err());
    }

    #[test]
    fn periodic_silicon_single_point_converges() {
        // A real SCF on bulk Si: small basis, modest cutoff, Γ only. This is the
        // heaviest test in the engine but still runs in seconds at this size.
        let mut request = PeriodicQmRequest::new(silicon_primitive());
        request.e_cut_ry = 100.0;
        request.max_iter = 80;
        let outcome = run_periodic_qm(request, no_cancel(), |_| {})
            .expect("periodic Si single point should succeed");
        assert!(
            outcome.converged,
            "SCF should converge: {}",
            outcome.summary
        );
        assert!(
            outcome.energy_hartree.is_finite() && outcome.energy_hartree < 0.0,
            "energy = {}",
            outcome.energy_hartree
        );
        assert!(outcome.optimized_structure.is_none());
        assert!(outcome.summary.contains("periodic GPW"));
        assert!(outcome.summary.contains("energy decomposition"));
    }
}
