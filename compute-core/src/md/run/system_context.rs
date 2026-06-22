//! What a completed "Build MD System" step tells "Run MD" about the system.
//!
//! This is split into two strictly separate layers so re-detection never clobbers
//! a user's manual correction:
//!
//! * [`MdSystemContext`] is the **immutable build-time detection record** — the
//!   one thing persisted (as `md_system_context.json`). Its `detected_*` fields
//!   are heuristic findings from residue names, not ground truth. A rebuild or an
//!   improved heuristic rewrites *only* this record.
//! * [`SystemTypeOverrides`] are **per-run user corrections** that live in the run
//!   configuration, never written back into the persisted record.
//!
//! [`MdSystemContext::with_overrides`] overlays the two into an
//! [`EffectiveContext`] — detection with overrides winning — which is what the
//! recommendation engine and the GUI consume. The effective view also reports,
//! per axis, whether a value came from detection or an override, so the UI can
//! honestly show "auto-detected X, you set Y".

use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::domain::{AtomCategory, Structure, biopolymer};

/// Biomolecular force-field family, used to pick the engine's nonbonded
/// treatment and the conventional water model. Classified from the force-field
/// token; `Other` covers monatomic-LJ / framework / custom systems with no
/// biomolecular nonbonded convention.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ForceFieldFamily {
    Charmm,
    Amber,
    Opls,
    Gromos,
    Other,
}

impl ForceFieldFamily {
    /// Classify from a force-field token (e.g. `amber99sb-ildn`, `charmm27`,
    /// `oplsaa`, `gromos54a7`).
    pub fn from_token(token: &str) -> Self {
        let t = token.trim().to_ascii_lowercase();
        if t.starts_with("charmm") {
            Self::Charmm
        } else if t.starts_with("amber") {
            Self::Amber
        } else if t.starts_with("opls") {
            Self::Opls
        } else if t.starts_with("gromos") {
            Self::Gromos
        } else {
            Self::Other
        }
    }

    /// Whether this family carries a biomolecular nonbonded convention (PME +
    /// force-field block), versus the homogeneous-LJ / framework fallback.
    pub fn is_biomolecular(self) -> bool {
        !matches!(self, Self::Other)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Charmm => "CHARMM",
            Self::Amber => "AMBER",
            Self::Opls => "OPLS",
            Self::Gromos => "GROMOS",
            Self::Other => "Generic",
        }
    }
}

/// The immutable build-time detection record. Persisted with the project;
/// **overrides are never written here**.
///
/// Most fields carry `#[serde(default)]` so a partial/older context file loads to
/// sensible defaults rather than failing — a release-robustness measure, not a
/// back-compat one (this codebase has no released formats and recreates data
/// freely). The two force-field-identity fields are deliberately left *required*:
/// without them the recommender and adapter cannot produce correct physics, so a
/// context missing them should fail loudly rather than silently default to a
/// wrong force field.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MdSystemContext {
    /// Engine-facing force-field token the build used (e.g. `amber99sb-ildn`).
    /// Required — core force-field identity, no meaningful fallback.
    pub force_field_token: String,
    /// Family classified from the token. Required — recommendation and the
    /// adapter's nonbonded block both key on it; there is no safe default.
    pub force_field_family: ForceFieldFamily,
    /// Water-model token the build used (e.g. `tip3p`), if the system was
    /// solvated. `None` for a dry / framework build.
    #[serde(default)]
    pub water_token: Option<String>,
    /// Standard amino-acid residues were found.
    #[serde(default)]
    pub detected_protein: bool,
    /// Nucleotide residues were found.
    #[serde(default)]
    pub detected_nucleic: bool,
    /// Lipid/sterol residues were found (membrane).
    #[serde(default)]
    pub detected_membrane: bool,
    /// A non-polymer organic residue (neither solvent nor monatomic ion) was
    /// found — a small-molecule ligand.
    #[serde(default)]
    pub detected_ligand: bool,
    /// The system is a periodic framework (nanosheet/material) rather than a
    /// solvated biomolecule.
    #[serde(default)]
    pub is_framework: bool,
    /// Net charge of the built system. Defaults to 0 (assume neutral) when absent.
    #[serde(default)]
    pub net_charge: f32,
    #[serde(default)]
    pub atom_count: usize,
    /// Position-restraint group labels available (posre itps the build wrote).
    #[serde(default)]
    pub restraint_groups: Vec<String>,
    /// Hydrogen-mass repartitioning was applied in the build (required for the
    /// long-timestep fast path). Defaults to `false` — the conservative choice,
    /// since it blocks the long-timestep path rather than silently enabling it.
    #[serde(default)]
    pub hmr_applied: bool,
}

