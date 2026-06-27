use std::sync::{Arc, atomic::AtomicBool, mpsc::Receiver};

use crate::engines::docking::{DockingOutcome, DockingRequest};
use crate::workflows::docking::{DockingProgress, run_docking_calculation};

/// A background molecular docking job the UI is polling. Like [`RunningQmJob`] the
/// Vina search is one opaque blocking call, so progress is a coarse stage label
/// and the worker delivers the ranked poses on `Finished`.
pub struct RunningDockingJob {
    pub cancel: Arc<AtomicBool>,
    pub receiver: Receiver<DockingWorkerMessage>,
}

pub enum DockingWorkerMessage {
    Progress { stage: String },
    Finished(Box<DockingOutcome>),
    Failed(String),
}

/// Spawn a molecular docking search on a worker thread and return the live handle.
/// The worker streams coarse stage updates, then a `Finished` outcome (ranked
/// poses) or `Failed` error. Caller stores the handle in [`JobManager`]. The Vina
/// search is one opaque blocking call, so cancel is best-effort (honored before
/// the search begins; an in-flight search runs to completion and is discarded).
pub fn spawn_docking_job(request: DockingRequest) -> RunningDockingJob {
    let (sender, receiver) = std::sync::mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_for_worker = Arc::clone(&cancel);

    std::thread::spawn(move || {
        let progress_sender = sender.clone();
        let result = run_docking_calculation(
            request,
            cancel_for_worker,
            move |DockingProgress { stage }| {
                let _ = progress_sender.send(DockingWorkerMessage::Progress { stage });
            },
        );
        match result {
            Ok(result) => {
                let _ = sender.send(DockingWorkerMessage::Finished(Box::new(result.outcome)));
            }
            Err(error) => {
                let _ = sender.send(DockingWorkerMessage::Failed(error.to_string()));
            }
        }
    });

    RunningDockingJob { cancel, receiver }
}
