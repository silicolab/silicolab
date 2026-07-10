use std::sync::mpsc::Receiver;

use crate::engines::qm::{QmJob, QmOutcome};
use crate::wire::{
    Engine, EngineOutcome, EngineRequest, Executor, JobCancelHandle, JobUpdate, run_job,
};

/// A background quantum-chemistry (hartree) job the UI is polling.
pub struct RunningQmJob {
    pub cancel: JobCancelHandle,
    pub receiver: Receiver<QmWorkerMessage>,
    pub latest_stage: Option<String>,
    pub cancel_requested: bool,
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
    let executor = if cfg!(test) {
        Executor::InProcess
    } else {
        Executor::LocalSubprocess
    };
    // hartree is built in: a QM job needs no external launch.
    let running = run_job(EngineRequest::builtin(Engine::Qm(job), threads), executor);
    // Production runs QM in a subprocess so Cancel can kill an opaque hartree
    // calculation without requiring in-process preemption hooks. Tests keep the
    // in-process path because Rust's test harness is not the self-exec worker.
    let cancel = running.cancel_handle();
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

    RunningQmJob {
        cancel,
        receiver,
        latest_stage: None,
        cancel_requested: false,
    }
}
