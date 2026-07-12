use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc::Receiver,
};

use super::docking::RunningDockingJob;
use super::engine::RunningEngineJob;
use super::qm::RunningQmJob;

/// An in-flight assistant model turn: one `provider.complete()` POST running on
/// a worker thread (network takes seconds-to-minutes, so it must be off the UI
/// thread). The driver drains the result in `poll_jobs` and runs any tool calls
/// back on the UI thread. `cancel` is shared with the retry loop so Esc aborts
/// between attempts.
pub struct RunningAgentTurn {
    pub cancel: Arc<AtomicBool>,
    pub receiver: Receiver<AgentTurnEvent>,
}

/// What the agent-turn worker streams back: incremental text while generating,
/// then a terminal `Done` with the full turn (or a classified error).
pub enum AgentTurnEvent {
    TextDelta(String),
    Done(Result<crate::io::llm::types::AssistantTurn, crate::io::llm::types::LlmError>),
}

/// A heavy compute job (MD or QM) the agent kicked off and is awaiting. Owned in
/// a slot separate from the Tasks-system `engine`/`qm` jobs so the agent captures
/// the raw result without interfering with task completion.
pub enum AgentHeavyJob {
    Qm(RunningQmJob),
    Engine(RunningEngineJob),
    Docking(RunningDockingJob),
}

impl AgentHeavyJob {
    /// Signal the worker to stop at its next cancellation checkpoint.
    pub fn cancel(&self) {
        match self {
            AgentHeavyJob::Qm(job) => job.cancel.cancel(),
            AgentHeavyJob::Engine(job) => job.cancel.store(true, Ordering::Relaxed),
            AgentHeavyJob::Docking(job) => job.cancel.store(true, Ordering::Relaxed),
        }
    }
}

/// A detached background heavy job the agent launched, tagged so its completion
/// routes back to the conversation that started it. The agent keeps running
/// while this computes; `poll_agent_jobs` drains it and wakes the model.
pub struct TrackedAgentJob {
    pub id: u64,
    pub conversation: crate::frontend::agent::AssistantConversationId,
    /// Short human label, e.g. "qm optimize".
    pub label: String,
    /// The unified `TaskRun` this worker reports into, so the run shows in the Task Monitor.
    pub task_run_id: u64,
    /// The bound execution identity — minted at launch exactly as a manually
    /// submitted job's is, so an assistant-launched run is first-class: its logs
    /// are Job-scoped and its lifecycle resolves through the same run graph. The
    /// assistant is only the launcher, recorded by `conversation`/`label`.
    pub job_id: crate::job::JobId,
    pub job: AgentHeavyJob,
}

/// Spawn one model turn on a worker thread and return the polling handle. The
/// blocking transport + bounded retry live entirely in `io/llm`; the worker
/// forwards streamed text deltas and then the terminal
/// [`AssistantTurn`](crate::io::llm::types::AssistantTurn) (or a classified error).
pub fn spawn_agent_turn(
    provider: Box<dyn crate::io::llm::provider::LlmProvider>,
    cfg: crate::io::llm::types::LlmConfig,
    tools: Vec<crate::io::llm::types::ToolDef>,
    history: Vec<crate::io::llm::types::ChatMessage>,
) -> RunningAgentTurn {
    use crate::io::llm::types::StreamEvent;
    let (sender, receiver) = std::sync::mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_for_worker = Arc::clone(&cancel);

    std::thread::spawn(move || {
        let delta_sender = sender.clone();
        let mut on_event = move |event: StreamEvent| {
            if let StreamEvent::TextDelta(text) = event {
                let _ = delta_sender.send(AgentTurnEvent::TextDelta(text));
            }
        };
        let result = crate::io::llm::retry::complete_with_retry(
            provider.as_ref(),
            &cfg,
            &tools,
            &history,
            &cancel_for_worker,
            &mut on_event,
        );
        let _ = sender.send(AgentTurnEvent::Done(result));
    });

    RunningAgentTurn { cancel, receiver }
}
