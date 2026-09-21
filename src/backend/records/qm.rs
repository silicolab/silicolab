use crate::engines::qm::QmOutcome;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Samples {
    pub points: usize,
    pub non_finite: usize,
    pub availability: String,
}
impl Samples {
    fn from_values(values: &[f64]) -> Self {
        Self {
            points: values.len(),
            non_finite: values.iter().filter(|v| !v.is_finite()).count(),
            availability: if values.is_empty() {
                "not supplied; not proof of not calculated"
            } else {
                "supplied"
            }
            .into(),
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct QmFacts {
    pub report_sha256: String,
    pub series_sha256: String,
    pub energy_hartree: Option<f64>,
    pub energy_unavailable: Option<String>,
    pub scf: Samples,
    pub scf_coverage: String,
    pub optimization: Samples,
    pub frequencies: Samples,
    pub frequency_range_cm_inverse: Option<[f64; 2]>,
    pub diagnostics_coverage: String,
    pub input_record: Option<String>,
}
impl QmFacts {
    pub fn from_outcome(outcome: &QmOutcome, input_record: Option<String>) -> Self {
        let finite: Vec<_> = outcome
            .frequencies
            .iter()
            .copied()
            .filter(|v| v.is_finite())
            .collect();
        let range = if finite.is_empty() {
            None
        } else {
            Some([
                finite.iter().copied().fold(f64::INFINITY, f64::min),
                finite.iter().copied().fold(f64::NEG_INFINITY, f64::max),
            ])
        };
        let mut series = Sha256::new();
        for values in [&outcome.scf_trace, &outcome.opt_trace, &outcome.frequencies] {
            series.update((values.len() as u64).to_le_bytes());
            for value in values {
                series.update(value.to_le_bytes());
            }
        }
        Self {
            report_sha256: format!("{:x}", Sha256::digest(outcome.summary.as_bytes())),
            series_sha256: format!("{:x}", series.finalize()),
            energy_hartree: outcome.energy_hartree.is_finite().then_some(outcome.energy_hartree),
            energy_unavailable: (!outcome.energy_hartree.is_finite()).then(|| "engine supplied non-finite energy".into()),
            scf: Samples::from_values(&outcome.scf_trace),
            scf_coverage: "For moving geometries, SCF may cover only the final geometry step".into(),
            optimization: Samples::from_values(&outcome.opt_trace),
            frequencies: Samples::from_values(&outcome.frequencies),
            frequency_range_cm_inverse: range,
            diagnostics_coverage: "Partial: method-quality warnings and diagnostics remain in the raw report; inspect report before interpreting results".into(),
            input_record,
        }
    }
}

pub fn completion(job: &str, outcome: &QmOutcome) -> String {
    format!(
        "QM execution complete; evidence qm:{job}; energy {:?} Eh; engine converged={}. Diagnostics coverage partial: explicitly inspect registered report for method-quality warnings.",
        outcome
            .energy_hartree
            .is_finite()
            .then_some(outcome.energy_hartree),
        outcome.converged
    )
}
