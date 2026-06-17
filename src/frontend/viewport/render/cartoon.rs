//! Cartoon (ribbon) rendering for biopolymers, split by pipeline stage:
//! - [`path`]: Cα trace, secondary-structure resolution, spline smoothing and
//!   the swept-frame samples shared by both render paths.
//! - [`geometry`]: cross-section profile, world-space rings, and cartoon
//!   styling/shading.
//! - [`mesh_cpu`]: the CPU painter path (projected, back-face-culled,
//!   depth-sorted triangles plus silhouettes).
//! - [`mesh_gpu`]: the camera-independent world mesh for the GPU pipeline.
//! - [`depth`]: the low-resolution screen-space depth buffer used to occlude the
//!   surface wireframe.

mod depth;
mod geometry;
mod mesh_cpu;
mod mesh_gpu;
mod path;

pub(crate) use depth::*;
pub(crate) use geometry::*;
pub(crate) use mesh_cpu::*;
pub(crate) use mesh_gpu::*;
pub(crate) use path::*;

#[cfg(test)]
mod tests;
