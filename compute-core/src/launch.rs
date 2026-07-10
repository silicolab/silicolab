//! How to launch an external engine — leaf launch data shared by the engine
//! registry and the remote-host descriptor.
//!
//! [`EngineLaunch`] is plain, serializable launch data with no dependency on the
//! engine machinery, so it sits at a leaf layer that both `engines` (which builds
//! a `ProcessConfig` from it) and `hosts` (which stores one per engine) can use
//! without either having to import the other. [`Compute`] pairs a launch with the
//! CPU/GPU envelope the engine subprocess may use; an engine translates that
//! envelope into its own flags.

use serde::{Deserialize, Serialize};

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
