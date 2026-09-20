//! Request assembly for assistant MD jobs. The launcher supplies the prepared task directory.

use super::*;

use std::path::PathBuf;

use anyhow::{Result, anyhow, bail};

use crate::frontend::md_support::protocol_stage_specs;
use crate::{
    engines::gromacs::{render_top, stage_specs_from_md_stages, topology::TopologySource},
    frontend::state::AppState,
    workflows::molecular_dynamics::{
        MdProtocolOptions,
        run::{
            MdStage, PresetId, PresetParams, StageKind, StageLength, assemble, has_errors,
            recommend, validate,
        },
    },
};

pub struct AgentMdDraft {
    request: crate::frontend::jobs::GromacsPipelineRequest,
}

impl AgentMdDraft {
    pub fn with_working_dir(
        mut self,
        working_dir: PathBuf,
    ) -> crate::frontend::jobs::GromacsPipelineRequest {
        self.request.working_dir = working_dir;
        self.request
    }
}

pub fn build_agent_md_request(state: &AppState, args: &[String]) -> Result<AgentMdDraft> {
    let Some(sub) = args.first().map(String::as_str) else {
        bail!("usage: md <run|simulate> [options]");
    };
    let request = match sub {
        "run" => build_agent_md_run(state, &args[1..]),
        "simulate" => build_agent_md_simulate(state, &args[1..]),
        other => {
            bail!("`md {other}` is not a runnable simulation (use `md run` or `md simulate`)")
        }
    }?;
    Ok(AgentMdDraft { request })
}

fn build_agent_md_run(
    state: &AppState,
    args: &[String],
) -> Result<crate::frontend::jobs::GromacsPipelineRequest> {
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

    let edits = build_stage_edits(&flags)?;
    let family = context.force_field_family;
    let stages: Vec<MdStage> = stages
        .into_iter()
        .map(|stage| assemble(stage, family, &edits))
        .collect();

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
    let entry_id = state.entries.active_entry_id();
    let framework_meta = entry_id
        .and_then(|id| crate::frontend::md_support::load_framework_metadata_for_entry(state, id));
    if let Some(meta) = &framework_meta {
        for spec in &mut specs {
            meta.apply_to(&mut spec.settings);
        }
    }
    let topology = resolve_run_topology(state, entry_id)?;
    let launch = resolve_launch(state)?;
    let working_dir = PathBuf::new();

    Ok(crate::frontend::jobs::GromacsPipelineRequest {
        structure,
        topology,
        stages: specs,
        working_dir,
        compute: launch.into(),
        max_duration_per_stage: STAGE_TIMEOUT,
        freeze: framework_meta
            .as_ref()
            .and_then(|meta| meta.freeze_selection()),
    })
}

fn build_agent_md_simulate(
    state: &AppState,
    args: &[String],
) -> Result<crate::frontend::jobs::GromacsPipelineRequest> {
    let flags = Flags::parse(args)?;
    let production_ps = match flags.str("time") {
        Some(t) => parse_time_ps(t)?,
        None => 1000.0,
    };
    let temperature_k = flags.f32("temperature")?.unwrap_or(300.0);
    let relax = !flags.flag("no-relax");
    let save_trajectory = !flags.flag("no-trajectory");
    let options = MdProtocolOptions {
        production_ps,
        timestep_ps: 0.002,
        temperature_k,
        relax_before_production: relax,
        save_trajectory,
    };

    let structure = require_boxed_structure(state)?;
    let topology = load_active_or_derive_md_topology(state)
        .map_err(|_| anyhow!("no MD system found; run `md build` first to prepare the system"))?;
    let framework_meta = state
        .entries
        .active_entry_id()
        .and_then(|id| crate::frontend::md_support::load_framework_metadata_for_entry(state, id));

    let mut stages = protocol_stage_specs(&options);
    if let Some(meta) = &framework_meta {
        for spec in &mut stages {
            meta.apply_to(&mut spec.settings);
        }
    }
    let launch = resolve_launch(state)?;
    let working_dir = PathBuf::new();

    Ok(crate::frontend::jobs::GromacsPipelineRequest {
        structure,
        topology: TopologySource::Inline(render_top(&topology)),
        stages,
        working_dir,
        compute: launch.into(),
        max_duration_per_stage: STAGE_TIMEOUT,
        freeze: framework_meta
            .as_ref()
            .and_then(|meta| meta.freeze_selection()),
    })
}
