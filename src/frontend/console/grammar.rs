//! The declarative clap grammar for the `.sls` command console — the single
//! source of truth for dispatch and help.
//!
//! The console is a REPL with no `argv[0]`, so [`Root`] is parsed with
//! `no_binary_name`. clap must never `exit()` here: parse errors are rendered to
//! a string and surfaced through the normal `Result` channel (see [`run`]) so
//! they show in the console/message line exactly like a failed command.
//!
//! Tokenization stays with the Windows-backslash-safe [`super::shell_words`];
//! this module only owns the token-list → typed-command step. Value parsing
//! (colors, `on`/`off`, atom-style aliases, …) lives in [`super::args`] so the
//! grammar below reads as pure structure.
//!
//! The domain commands (`md`, `qm`, `dock`/`score`, `disorder`/`pack`) capture
//! their tail verbatim (`trailing_var_arg`) and hand it to the existing
//! `*_command(state, &[String])` parsers; converting their internal grammars is
//! a separate, per-module step.

use std::path::PathBuf;

use anyhow::{Result, anyhow};
use clap::{Args, CommandFactory, FromArgMatches, Parser, Subcommand};
use eframe::egui::Color32;

use super::{
    ScriptContext, parse_atom_style, parse_chain, parse_color_value, parse_light_preset,
    parse_onoff, parse_surface_style,
};
use crate::frontend::{
    LightPreset, SurfaceStyle,
    state::{AppState, AtomStyle},
};

/// Root of the console grammar. `no_binary_name` because the first token is the
/// command, not an executable path; the auto `help` subcommand is disabled in
/// favor of an explicit [`Command::Help`] so the command list is introspectable.
#[derive(Debug, Parser)]
#[command(
    name = "",
    no_binary_name = true,
    disable_help_subcommand = true,
    about = "SilicoLab .sls console commands.",
    help_template = "{about}\n\nCommands:\n{subcommands}\n\nOptions:\n{options}"
)]
pub(crate) struct Root {
    #[command(subcommand)]
    pub(crate) command: Command,
}

/// Every top-level console command. New variants must also be documented in
/// [`super::command_catalog`] (enforced by a drift-guard test) unless they are
/// scripting/meta plumbing.
#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    /// Load a structure file (.pdb/.cif/.xyz/.mol2/.gro) as a new entry.
    Open { path: String },

    /// Make an already-open entry active (`#id`, a bare id, or a name).
    #[command(visible_alias = "focus")]
    Activate { reference: String },

    /// Build a 3D structure from a SMILES string as a new entry.
    Sketch {
        smiles: String,
        /// Name for the new entry (defaults to the SMILES text).
        #[arg(long)]
        name: Option<String>,
    },

    /// Download a structure by PDB id.
    Fetch {
        /// PDB id, e.g. `4hhb`.
        id: String,
        /// Override the RCSB base URL.
        #[arg(long)]
        db: Option<String>,
        /// Directory to download into.
        #[arg(long)]
        dir: Option<PathBuf>,
    },

    /// Run a `.sls` script file.
    #[command(visible_alias = "run")]
    Source { path: PathBuf },

    /// Viewport settings (active entry; add `--global` for project-wide).
    View(ViewArgs),

    /// Cartoon ribbon styling.
    Cartoon(CartoonArgs),

    /// Coloring of chains, ions, and hetero atoms.
    Color(ColorArgs),

    /// Molecular surface display.
    Surface(SurfaceArgs),

    /// Reveal specific atoms (e.g. nearby ions).
    Show(ShowArgs),

    /// Per-atom representation style for the active entry.
    Representation(RepresentationArgs),

    /// Add missing hydrogens to the active structure.
    #[command(visible_alias = "hydrogens")]
    Hydrogen {
        #[command(subcommand)]
        action: HydrogenAction,
    },

    /// Delete parts of the active structure.
    Delete {
        #[command(subcommand)]
        target: DeleteTarget,
    },

    /// Save an image or a replayable view script.
    Save {
        #[command(subcommand)]
        target: SaveTarget,
    },

    // `md`/`qm`/`disorder` keep the verbatim pass-through: their hand-written
    // parsers are large, share the assistant's heavy-path request builders, and
    // (for `disorder`) carry order-dependent `--of`/`--count` attachment that
    // clap derive cannot express. TODO(clap): convert their internal grammars.
    /// Molecular dynamics (run `md` for subcommands).
    #[command(disable_help_flag = true)]
    Md {
        #[arg(allow_hyphen_values = true, trailing_var_arg = true, num_args = 0..)]
        args: Vec<String>,
    },

    /// Pack molecules into a region (alias: `pack`).
    #[command(visible_alias = "pack", disable_help_flag = true)]
    Disorder {
        #[arg(allow_hyphen_values = true, trailing_var_arg = true, num_args = 0..)]
        args: Vec<String>,
    },

    /// Quantum chemistry (run `qm` for subcommands).
    #[command(disable_help_flag = true)]
    Qm {
        #[arg(allow_hyphen_values = true, trailing_var_arg = true, num_args = 0..)]
        args: Vec<String>,
    },

    /// Dock a ligand into a receptor (Vina).
    Dock(DockArgs),

    /// Single-point score of a ligand's input pose.
    Score(DockArgs),

    /// Print this help.
    Help,
}

