//! Pre-run memory guard for in-core SCF. Conventional (in-core) SCF stores the
//! full nao⁴ ERI tensor, so a routine small molecule at a modest basis can ask
//! for tens of GB. We ask hartree to estimate that allocation (without running
//! SCF) and let callers compare it to a RAM budget.

use hartree::EstimateBackend;

use super::build::build_job;
use super::types::{QmKind, QmRequest, QmScfBackend};

/// Estimate the peak bytes an in-core job would allocate, via hartree's
/// `estimate_memory` on the resolved hartree `Job`. It models the in-core ERI
/// tensor plus the post-HF blocks a raw `nao⁴` figure misses (e.g. CCSD `vvvv`),
/// and matches the estimate hartree's own budget guard uses. Returns `None` for
/// non-in-core backends or when the job can't be built (the real run surfaces
/// that error).
pub fn estimate_incore_memory_bytes(request: &QmRequest) -> Option<u64> {
    if request.options.scf_backend != QmScfBackend::InCore {
        return None;
    }
    let resolved = build_job(
        &request.structure,
        &request.method,
        &request.basis,
        request.charge,
        request.multiplicity,
        request.kind,
        &request.options,
        request.ts.as_ref(),
    )
    .ok()?;
    hartree::estimate_memory(&resolved.job)
        .ok()
        .map(|estimate| estimate.peak_bytes)
}

/// An on-demand peak-memory estimate for the current molecular configuration,
/// shown by the QM panel's "Estimate memory" button. Unlike the pre-run guard,
/// this reports for whatever SCF backend the form selected (in-core, direct, or
/// RI), so the user can see what each choice would cost before launching.
#[derive(Debug, Clone)]
pub struct QmMemoryReport {
    /// Estimated peak working set, in bytes (hartree's `MemoryEstimate::peak_bytes`).
    pub peak_bytes: u64,
    /// The safe RAM budget this was compared against.
    pub budget_bytes: u64,
    /// The integral backend the estimate assumed.
    pub backend_label: String,
    /// The method label, for a self-describing display line.
    pub method_label: String,
    /// The basis actually used (composite-resolved), for the display line.
    pub basis_label: String,
}

impl QmMemoryReport {
    /// Whether the estimate sits within the safe budget.
    pub fn fits(&self) -> bool {
        self.peak_bytes <= self.budget_bytes
    }
}

/// Estimate the peak memory the current molecular `request` would use, for its
/// chosen backend, and pair it with `budget_bytes`. Returns `Err` for the same
/// up-front reasons a real run would reject the job (unknown element/basis, bad
/// charge/spin), so the panel can surface that message instead of a number.
pub fn estimate_request_memory(
    request: &QmRequest,
    budget_bytes: u64,
) -> Result<QmMemoryReport, String> {
    let resolved = build_job(
        &request.structure,
        &request.method,
        &request.basis,
        request.charge,
        request.multiplicity,
        request.kind,
        &request.options,
        request.ts.as_ref(),
    )
    .map_err(|error| error.to_string())?;
    let estimate = hartree::estimate_memory(&resolved.job)?;
    let backend_label = match estimate.backend {
        EstimateBackend::Conventional => "in-core".to_string(),
        EstimateBackend::Direct => "integral-direct".to_string(),
        EstimateBackend::Ri => "RI-JK density fitting".to_string(),
        // `EstimateBackend` is #[non_exhaustive]; fall back to its own name.
        other => other.to_string(),
    };
    Ok(QmMemoryReport {
        peak_bytes: estimate.peak_bytes,
        budget_bytes,
        backend_label,
        method_label: request.method.label(),
        basis_label: resolved.basis,
    })
}

/// Outcome of the pre-run memory check. `Exceeds*` distinguishes the case where
/// a cheaper backend exists (single-point HF/DFT can switch to integral-direct)
/// from the case where in-core is mandatory (optimize/freq/post-HF).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemoryVerdict {
    Ok,
    ExceedsCanDirect { estimate: u64, budget: u64 },
    ExceedsMustReduce { estimate: u64, budget: u64 },
}

