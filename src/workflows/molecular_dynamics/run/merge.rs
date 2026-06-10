//! Layered assembly of a final stage from its contributing layers.
//!
//! The composition is a last-wins merge:
//!
//! ```text
//! global defaults
//!   → force-field-appropriate nonbonded intent   (from the FF family)
//!   → preset stage intent
//!   → system-type injection                       (already folded into the preset)
//!   → user edits (basic / standard / advanced)
//!   → raw passthrough (may introduce new keys)
//! ```
//!
//! The preset builder already folds the first defaults and the system-type
//! injection into its [`MdStage`], so [`assemble`] applies the remaining layers:
//! force-field nonbonded intent (filled only where the preset left a value unset),
//! then user edits, then raw passthrough. The result is a fully-resolved *neutral*
//! stage; turning it into concrete engine syntax is the adapter's job.

use super::stage::{MdParameters, MdStage, StageLength};
use super::system_context::ForceFieldFamily;

/// Per-stage user edits overlaid during assembly. Scalar `Option`s override when
/// `Some`; `params` overlays field-by-field; `raw_passthrough` is appended last.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StageEdits {
    pub temperature_k: Option<f32>,
    pub timestep_ps: Option<f32>,
    pub length: Option<StageLength>,
    pub trajectory_target_frames: Option<u64>,
    pub params: MdParameters,
    pub raw_passthrough: Vec<(String, String)>,
}

/// The force-field-appropriate real-space cutoff *intent* (coulomb, vdw) in nm.
/// The adapter chooses the actual nonbonded block (force-switch vs.
/// potential-shift, dispersion correction); this only carries the cutoff lengths
/// the families conventionally pair with.
pub fn family_nonbonded_intent(family: ForceFieldFamily) -> (f32, f32) {
    match family {
        ForceFieldFamily::Charmm => (1.2, 1.2),
        ForceFieldFamily::Gromos => (1.4, 1.4),
        // AMBER, OPLS, and the generic fallback use 1.0 nm.
        ForceFieldFamily::Amber | ForceFieldFamily::Opls | ForceFieldFamily::Other => (1.0, 1.0),
    }
}

/// Assemble the final neutral stage by applying the force-field nonbonded intent
/// (where unset), then the user edits, then the raw passthrough.
pub fn assemble(mut base: MdStage, family: ForceFieldFamily, edits: &StageEdits) -> MdStage {
    // Force-field nonbonded intent: fill only where the preset left it unset, so a
    // preset that deliberately set a cutoff (a later layer) is not overridden.
    let (rc, rv) = family_nonbonded_intent(family);
    base.params.coulomb_cutoff_nm.get_or_insert(rc);
    base.params.vdw_cutoff_nm.get_or_insert(rv);

    // User edits win over everything above.
    if let Some(t) = edits.temperature_k {
        base.temperature_k = t;
    }
    if let Some(dt) = edits.timestep_ps {
        base.timestep_ps = dt;
    }
    if let Some(length) = edits.length {
        base.length = length;
    }
    if let Some(frames) = edits.trajectory_target_frames {
        base.trajectory_target_frames = Some(frames);
    }
    base.params.overlay(&edits.params);

    // Raw passthrough is merged last and may introduce any key.
    base.raw_passthrough
        .extend(edits.raw_passthrough.iter().cloned());

    base
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflows::molecular_dynamics::run::stage::MdStage;

    #[test]
    fn charmm_intent_fills_one_point_two_cutoffs_when_unset() {
        let stage = assemble(
            MdStage::nvt(300.0),
            ForceFieldFamily::Charmm,
            &StageEdits::default(),
        );
        assert_eq!(stage.params.coulomb_cutoff_nm, Some(1.2));
        assert_eq!(stage.params.vdw_cutoff_nm, Some(1.2));
    }

    #[test]
    fn user_edit_overrides_family_intent_and_preset() {
        let edits = StageEdits {
            temperature_k: Some(310.0),
            params: MdParameters {
                coulomb_cutoff_nm: Some(0.9),
                ..Default::default()
            },
            ..Default::default()
        };
        let stage = assemble(MdStage::npt(300.0), ForceFieldFamily::Amber, &edits);
        // Edit wins over the AMBER 1.0 nm intent.
        assert_eq!(stage.params.coulomb_cutoff_nm, Some(0.9));
        assert_eq!(stage.temperature_k, 310.0);
    }

    #[test]
    fn preset_set_cutoff_is_not_overridden_by_family_intent() {
        let mut base = MdStage::npt(300.0);
        base.params.coulomb_cutoff_nm = Some(1.1); // as if a preset set it
        let stage = assemble(base, ForceFieldFamily::Amber, &StageEdits::default());
        assert_eq!(stage.params.coulomb_cutoff_nm, Some(1.1));
    }

    #[test]
    fn raw_passthrough_is_appended_last() {
        let edits = StageEdits {
            raw_passthrough: vec![("define".to_string(), "-DPOSRES_FC=500".to_string())],
            ..Default::default()
        };
        let stage = assemble(MdStage::produce(300.0), ForceFieldFamily::Amber, &edits);
        assert_eq!(
            stage.raw_passthrough,
            vec![("define".to_string(), "-DPOSRES_FC=500".to_string())]
        );
    }
}
