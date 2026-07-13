//! Cartoon (ribbon) rendering for biopolymers, split by pipeline stage:
//! - [`path`]: Cα trace, secondary-structure resolution, spline smoothing and
//!   the swept-frame samples shared by both render paths.
//! - [`geometry`]: cross-section profile, world-space rings, and cartoon
//!   styling/shading.
//! - [`mesh_gpu`]: the camera-independent world mesh for the GPU pipeline.

mod geometry;
mod mesh_gpu;
mod path;

pub(crate) use geometry::*;
pub(crate) use mesh_gpu::*;
pub(crate) use path::*;

#[cfg(test)]
mod tests;
