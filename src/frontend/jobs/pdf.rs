use std::ops::RangeInclusive;
use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};

use crate::io::pdf::{self, PdfError};

pub struct PdfReadRequest {
    pub path: PathBuf,
    pub pages: Option<RangeInclusive<u32>>,
    pub query: Option<String>,
    pub budget_chars: usize,
}

pub struct RunningPdfReadJob {
    pub receiver: mpsc::Receiver<Result<String, PdfError>>,
    cancel: Arc<AtomicBool>,
}

impl RunningPdfReadJob {
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

pub struct TrackedAgentPdfJob {
    pub id: u64,
    pub conversation: crate::frontend::agent::AssistantConversationId,
    pub label: String,
    pub running: RunningPdfReadJob,
}

pub fn spawn_pdf_read(request: PdfReadRequest) -> RunningPdfReadJob {
    let (sender, receiver) = mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let worker_cancel = Arc::clone(&cancel);
    std::thread::spawn(move || {
        let result = match &request.query {
            Some(query) => pdf::search(&request.path, query, &worker_cancel)
                .map(|(info, hits)| pdf::render_hits(&info, query, &hits)),
            None => pdf::extract_pages(&request.path, request.pages.clone(), &worker_cancel)
                .map(|(info, pages)| pdf::render_paged(&info, &pages, request.budget_chars)),
        };
        // The receiver is gone when the conversation was stopped or deleted.
        let _ = sender.send(result);
    });
    RunningPdfReadJob { receiver, cancel }
}
