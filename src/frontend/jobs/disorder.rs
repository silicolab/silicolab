use std::sync::{Arc, atomic::AtomicBool, mpsc::Receiver};

use crate::domain::Structure;
use crate::workflows::packing::{PackProgress, PackReport, PackRequest, pack};

/// A background "Build Disordered System" packing job the UI is
/// polling. Mirrors [`RunningOptimization`]: the worker streams intermediate
/// structures into the viewport, then a `Finished` result or `Failed` error.
pub struct RunningDisorderJob {
    pub cancel: Arc<AtomicBool>,
    pub receiver: Receiver<DisorderWorkerMessage>,
    pub latest_report: Option<PackReport>,
    /// The entry the packing streams into (created up front by the dispatcher so
    /// the in-progress structure is visible without touching the source entry).
    pub result_entry_id: u64,
}

pub enum DisorderWorkerMessage {
    Progress {
        structure: Structure,
        report: PackReport,
    },
    Finished {
        structure: Structure,
        report: PackReport,
    },
    Failed(String),
}

/// Spawn a Build Disordered System packing job on a worker
/// thread and return the live handle. Mirrors [`spawn_optimization_job`]: the
/// worker streams intermediate structures into the viewport, then a `Finished`
/// result or `Failed` error. Caller stores the handle in [`JobManager`].
pub fn spawn_disorder_job(request: PackRequest) -> RunningDisorderJob {
    let (sender, receiver) = std::sync::mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_for_worker = Arc::clone(&cancel);

    std::thread::spawn(move || {
        let result = pack(
            request,
            cancel_for_worker,
            |PackProgress { structure, report }| {
                sender
                    .send(DisorderWorkerMessage::Progress { structure, report })
                    .map_err(|error| anyhow::anyhow!(error.to_string()))
            },
        );
        match result {
            Ok(result) => {
                let _ = sender.send(DisorderWorkerMessage::Finished {
                    structure: result.structure,
                    report: result.report,
                });
            }
            Err(error) => {
                let _ = sender.send(DisorderWorkerMessage::Failed(error.to_string()));
            }
        }
    });

    RunningDisorderJob {
        cancel,
        receiver,
        latest_report: None,
        result_entry_id: 0,
    }
}
