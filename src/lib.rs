pub mod backend;
pub mod frontend;
pub mod plot;

// The compute stack (domain types, IO, engines, workflows, the host descriptor,
// and the serializable payload/wire bridge) lives in `compute-core`. Re-export it
// under the historical module paths so the rest of the app keeps using
// `crate::domain`, `crate::engines`, etc. unchanged.
pub use compute_core::{domain, engines, hosts, io, job, launch, payload, skills, wire, workflows};