/// Flags shared by `dock` and `score`. `--center`/`--size` accept hyphen-leading
/// values so a box centered on negative coordinates (`--center -1,2,3`) parses;
/// `receptor`/`ligand` stay optional so the body can report the verb-specific
/// "dock requires --receptor" / "score requires --receptor" message.
#[derive(Debug, Args)]
pub(crate) struct DockArgs {
    /// Receptor entry: `active`, `#id`/`id`, or a name.
    #[arg(long)]
    pub receptor: Option<String>,
    /// Ligand entry: `active`, `#id`/`id`, or a name.
    #[arg(long)]
    pub ligand: Option<String>,
    /// Search-box center `x,y,z` (default: the receptor centroid).
    #[arg(long, allow_hyphen_values = true)]
    pub center: Option<String>,
    /// Search-box size `x,y,z` in Angstrom (default: 22.5 cube).
    #[arg(long, allow_hyphen_values = true)]
    pub size: Option<String>,
    /// Search exhaustiveness (default 8).
    #[arg(long)]
    pub exhaustiveness: Option<u32>,
    /// Number of binding modes to report (default 9).
    #[arg(long)]
    pub modes: Option<u32>,
    /// RNG seed (default 0).
    #[arg(long)]
    pub seed: Option<u32>,
}

/// `--global`: apply a render command project-wide, not just to the active
/// entry. Flattened into every render command; `global = true` so it is accepted
/// before or after the subcommand keyword (matching the old "strip from
/// anywhere" behavior).
#[derive(Debug, Args)]
pub(crate) struct GlobalArgs {
    #[arg(long, global = true)]
    pub(crate) global: bool,
}

#[derive(Debug, Args)]
pub(crate) struct ViewArgs {
    #[command(subcommand)]
    pub(crate) kind: ViewKind,
    #[command(flatten)]
    pub(crate) global: GlobalArgs,
}

#[derive(Debug, Subcommand)]
pub(crate) enum ViewKind {
    /// Background color (named or `#rrggbb`).
    Background {
        #[arg(value_parser = parse_color_value)]
        color: Color32,
    },
    /// Requested off-screen render size in pixels.
    Size { width: f32, height: f32 },
    /// Show or hide the unit cell.
    Cell {
        // `action = Set` overrides clap's `bool` default (a `SetTrue` flag); this
        // is a positional that takes an `on`/`off` value.
        #[arg(value_parser = parse_onoff, action = clap::ArgAction::Set)]
        on: bool,
    },
    /// Show or hide solvent.
    Water {
        #[arg(value_parser = parse_onoff, action = clap::ArgAction::Set)]
        on: bool,
    },
    /// Lighting preset.
    Light {
        #[arg(value_parser = parse_light_preset)]
        preset: LightPreset,
    },
    /// Silhouette outlines.
    #[command(visible_alias = "silhouettes")]
    Silhouette {
        #[arg(value_parser = parse_onoff, action = clap::ArgAction::Set)]
        on: bool,
        #[arg(long)]
        width: Option<f32>,
    },
}

