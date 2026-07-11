//! The uniform lifecycle the five local compute pollers share. A
//! [`JobRuntime`] adapter only *collects* its worker events and applies its own
//! outcome; [`drive`] owns the generic lifecycle — resolving attribution by
//! `JobId`, the cancel-state transition, execution-state persistence, the
//! `ChannelLost → Interrupted` invariant, and the debounced autosave — so those
//! concerns live in one place.

use super::super::*;

use crate::frontend::jobs::LocalJobSlot;
use crate::job::{CancelSignal, ExecutionState, JobId};

/// Attribution the driver resolves once per frame from the slot's launch binding
/// and hands to the adapter for materialization.
pub(crate) struct JobContext {
    pub job_id: Option<JobId>,
    pub task_run_id: Option<u64>,
}

/// The lifecycle result of draining one runtime for a frame.
pub(crate) enum JobPoll {
    /// Still running: the driver puts the handle back and schedules the next poll.
    Running,
    /// Reached a terminal state; the adapter has already applied its outcome. The
    /// driver records the terminal execution/task status and drops the binding.
    Terminal(TaskStatus),
    /// The worker channel disconnected without a terminal message (local
    /// `ChannelLost`): the driver finalizes the execution as `Interrupted`.
    ChannelLost,
}

/// One background compute job behind a uniform lifecycle. The adapter only
/// collects and applies its own events; the driver ([`drive`]) owns attribution,
/// the cancel-state transition, execution-state persistence, and the invariants.
pub(crate) trait JobRuntime {
    fn slot(&self) -> LocalJobSlot;

    /// Signal the worker to cancel (set the flag / kill the subprocess) and report
    /// what the request achieved. The single cancel entry point; the driver applies
    /// the resulting state transition.
    fn request_cancel(&mut self, state: &mut AppState) -> CancelSignal;

    /// Drain the worker channel, applying each event to `state`, and report the
    /// lifecycle result. A terminal event applies the outcome (entries, ledger)
    /// here; recording the terminal execution/task state is the driver's job.
    /// Everything frame-scoped (repaint, cadence, autosave) is the driver's, so an
    /// adapter never touches the egui context.
    fn poll(&mut self, state: &mut AppState, cx: &JobContext) -> JobPoll;
}

/// Poll the five local compute jobs behind the runtime driver, once per frame.
/// Each thin poller takes its slot's handle (if present), drives one frame, and
/// puts it back while still running — so this is the whole traversal over the
/// local runtime adapters, in place of five hand-written poll bodies.
pub(crate) fn poll_compute_jobs(state: &mut AppState, ctx: &egui::Context) {
    poll_engine_job(state, ctx);
    poll_optimization_job(state, ctx);
    poll_disorder_job(state, ctx);
    poll_qm_job(state, ctx);
    poll_docking_job(state, ctx);
}

/// Drive one taken runtime for a frame: resolve attribution, honour an Esc-cancel,
/// drain events, and finalize. Returns the runtime to put back while it is still
/// running, or `None` once it is terminal — so a thin per-slot poller is just
/// `take_x()` → `drive` → `set_x` on the returned handle.
pub(crate) fn drive<R: JobRuntime>(
    state: &mut AppState,
    ctx: &egui::Context,
    mut runtime: R,
) -> Option<R> {
    let slot = runtime.slot();
    let job_id = state.jobs.local_execution(slot);
    let task_run_id = job_id.and_then(|id| state.tasks.runs.task_run_id_for_job(&id.to_string()));
    let cx = JobContext {
        job_id,
        task_run_id,
    };

    if ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
        let signal = runtime.request_cancel(state);
        apply_cancel_signal(state, &cx, signal);
    }

    let fingerprint_before = state.entries_fingerprint();
    match runtime.poll(state, &cx) {
        JobPoll::Running => {
            request_next_optimization_poll(ctx);
            Some(runtime)
        }
        JobPoll::Terminal(status) => {
            finalize(state, ctx, slot, job_id, status, fingerprint_before);
            None
        }
        JobPoll::ChannelLost => {
            finalize(
                state,
                ctx,
                slot,
                job_id,
                TaskStatus::Interrupted,
                fingerprint_before,
            );
            None
        }
    }
}

/// Apply a cancel request's outcome: `Accepted` moves the execution and its
/// task to `Cancelling` (non-terminal); the others leave the state as-is, so a job
/// the runtime cannot stop is never falsely shown as cancelling.
fn apply_cancel_signal(state: &mut AppState, cx: &JobContext, signal: CancelSignal) {
    if signal != CancelSignal::Accepted {
        return;
    }
    if let Some(job_id) = cx.job_id {
        let now = crate::backend::storage::jobs::now_ms().max(0) as u64;
        state
            .tasks
            .runs
            .set_execution_state(&job_id.to_string(), ExecutionState::Cancelling, now);
    }
    if let Some(task_run_id) = cx.task_run_id {
        mark_task_status(state, task_run_id, TaskStatus::Cancelling);
    }
}

/// Record a finished local job's terminal state, drop its slot binding, and
/// persist any entry change once. Reuses [`complete_local_job`], which walks
/// `JobId → RunAttempt → TaskRun`, enforces terminal-irreversibility, and
/// clears the ambient active run only when it is the completed one.
fn finalize(
    state: &mut AppState,
    ctx: &egui::Context,
    slot: LocalJobSlot,
    job_id: Option<JobId>,
    status: TaskStatus,
    fingerprint_before: u64,
) {
    complete_local_job(state, job_id, status);
    state.jobs.take_local_execution(slot);
    if state.entries_fingerprint() != fingerprint_before {
        let now = ctx.input(|input| input.time);
        state.request_autosave(now, AUTOSAVE_DEBOUNCE_SECS);
    }
    ctx.request_repaint();
}
