//! In-process molecular docking engine.
//!
//! Wraps the `docking` crate (a pure-Rust AutoDock Vina reimplementation) so the
//! rest of the app can dock a ligand into a receptor, or score a single pose, from
//! [`Structure`]s without knowing the crate's types. Like the QM engine this is a
//! library call that runs in-process on a worker thread, not an external
//! subprocess.
//!
//! The request/outcome edge ([`DockingRequest`], [`DockingOutcome`]) keeps the
//! crate's types off the public API. Inputs are prepared PDBQT: silicolab either
//! converts a [`Structure`] (best-effort atom typing + torsion tree, see
//! [`crate::io::formats::pdbqt`]) or passes through already-prepared PDBQT text.
//! Vina scoring ignores partial charges, so the preparation only needs correct
//! AutoDock atom types and, for the ligand, a rotatable-bond torsion tree.
//!
//! [`Structure`]: crate::domain::Structure

mod run;
mod types;

pub use run::run_docking;
pub use types::*;
