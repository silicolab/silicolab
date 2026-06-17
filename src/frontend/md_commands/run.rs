//! `md presets` and `md run`: the preset-driven path. `presets` lists the
//! library (marking the recommended preset); `run` is the scriptable mirror of
//! the GUI Run MD panel — pick a preset, apply overrides, validate, then realize
//! and run the GROMACS pipeline.

use super::*;

use std::sync::{Arc, atomic::AtomicBool};

use anyhow::{Result, anyhow, bail};

use crate::engines::gromacs::run_pipeline;
use crate::{
    backend::tasks::TaskStatus,
    engines::gromacs::{
        runner::{PrepareSystemRequest, prepare_system},
        stage_specs_from_md_stages,
    },
    frontend::state::AppState,
    io::structure_io,
    workflows::molecular_dynamics::run::{
        MdStage, PresetId, PresetParams, StageKind, StageLength, assemble, has_errors, recommend,
        validate,
    },
};

/// `md presets`: list the preset library, marking the one recommended for the
/// active system and flagging presets that don't apply to it.
pub fn md_presets(state: &mut AppState, args: &[String]) -> Result<String> {
    let flags = Flags::parse(args)?;
    let context = load_or_derive_context(state);
    let eff = context.with_overrides(parse_overrides(&flags));
    let recommended = recommend(&eff).preset;

    let mut out =
        String::from("Presets (* = recommended for this system; (n/a) = not applicable):\n");
    for preset in PresetId::all() {
        let mark = if *preset == recommended { "*" } else { " " };
        let na = if preset.applies_to(&eff) {
            ""
        } else {
            "  (n/a)"
        };
        out.push_str(&format!(
            "  {mark} {:<16} {}{na}\n",
            preset.token(),
            preset.title()
        ));
    }
    Ok(out.trim_end().to_string())
}

/// `md run`: build a preset's stage sequence for the inherited system context
/// (recommended preset by default), apply CLI overrides, validate, then realize
/// and run the GROMACS pipeline. The scriptable mirror of the GUI Run MD panel.
///
/// Options: `--preset <id>` (default: recommended), `--temperature <K>`,
/// `--length <time>` (production length), `--timestep <ps>`, the system-type
/// overrides `--membrane|--no-membrane` (and `ligand`/`nucleic`),
/// `--no-trajectory`, `--set key=val,...` (tiered parameters), and
/// `--raw "key=val;..."` (verbatim engine passthrough).
pub fn md_run(state: &mut AppState, args: &[String]) -> Result<String> {
    let flags = Flags::parse(args)?;
    let structure = require_boxed_structure(state)?;
    let context = load_or_derive_context(state);
    let eff = context.with_overrides(parse_overrides(&flags));

    let preset = match flags.str("preset") {
        Some(token) => PresetId::from_token(token)
            .ok_or_else(|| anyhow!("unknown preset `{token}` (run `md presets` to list them)"))?,
        None => recommend(&eff).preset,
    };

    let mut params = PresetParams::default();
    if let Some(temperature) = flags.f32("temperature")? {
        params.temperature_k = temperature;
    }
    if let Some(timestep) = flags.f32("timestep")? {
        params.timestep_ps = timestep;
    }

    let mut stages = preset.build(&eff, &params);
    if let Some(length) = flags.str("length") {
        let ps = parse_time_ps(length)?;
        for stage in &mut stages {
            if matches!(stage.kind, StageKind::Produce | StageKind::Extend) {
                stage.length = StageLength::Picoseconds(ps);
            }
        }
    }
    if flags.flag("no-trajectory") {
        for stage in &mut stages {
            stage.trajectory_target_frames = None;
        }
    }

    // Apply tiered-parameter and raw-passthrough edits through the layered merge.
    let edits = build_stage_edits(&flags)?;
    let family = context.force_field_family;
    let stages: Vec<MdStage> = stages
        .into_iter()
        .map(|stage| assemble(stage, family, &edits))
        .collect();

    // Validate before doing any work; errors block the run.
    let issues = validate(&stages, &eff);
    if has_errors(&issues) {
        let errors: Vec<String> = issues
            .iter()
            .filter(|issue| {
                issue.severity == crate::workflows::molecular_dynamics::run::IssueSeverity::Error
            })
            .map(|issue| issue.message.clone())
            .collect();
        bail!("cannot run `{}`: {}", preset.token(), errors.join("; "));
    }

    let mut specs = stage_specs_from_md_stages(&stages, family, None);

    let task_run_id = create_cli_task_run(state, "run-md")?;
    let work_dir = ensure_cli_task_run_dir(state, task_run_id)?;
    state
        .tasks
        .set_engine_label(task_run_id, Some("GROMACS".to_string()));
    sync_cli_task_manifest(state, task_run_id)?;
    mark_cli_task_status(state, task_run_id, TaskStatus::Running)?;

    let result = (|| {
        let entry_id = state.entries.active_entry_id();
        // Framework systems carry freeze/periodic hints applied to every stage.
        let framework_meta = entry_id.and_then(|id| {
            crate::frontend::md_support::load_framework_metadata_for_entry(state, id)
        });
        if let Some(meta) = &framework_meta {
            for spec in &mut specs {
                meta.apply_to(&mut spec.settings);
            }
        }

        let topology = resolve_run_topology(state, entry_id)?;
        let launch = resolve_launch(state)?;
        let system = prepare_system(PrepareSystemRequest {
            structure,
            topology,
            working_dir: work_dir.clone(),
            freeze: framework_meta.as_ref().and_then(|m| m.freeze_selection()),
        })?;

        let cancel = Arc::new(AtomicBool::new(false));
        let results = run_pipeline(
            system,
            specs,
            launch.clone().into(),
            STAGE_TIMEOUT,
            Arc::clone(&cancel),
            |_| {},
        )?;

        let production = results
            .last()
            .ok_or_else(|| anyhow!("pipeline produced no stages"))?;
        let trajectory = results
            .iter()
            .rev()
            .find_map(|stage| stage.trajectory.clone());
        let save_path = structure_io::default_structure_save_path(&production.structure, None);
        let entry_id = state
            .entries
            .add_entry(production.structure.clone(), None, save_path);
        state.show_entry(entry_id);
        let project_root = state
            .workspace
            .project()
            .map(|project| project.root.clone());
        let origin = super::super::dispatcher::md_run_origin(trajectory, project_root.as_deref());
        state.entries.set_entry_origin(entry_id, origin);
        record_cli_task_result_entry(state, task_run_id, entry_id)?;

        let analysis_summary = run_analysis(&work_dir, &launch, production, cancel);
        Ok(format!(
            "molecular dynamics complete (preset {}, {} stages){analysis_summary}",
            preset.token(),
            results.len()
        ))
    })();

    finish_cli_task(state, task_run_id, result)
}
