use std::{path::Path, path::PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::{
    backend::tasks::TaskKind,
    domain::Structure,
    engines::gromacs::{
        FreezeSelection, MdpSettings, StageSpec, framework_freeze_selection, input::FreezeGroup,
    },
    frontend::state::AppState,
    workflows::molecular_dynamics::{
        MdProtocolOptions, MdSystemContext, MdTopology, full_protocol,
    },
};

pub const MD_TOPOLOGY_FILE: &str = "system_topology.json";

/// The GROMACS topology a [`TaskKind::BuildMdSystem`] run writes when GROMACS is
/// the build engine (`pdb2gmx -p topol.top`, updated by solvate/genion).
pub const MD_GROMACS_TOPOLOGY_FILE: &str = "topol.top";

/// Run hints a framework (nanosheet) build records so a later MD run applies the
/// right `.mdp`/freeze settings — written into the build run directory.
pub const MD_FRAMEWORK_FILE: &str = "framework_run.json";

/// The engine-neutral MD system context (force-field family, water, detected
/// system types, net charge, restraint availability) a build records so a later
/// "Run MD" can recommend a preset and values. Written into the build run
/// directory, loaded the same way as [`MD_FRAMEWORK_FILE`].
pub const MD_SYSTEM_CONTEXT_FILE: &str = "md_system_context.json";

/// Build and persist the MD system context for a completed build into its run
/// directory. `solute` supplies residue-based type detection (it carries the
/// biopolymer metadata the solvated output may lack); `full_atom_count` is the
/// final, post-solvation system size. Best-effort: a write failure is non-fatal —
/// a later run simply falls back to a blank recommendation.
#[allow(clippy::too_many_arguments)]
pub fn write_md_system_context(
    working_dir: &Path,
    solute: &Structure,
    full_atom_count: usize,
    force_field_token: &str,
    water_token: Option<&str>,
    is_framework: bool,
    net_charge: f32,
    hmr_applied: bool,
    restraint_groups: Vec<String>,
) {
    let mut context = MdSystemContext::from_built(
        solute,
        force_field_token,
        water_token,
        is_framework,
        net_charge,
        hmr_applied,
        restraint_groups,
    );
    // Detection comes from the solute; the recorded size is the full solvated
    // system, which is what "large system" recommendations key on.
    context.atom_count = full_atom_count;
    let _ = context.save(&working_dir.join(MD_SYSTEM_CONTEXT_FILE));
}

/// Load the MD system context recorded by the entry's latest completed MD system
/// build, if any. Mirrors [`load_framework_metadata_for_entry`]. The first
/// consumer is the Run MD recommendation surfaced in the GUI/console (next
/// phase); the data is recorded now so it is available when that lands.
#[allow(dead_code)]
pub fn load_md_system_context_for_entry(
    state: &AppState,
    entry_id: u64,
) -> Option<MdSystemContext> {
    let run = state
        .tasks
        .latest_completed_run_for_result(TaskKind::BuildMdSystem, entry_id)?;
    let path = run.run_dir.as_ref()?.join(MD_SYSTEM_CONTEXT_FILE);
    path.exists().then(|| MdSystemContext::load(&path).ok())?
}

/// What a framework MD system needs a run to do: keep the molecule periodic
/// (flexible model) and/or freeze the sheet (rigid model). Persisted by the
/// build and reapplied to every stage of the run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrameworkRunMetadata {
    pub periodic_molecules: bool,
    pub freeze_group: Option<String>,
    pub framework_atom_count: usize,
}

impl FrameworkRunMetadata {
    pub fn save(&self, path: &Path) -> Result<()> {
        let json = serde_json::to_string_pretty(self).context("serializing framework run data")?;
        std::fs::write(path, json).with_context(|| format!("writing {}", path.display()))
    }

    pub fn load(path: &Path) -> Result<Self> {
        let json =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        serde_json::from_str(&json).with_context(|| format!("parsing {}", path.display()))
    }

    /// The freeze selection a run's [`prepare_system`] needs, if this system
    /// freezes its framework.
    ///
    /// [`prepare_system`]: crate::engines::gromacs::prepare_system
    pub fn freeze_selection(&self) -> Option<FreezeSelection> {
        self.freeze_group
            .as_ref()
            .map(|_| framework_freeze_selection(self.framework_atom_count))
    }

    /// Apply this system's run hints to a stage's `.mdp` settings.
    pub fn apply_to(&self, settings: &mut MdpSettings) {
        settings.periodic_molecules = self.periodic_molecules;
        settings.freeze = self.freeze_group.clone().map(|group| FreezeGroup { group });
    }
}

