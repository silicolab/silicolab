//! Smart preset/value recommendation from the inherited system context.
//!
//! [`recommend`] reads only an [`EffectiveContext`] (detection overlaid with the
//! user's overrides) — no engine discovery, no version probing — and returns a
//! suggested preset, pre-filled values, a short inline "why" for each suggestion,
//! and any warnings. Everything is a pre-fill, never a lock.

use super::preset::{PresetId, PresetParams, ProductionLength};
use super::stage::DEFAULT_TRAJECTORY_FRAMES;
use super::system_context::{EffectiveContext, ValueSource};

/// Atom count above which a larger trajectory stride / GPU offload is worth
/// suggesting.
const LARGE_SYSTEM_ATOMS: usize = 150_000;

/// One inline "why" note: the inherited fact and what it recommends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecNote {
    pub reason: String,
    pub intent: String,
}

impl RecNote {
    fn new(reason: impl Into<String>, intent: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
            intent: intent.into(),
        }
    }
}

/// The recommendation: a preset, pre-filled top-level values, notes, and
/// warnings. `physiological_offered` flags the one-click 310 K toggle.
#[derive(Debug, Clone, PartialEq)]
pub struct Recommendation {
    pub preset: PresetId,
    pub prefill: PresetParams,
    pub notes: Vec<RecNote>,
    pub warnings: Vec<String>,
    /// A larger trajectory stride is recommended (large system).
    pub larger_stride: bool,
    /// Offer the one-click physiological (310 K) temperature.
    pub physiological_offered: bool,
}

/// Recommend a preset and values for the system. Pure; reads only `eff`.
pub fn recommend(eff: &EffectiveContext) -> Recommendation {
    let preset = pick_preset(eff);
    let prefill = PresetParams {
        temperature_k: 300.0,
        production: ProductionLength::Standard,
        timestep_ps: 0.002,
    };

    let mut notes = Vec::new();
    let mut warnings = Vec::new();

    // Force-field family → nonbonded treatment + conventional water + Standard.
    let family = eff.force_field_family();
    if family.is_biomolecular() {
        let water = eff.water_token().unwrap_or("the family's conventional");
        notes.push(RecNote::new(
            format!("Force field is {}", family.label()),
            format!(
                "apply the {} nonbonded treatment with {water} water",
                family.label()
            ),
        ));
    }

    // ff19SB pairs with OPC water (advisory).
    if eff.force_field_token().eq_ignore_ascii_case("amber19sb")
        && eff
            .water_token()
            .is_some_and(|w| !w.eq_ignore_ascii_case("opc"))
    {
        notes.push(RecNote::new(
            "Force field is ff19SB",
            "ff19SB is parameterized for OPC water — consider rebuilding with OPC",
        ));
    }

    if eff.has_membrane() {
        notes.push(RecNote::new(
            "Membrane / lipids present",
            "semi-isotropic pressure, 3-group coupling, staged restraint release",
        ));
    }
    if eff.has_ligand() {
        notes.push(RecNote::new(
            "Ligand present",
            "restrain the ligand's heavy atoms during equilibration",
        ));
    }
    if eff.has_nucleic() {
        notes.push(RecNote::new(
            "Nucleic acid present",
            "nucleic/solvent coupling; higher salt (>0.15 M) is common",
        ));
    }

    // Temperature: default 300 K, offer one-click 310 K.
    notes.push(RecNote::new(
        "Temperature unset",
        "defaulting to 300 K; a one-click 310 K (physiological) toggle is available",
    ));

    // Large system → larger stride + GPU offload (a submission concern).
    let larger_stride = eff.atom_count() >= LARGE_SYSTEM_ATOMS;
    if larger_stride {
        notes.push(RecNote::new(
            format!("Large system ({} atoms)", eff.atom_count()),
            "use a larger trajectory stride; consider GPU offload at submission",
        ));
    }

    // Low-confidence note: nothing of interest detected and the user hasn't said
    // otherwise — heuristics can't tell "absent" from "unrecognized".
    if low_confidence(eff) {
        notes.push(RecNote::new(
            "No membrane, ligand, or nucleic acid detected",
            "based on known residue names — check the toggles if your system is unusual",
        ));
    }

    // Net charge should have been neutralized in Build.
    if eff.net_charge().abs() > 0.01 {
        warnings.push(format!(
            "System net charge is {:+.2}; the Build step should have neutralized it. \
             A charged box distorts PME electrostatics.",
            eff.net_charge()
        ));
    }

    Recommendation {
        preset,
        prefill,
        notes,
        warnings,
        larger_stride,
        physiological_offered: true,
    }
}

