//! Argument structs for the six protein post-translational-modification verbs.
//! They mirror [`GlycosylateArgs`](super::GlycosylateArgs): the runtime-required
//! references (`--protein`, `--at`) stay `Option<String>` so the command body can
//! report a verb-specific "phosphorylate requires --protein" message, while the
//! `--kind`/`--degree`/`--ubl` selectors carry sensible defaults.

use clap::Args;

#[derive(Debug, Args)]
pub(crate) struct PhosphorylateArgs {
    #[arg(long)]
    pub(crate) protein: Option<String>,
    #[arg(long)]
    pub(crate) at: Option<String>,
    #[arg(long)]
    pub(crate) name: Option<String>,
}

#[derive(Debug, Args)]
pub(crate) struct AcetylateArgs {
    #[arg(long)]
    pub(crate) protein: Option<String>,
    #[arg(long)]
    pub(crate) at: Option<String>,
    /// Cap the chain N-terminus instead of the Lys side-chain NZ.
    #[arg(long = "n-terminal")]
    pub(crate) n_terminal: bool,
    #[arg(long)]
    pub(crate) name: Option<String>,
}

#[derive(Debug, Args)]
pub(crate) struct MethylateArgs {
    #[arg(long)]
    pub(crate) protein: Option<String>,
    #[arg(long)]
    pub(crate) at: Option<String>,
    /// Methylation degree: `mono`, `di`, or `tri`.
    #[arg(long, default_value = "mono")]
    pub(crate) degree: String,
    #[arg(long)]
    pub(crate) name: Option<String>,
}

#[derive(Debug, Args)]
pub(crate) struct LipidateArgs {
    #[arg(long)]
    pub(crate) protein: Option<String>,
    #[arg(long)]
    pub(crate) at: Option<String>,
    /// Lipid: `palmitoyl`, `myristoyl`, `farnesyl`, or `geranylgeranyl`.
    #[arg(long, default_value = "palmitoyl")]
    pub(crate) kind: String,
    #[arg(long)]
    pub(crate) name: Option<String>,
}

#[derive(Debug, Args)]
pub(crate) struct UbiquitinateArgs {
    #[arg(long)]
    pub(crate) protein: Option<String>,
    #[arg(long)]
    pub(crate) at: Option<String>,
    /// Ubiquitin-like modifier: `ubiquitin`, `sumo`, or `nedd8`.
    #[arg(long, default_value = "ubiquitin")]
    pub(crate) ubl: String,
    /// Override the bundled UBL template with an open entry (`#id` or a name).
    #[arg(long)]
    pub(crate) with: Option<String>,
    #[arg(long)]
    pub(crate) name: Option<String>,
}
