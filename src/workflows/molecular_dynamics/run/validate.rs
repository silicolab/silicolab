//! Engine-neutral validation of an assembled stage sequence (Appendix E rules).
//!
//! [`validate`] checks a `[MdStage]` chain against the system context and returns
//! issues to surface before submission. Errors should block a run; warnings are
//! advisory. The checks are expressed in neutral terms (timestep/constraint
//! consistency, HMR requirement, restraint availability, pressure-shape for
//! membranes, equilibration-only barostats, unequilibrated Parrinello–Rahman,
//! raw-passthrough flagging) so any engine adapter benefits.

use super::stage::{BarostatKind, ConstraintScope, MdStage, PressureShape, StageKind};
use super::system_context::EffectiveContext;

/// Timestep (ps) above which hydrogen-mass repartitioning is required.
const LONG_TIMESTEP_PS: f32 = 0.0025;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IssueSeverity {
    Warning,
    Error,
}

/// One validation finding, optionally tied to a stage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationIssue {
    pub severity: IssueSeverity,
    pub stage: Option<String>,
    pub message: String,
}

impl ValidationIssue {
    fn error(stage: Option<&str>, message: impl Into<String>) -> Self {
        Self {
            severity: IssueSeverity::Error,
            stage: stage.map(str::to_string),
            message: message.into(),
        }
    }

    fn warning(stage: Option<&str>, message: impl Into<String>) -> Self {
        Self {
            severity: IssueSeverity::Warning,
            stage: stage.map(str::to_string),
            message: message.into(),
        }
    }
}

/// Validate a stage sequence against the system context.
pub fn validate(stages: &[MdStage], eff: &EffectiveContext) -> Vec<ValidationIssue> {
    let mut issues = Vec::new();

    if stages.is_empty() {
        issues.push(ValidationIssue::error(None, "Add at least one stage."));
        return issues;
    }

    let mut npt_equilibrated = false;

    for stage in stages {
        let name = stage.name.as_str();

        if stage.kind.is_dynamics() {
            // Timestep / constraint / HMR consistency.
            if stage.timestep_ps > LONG_TIMESTEP_PS && !eff.hmr_applied() {
                issues.push(ValidationIssue::error(
                    Some(name),
                    format!(
                        "Timestep {:.4} ps needs hydrogen-mass repartitioning, which was not \
                         applied in Build. Rebuild with HMR or lower the timestep to 2 fs.",
                        stage.timestep_ps
                    ),
                ));
            } else if stage.timestep_ps >= 0.002
                && stage.params.constraints == Some(ConstraintScope::None)
            {
                issues.push(ValidationIssue::error(
                    Some(name),
                    "A 2 fs timestep requires constraining hydrogen bonds; constraints are set \
                     to none.",
                ));
            }
        }

        // Restraints require a restraint group to exist from Build.
        if stage.restraint.is_restrained() && eff.restraint_groups().is_empty() {
            issues.push(ValidationIssue::error(
                Some(name),
                "This stage applies position restraints, but the Build step recorded no \
                 restraint groups (no posre itp). Rebuild with restraints or remove them.",
            ));
        }

        if let Some(pressure) = stage.pressure {
            // Membrane systems must use semi-isotropic pressure coupling.
            if eff.has_membrane() && pressure.shape == PressureShape::Isotropic {
                issues.push(ValidationIssue::warning(
                    Some(name),
                    "Membrane system using isotropic pressure coupling; semi-isotropic is \
                     expected so the bilayer area can relax.",
                ));
            }

            // Equilibration-only barostats must not drive production.
            if pressure.barostat.is_equilibration_only()
                && matches!(stage.kind, StageKind::Produce | StageKind::Extend)
            {
                issues.push(ValidationIssue::error(
                    Some(name),
                    "Production stage uses an equilibration-only barostat (Berendsen); it does \
                     not sample the correct ensemble. Use stochastic cell rescaling or \
                     Parrinello–Rahman.",
                ));
            }

            // Parrinello–Rahman must not start from an unequilibrated box.
            if pressure.barostat == BarostatKind::ParrinelloRahman && !npt_equilibrated {
                issues.push(ValidationIssue::warning(
                    Some(name),
                    "Parrinello–Rahman is starting before any NPT equilibration; it oscillates \
                     from an unequilibrated configuration. Equilibrate first, or use stochastic \
                     cell rescaling.",
                ));
            }
        }

        // Flag every raw-passthrough key: it is written verbatim and unvalidated.
        for (key, _) in &stage.raw_passthrough {
            issues.push(ValidationIssue::warning(
                Some(name),
                format!("Raw key `{key}` is passed through verbatim and not validated."),
            ));
        }

        if matches!(stage.kind, StageKind::NptEquilibrate) {
            npt_equilibrated = true;
        }
    }

    issues
}

