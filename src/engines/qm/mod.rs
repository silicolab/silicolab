//! In-process quantum-chemistry engine.
//!
//! Wraps the `chemx` crate (pure-Rust HF/DFT/MP2/CC) so the rest of the app can
//! request single-point energies, geometry optimization, and properties or
//! vibrational frequencies from a [`Structure`] without knowing chemx's types.
//! Unlike the GROMACS engine this is a library call — it runs in-process on a
//! worker thread, not as an external subprocess.
//!
//! The request/outcome edge ([`QmRequest`], [`QmOutcome`]) deliberately keeps
//! chemx's types off the public API: every chemx option silicolab exposes is
//! mirrored by a plain enum/struct here, and every chemx result field we report
//! is folded into the formatted [`QmOutcome::summary`]. That boundary is what
//! would let a future build run chemx as an out-of-process engine (see the
//! `chemx` binary) without touching any caller.
//!
//! [`Structure`]: crate::domain::Structure

pub mod periodic;

mod build;
mod run;
mod summary;
mod types;

pub use periodic::{KMesh, PeriodicFunctional, PeriodicQmRequest, run_periodic_qm};

pub(crate) use build::*;
pub use run::*;
pub(crate) use summary::*;
pub use types::*;

#[cfg(test)]
mod tests;
