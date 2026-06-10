//! MD protocol: the ordered stage chain a simulation run performs internally.
//!
//! Given a simulation-ready system this builds the energy-minimization → NVT →
//! NPT → production sequence as [`StageSpec`]s wired with [`StageLinks`], so
//! [`crate::engines::gromacs::run_pipeline`] threads each stage's coordinates and
//! continuation checkpoint to the next. Callers express physical intent
//! (temperature, simulation time, whether to relax first); this module
//! translates that into the engine stage chain.

use crate::engines::gromacs::{
    FileRef, MdpSettings, OutputFrequency, StageFileRole, StageLinks, StageSpec,
};

/// Canonical stage names, used both as `-deffnm` basenames and as the keys
/// [`StageLinks`] reference.
pub const STAGE_EM: &str = "em";
pub const STAGE_NVT: &str = "nvt";
pub const STAGE_NPT: &str = "npt";
pub const STAGE_PROD: &str = "md";

/// Roughly how many trajectory frames each stage should write, so even a short
/// stage produces a watchable track. The write interval is derived per stage
/// from its step count; the playback reader subsamples to bound memory, so a
/// long stage is not penalised for writing more frames than this.
const TRAJECTORY_TARGET_FRAMES: u64 = 250;
/// Bounds on the per-stage compressed-trajectory write interval (steps). The
/// floor keeps a tiny stage from writing every step; the ceiling keeps a very
/// long stage from writing absurdly often.
const TRAJECTORY_MIN_INTERVAL: u64 = 1;
const TRAJECTORY_MAX_INTERVAL: u64 = 5_000;

/// Give every dynamics stage a compressed-trajectory write cadence (or clear
/// it).
///
/// MD output is saved by default so each dynamics step of a run is playable;
/// passing `save_trajectory = false` disables it (only final structures are
/// kept). The interval targets [`TRAJECTORY_TARGET_FRAMES`] frames for the
/// stage's length, so a short equilibration step still yields a usable track
/// instead of two or three frames.
///
/// Minimization stages are left untouched: steepest-descent energy minimization
/// relaxes to a local minimum rather than producing a motion trajectory, and
/// GROMACS does not write a meaningful `.xtc` for it.
pub fn apply_trajectory_output(stages: &mut [StageSpec], save_trajectory: bool) {
    for spec in stages.iter_mut() {
        if spec.settings.integrator.is_minimization() {
            continue;
        }
        let interval = (spec.settings.nsteps / TRAJECTORY_TARGET_FRAMES)
            .clamp(TRAJECTORY_MIN_INTERVAL, TRAJECTORY_MAX_INTERVAL) as u32;
        let mut output = spec
            .settings
            .output
            .unwrap_or_else(OutputFrequency::equilibration);
        output.nstxout_compressed = if save_trajectory { interval } else { 0 };
        spec.settings.output = Some(output);
    }
}

/// Physical parameters for a molecular-dynamics run — the choices a user makes
/// in the MD panel. Everything else is derived internally.
#[derive(Debug, Clone, Copy)]
pub struct MdProtocolOptions {
    /// Production simulation length, picoseconds.
    pub production_ps: f64,
    /// MD integration timestep, picoseconds (2 fs default).
    pub timestep_ps: f32,
    /// Target temperature, kelvin.
    pub temperature_k: f32,
    /// Run EM → NVT → NPT equilibration before production ("relax model system
    /// before simulation"). When false, only production runs.
    pub relax_before_production: bool,
    /// Save a compressed trajectory for every stage so the run is playable. On
    /// by default; disable only to skip writing trajectory files entirely.
    pub save_trajectory: bool,
}

impl Default for MdProtocolOptions {
    fn default() -> Self {
        Self {
            production_ps: 1_000.0,
            timestep_ps: 0.002,
            temperature_k: 300.0,
            relax_before_production: true,
            save_trajectory: true,
        }
    }
}

impl MdProtocolOptions {
    /// Production length expressed as a step count for the given timestep.
    pub fn production_steps(&self) -> u64 {
        (self.production_ps / self.timestep_ps as f64).round() as u64
    }
}

/// A [`FileRef`] pointing at a named stage's produced file.
fn stage_ref(stage: &str, role: StageFileRole) -> FileRef {
    FileRef::Stage {
        stage: stage.to_string(),
        role,
    }
}

/// Build the equilibration stage specs: EM → NVT (from the EM coordinates) →
/// NPT (continues from the NVT checkpoint).
pub fn equilibration_stages(options: &MdProtocolOptions) -> Vec<StageSpec> {
    let t = options.temperature_k;

    let em = StageSpec {
        stage_name: STAGE_EM.to_string(),
        settings: MdpSettings::energy_minimization(),
        links: StageLinks::from_prepared(),
    };

    let nvt = StageSpec {
        stage_name: STAGE_NVT.to_string(),
        settings: MdpSettings::nvt(t),
        links: StageLinks {
            coordinates: stage_ref(STAGE_EM, StageFileRole::OutputGro),
            checkpoint: None,
        },
    };

    let npt = StageSpec {
        stage_name: STAGE_NPT.to_string(),
        settings: MdpSettings::npt(t),
        links: StageLinks {
            coordinates: stage_ref(STAGE_NVT, StageFileRole::OutputGro),
            checkpoint: Some(stage_ref(STAGE_NVT, StageFileRole::Checkpoint)),
        },
    };

    vec![em, nvt, npt]
}

