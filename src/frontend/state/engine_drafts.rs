/// Editable draft for one engine's launch override in the Settings panel.
/// `command_prefix` is held as a single whitespace-separated line for easy
/// editing (e.g. `wsl.exe -e`); it is split on apply.
#[derive(Debug, Clone, Default)]
pub struct EngineDraft {
    pub command_prefix: String,
    pub program: String,
}

impl EngineDraft {
    pub fn from_launch(launch: &crate::engines::registry::EngineLaunch) -> Self {
        Self {
            command_prefix: launch.command_prefix.join(" "),
            program: launch.program.clone(),
        }
    }

    /// Build an [`EngineLaunch`] from the draft, or `None` if no program is
    /// set (which the dispatcher treats as "clear this override").
    pub fn to_launch(&self) -> Option<crate::engines::registry::EngineLaunch> {
        let program = self.program.trim();
        if program.is_empty() {
            return None;
        }
        Some(crate::engines::registry::EngineLaunch {
            command_prefix: self
                .command_prefix
                .split_whitespace()
                .map(str::to_string)
                .collect(),
            program: program.to_string(),
        })
    }
}

/// Editable draft for one remote host in the Settings panel. All fields are held
/// as text for direct editing and parsed/validated on save (`port`, `prelude`,
/// and `gmx_program` in particular). Mirrors [`EngineDraft`].
#[derive(Debug, Clone, Default)]
pub struct RemoteHostDraft {
    pub label: String,
    pub hostname: String,
    pub username: String,
    pub port: String,
    pub work_root: String,
    /// One shell setup line per text row (`module load gromacs`, `source GMXRC`).
    pub prelude: String,
    /// Remote path to `gmx` (or a bare name resolved via the prelude/PATH).
    pub gmx_program: String,
}

impl RemoteHostDraft {
    pub fn from_host(host: &crate::backend::config::RemoteHost) -> Self {
        let gmx_program = host
            .engines
            .get(crate::engines::registry::EngineId::GROMACS.as_str())
            .map(|launch| launch.program.clone())
            .unwrap_or_default();
        Self {
            label: host.label.clone(),
            hostname: host.hostname.clone(),
            username: host.username.clone(),
            port: host.port.to_string(),
            work_root: host.work_root.clone(),
            prelude: host.prelude.join("\n"),
            gmx_program,
        }
    }
}

/// Connection status of a remote host, shown as an indicator in the panel.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum RemoteHostStatus {
    /// Not yet probed.
    #[default]
    Unknown,
    /// A probe (passwordless check / detect) is in flight.
    Checking,
    /// Passwordless login works.
    Ready,
    /// Reachable, but passwordless login is not set up yet.
    NeedsSetup,
    /// The probe failed (unreachable / auth error). Carries a short reason.
    Unreachable(String),
}
