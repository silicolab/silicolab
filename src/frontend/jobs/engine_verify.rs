//! Verifying one engine on one compute target, off the UI thread.
//!
//! Both arms block — SSH for a remote host, a WSL cold start for the local one —
//! so neither may run inside `dispatch`. The target is cloned into the worker
//! because the user can keep editing settings while the probe is in flight.

use std::sync::mpsc::Receiver;

use crate::backend::config::{ComputeTarget, RemoteHost};
use crate::backend::engine_launch::{LaunchTarget, VerifyOutcome, verify_engine};
use crate::engines::registry::{EngineId, EngineLaunch, EngineLaunches};

/// An owned compute target, detached from `AppState` so the worker can hold it.
pub enum VerifyTarget {
    Local(EngineLaunches),
    Remote(Box<RemoteHost>),
}

/// An in-flight engine verification. The dispatcher drains it each frame, like
/// [`RunningRemoteProbe`](super::RunningRemoteProbe).
pub struct RunningEngineVerify {
    pub target: ComputeTarget,
    pub engine: EngineId,
    pub checked_launch: Option<EngineLaunch>,
    pub receiver: Receiver<VerifyOutcome>,
}

/// Run `engine`'s verification against `target` on a worker thread.
pub fn spawn_engine_verify(
    target: ComputeTarget,
    launches: VerifyTarget,
    engine: EngineId,
) -> RunningEngineVerify {
    let checked_launch = match &launches {
        VerifyTarget::Local(overrides) => overrides.get(engine),
        VerifyTarget::Remote(host) => host.engines.get(engine),
    }
    .cloned();
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let borrowed = match &launches {
            VerifyTarget::Local(overrides) => LaunchTarget::Local(overrides),
            VerifyTarget::Remote(host) => LaunchTarget::Remote(host),
        };
        let outcome =
            verify_engine(borrowed, engine).unwrap_or_else(|error| VerifyOutcome::NotFound {
                reason: error.to_string(),
            });
        let _ = sender.send(outcome);
    });
    RunningEngineVerify {
        target,
        engine,
        checked_launch,
        receiver,
    }
}