/// Compare the in-core estimate against `budget_bytes`. Pure: the caller supplies
/// the budget (e.g. `backend::hardware::qm_incore_budget_bytes()`), which keeps
/// this unit-testable without probing the host.
pub fn memory_verdict(request: &QmRequest, budget_bytes: u64) -> MemoryVerdict {
    let Some(estimate) = estimate_incore_memory_bytes(request) else {
        return MemoryVerdict::Ok;
    };
    if estimate <= budget_bytes {
        return MemoryVerdict::Ok;
    }
    // integral-direct / RI / COSX are SCF (HF/DFT) single-point backends only;
    // optimize, frequencies, and post-HF all still need the in-core ERI tensor.
    let can_direct = request.kind == QmKind::SinglePoint && !request.method.is_post_hf();
    if can_direct {
        MemoryVerdict::ExceedsCanDirect {
            estimate,
            budget: budget_bytes,
        }
    } else {
        MemoryVerdict::ExceedsMustReduce {
            estimate,
            budget: budget_bytes,
        }
    }
}

fn gib(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0 * 1024.0)
}

impl MemoryVerdict {
    /// Human one-liner naming the estimate and the safe budget, attributed to
    /// `location` (e.g. `"this machine"` or a remote host's label); `None` for
    /// `Ok`. The location is supplied by the caller so this stays host-agnostic.
    pub fn detail(&self, location: &str) -> Option<String> {
        match self {
            MemoryVerdict::Ok => None,
            MemoryVerdict::ExceedsCanDirect { estimate, budget }
            | MemoryVerdict::ExceedsMustReduce { estimate, budget } => Some(format!(
                "This in-core calculation needs about {:.1} GiB, but only {:.1} GiB is safe to use on {location}.",
                gib(*estimate),
                gib(*budget),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Atom, Structure};
    use crate::engines::qm::{QmKind, QmMethod, QmOptions};
    use nalgebra::Point3;

    fn water_request(backend: QmScfBackend) -> QmRequest {
        let structure = Structure::new(
            "water",
            vec![
                Atom {
                    element: "O".into(),
                    position: Point3::new(0.0, 0.0, 0.117),
                    charge: 0.0,
                },
                Atom {
                    element: "H".into(),
                    position: Point3::new(0.0, 0.757, -0.469),
                    charge: 0.0,
                },
                Atom {
                    element: "H".into(),
                    position: Point3::new(0.0, -0.757, -0.469),
                    charge: 0.0,
                },
            ],
        );
        QmRequest {
            structure,
            method: QmMethod::Rhf,
            basis: "def2-svp".into(),
            charge: 0,
            multiplicity: 1,
            kind: QmKind::SinglePoint,
            options: QmOptions {
                scf_backend: backend,
                ..Default::default()
            },
            ts: None,
        }
    }

    #[test]
    fn verdict_offers_direct_only_for_scf_single_points() {
        let req = water_request(QmScfBackend::InCore);
        // Generous budget → Ok.
        assert!(matches!(memory_verdict(&req, u64::MAX), MemoryVerdict::Ok));
        // Zero budget → exceeds; RHF single point can fall back to integral-direct.
        assert!(matches!(
            memory_verdict(&req, 0),
            MemoryVerdict::ExceedsCanDirect { .. }
        ));

        // An optimization requires in-core → must reduce, never "use direct".
        let mut opt = water_request(QmScfBackend::InCore);
        opt.kind = crate::engines::qm::QmKind::Optimize;
        assert!(matches!(
            memory_verdict(&opt, 0),
            MemoryVerdict::ExceedsMustReduce { .. }
        ));

        // Post-HF single point also requires in-core integrals → must reduce.
        let mut mp2 = water_request(QmScfBackend::InCore);
        mp2.method = QmMethod::Mp2;
        assert!(matches!(
            memory_verdict(&mp2, 0),
            MemoryVerdict::ExceedsMustReduce { .. }
        ));

        assert!(MemoryVerdict::Ok.detail("this machine").is_none());
        let msg = MemoryVerdict::ExceedsCanDirect {
            estimate: 20_000_000_000,
            budget: 16_000_000_000,
        }
        .detail("a remote host")
        .unwrap();
        assert!(msg.contains("GiB"));
        assert!(msg.contains("a remote host"));
    }

    #[test]
    fn estimates_incore_and_skips_direct() {
        let est = estimate_incore_memory_bytes(&water_request(QmScfBackend::InCore))
            .expect("in-core RHF/def2-svp water should estimate");
        // water/def2-svp ≈ 24 basis functions → 24⁴·8 ≈ 2.5 MB. Just assert a
        // sane positive magnitude rather than an exact count.
        assert!(
            est > 1_000_000 && est < 100_000_000,
            "unexpected estimate: {est}"
        );
        assert!(estimate_incore_memory_bytes(&water_request(QmScfBackend::Direct)).is_none());
    }
}