impl MdSystemContext {
    /// Build the detection record from a freshly built system.
    ///
    /// System-type flags are detected from the structure's per-residue
    /// composition (reusing the shared residue detectors); `net_charge`,
    /// `is_framework`, `hmr_applied` and `restraint_groups` are supplied by the
    /// build path, which knows them directly.
    pub fn from_built(
        structure: &Structure,
        force_field_token: &str,
        water_token: Option<&str>,
        is_framework: bool,
        net_charge: f32,
        hmr_applied: bool,
        restraint_groups: Vec<String>,
    ) -> Self {
        let detection = detect_system_types(structure);
        Self {
            force_field_token: force_field_token.to_string(),
            force_field_family: ForceFieldFamily::from_token(force_field_token),
            water_token: water_token.map(str::to_string),
            detected_protein: detection.protein,
            detected_nucleic: detection.nucleic,
            detected_membrane: detection.membrane,
            detected_ligand: detection.ligand,
            is_framework,
            net_charge,
            atom_count: structure.atoms.len(),
            restraint_groups,
            hmr_applied,
        }
    }

    /// Overlay user overrides to produce the effective view callers consume.
    pub fn with_overrides(&self, overrides: SystemTypeOverrides) -> EffectiveContext<'_> {
        EffectiveContext {
            context: self,
            overrides,
        }
    }

    /// Persist as JSON.
    pub fn save(&self, path: &Path) -> Result<()> {
        let json = serde_json::to_string_pretty(self).context("serializing MD system context")?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        std::fs::write(path, json).with_context(|| format!("writing {}", path.display()))
    }

    /// Load a persisted detection record.
    pub fn load(path: &Path) -> Result<Self> {
        let json =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        serde_json::from_str(&json).with_context(|| format!("parsing {}", path.display()))
    }
}

/// Per-run user corrections to the detected system types. `None` means "trust
/// detection". Lives in the run configuration, **not** in the persisted context.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemTypeOverrides {
    pub membrane: Option<bool>,
    pub ligand: Option<bool>,
    pub nucleic: Option<bool>,
}

impl SystemTypeOverrides {
    /// Whether any override is set.
    pub fn any(&self) -> bool {
        self.membrane.is_some() || self.ligand.is_some() || self.nucleic.is_some()
    }
}

/// Where an effective value came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueSource {
    Detected,
    Overridden,
}

/// Detection overlaid with overrides (override wins). Borrows the immutable
/// context; cheap to construct per frame.
#[derive(Debug, Clone, Copy)]
pub struct EffectiveContext<'a> {
    pub context: &'a MdSystemContext,
    pub overrides: SystemTypeOverrides,
}

