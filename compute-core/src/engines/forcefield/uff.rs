//! Universal Force Field (UFF) energy evaluation and geometry optimization.
//!
//! The implementation is split into:
//! - `params`: atom typing and the published UFF parameter tables.
//! - `energy`: the bonded and nonbonded energy terms.
//! - `optimize`: numerical gradients and the gradient-descent step driver.

mod energy;
mod optimize;
mod params;

pub use energy::*;
pub(crate) use optimize::*;
pub(crate) use params::*;

#[cfg(test)]
mod tests;
