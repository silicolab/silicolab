//! Readers for molecular-dynamics trajectory files.
//!
//! These decode coordinate frames produced by external MD engines (kept in the
//! task run directory, not in the project database) into a
//! [`crate::domain::Trajectory`] for in-app playback.

pub mod xtc;

pub use xtc::read_xtc;
