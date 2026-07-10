//! How to launch an external engine — leaf launch data shared by the engine
//! registry and the remote-host descriptor.
//!
//! [`EngineLaunch`] is plain, serializable launch data with no dependency on the
//! engine machinery, so it sits at a leaf layer that both `engines` (which builds
//! a `ProcessConfig` from it) and `hosts` (which stores one per engine) can use
//! without either having to import the other. [`Compute`] pairs a launch with the
//! CPU/GPU envelope the engine subprocess may use; an engine translates that
//! envelope into its own flags.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Stable identifier used everywhere a specific engine is referenced. Lives here,
/// at the leaf, because [`EngineLaunches`] is keyed by it and `hosts` stores one
/// map per remote host without importing the engine machinery.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EngineId(pub &'static str);

impl EngineId {
    pub const UFF: Self = Self("uff");
    pub const HARTREE: Self = Self("hartree");
    pub const GROMACS: Self = Self("gromacs");
    pub const DOCKING: Self = Self("docking");

    pub fn as_str(self) -> &'static str {
        self.0
    }
}

/// How to launch an external engine: a program plus an optional command
/// prefix. Native = empty prefix; WSL = `["wsl.exe", "-e"]`; a container or
/// wrapper script is just a different prefix.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EngineLaunch {
    /// Leading command tokens, e.g. `["wsl.exe", "-e"]`. Empty for native.
    #[serde(default)]
    pub command_prefix: Vec<String>,
    /// The engine executable, resolved on the *target* environment. For WSL
    /// this is a Linux path like `/usr/local/gromacs/bin/gmx`; for native it
    /// is a Windows path or a bare name found on PATH.
    pub program: String,
}

impl EngineLaunch {
    /// A native launch with no prefix.
    pub fn native(program: impl Into<String>) -> Self {
        Self {
            command_prefix: Vec::new(),
            program: program.into(),
        }
    }

    /// True when there is no usable program configured.
    pub fn is_empty(&self) -> bool {
        self.program.trim().is_empty()
    }

    /// Human-readable rendering of the effective command, for settings UI.
    pub fn display_command(&self) -> String {
        if self.command_prefix.is_empty() {
            self.program.clone()
        } else {
            format!("{} {}", self.command_prefix.join(" "), self.program)
        }
    }
}

/// The per-engine launches configured for one compute target — the local machine
/// (`AppConfig::engine_overrides`) or a remote host (`RemoteHost::engines`). One
/// type for both, so "how do I launch engine E on target T" has a single answer
/// and a single place to resolve it.
///
/// An entry with an empty `program` is treated as absent: the settings UI writes
/// one when the user clears the field.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EngineLaunches(HashMap<String, EngineLaunch>);

impl EngineLaunches {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&self, id: EngineId) -> Option<&EngineLaunch> {
        self.0.get(id.as_str()).filter(|launch| !launch.is_empty())
    }

    pub fn contains(&self, id: EngineId) -> bool {
        self.get(id).is_some()
    }

    pub fn insert(&mut self, id: EngineId, launch: EngineLaunch) {
        self.0.insert(id.as_str().to_string(), launch);
    }

    /// Record an auto-detected launch, leaving a configured one untouched.
    /// Returns `true` when it was newly inserted, so the caller knows to persist.
    pub fn cache_detected(&mut self, id: EngineId, launch: EngineLaunch) -> bool {
        if self.contains(id) {
            return false;
        }
        self.insert(id, launch);
        true
    }

    pub fn remove(&mut self, id: EngineId) {
        self.0.remove(id.as_str());
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// CPU/GPU resources an engine subprocess may use. `0` means "let the engine
/// decide" (its own default — all cores / detected GPUs). Engine-neutral: the
/// GROMACS runner maps this onto `mdrun` flags, another engine maps it onto its
/// own. Serializable so a relayed remote job carries the request to the worker.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComputeResources {
    /// CPU threads for the engine; `0` = engine default (all available cores).
    pub cores: u32,
    /// GPUs to offload to; `0` = none / engine auto-detect.
    pub gpu: u32,
}

/// How to invoke an external engine: the launch descriptor plus the resource
/// envelope, threaded through an engine pipeline so a run and its launch travel
/// together. The engine always runs as a local subprocess of whichever host
/// executes the pipeline — the laptop for a local run, the compute node for a
/// relayed remote run — so there is no transport here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Compute {
    pub launch: EngineLaunch,
    pub resources: ComputeResources,
}

impl Compute {
    /// Run the engine with this launch, letting it pick its own CPU/GPU defaults.
    pub fn local(launch: EngineLaunch) -> Self {
        Self {
            launch,
            resources: ComputeResources::default(),
        }
    }

    /// Run the engine with an explicit CPU/GPU resource request.
    pub fn local_with_resources(launch: EngineLaunch, resources: ComputeResources) -> Self {
        Self { launch, resources }
    }
}

impl From<EngineLaunch> for Compute {
    /// Keeps existing call sites terse (`launch.into()`).
    fn from(launch: EngineLaunch) -> Self {
        Self::local(launch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caching_a_detected_launch_inserts_once_and_never_clobbers() {
        let mut launches = EngineLaunches::new();
        let detected = EngineLaunch {
            command_prefix: vec!["wsl.exe".to_string(), "-e".to_string()],
            program: "/usr/local/gromacs/bin/gmx".to_string(),
        };
        assert!(launches.cache_detected(EngineId::GROMACS, detected));
        assert_eq!(
            launches.get(EngineId::GROMACS).map(|l| l.program.as_str()),
            Some("/usr/local/gromacs/bin/gmx")
        );

        // A later detection must not overwrite a launch already configured.
        assert!(!launches.cache_detected(EngineId::GROMACS, EngineLaunch::native("gmx")));
        assert_eq!(
            launches.get(EngineId::GROMACS).map(|l| l.program.as_str()),
            Some("/usr/local/gromacs/bin/gmx")
        );
    }

    /// The settings UI writes an empty program when the user clears the field;
    /// that must read back as "not configured", not as a launch of `""`.
    #[test]
    fn an_empty_program_reads_as_absent() {
        let mut launches = EngineLaunches::new();
        launches.insert(EngineId::GROMACS, EngineLaunch::native(""));
        assert!(!launches.contains(EngineId::GROMACS));
        assert!(launches.get(EngineId::GROMACS).is_none());
        // …and a real detection still caches over it.
        assert!(launches.cache_detected(EngineId::GROMACS, EngineLaunch::native("gmx")));
    }
}