/// The recommended trajectory frame target, scaled down for large systems.
pub fn recommended_trajectory_frames(eff: &EffectiveContext) -> u64 {
    if eff.atom_count() >= LARGE_SYSTEM_ATOMS {
        DEFAULT_TRAJECTORY_FRAMES / 2
    } else {
        DEFAULT_TRAJECTORY_FRAMES
    }
}

/// Pick the most specific applicable preset.
fn pick_preset(eff: &EffectiveContext) -> PresetId {
    if eff.has_membrane() {
        PresetId::MembraneProtein
    } else if eff.has_ligand() {
        PresetId::ProteinLigand
    } else if eff.has_nucleic() && !eff.has_protein() {
        PresetId::NucleicAcid
    } else if eff.force_field_family().is_biomolecular() && (eff.has_protein() || eff.has_nucleic())
    {
        PresetId::StandardBiomolecule
    } else {
        // Framework / homogeneous-LJ / unrecognized: a safe generic relax.
        PresetId::QuickRelax
    }
}

/// Whether nothing of interest was found *and* the user hasn't overridden — the
/// only case where the low-confidence note is meaningful.
fn low_confidence(eff: &EffectiveContext) -> bool {
    let axes = [eff.membrane(), eff.ligand(), eff.nucleic()];
    axes.iter()
        .all(|(value, source)| !value && *source == ValueSource::Detected)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflows::molecular_dynamics::run::system_context::{
        ForceFieldFamily, MdSystemContext, SystemTypeOverrides,
    };

    fn ctx() -> MdSystemContext {
        MdSystemContext {
            force_field_token: "amber14sb".to_string(),
            force_field_family: ForceFieldFamily::Amber,
            water_token: Some("tip3p".to_string()),
            detected_protein: true,
            detected_nucleic: false,
            detected_membrane: false,
            detected_ligand: false,
            is_framework: false,
            net_charge: 0.0,
            atom_count: 20_000,
            restraint_groups: vec![],
            hmr_applied: false,
        }
    }

    #[test]
    fn plain_protein_recommends_standard() {
        let c = ctx();
        let rec = recommend(&c.with_overrides(SystemTypeOverrides::default()));
        assert_eq!(rec.preset, PresetId::StandardBiomolecule);
        assert!(rec.warnings.is_empty());
        // The ff-family note is present.
        assert!(rec.notes.iter().any(|n| n.reason.contains("AMBER")));
    }

    #[test]
    fn membrane_override_steers_to_membrane_preset() {
        let c = ctx();
        // Detection found no membrane; the user overrides it on.
        let eff = c.with_overrides(SystemTypeOverrides {
            membrane: Some(true),
            ..Default::default()
        });
        let rec = recommend(&eff);
        assert_eq!(rec.preset, PresetId::MembraneProtein);
        // No low-confidence note now, since the user asserted a membrane.
        assert!(
            !rec.notes
                .iter()
                .any(|n| n.reason.starts_with("No membrane"))
        );
    }

    #[test]
    fn nothing_detected_emits_low_confidence_note() {
        let mut c = ctx();
        c.detected_protein = false;
        let rec = recommend(&c.with_overrides(SystemTypeOverrides::default()));
        assert!(
            rec.notes
                .iter()
                .any(|n| n.reason.starts_with("No membrane")),
            "expected a low-confidence note when nothing is detected"
        );
    }

    #[test]
    fn nonzero_net_charge_warns() {
        let mut c = ctx();
        c.net_charge = -3.0;
        let rec = recommend(&c.with_overrides(SystemTypeOverrides::default()));
        assert_eq!(rec.warnings.len(), 1);
        assert!(rec.warnings[0].contains("net charge"));
    }

    #[test]
    fn large_system_recommends_larger_stride() {
        let mut c = ctx();
        c.atom_count = 250_000;
        let eff = c.with_overrides(SystemTypeOverrides::default());
        let rec = recommend(&eff);
        assert!(rec.larger_stride);
        assert!(recommended_trajectory_frames(&eff) < DEFAULT_TRAJECTORY_FRAMES);
    }

    #[test]
    fn ligand_beats_nucleic_in_preset_pick() {
        let mut c = ctx();
        c.detected_ligand = true;
        c.detected_nucleic = true;
        let rec = recommend(&c.with_overrides(SystemTypeOverrides::default()));
        assert_eq!(rec.preset, PresetId::ProteinLigand);
    }
}