#[derive(Debug, Args)]
pub(crate) struct CartoonArgs {
    #[command(subcommand)]
    pub(crate) kind: CartoonKind,
    #[command(flatten)]
    pub(crate) global: GlobalArgs,
}

/// Shared `--width`/`--thickness` for the helix/sheet/coil sections.
#[derive(Debug, Args)]
pub(crate) struct CartoonSection {
    #[arg(long)]
    pub(crate) width: Option<f32>,
    #[arg(long)]
    pub(crate) thickness: Option<f32>,
}

#[derive(Debug, Subcommand)]
pub(crate) enum CartoonKind {
    Helix(CartoonSection),
    Sheet(CartoonSection),
    Coil(CartoonSection),
    /// Ribbon path smoothing iterations.
    Smoothing {
        value: usize,
    },
    /// Cross-section profile segment count.
    Profile {
        value: usize,
    },
}

#[derive(Debug, Args)]
pub(crate) struct ColorArgs {
    #[command(subcommand)]
    pub(crate) kind: ColorKind,
    #[command(flatten)]
    pub(crate) global: GlobalArgs,
}

#[derive(Debug, Subcommand)]
pub(crate) enum ColorKind {
    /// Color a chain by id.
    Chain {
        #[arg(value_parser = parse_chain)]
        id: char,
        #[arg(value_parser = parse_color_value)]
        color: Color32,
    },
    /// Color ions.
    Ions {
        #[arg(value_parser = parse_color_value)]
        color: Color32,
    },
    /// Use per-element hetero-atom colors.
    Hetero,
}

#[derive(Debug, Args)]
pub(crate) struct SurfaceArgs {
    #[command(subcommand)]
    pub(crate) kind: SurfaceKind,
    #[command(flatten)]
    pub(crate) global: GlobalArgs,
}

#[derive(Debug, Subcommand)]
pub(crate) enum SurfaceKind {
    /// Enable a surface for a chain.
    Chain {
        #[arg(value_parser = parse_chain)]
        id: char,
    },
    /// Surface fill style.
    Style {
        #[arg(value_parser = parse_surface_style)]
        value: SurfaceStyle,
    },
    /// Surface transparency, 0-100.
    Transparency { value: f32 },
    /// Clear all surfaces.
    Clear,
}

#[derive(Debug, Args)]
pub(crate) struct ShowArgs {
    #[command(subcommand)]
    pub(crate) kind: ShowKind,
    #[command(flatten)]
    pub(crate) global: GlobalArgs,
}

#[derive(Debug, Subcommand)]
pub(crate) enum ShowKind {
    /// Show ions, optionally only those within a distance of the structure.
    Ions {
        #[arg(long)]
        within: Option<f32>,
    },
}

#[derive(Debug, Args)]
pub(crate) struct RepresentationArgs {
    #[arg(value_parser = parse_atom_style)]
    pub(crate) style: AtomStyle,
    // Per-atom styles are entry-specific, so `--global` is accepted but ignored.
    #[command(flatten)]
    pub(crate) global: GlobalArgs,
}

#[derive(Debug, Subcommand)]
pub(crate) enum HydrogenAction {
    /// Add missing hydrogens.
    #[command(visible_alias = "fill")]
    Add,
}

#[derive(Debug, Subcommand)]
pub(crate) enum DeleteTarget {
    /// Delete the listed chains (comma-separated, e.g. `A,B`).
    Chain { spec: String },
}

#[derive(Debug, Subcommand)]
pub(crate) enum SaveTarget {
    /// Render the viewport to a PNG.
    Image { path: PathBuf },
    /// Save the current view as a replayable `.sls` script.
    View { path: PathBuf },
}

