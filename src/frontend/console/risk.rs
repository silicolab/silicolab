//! Approval risk: how consequential each `.sls` command is, which is what the
//! assistant's approval gate keys on.

use super::{Command, JobsAction};

/// How consequential a command is — the input to the assistant's approval gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum RiskLevel {
    /// View/inspection and additive, low-stakes building; safe to auto-run.
    ReadOnly,
    /// Edits the active structure in memory (reversible by reloading).
    Mutating,
    /// Writes a file to disk, which may overwrite an existing one — not
    /// reversible from the app, so it confirms even in the safe-auto default.
    FileWrite,
    /// Launches a compute job (minutes/GPU).
    Expensive,
    /// Irreversibly removes data, or executes an arbitrary script whose effects
    /// cannot be known in advance — always prompts, in every mode.
    Destructive,
}

impl RiskLevel {
    /// A short human noun for the level, for approval cards and prose.
    pub(crate) fn label(self) -> &'static str {
        match self {
            RiskLevel::ReadOnly => "read-only",
            RiskLevel::Mutating => "structure edit",
            RiskLevel::FileWrite => "file write",
            RiskLevel::Expensive => "compute",
            RiskLevel::Destructive => "destructive",
        }
    }
}

impl Command {
    /// This command's approval risk. The match is deliberately wildcard-free, so
    /// a new `Command` variant fails to compile until it is classified — never
    /// default a new command to safe.
    pub(crate) fn risk(&self) -> RiskLevel {
        use RiskLevel::*;
        match self {
            Command::Open { .. }
            | Command::Activate { .. }
            | Command::Sketch { .. }
            | Command::Find { .. }
            | Command::Fetch { .. }
            | Command::View(_)
            | Command::Cartoon(_)
            | Command::Color(_)
            | Command::Surface(_)
            | Command::Show(_)
            | Command::Representation(_)
            | Command::Hydrogen { .. }
            | Command::Jobs {
                action: None | Some(JobsAction::Status),
            }
            | Command::Help => ReadOnly,
            // `qm recommend` only prints the level-of-theory table (read-only);
            // every other `qm` sub-verb launches a calculation.
            Command::Qm { args } => match args.first().map(String::as_str) {
                Some("recommend" | "status") => ReadOnly,
                Some("cancel") => Destructive,
                _ => Expensive,
            },
            Command::Glycan(_)
            | Command::Glycosylate(_)
            | Command::Phosphorylate(_)
            | Command::Acetylate(_)
            | Command::Methylate(_)
            | Command::Lipidate(_)
            | Command::Ubiquitinate(_) => Mutating,
            Command::Save { .. } | Command::Export(_) => FileWrite,
            Command::Md { .. }
            | Command::Disorder { .. }
            | Command::Dock(_)
            | Command::Score(_) => Expensive,
            // `Source` runs a script's lines straight through the console with no
            // per-line gate, so it can `delete` — it must clear the floor itself.
            Command::Delete { .. }
            | Command::Source { .. }
            | Command::Jobs {
                action: Some(JobsAction::Cancel { .. }),
            } => Destructive,
        }
    }
}