impl EffectiveContext<'_> {
    fn resolve(detected: bool, over: Option<bool>) -> (bool, ValueSource) {
        match over {
            Some(value) => (value, ValueSource::Overridden),
            None => (detected, ValueSource::Detected),
        }
    }

    /// Effective membrane flag and its source.
    pub fn membrane(&self) -> (bool, ValueSource) {
        Self::resolve(self.context.detected_membrane, self.overrides.membrane)
    }

    /// Effective ligand flag and its source.
    pub fn ligand(&self) -> (bool, ValueSource) {
        Self::resolve(self.context.detected_ligand, self.overrides.ligand)
    }

    /// Effective nucleic-acid flag and its source.
    pub fn nucleic(&self) -> (bool, ValueSource) {
        Self::resolve(self.context.detected_nucleic, self.overrides.nucleic)
    }

    pub fn has_membrane(&self) -> bool {
        self.membrane().0
    }
    pub fn has_ligand(&self) -> bool {
        self.ligand().0
    }
    pub fn has_nucleic(&self) -> bool {
        self.nucleic().0
    }
    /// Protein detection is reliable from amino-acid residues; it has no override.
    pub fn has_protein(&self) -> bool {
        self.context.detected_protein
    }

    pub fn force_field_family(&self) -> ForceFieldFamily {
        self.context.force_field_family
    }
    pub fn force_field_token(&self) -> &str {
        &self.context.force_field_token
    }
    pub fn water_token(&self) -> Option<&str> {
        self.context.water_token.as_deref()
    }
    pub fn net_charge(&self) -> f32 {
        self.context.net_charge
    }
    pub fn atom_count(&self) -> usize {
        self.context.atom_count
    }
    pub fn is_framework(&self) -> bool {
        self.context.is_framework
    }
    pub fn hmr_applied(&self) -> bool {
        self.context.hmr_applied
    }
    pub fn restraint_groups(&self) -> &[String] {
        &self.context.restraint_groups
    }
}

/// Detected system-type flags.
struct Detection {
    protein: bool,
    nucleic: bool,
    membrane: bool,
    ligand: bool,
}

