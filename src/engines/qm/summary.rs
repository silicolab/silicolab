use super::*;

use chemx::scf::Reference;
use chemx::{BasisSet, JobResult, PostHfResult, ecp_summary};

use crate::domain::Structure;

/// Render a human-readable report of `result` covering every section chemx
/// populates. Mirrors chemx-cli's `report*` formatters (trimming only the
/// per-iteration SCF/optimizer history tables, which are noise in a panel).
pub(crate) fn format_summary(
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
