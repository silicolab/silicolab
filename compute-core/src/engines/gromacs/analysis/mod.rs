//! Parsing and orchestration of GROMACS analysis output.
//!
//! GROMACS analysis tools emit `.xvg` data files; [`xvg`] parses them into
//! numeric series. [`energy`] builds the stdin selections those tools read.
//! [`tools`] orchestrates the actual `gmx energy` run on top of those pure
//! pieces.

pub mod energy;
pub mod tools;
pub mod xvg;

pub use energy::energy_term_selection;
pub use tools::{AnalysisContext, gmx_energy};
pub use xvg::{Xvg, parse_xvg};
