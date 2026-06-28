//! Shared structural infrastructure for protein post-translational modifications.
//! The reusable pieces every PTM family builds on: idealized modifying-group
//! [`fragments`] and the host-anchor resolver. Per-family workflows (mapping a
//! [`PtmKind`](crate::domain::modification::PtmKind) to a fragment and anchor)
//! build on these and reuse the one
//! [`condense`](crate::workflows::assembly::condense) attachment path.

mod acetylate;
mod attach;
pub mod fragments;
mod host;
mod lipidate;
mod methylate;
mod phosphorylate;
mod ubl;

#[cfg(test)]
mod testkit;

pub use acetylate::acetylate_protein;
pub use fragments::{Fragment, acetyl, acyl, isoprenoid, methyl, phosphate};
pub use host::{resolve_acceptor, resolve_n_terminus};
pub use lipidate::{acylate_protein, prenylate_protein};
pub use methylate::methylate_protein;
pub use phosphorylate::phosphorylate_protein;
pub use ubl::{ubiquitinate_protein, ubl_template};
