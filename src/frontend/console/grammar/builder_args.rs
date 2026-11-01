//! Argument types for the organic-materials verbs (`forcefield`,
//! `membrane`), kept beside [`super::ptm_args`] so the root grammar stays within
//! the file-size rule.

use std::path::PathBuf;

use clap::{Args, Subcommand};

#[derive(Debug, Subcommand)]
pub(crate) enum ForcefieldAction {
    /// Type one organic molecule with GAFF2 and AM1-BCC charges through acpype and
    /// save the result in the force-field library, e.g. `forcefield type active --name benzene`.
    Type(ForcefieldTypeArgs),
    /// List the saved force fields.
    List,
}

#[derive(Debug, Args)]
pub(crate) struct ForcefieldTypeArgs {
    /// Entry to type: `active`, `#id`/`id`, or a name (default: active).
    #[arg(default_value = "active")]
    pub(crate) entry: String,
    /// Net charge of the molecule (default: the sum of its formal charges).
    #[arg(long, allow_hyphen_values = true)]
    pub(crate) charge: Option<i32>,
    /// Charge method: `bcc` (AM1-BCC, the GAFF reference) or `gasteiger` (instant, rough).
    #[arg(long, default_value = "bcc")]
    pub(crate) method: String,
    /// Atom-type set: `gaff2` or `gaff`.
    #[arg(long, default_value = "gaff2")]
    pub(crate) atom_types: String,
    /// Library name to save under (default: the entry name).
    #[arg(long)]
    pub(crate) name: Option<String>,
    /// Give up after this many minutes.
    #[arg(long, default_value_t = 30)]
    pub(crate) timeout: u64,
}

#[derive(Debug, Subcommand)]
pub(crate) enum MembraneAction {
    /// Mean-squared displacement and self-diffusion coefficient of molecules in an
    /// MD trajectory, e.g. `membrane diffusion --select BEN --out msd.csv`.
    Diffusion(DiffusionArgs),
    /// Probe-accessible free volume of a periodic cell (optionally averaged over
    /// trajectory frames), e.g. `membrane free-volume --probe 1.3`.
    FreeVolume(FreeVolumeArgs),
}

#[derive(Debug, Args)]
pub(crate) struct DiffusionArgs {
    /// Trajectory file (`.xtc`, `.trj`, `.xyz`); default: the entry's recorded MD trajectory.
    pub(crate) trajectory: Option<PathBuf>,
    /// Entry whose topology the trajectory belongs to: `active`, `#id`, or a name.
    #[arg(long, default_value = "active")]
    pub(crate) entry: String,
    /// Molecules to track: `all`, an element symbol, or a residue name such as `BEN`.
    #[arg(long, default_value = "all")]
    pub(crate) select: String,
    /// Longest lag time (ps) to evaluate; default: half the trajectory.
    #[arg(long)]
    pub(crate) max_lag: Option<f64>,
    /// Fit window as fractions of the lag range, `from,to` (default `0.2,0.8`).
    #[arg(long, default_value = "0.2,0.8")]
    pub(crate) fit: String,
    /// Write the MSD curve as CSV to this path.
    #[arg(long)]
    pub(crate) out: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub(crate) struct FreeVolumeArgs {
    /// Entry to analyse: `active`, `#id`/`id`, or a name (default: active).
    #[arg(default_value = "active")]
    pub(crate) entry: String,
    /// Probe radius in Å (`0` for the plain unoccupied fraction).
    #[arg(long, default_value_t = 1.0)]
    pub(crate) probe: f32,
    /// Grid spacing in Å.
    #[arg(long, default_value_t = 0.5)]
    pub(crate) grid: f32,
    /// Average over the last `--frames` frames of this trajectory instead of the
    /// entry's own coordinates; default: the entry's recorded MD trajectory when
    /// `--frames` is given.
    #[arg(long)]
    pub(crate) trajectory: Option<PathBuf>,
    /// Number of trailing trajectory frames to average over.
    #[arg(long)]
    pub(crate) frames: Option<usize>,
}