impl Command {
    /// Run the parsed command against `state`, mirroring the old `match` arms.
    /// The effect bodies live in the sibling modules; this only routes.
    pub(crate) fn dispatch(
        self,
        state: &mut AppState,
        context: &mut ScriptContext,
    ) -> Result<String> {
        use crate::frontend::{disorder_commands, docking_commands, md_commands, qm_commands};
        match self {
            Command::Open { path } => super::open_command(state, context, &path),
            Command::Activate { reference } => super::activate_command(state, &reference),
            Command::Sketch { smiles, name } => {
                super::sketch_command(state, &smiles, name.as_deref())
            }
            Command::Fetch { id, db, dir } => super::fetch_command(state, &id, db.as_deref(), dir),
            Command::Source { path } => {
                super::run_script_path_with_context(state, context, &path.display().to_string())?;
                Ok(String::new())
            }
            Command::View(args) => super::view_command(state, args),
            Command::Cartoon(args) => super::cartoon_command(state, args),
            Command::Color(args) => super::color_command(state, args),
            Command::Surface(args) => super::surface_command(state, args),
            Command::Show(args) => super::show_command(state, args),
            Command::Representation(args) => super::representation_command(state, args),
            Command::Hydrogen { action } => super::hydrogen_command(state, action),
            Command::Delete { target } => super::delete_command(state, target),
            Command::Save { target } => super::save_command(state, context, target),
            Command::Md { args } => md_commands::md_command(state, &args),
            Command::Disorder { args } => disorder_commands::disorder_command(state, &args),
            Command::Qm { args } => qm_commands::qm_command(state, &args),
            Command::Dock(args) => docking_commands::dock_command(state, args),
            Command::Score(args) => docking_commands::score_command(state, args),
            Command::Help => Ok(long_help()),
        }
    }
}

/// Build the root command with color forced off so rendered errors/help never
/// leak ANSI escapes into the egui console (which is not a terminal).
fn root_command() -> clap::Command {
    Root::command().color(clap::ColorChoice::Never)
}

/// Tokenized line → parse → dispatch. Parse failures are rendered to a string:
/// help/version requests come back as `Ok` (so they print in the console), all
/// other clap errors as `Err` (shown like any failed command).
pub(crate) fn run(
    state: &mut AppState,
    context: &mut ScriptContext,
    words: &[String],
) -> Result<String> {
    let matches = match root_command().try_get_matches_from(words) {
        Ok(matches) => matches,
        Err(err) => return render_clap_error(err),
    };
    let root = Root::from_arg_matches(&matches).map_err(|err| anyhow!("{err}"))?;
    root.command.dispatch(state, context)
}

fn render_clap_error(err: clap::Error) -> Result<String> {
    use clap::error::ErrorKind;
    match err.kind() {
        ErrorKind::DisplayHelp
        | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
        | ErrorKind::DisplayVersion => Ok(err.to_string()),
        _ => Err(anyhow!("{err}")),
    }
}

/// Parse the tail of a `dock` line (the tokens after the verb) into [`DockArgs`].
/// The assistant's heavy `dock` path builds its request straight from this
/// (bypassing the console), so it must parse exactly as `dock` does — hence we
/// route through the same [`Root`] grammar by re-prepending the verb.
pub(crate) fn parse_dock_args(args: &[String]) -> Result<DockArgs> {
    let mut tokens = Vec::with_capacity(args.len() + 1);
    tokens.push("dock".to_string());
    tokens.extend(args.iter().cloned());
    let matches = root_command()
        .try_get_matches_from(&tokens)
        .map_err(|err| anyhow!("{err}"))?;
    match Root::from_arg_matches(&matches)
        .map_err(|err| anyhow!("{err}"))?
        .command
    {
        Command::Dock(dock_args) => Ok(dock_args),
        _ => unreachable!("prepended the `dock` verb"),
    }
}

/// Clap-rendered long help — the replacement for the old hand-written
/// `help_text()`.
pub(crate) fn long_help() -> String {
    root_command().render_long_help().to_string()
}

/// Primary names of every top-level subcommand, for the catalog drift-guard test.
#[cfg(test)]
pub(crate) fn top_level_command_names() -> Vec<String> {
    Root::command()
        .get_subcommands()
        .map(|cmd| cmd.get_name().to_string())
        .collect()
}

/// Parse a token list to a [`Command`] for parse-level tests (no dispatch).
#[cfg(test)]
pub(crate) fn parse_command(words: &[String]) -> std::result::Result<Command, String> {
    let matches = root_command()
        .try_get_matches_from(words)
        .map_err(|err| err.to_string())?;
    Root::from_arg_matches(&matches)
        .map(|root| root.command)
        .map_err(|err| err.to_string())
}