/// Build the production stage spec. Continues from the NPT checkpoint (or, if
/// equilibration was skipped, from the prepared coordinates).
pub fn production_stage(options: &MdProtocolOptions) -> StageSpec {
    let mut settings = MdpSettings::production(options.production_steps(), options.temperature_k);
    settings.timestep_ps = options.timestep_ps;

    let links = if options.relax_before_production {
        StageLinks {
            coordinates: stage_ref(STAGE_NPT, StageFileRole::OutputGro),
            checkpoint: Some(stage_ref(STAGE_NPT, StageFileRole::Checkpoint)),
        }
    } else {
        StageLinks::from_prepared()
    };

    StageSpec {
        stage_name: STAGE_PROD.to_string(),
        settings,
        links,
    }
}

/// The full stage chain a run executes: equilibration (if requested) followed by
/// production.
pub fn full_protocol(options: &MdProtocolOptions) -> Vec<StageSpec> {
    let mut stages = if options.relax_before_production {
        equilibration_stages(options)
    } else {
        Vec::new()
    };
    stages.push(production_stage(options));
    apply_trajectory_output(&mut stages, options.save_trajectory);
    stages
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stage<'a>(stages: &'a [StageSpec], name: &str) -> &'a StageSpec {
        stages
            .iter()
            .find(|s| s.stage_name == name)
            .expect("stage present")
    }

    #[test]
    fn full_protocol_has_four_stages_when_relaxing() {
        let stages = full_protocol(&MdProtocolOptions::default());
        let names: Vec<&str> = stages.iter().map(|s| s.stage_name.as_str()).collect();
        assert_eq!(names, vec![STAGE_EM, STAGE_NVT, STAGE_NPT, STAGE_PROD]);
    }

    /// The compressed-trajectory write interval a stage will use (0 = none).
    fn trajectory_interval(spec: &StageSpec) -> u32 {
        spec.settings
            .output
            .as_ref()
            .map_or(0, |output| output.nstxout_compressed)
    }

    #[test]
    fn dynamics_stages_write_a_trajectory_by_default() {
        // Saving on by default: every dynamics stage (NVT/NPT/production, not
        // just production) gets a compressed-trajectory cadence, so each is
        // playable. Minimization writes no track.
        let stages = full_protocol(&MdProtocolOptions::default());
        for spec in &stages {
            if spec.settings.integrator.is_minimization() {
                assert_eq!(
                    trajectory_interval(spec),
                    0,
                    "minimization stage '{}' should not write a trajectory",
                    spec.stage_name
                );
            } else {
                assert!(
                    trajectory_interval(spec) > 0,
                    "dynamics stage '{}' should write a trajectory by default",
                    spec.stage_name
                );
            }
        }
    }

    #[test]
    fn opting_out_clears_every_stage_trajectory() {
        let opts = MdProtocolOptions {
            save_trajectory: false,
            ..MdProtocolOptions::default()
        };
        let stages = full_protocol(&opts);
        for spec in &stages {
            assert_eq!(
                trajectory_interval(spec),
                0,
                "stage '{}' should write no trajectory when saving is off",
                spec.stage_name
            );
        }
    }

    #[test]
    fn trajectory_interval_targets_a_watchable_frame_count() {
        // A short stage still yields many frames (interval well under nsteps),
        // not the two or three a coarse fixed interval would give.
        let mut stages = full_protocol(&MdProtocolOptions::default());
        apply_trajectory_output(&mut stages, true);
        let prod = stage(&stages, STAGE_PROD);
        let interval = prod.settings.output.as_ref().unwrap().nstxout_compressed as u64;
        let frames = prod.settings.nsteps / interval.max(1);
        assert!(
            frames >= 100,
            "production should yield a watchable track, got {frames} frames"
        );
    }

    #[test]
    fn skipping_relaxation_runs_production_only_from_prepared_coords() {
        let opts = MdProtocolOptions {
            relax_before_production: false,
            ..MdProtocolOptions::default()
        };
        let stages = full_protocol(&opts);
        assert_eq!(stages.len(), 1);
        assert_eq!(stages[0].stage_name, STAGE_PROD);
        assert_eq!(stages[0].links.coordinates, FileRef::PreparedConf);
        assert!(stages[0].links.checkpoint.is_none());
    }

    #[test]
    fn nvt_starts_from_em_output() {
        let stages = equilibration_stages(&MdProtocolOptions::default());
        assert_eq!(
            stage(&stages, STAGE_NVT).links.coordinates,
            FileRef::Stage {
                stage: STAGE_EM.to_string(),
                role: StageFileRole::OutputGro,
            }
        );
    }

    #[test]
    fn npt_continues_from_nvt_checkpoint() {
        let stages = equilibration_stages(&MdProtocolOptions::default());
        assert_eq!(
            stage(&stages, STAGE_NPT).links.checkpoint,
            Some(FileRef::Stage {
                stage: STAGE_NVT.to_string(),
                role: StageFileRole::Checkpoint,
            })
        );
    }

    #[test]
    fn production_continues_from_npt() {
        let prod = production_stage(&MdProtocolOptions::default());
        assert_eq!(
            prod.links.checkpoint,
            Some(FileRef::Stage {
                stage: STAGE_NPT.to_string(),
                role: StageFileRole::Checkpoint,
            })
        );
    }

    #[test]
    fn production_steps_derive_from_time_and_timestep() {
        let opts = MdProtocolOptions {
            production_ps: 1_000.0,
            timestep_ps: 0.002,
            ..MdProtocolOptions::default()
        };
        assert_eq!(opts.production_steps(), 500_000);
    }
}