/// Detect system types from a structure's per-residue composition. Reuses the
/// shared residue detectors and [`Structure::atom_category`] (which already
/// excludes solvent and monatomic ions) so a lipid is reported as membrane and an
/// organic non-polymer as a ligand.
fn detect_system_types(structure: &Structure) -> Detection {
    let mut detection = Detection {
        protein: false,
        nucleic: false,
        membrane: false,
        ligand: false,
    };

    let Some(bio) = structure
        .biopolymer
        .as_ref()
        .filter(|bio| bio.is_compatible_with_atom_count(structure.atoms.len()))
    else {
        return detection;
    };

    for residue in &bio.residues {
        let name = residue.residue_name.trim();
        if residue.is_standard_amino_acid {
            detection.protein = true;
            continue;
        }
        if biopolymer::is_nucleic_acid_residue(name) {
            detection.nucleic = true;
            continue;
        }
        if biopolymer::is_lipid_residue(name) {
            detection.membrane = true;
            continue;
        }
        if biopolymer::is_water_residue(name) {
            continue;
        }
        // Fall back to the atom-level category of the residue's first atom, which
        // distinguishes a monatomic ion from an organic ligand.
        if let Some(&first) = residue.atom_indices.first()
            && structure.atom_category(first) == AtomCategory::Ligand
        {
            detection.ligand = true;
        }
    }

    detection
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn family_classifies_from_token() {
        assert_eq!(
            ForceFieldFamily::from_token("amber99sb-ildn"),
            ForceFieldFamily::Amber
        );
        assert_eq!(
            ForceFieldFamily::from_token("CHARMM36m"),
            ForceFieldFamily::Charmm
        );
        assert_eq!(
            ForceFieldFamily::from_token("oplsaa"),
            ForceFieldFamily::Opls
        );
        assert_eq!(
            ForceFieldFamily::from_token("gromos54a7"),
            ForceFieldFamily::Gromos
        );
        // A framework / custom token has no biomolecular convention.
        let other = ForceFieldFamily::from_token("nanosheet-custom");
        assert_eq!(other, ForceFieldFamily::Other);
        assert!(!other.is_biomolecular());
        assert!(ForceFieldFamily::Amber.is_biomolecular());
    }

    #[test]
    fn overrides_win_over_detection_and_report_source() {
        let ctx = MdSystemContext {
            force_field_token: "amber14sb".to_string(),
            force_field_family: ForceFieldFamily::Amber,
            water_token: Some("tip3p".to_string()),
            detected_protein: true,
            detected_nucleic: false,
            detected_membrane: false,
            detected_ligand: false,
            is_framework: false,
            net_charge: 0.0,
            atom_count: 1234,
            restraint_groups: vec![],
            hmr_applied: false,
        };

        // No overrides: effective == detection, sourced as detected.
        let plain = ctx.with_overrides(SystemTypeOverrides::default());
        assert_eq!(plain.membrane(), (false, ValueSource::Detected));
        assert!(!plain.has_membrane());

        // A membrane override flips the effective value and is sourced as set.
        let overridden = ctx.with_overrides(SystemTypeOverrides {
            membrane: Some(true),
            ..Default::default()
        });
        assert_eq!(overridden.membrane(), (true, ValueSource::Overridden));
        assert!(overridden.has_membrane());

        // Crucially, the persisted detection record is untouched by the override —
        // a later re-detection cannot clobber the user's correction because the
        // override does not live here.
        assert!(!ctx.detected_membrane);
    }

    #[test]
    fn context_round_trips_through_json() {
        let ctx = MdSystemContext {
            force_field_token: "charmm27".to_string(),
            force_field_family: ForceFieldFamily::Charmm,
            water_token: Some("tip3p".to_string()),
            detected_protein: true,
            detected_nucleic: false,
            detected_membrane: true,
            detected_ligand: true,
            is_framework: false,
            net_charge: -2.0,
            atom_count: 9001,
            restraint_groups: vec!["solute".to_string()],
            hmr_applied: true,
        };
        let json = serde_json::to_string(&ctx).unwrap();
        let back: MdSystemContext = serde_json::from_str(&json).unwrap();
        assert_eq!(ctx, back);
    }

    #[test]
    fn partial_context_loads_with_field_defaults() {
        // Only the two required force-field-identity fields are present;
        // everything else is absent and must fall back to sensible defaults
        // rather than failing to deserialize.
        let json = r#"{
            "force_field_token": "amber14sb",
            "force_field_family": "Amber"
        }"#;
        let ctx: MdSystemContext = serde_json::from_str(json).expect("partial context loads");
        assert_eq!(ctx.force_field_token, "amber14sb");
        assert_eq!(ctx.force_field_family, ForceFieldFamily::Amber);
        // Defaulted fields.
        assert_eq!(ctx.water_token, None);
        assert!(!ctx.detected_protein);
        assert!(!ctx.detected_nucleic);
        assert!(!ctx.detected_membrane);
        assert!(!ctx.detected_ligand);
        assert!(!ctx.is_framework);
        assert_eq!(ctx.net_charge, 0.0);
        assert_eq!(ctx.atom_count, 0);
        assert!(ctx.restraint_groups.is_empty());
        assert!(!ctx.hmr_applied);
    }

    #[test]
    fn context_missing_required_force_field_fails_loudly() {
        // The force-field identity has no safe default; a context missing it must
        // fail to deserialize rather than silently pick a wrong force field.
        let missing_family = r#"{ "force_field_token": "amber14sb" }"#;
        assert!(serde_json::from_str::<MdSystemContext>(missing_family).is_err());
        let missing_token = r#"{ "force_field_family": "Amber" }"#;
        assert!(serde_json::from_str::<MdSystemContext>(missing_token).is_err());
    }

    #[test]
    fn bare_structure_detects_nothing() {
        // No biopolymer metadata -> all flags false (the recommendation engine
        // turns this into a low-confidence note rather than a hard "no").
        let s = Structure::new("bare", vec![]);
        let d = detect_system_types(&s);
        assert!(!d.protein && !d.nucleic && !d.membrane && !d.ligand);
    }
}
