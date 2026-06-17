//! `md simulate`: the fixed EM → NVT → NPT → production protocol (legacy
//! physical-intent command), run synchronously through the GROMACS pipeline.

use super::*;

use std::sync::{Arc, atomic::AtomicBool};

use anyhow::{Result, anyhow};

use crate::engines::gromacs::run_pipeline;
use crate::{
    backend::tasks::TaskStatus,
    engines::gromacs::{
        render_top,
        runner::{PrepareSystemRequest, prepare_system},
        topology::TopologySource,
    },
    frontend::{md_support::protocol_stage_specs, state::AppState},
    io::structure_io,
    workflows::molecular_dynamics::MdProtocolOptions,
};

pub fn md_simulate(state: &mut AppState, args: &[String]) -> Result<String> {
    let flags = Flags::parse(args)?;

    let production_ps = match flags.str("time") {
        Some(t) => parse_time_ps(t)?,
        None => 1000.0,
    };
    let temperature_k = flags.f32("temperature")?.unwrap_or(300.0);
    // Relax (EM/NVT/NPT) by default; `--no-relax` skips straight to production.
    let relax = !flags.flag("no-relax");
    // Save a playable trajectory for every stage by default; `--no-trajectory`
    // keeps only the final structures.
    let save_trajectory = !flags.flag("no-trajectory");

    let options = MdProtocolOptions {
        production_ps,
        timestep_ps: 0.002,
        temperature_k,
        relax_before_production: relax,
        save_trajectory,
    };

    let structure = require_boxed_structure(state)?;

    let task_run_id = create_cli_task_run(state, "run-md")?;
    let work_dir = ensure_cli_task_run_dir(state, task_run_id)?;
    state
        .tasks
        .set_engine_label(task_run_id, Some("GROMACS".to_string()));
    sync_cli_task_manifest(state, task_run_id)?;
    mark_cli_task_status(state, task_run_id, TaskStatus::Running)?;

    let result = (|| {
        // Consume the topology captured at build time, then have the engine
        // render it to a GROMACS `.top` here, at run time.
        let topology = load_active_or_derive_md_topology(state).map_err(|_| {
            anyhow!("no MD system found; run `md build` first to prepare the system")
        })?;

        // Framework (nanosheet) systems carry run hints: keep the molecule
        // periodic (flexible) and/or freeze the sheet (rigid).
        let framework_meta = state.entries.active_entry_id().and_then(|id| {
            crate::frontend::md_support::load_framework_metadata_for_entry(state, id)
        });

        let launch = resolve_launch(state)?;
        let system = prepare_system(PrepareSystemRequest {
            structure,
            topology: TopologySource::Inline(render_top(&topology)),
            working_dir: work_dir.clone(),
            freeze: framework_meta.as_ref().and_then(|m| m.freeze_selection()),
        })?;

        let mut stages = protocol_stage_specs(&options);
        if let Some(meta) = &framework_meta {
            for spec in &mut stages {
                meta.apply_to(&mut spec.settings);
            }
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let results = run_pipeline(
            system,
            stages,
            launch.clone().into(),
            STAGE_TIMEOUT,
            Arc::clone(&cancel),
            |_| {},
        )?;

        let production = results
            .last()
            .ok_or_else(|| anyhow!("pipeline produced no stages"))?;
        // The production stage writes the compressed `.xtc`; take the last stage
        // that produced one so playback follows the actual MD trajectory.
        let trajectory = results
            .iter()
            .rev()
            .find_map(|stage| stage.trajectory.clone());
        let save_path = structure_io::default_structure_save_path(&production.structure, None);
        let entry_id = state
            .entries
            .add_entry(production.structure.clone(), None, save_path);
        state.show_entry(entry_id);
        // Mark the entry as an MD-run output (provenance badge + playback gating),
        // mirroring the GUI run path.
        let project_root = state
            .workspace
            .project()
            .map(|project| project.root.clone());
        let origin = super::super::dispatcher::md_run_origin(trajectory, project_root.as_deref());
        state.entries.set_entry_origin(entry_id, origin);
        record_cli_task_result_entry(state, task_run_id, entry_id)?;

        let analysis_summary = run_analysis(&work_dir, &launch, production, cancel);

        Ok(format!(
            "molecular dynamics complete ({production_ps:.0} ps production at {temperature_k:.0} K){analysis_summary}"
        ))
    })();

    finish_cli_task(state, task_run_id, result)
}
