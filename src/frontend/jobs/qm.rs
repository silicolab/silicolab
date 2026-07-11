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
#[cfg_attr(not(test), allow(dead_code))]
pub fn spawn_qm_job(job: QmJob, threads: Option<usize>) -> RunningQmJob {
    spawn_qm_job_with_launches(
        job,
        threads,
        crate::engines::registry::EngineLaunches::new(),
    )
    .expect("built-in QM job does not require an external launch")
}

pub fn spawn_qm_job_with_launches(
    job: QmJob,
    threads: Option<usize>,
    launches: crate::engines::registry::EngineLaunches,
) -> anyhow::Result<RunningQmJob> {
    let executor = if cfg!(test) {
        Executor::InProcess
    } else {
        Executor::LocalSubprocess
    };
    let request = EngineRequest::new(Engine::Qm(job), threads, launches)?;
    let running = run_job(request, executor);
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

    Ok(RunningQmJob {
        cancel,
        receiver,
        latest_stage: None,
        cancel_requested: false,
    })
}
