//! Compute engines, workflows, and the engine-job wire contract.
//!
//! This crate owns the pieces that run a calculation regardless of where it
//! runs: the `domain` types, IO parsing, the engines, the workflows that drive
//! them, the remote-host descriptor, and the serializable `payload`/wire bridge
//! that lets a typed job cross a process boundary. The GUI application and the
//! headless worker both link this crate, so engine logic is written once.
//!
//! Internal layering, lowest to highest: `domain` <- `io` <- `md` <- `engines`
//! <- `workflows`. `md` is the engine-neutral molecular-dynamics model (system
//! building, topology, solvation, force fields, the stage model) that both the
//! MD engines and the MD workflow build on. `hosts`, `launch`, and `payload` are
//! leaf utilities; `wire` sits at the top and ties an engine job to an executor.

pub mod domain;
pub mod engines;
pub mod hosts;
pub mod io;
pub mod launch;
pub mod md;
pub mod payload;
pub mod wire;
pub mod workflows;
