//! Resolving how to launch an external engine on a compute target.
//!
//! One rule for both targets: a configured launch wins, else probe the target,
//! else fail with the candidates that were tried. The local and remote arms differ
//! only in *how* they probe — a PATH/WSL lookup here, an SSH `--version` there.
//!
//! Resolution happens on the client, for whichever target the job was submitted
//! to, and the result travels in the job's `request.json`. An executor never
//! discovers an engine for itself: a worker that re-probed the node would run
//! whichever binary happened to be installed there, silently ignoring the launch
//! the user configured.
//!
//! Both arms block — the remote one on SSH, the local one potentially on a WSL
//! cold start — so callers must run them off the UI thread.

use anyhow::{Result, bail};

use crate::backend::config::RemoteHost;
use crate::engines::registry::{EngineId, EngineLaunch, EngineLaunches, engine_spec, probe_native};

/// Where an engine is to be launched, with the launches already configured there.
pub enum LaunchTarget<'a> {
    Local(&'a EngineLaunches),
    Remote(&'a RemoteHost),
}

/// A resolved launch, plus what the caller should persist.
#[derive(Debug)]
pub struct ResolvedLaunch {
    pub launch: EngineLaunch,
    /// The engine's reported version, when resolution came from a fresh probe.
    pub version: Option<String>,
    /// True when the launch was probed rather than read from configuration, so the
    /// caller should cache it back onto the target.
    pub detected: bool,
}

/// Resolve the launch for `id` on `target`. Blocking; never call from the UI
/// thread.
pub fn resolve_engine_launch(target: LaunchTarget<'_>, id: EngineId) -> Result<ResolvedLaunch> {
    let Some(spec) = engine_spec(id) else {
        bail!("`{}` is a built-in engine and has no launch", id.as_str());
    };
    let configured = match &target {
        LaunchTarget::Local(overrides) => overrides.get(id),
        LaunchTarget::Remote(host) => host.engines.get(id),
    };
    if let Some(launch) = configured {
        return Ok(ResolvedLaunch {
            launch: launch.clone(),
            version: None,
            detected: false,
        });
    }

    match target {
        LaunchTarget::Local(_) => {
            if let Some(program) = probe_native(spec) {
                return Ok(ResolvedLaunch {
                    launch: EngineLaunch::native(program),
                    version: None,
                    detected: true,
                });
            }
            // On Windows the engine conventionally lives inside WSL.
            if id == EngineId::GROMACS
                && let Some(launch) = crate::engines::registry::detect_wsl_gromacs_launch()
            {
                return Ok(ResolvedLaunch {
                    launch,
                    version: None,
                    detected: true,
                });
            }
            bail!(
                "Could not find {}. Install it and ensure `{}` is on PATH, set up WSL with it installed, or configure its launch in Settings -> Engines.",
                spec.name,
                spec.candidate_executables.first().copied().unwrap_or("it")
            )
        }
        LaunchTarget::Remote(host) => {
            let target = crate::engines::remote::RemoteTarget::for_run(host, "probe");
            match crate::engines::remote::detect_remote_engine(&target, spec) {
                Some((program, version)) => Ok(ResolvedLaunch {
                    launch: EngineLaunch::native(program),
                    version: Some(version),
                    detected: true,
                }),
                None => bail!(
                    "no working `{}` found on {} (tried {:?}). Set its path in Settings -> Remote hosts, or add a setup line to the host's prelude (e.g. `module load {}`).",
                    spec.candidate_executables.first().copied().unwrap_or("it"),
                    host.label,
                    spec.candidate_executables,
                    spec.id.as_str()
                ),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host_with(engines: EngineLaunches) -> RemoteHost {
        RemoteHost {
            id: "h".into(),
            label: "Cluster".into(),
            hostname: "login.example.edu".into(),
            username: "alice".into(),
            port: 22,
            work_root: "~/.silicolab".into(),
            prelude: Vec::new(),
            engines,
            engine_versions: Default::default(),
            resources: Default::default(),
            scheduler: Default::default(),
        }
    }

    /// A configured launch is returned verbatim and never re-probed — the whole
    /// point of the setting. Exercised on the remote arm because probing it would
    /// otherwise require SSH, which proves no probe was attempted.
    #[test]
    fn a_configured_remote_launch_wins_without_probing() {
        let mut engines = EngineLaunches::new();
        engines.insert(EngineId::GROMACS, EngineLaunch::native("/opt/g/bin/gmx"));
        let host = host_with(engines);

        let resolved = resolve_engine_launch(LaunchTarget::Remote(&host), EngineId::GROMACS)
            .expect("configured launch resolves");
        assert_eq!(resolved.launch.program, "/opt/g/bin/gmx");
        assert!(!resolved.detected, "a configured launch is not a detection");
    }

    #[test]
    fn a_built_in_engine_has_no_launch_to_resolve() {
        let overrides = EngineLaunches::new();
        let error = resolve_engine_launch(LaunchTarget::Local(&overrides), EngineId::HARTREE)
            .expect_err("hartree is built in");
        assert!(error.to_string().contains("built-in"), "{error}");
    }
}
