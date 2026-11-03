//! Argument types for the verbs that parameterize organic molecules
//! (`forcefield`), kept beside [`super::ptm_args`] so the
//! root grammar stays within the file-size rule.

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