/// Load the framework run hints recorded by the entry's latest completed MD
/// system build, if it was a framework build.
pub fn load_framework_metadata_for_entry(
    state: &AppState,
    entry_id: u64,
) -> Option<FrameworkRunMetadata> {
    let run = state
        .tasks
        .latest_completed_run_for_result(TaskKind::BuildMdSystem, entry_id)?;
    let path = run.run_dir.as_ref()?.join(MD_FRAMEWORK_FILE);
    path.exists()
        .then(|| FrameworkRunMetadata::load(&path).ok())?
}

pub fn md_topology_path_for_entry(state: &AppState, entry_id: u64) -> Option<PathBuf> {
    let run = state
        .tasks
        .latest_completed_run_for_result(TaskKind::BuildMdSystem, entry_id)?;
    let run_dir = run.run_dir.as_ref()?;
    let path = run_dir.join(MD_TOPOLOGY_FILE);
    path.exists().then_some(path)
}

/// Path to the GROMACS `topol.top` produced by the entry's latest completed MD
/// system build, if that build used the GROMACS engine. This is the force-field
/// topology a run reuses directly.
pub fn gromacs_topology_path_for_entry(state: &AppState, entry_id: u64) -> Option<PathBuf> {
    let run = state
        .tasks
        .latest_completed_run_for_result(TaskKind::BuildMdSystem, entry_id)?;
    let run_dir = run.run_dir.as_ref()?;
    let path = run_dir.join(MD_GROMACS_TOPOLOGY_FILE);
    path.exists().then_some(path)
}

pub fn load_md_topology_for_entry(state: &AppState, entry_id: u64) -> Option<MdTopology> {
    let path = md_topology_path_for_entry(state, entry_id)?;
    MdTopology::load(&path).ok()
}

pub fn protocol_stage_specs(options: &MdProtocolOptions) -> Vec<StageSpec> {
    full_protocol(options)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_md_system_context_records_full_count_and_classifies_family() {
        use crate::workflows::molecular_dynamics::ForceFieldFamily;
        use nalgebra::Point3;

        // A one-atom solute stands in for the pre-solvation system; the recorded
        // size is the (larger) solvated count, while detection reads the solute.
        let solute = Structure::new(
            "solute",
            vec![crate::domain::Atom {
                element: "C".to_string(),
                position: Point3::origin(),
                charge: 0.0,
            }],
        );
        let dir = std::env::temp_dir().join("silicolab_md_context_write_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        write_md_system_context(
            &dir,
            &solute,
            5_000,
            "amber14sb",
            Some("tip3p"),
            false,
            0.0,
            false,
            vec!["solute".to_string()],
        );

        let ctx = MdSystemContext::load(&dir.join(MD_SYSTEM_CONTEXT_FILE)).unwrap();
        // The recorded size is the full solvated system, not the 1-atom solute.
        assert_eq!(ctx.atom_count, 5_000);
        assert_eq!(ctx.force_field_family, ForceFieldFamily::Amber);
        assert_eq!(ctx.water_token.as_deref(), Some("tip3p"));
        assert_eq!(ctx.restraint_groups, vec!["solute".to_string()]);
        // A bare carbon atom has no biopolymer metadata: nothing is detected.
        assert!(!ctx.detected_protein);
    }

    #[test]
    fn rigid_framework_metadata_freezes_and_round_trips() {
        let meta = FrameworkRunMetadata {
            periodic_molecules: false,
            freeze_group: Some("Framework".to_string()),
            framework_atom_count: 50,
        };
        // Round-trips through JSON.
        let dir = std::env::temp_dir().join("silicolab_framework_meta_roundtrip");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join(MD_FRAMEWORK_FILE);
        meta.save(&path).unwrap();
        assert_eq!(FrameworkRunMetadata::load(&path).unwrap(), meta);

        // A rigid system freezes its first N atoms and does not mark periodicity.
        let sel = meta.freeze_selection().expect("rigid freezes");
        assert_eq!(sel.atom_indices.len(), 50);
        let mut settings = MdpSettings::nvt(300.0);
        meta.apply_to(&mut settings);
        assert!(settings.freeze.is_some());
        assert!(!settings.periodic_molecules);
    }

    #[test]
    fn flexible_framework_metadata_marks_periodic_without_freezing() {
        let meta = FrameworkRunMetadata {
            periodic_molecules: true,
            freeze_group: None,
            framework_atom_count: 50,
        };
        assert!(meta.freeze_selection().is_none());
        let mut settings = MdpSettings::nvt(300.0);
        meta.apply_to(&mut settings);
        assert!(settings.freeze.is_none());
        assert!(settings.periodic_molecules);
    }
}
