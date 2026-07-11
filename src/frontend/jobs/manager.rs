use crate::frontend::remote_jobs::{
    RunningRemoteCancel, RunningRemoteCleanup, RunningRemoteJobsRefresh, RunningRemoteSubmit,
};

use std::collections::HashMap;

use crate::job::JobId;

use super::agent::{RunningAgentTurn, TrackedAgentJob};
use super::control::LocalJobSlot;
use super::disorder::RunningDisorderJob;
use super::docking::RunningDockingJob;
use super::engine::RunningEngineJob;
use super::engine_verify::RunningEngineVerify;
use super::metrics::RunningMetricsSampler;
use super::optimization::RunningOptimization;
use super::qm::RunningQmJob;
use super::remote::{RunningRemoteGpuMonitor, RunningRemoteHardwareFetch, RunningRemoteProbe};
use super::update::{RunningModelFetch, RunningSelfUpdate, RunningUpdateCheck};

#[derive(Default)]
pub struct JobManager {
    pub optimizer: Option<RunningOptimization>,
    /// In-flight Build Disordered System (packing) job.
    pub disorder: Option<RunningDisorderJob>,
    pub qm: Option<RunningQmJob>,
    /// In-flight molecular docking (Vina) search.
    pub docking: Option<RunningDockingJob>,
    pub engine: Option<RunningEngineJob>,
    /// In-flight background decode of an entry's trajectory file for playback.
    pub trajectory_load: Option<crate::frontend::trajectory::RunningTrajectoryLoad>,
    /// In-flight check of GitHub Releases for a newer version (startup, or the
    /// moment the setting is switched on).
    pub update_check: Option<RunningUpdateCheck>,
    /// In-flight one-click self-update (download + replace the executable),
    /// started when the user clicks the update badge.
    pub self_update: Option<RunningSelfUpdate>,
    /// In-flight Remote Hosts settings probe (passwordless check / Slurm).
    pub remote_probe: Option<RunningRemoteProbe>,
    /// In-flight engine verification for one compute target (Settings ▸ Compute).
    pub engine_verify: Option<RunningEngineVerify>,
    /// In-flight remote hardware inventory probe (Settings ▸ Hardware ▸ Remote).
    pub remote_hardware: Option<RunningRemoteHardwareFetch>,
    /// In-flight assistant model turn (one `provider.complete()` POST). One
    /// `RunningAgentTurn` == one model turn; the agent loop drives the next.
    pub agent: Option<RunningAgentTurn>,
    /// Detached background heavy jobs (qm/md/dock) the agent launched. The agent
    /// does not block on them; `poll_agent_jobs` drains completions and wakes the
    /// model through the queue.
    pub agent_jobs: Vec<TrackedAgentJob>,
    /// Monotonic id source for `agent_jobs`.
    pub next_agent_job_id: u64,
    /// In-flight live model-list fetch for the active provider's `/models`
    /// endpoint, started by the "Refresh models" button in settings.
    pub model_fetch: Option<RunningModelFetch>,
    /// Live CPU/GPU utilization sampler, running while `show_utilization_bars` is
    /// on. Dropping this handle stops the background thread.
    pub metrics: Option<RunningMetricsSampler>,
    /// Live remote-GPU sampler (Settings ▸ Hardware ▸ Remote host ▸ Live GPU).
    /// Dropping or `cancel()`-ing this handle ends the background SSH polling.
    pub remote_gpu_monitor: Option<RunningRemoteGpuMonitor>,
    /// In-flight off-thread submission of a detached remote job (deploy + stage +
    /// launch), for any engine. Drained into the `jobs.db` registry on completion.
    pub remote_submit: Option<RunningRemoteSubmit>,
    /// In-flight off-thread refresh of the detached remote jobs (liveness probe +
    /// outcome retrieval). Opt-in, never an automatic loop.
    pub remote_jobs_refresh: Option<RunningRemoteJobsRefresh>,
    pub remote_cancel: Option<RunningRemoteCancel>,
    pub remote_cleanup: Option<RunningRemoteCleanup>,
    /// Whether the registry snapshot has been loaded into the UI this session
    /// (a one-shot reconnect read on the first frame).
    pub remote_jobs_loaded: bool,
    /// The `JobId` of the live job in each local slot, bound at launch. A poller
    /// attributes its completion through this — resolving `JobId → RunAttempt →
    /// TaskRun` — instead of the ambient active task run, so two concurrent local
    /// jobs of different kinds never cross attribution.
    local_executions: HashMap<LocalJobSlot, JobId>,
}

impl JobManager {
    pub fn optimization_running(&self) -> bool {
        self.optimizer.is_some()
    }

    pub fn take_optimizer(&mut self) -> Option<RunningOptimization> {
        self.optimizer.take()
    }

    pub fn set_optimizer(&mut self, optimizer: RunningOptimization) {
        self.optimizer = Some(optimizer);
    }

    pub fn disorder_running(&self) -> bool {
        self.disorder.is_some()
    }

    pub fn take_disorder(&mut self) -> Option<RunningDisorderJob> {
        self.disorder.take()
    }

    pub fn set_disorder(&mut self, disorder: RunningDisorderJob) {
        self.disorder = Some(disorder);
    }

    pub fn qm_running(&self) -> bool {
        self.qm.is_some()
    }

    pub fn take_qm(&mut self) -> Option<RunningQmJob> {
        self.qm.take()
    }

    pub fn set_qm(&mut self, qm: RunningQmJob) {
        self.qm = Some(qm);
    }

    pub fn docking_running(&self) -> bool {
        self.docking.is_some()
    }

    pub fn take_docking(&mut self) -> Option<RunningDockingJob> {
        self.docking.take()
    }

    pub fn set_docking(&mut self, docking: RunningDockingJob) {
        self.docking = Some(docking);
    }

    pub fn engine_running(&self) -> bool {
        self.engine.is_some()
    }

    pub fn take_engine(&mut self) -> Option<RunningEngineJob> {
        self.engine.take()
    }

    pub fn set_engine(&mut self, engine: RunningEngineJob) {
        self.engine = Some(engine);
    }

    /// Bind the `JobId` of the job just launched into a local slot.
    pub fn bind_local_execution(&mut self, slot: LocalJobSlot, job_id: JobId) {
        self.local_executions.insert(slot, job_id);
    }

    /// The `JobId` bound to a local slot, read by its poller to attribute progress
    /// and completion.
    pub fn local_execution(&self, slot: LocalJobSlot) -> Option<JobId> {
        self.local_executions.get(&slot).copied()
    }

    /// Remove and return a local slot's `JobId` once its job reaches a terminal
    /// state.
    pub fn take_local_execution(&mut self, slot: LocalJobSlot) -> Option<JobId> {
        self.local_executions.remove(&slot)
    }

    /// Drop every local-slot binding — on a workspace switch, where the tasks the
    /// bindings resolve to no longer exist.
    pub fn clear_local_executions(&mut self) {
        self.local_executions.clear();
    }
}
