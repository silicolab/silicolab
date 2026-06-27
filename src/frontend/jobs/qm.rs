use std::sync::{Arc, atomic::AtomicBool, mpsc::Receiver};

use crate::engines::qm::{QmJob, QmOutcome};
use crate::wire::{Engine, EngineOutcome, EngineRequest, Executor, JobUpdate, run_job};

/// A background quantum-chemistry (hartree) job the UI is polling.
pub struct RunningQmJob {
    pub cancel: Arc<AtomicBool>,
    pub receiver: Receiver<QmWorkerMessage>,
}

pub enum QmWorkerMessage {
    Progress { stage: String },
    Finished(Box<QmOutcome>),
    Failed(String),
}

/// Spawn a quantum-chemistry calculation (molecular or periodic) on a worker
/// thread and return the live handle. The worker streams coarse stage updates,
/// then a `Finished` outcome or `Failed` error. Caller stores the handle in
/// [`JobManager`].
pub fn spawn_qm_job(job: QmJob, threads: Option<usize>) -> RunningQmJob {
    let running = run_job(
        EngineRequest::with_cores(Engine::Qm(job), threads),
        Executor::InProcess,
    );
    // The QM job rides the shared run handle; adapt its updates to the message the
    // task UI already polls. An in-process job cancels through the shared flag.
    let cancel = running
        .cancel_flag()
        .expect("an in-process job cancels via the cooperative flag");
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        while let Ok(update) = running.updates().recv() {
            let message = match update {
                JobUpdate::Progress { stage } => QmWorkerMessage::Progress { stage },
                JobUpdate::Finished(outcome) => match *outcome {
                    EngineOutcome::Qm(outcome) => QmWorkerMessage::Finished(Box::new(outcome)),
                    // This relay only ever drives a QM request, so a non-QM outcome
                    // is an internal contract break rather than a user-facing error.
                    _ => QmWorkerMessage::Failed("QM job returned a non-QM outcome".to_string()),
                },
                JobUpdate::Failed(error) => QmWorkerMessage::Failed(error),
            };
            if sender.send(message).is_err() {
                break;
            }
        }
    });

    RunningQmJob { cancel, receiver }
}
