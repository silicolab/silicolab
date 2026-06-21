//! How to launch an external engine — leaf launch data shared by the engine
//! registry and the remote-host descriptor.
//!
//! [`EngineLaunch`] is plain, serializable launch data with no dependency on the
//! engine machinery, so it sits at a leaf layer that both `engines` (which builds
//! a `ProcessConfig` from it) and `hosts` (which stores one per engine) can use
//! without either having to import the other.

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