/// Whether any issue is an error (a run should be blocked).
pub fn has_errors(issues: &[ValidationIssue]) -> bool {
    issues.iter().any(|i| i.severity == IssueSeverity::Error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflows::molecular_dynamics::run::preset::{PresetId, PresetParams};
    use crate::workflows::molecular_dynamics::run::stage::{MdStage, PressureCoupling};
    use crate::workflows::molecular_dynamics::run::system_context::{
        ForceFieldFamily, MdSystemContext, SystemTypeOverrides,
    };

    fn ctx(hmr: bool, restraints: bool) -> MdSystemContext {
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
            atom_count: 10_000,
            restraint_groups: if restraints {
                vec!["solute".to_string()]
            } else {
                vec![]
            },
            hmr_applied: hmr,
        }
    }

    #[test]
    fn standard_preset_validates_clean_when_restraints_exist() {
        let c = ctx(false, true);
        let eff = c.with_overrides(SystemTypeOverrides::default());
        let stages = PresetId::StandardBiomolecule.build(&eff, &PresetParams::default());
        let issues = validate(&stages, &eff);
        assert!(!has_errors(&issues), "unexpected errors: {issues:?}");
    }

    #[test]
    fn long_timestep_without_hmr_is_an_error() {
        let c = ctx(false, true);
        let eff = c.with_overrides(SystemTypeOverrides::default());
        let stages = PresetId::FastProduction.build(&eff, &PresetParams::default());
        let issues = validate(&stages, &eff);
        assert!(has_errors(&issues));
        assert!(issues.iter().any(|i| i.message.contains("hydrogen-mass")));

        // With HMR applied, the same preset is clean.
        let c = ctx(true, true);
        let eff = c.with_overrides(SystemTypeOverrides::default());
        let stages = PresetId::FastProduction.build(&eff, &PresetParams::default());
        assert!(!has_errors(&validate(&stages, &eff)));
    }

    #[test]
    fn restraints_without_a_group_error() {
        let c = ctx(false, false); // no restraint groups recorded
        let eff = c.with_overrides(SystemTypeOverrides::default());
        let stages = PresetId::StandardBiomolecule.build(&eff, &PresetParams::default());
        let issues = validate(&stages, &eff);
        assert!(has_errors(&issues));
        assert!(
            issues
                .iter()
                .any(|i| i.message.contains("no restraint groups"))
        );
    }

    #[test]
    fn berendsen_production_barostat_is_rejected() {
        let c = ctx(false, true);
        let eff = c.with_overrides(SystemTypeOverrides::default());
        let mut prod = MdStage::produce(300.0);
        prod.pressure = Some(PressureCoupling {
            barostat: BarostatKind::Berendsen,
            ..PressureCoupling::isotropic()
        });
        let issues = validate(&[prod], &eff);
        assert!(has_errors(&issues));
        assert!(
            issues
                .iter()
                .any(|i| i.message.contains("equilibration-only barostat"))
        );
    }

    #[test]
    fn raw_passthrough_keys_are_flagged() {
        let c = ctx(false, true);
        let eff = c.with_overrides(SystemTypeOverrides::default());
        let mut prod = MdStage::produce(300.0);
        prod.raw_passthrough
            .push(("nstcomm".to_string(), "100".to_string()));
        let issues = validate(&[prod], &eff);
        assert!(issues.iter().any(|i| i.message.contains("nstcomm")));
    }
}
