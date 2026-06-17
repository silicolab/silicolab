//! `md build` and `md solvate`: the MD System Builder steps. `build` wraps the
//! active structure in a box and captures the engine-neutral [`MdTopology`];
//! `solvate` fills that box with water and ions (SilicoLab-native).

use super::*;

use anyhow::{Result, bail};

use crate::engines::gromacs::framework_run_hints;
use crate::{
    backend::tasks::TaskStatus,
    frontend::{
        md_support::{
            FrameworkRunMetadata, MD_FRAMEWORK_FILE, MD_TOPOLOGY_FILE, write_md_system_context,
        },
        state::AppState,
    },
    io::structure_io,
    workflows::molecular_dynamics::{
        FrameworkMode, MdSystemConfig, MdTopology, SolvationOptions, build_md_system,
        is_framework_shape, solvate,
    },
};

pub fn md_build(state: &mut AppState, args: &[String]) -> Result<String> {
    if state.structure().atoms.is_empty() {
        bail!("no active structure; open one before `md build`");
    }
    let flags = Flags::parse(args)?;
    // A periodic framework (nanosheet) is captured with its bond-derived
    // topology; `--framework rigid|flexible` picks the model (default rigid).
    let framework_mode = match flags.str("framework") {
        Some("flexible") => Some(FrameworkMode::Flexible),
        Some("rigid") | None => Some(FrameworkMode::Rigid),
        Some(other) => bail!("unknown --framework mode `{other}`; use rigid or flexible"),
    };
    // `--custom-ff <name>` merges a saved custom force field, enabling elements
    // the built-in tables lack (or overriding their types) for a framework build.
    let custom_force_field = match flags.str("custom-ff") {
        Some(name) => Some(crate::backend::force_fields::load_force_field(name)?),
        None => None,
    };

    let task_run_id = create_cli_task_run(state, "build-md-system")?;
    let run_dir = ensure_cli_task_run_dir(state, task_run_id)?;
    mark_cli_task_status(state, task_run_id, TaskStatus::Running)?;

    let result = (|| {
        if is_framework_shape(state.structure()) {
            // Keep the periodic cell as built (re-boxing would break the
            // sheet's bonds to its periodic images); capture the framework
            // topology and the run hints a later `md simulate` applies. A custom
            // force field, when given, covers elements the built-in tables lack
            // and is inlined into the captured topology.
            let mode = framework_mode.unwrap_or(FrameworkMode::Rigid);
            let structure = state.structure().clone();
            let custom_types = custom_force_field
                .as_deref()
                .map(crate::engines::gromacs::custom_ff::custom_types)
                .unwrap_or_default();
            let mut topology = MdTopology::framework_with_custom(&structure, mode, &custom_types)?;
            topology.inline_force_field = custom_force_field.clone();
            let atom_count = structure.atoms.len();
            let net_charge = topology.net_charge();
            let solute = structure.clone();
            let save_path = structure_io::default_structure_save_path(&structure, None);
            let entry_id = state.entries.add_entry(structure, None, save_path);
            state.show_entry(entry_id);
            record_cli_task_result_entry(state, task_run_id, entry_id)?;

            topology.save(&run_dir.join(MD_TOPOLOGY_FILE))?;
            let hints = framework_run_hints(mode);
            FrameworkRunMetadata {
                periodic_molecules: hints.periodic_molecules,
                freeze_group: hints.freeze_group,
                framework_atom_count: atom_count,
            }
            .save(&run_dir.join(MD_FRAMEWORK_FILE))?;
            // A framework has no biomolecular force-field convention (token
            // classifies to the generic family) and uses freeze, not restraints.
            write_md_system_context(
                &run_dir,
                &solute,
                atom_count,
                "framework",
                None,
                true,
                net_charge,
                false,
                Vec::new(),
            );

            return Ok(format!(
                "Framework MD system ready ({} model): {atom_count} atoms; topology captured",
                mode.label()
            ));
        }

        let structure = if state.structure().cell.is_none() {
            let (boxed, _report) = build_md_system(state.structure(), &MdSystemConfig::default())?;
            boxed
        } else {
            state.structure().clone()
        };
        let solute = structure.clone();
        let save_path = structure_io::default_structure_save_path(&structure, None);
        let entry_id = state.entries.add_entry(structure, None, save_path);
        state.show_entry(entry_id);
        record_cli_task_result_entry(state, task_run_id, entry_id)?;

        let topology = MdTopology::from_structure(state.structure())?;
        topology.save(&run_dir.join(MD_TOPOLOGY_FILE))?;
        // Geometry-only build: record the generic family (a later run uses the
        // captured engine-neutral topology, not a biomolecular nonbonded block).
        write_md_system_context(
            &run_dir,
            &solute,
            topology.atom_count(),
            "builtin",
            None,
            false,
            topology.net_charge(),
            false,
            Vec::new(),
        );

        Ok(format!(
            "MD system ready: {} atoms, {} species; topology captured",
            topology.atom_count(),
            topology.species.len()
        ))
    })();

    finish_cli_task(state, task_run_id, result)
}

/// Fill the simulation box with water and ions (SilicoLab-native solvation),
/// replacing the active structure with the solvated system and updating the
/// captured topology.
///
/// Options: `--water spc|spce|tip3p|tip4p|...`, `--conc <mol/L>`, `--cation NA`,
/// `--anion CL`, `--no-neutralize`. Placement is geometry only — no force field.
pub fn md_solvate(state: &mut AppState, args: &[String]) -> Result<String> {
    let flags = Flags::parse(args)?;

    if state.structure().atoms.is_empty() {
        bail!("no active structure; open one and run `md build` first");
    }
    // Solvation needs a periodic box; build a default one if missing.
    if state.structure().cell.is_none() {
        let (boxed, _report) = build_md_system(state.structure(), &MdSystemConfig::default())?;
        *state.structure_mut() = boxed;
        state.mark_structure_changed();
    }

    let mut options = SolvationOptions::default();
    if let Some(w) = flags.str("water") {
        options.water = parse_water_model(w)?;
    }
    if let Some(c) = flags.str("cation") {
        options.positive_ion = c.to_ascii_uppercase();
    }
    if let Some(a) = flags.str("anion") {
        options.negative_ion = a.to_ascii_uppercase();
    }
    if let Some(conc) = flags.f32("conc")? {
        options.concentration_molar = Some(conc);
    }
    if flags.flag("no-neutralize") {
        options.neutralize = false;
    }

    let (solvated, report) = solvate(state.structure(), &options)?;
    let atom_count = solvated.atoms.len();
    *state.structure_mut() = solvated;
    state.mark_structure_changed();

    Ok(format!(
        "Solvated with {}: added {} water, {} {}, {} {}; system now {} atoms",
        options.water.label(),
        report.water_added,
        report.cations_added,
        options.positive_ion,
        report.anions_added,
        options.negative_ion,
        atom_count,
    ))
}
