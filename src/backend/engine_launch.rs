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
//! Remote resolution blocks on SSH and must run off the UI thread. Local
//! resolution only selects a plausible executable; the background job reports
//! if that executable cannot actually run.

use anyhow::{Result, bail};

use crate::backend::config::RemoteHost;
use crate::engines::registry::{EngineId, EngineLaunch, EngineLaunches, EngineSpec, engine_spec};

/// Where an engine is to be launched, with the launches already configured there.
pub enum LaunchTarget<'a> {
    Local(&'a EngineLaunches),
    Remote(&'a RemoteHost),
}

impl LaunchTarget<'_> {
    fn configured(&self, id: EngineId) -> Option<&EngineLaunch> {
        match self {
            Self::Local(overrides) => overrides.get(id),
            Self::Remote(host) => host.engines.get(id),
        }
    }

    /// Run `launch` on this target and confirm it is `spec`'s engine, returning the
    /// version it reported or why it did not answer.
    fn verify(&self, launch: &EngineLaunch, spec: &EngineSpec) -> Result<String, String> {
        match self {
            Self::Local(_) => crate::engines::registry::verify_launch(launch, spec),
            Self::Remote(host) => {
                let target = crate::engines::remote::RemoteTarget::for_run(host, "probe");
                crate::engines::remote::verify_remote_launch(&target, launch, spec)
            }
        }
    }

    /// Resolve an unconfigured launch. Local discovery never starts the engine;
    /// remote discovery runs in the remote submission worker and verifies it.
    fn resolve_unconfigured(&self, spec: &EngineSpec) -> Option<(EngineLaunch, Option<String>)> {
        if spec.requires_configured_path {
            return None;
        }
        match self {
            Self::Local(_) => {
                crate::engines::registry::local_launch_candidate(spec).map(|launch| (launch, None))
            }
            Self::Remote(host) => {
                let target = crate::engines::remote::RemoteTarget::for_run(host, "probe");
                crate::engines::remote::probe_remote(&target, spec)
                    .map(|(launch, version)| (launch, Some(version)))
            }
        }
    }

    /// Find a verified launch for an explicit Verify action.
    fn probe_verified(&self, spec: &EngineSpec) -> Option<(EngineLaunch, String)> {
        if spec.requires_configured_path {
            return None;
        }
        match self {
            Self::Local(_) => crate::engines::registry::probe_local(spec),
            Self::Remote(host) => {
                let target = crate::engines::remote::RemoteTarget::for_run(host, "probe");
                crate::engines::remote::probe_remote(&target, spec)
            }
        }
    }

    fn describe(&self) -> String {
        match self {
            Self::Local(_) => "this machine".to_string(),
            Self::Remote(host) => host.label.clone(),
        }
    }
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

/// Resolve the launch for `id` on `target` so a job can be submitted against it: a
/// configured launch is taken at its word (verifying it here would make every run
/// pay a WSL cold start), else the target is discovered. Local discovery is a
/// cheap executable lookup; remote discovery blocks and must run off the UI thread.
pub fn resolve_engine_launch(target: LaunchTarget<'_>, id: EngineId) -> Result<ResolvedLaunch> {
    let spec = launch_spec(id)?;
    if let Some(launch) = target.configured(id) {
        return Ok(ResolvedLaunch {
            launch: launch.clone(),
            version: None,
            detected: false,
        });
    }
    match target.resolve_unconfigured(spec) {
        Some((launch, version)) => Ok(ResolvedLaunch {
            launch,
            version,
            detected: true,
        }),
        None => bail!("{}", not_found_message(&target, spec)),
    }
}

/// What a Verify action learned about `id` on `target`.
pub enum VerifyOutcome {
    /// This exact launch ran and identified itself.
    Verified {
        launch: EngineLaunch,
        version: String,
    },
    /// The configured launch ran but did not answer for itself.
    Failed {
        launch: EngineLaunch,
        reason: String,
    },
    /// Nothing was configured and no candidate answered.
    NotFound { reason: String },
}

/// Verify `id` on `target`, the single action behind every Verify button.
///
/// A configured launch is verified *as configured* — that is the whole point, and
/// the reason this is not a search. Only an empty configuration falls back to
/// probing the target's candidates, and then the launch it found is handed back so
/// the caller can show the user what it filled in.
///
/// Blocking (SSH, or a WSL cold start); never call from the UI thread.
pub fn verify_engine(target: LaunchTarget<'_>, id: EngineId) -> Result<VerifyOutcome> {
    let spec = launch_spec(id)?;
    let Some(launch) = target.configured(id).cloned() else {
        return Ok(match target.probe_verified(spec) {
            Some((launch, version)) => VerifyOutcome::Verified { launch, version },
            None => VerifyOutcome::NotFound {
                reason: not_found_message(&target, spec),
            },
        });
    };
    Ok(match target.verify(&launch, spec) {
        Ok(version) => VerifyOutcome::Verified { launch, version },
        Err(reason) => VerifyOutcome::Failed { launch, reason },
    })
}

fn launch_spec(id: EngineId) -> Result<&'static EngineSpec> {
    match engine_spec(id) {
        Some(spec) => Ok(spec),
        None => bail!("`{}` is a built-in engine and has no launch", id.as_str()),
    }
}

fn not_found_message(target: &LaunchTarget<'_>, spec: &EngineSpec) -> String {
    if spec.requires_configured_path {
        return format!(
            "{} requires a user-specified program path in Settings -> Compute targets for {}.",
            spec.name,
            target.describe()
        );
    }
    let first = spec.candidate_executables.first().copied().unwrap_or("it");
    match target {
        LaunchTarget::Local(_) => format!(
            "Could not find {}. Install it and ensure `{first}` is on PATH, set up WSL with it \
             installed, or set its program in Settings -> Compute targets -> This machine.",
            spec.name
        ),
        LaunchTarget::Remote(_) => format!(
            "No working `{first}` found on {} (tried {:?}). Set its program in \
             Settings -> Compute targets, or add a setup line to the host's prelude \
             (e.g. `module load {}`).",
            target.describe(),
            spec.candidate_executables,
            spec.id.as_str()
        ),
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
            engines,
            ..Default::default()
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

    /// Verify checks the launch the user configured, rather than searching for one
    /// that happens to work. A program outside the spec's candidate list is exactly
    /// the case a search can never reach, so its failure must name *that* program.
    #[test]
    fn verify_reports_on_the_configured_launch_not_a_candidate() {
        let mut overrides = EngineLaunches::new();
        overrides.insert(EngineId::GROMACS, EngineLaunch::native("/nonsense/gmx"));

        let outcome = verify_engine(LaunchTarget::Local(&overrides), EngineId::GROMACS)
            .expect("gromacs has a spec");
        let VerifyOutcome::Failed { launch, reason } = outcome else {
            panic!("a nonexistent program must not verify");
        };
        assert_eq!(launch.program, "/nonsense/gmx");
        assert!(!reason.is_empty());
    }
}
