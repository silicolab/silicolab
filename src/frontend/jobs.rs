use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::Receiver,
    },
    time::Duration,
};

use eframe::egui;
use serde_json::Value;

use crate::{
    domain::Structure,
    engines::{
        docking::{DockingOutcome, DockingRequest},
        forcefield::{OptimizationOptions, OptimizationReport},
        gromacs::{
            BuildRequest, GromacsProgress, MaterialBuildRequest, StageResult, StageSpec,
            TopologySource, build_material_system, build_system, prepare_system, run_pipeline,
        },
        qm::{QmJob, QmOutcome},
        remote::Compute,
    },
    frontend::md_support::{FrameworkRunMetadata, MD_FRAMEWORK_FILE, write_md_system_context},
    workflows::{
        docking::{DockingProgress, run_docking_calculation},
        optimization::{
            GeometryOptimizationProgress, GeometryOptimizationRequest, run_geometry_optimization,
        },
        packing::{PackProgress, PackReport, PackRequest, pack},
        qm::{QmCalculationProgress, run_qm_calculation},
    },
};

pub const OPTIMIZATION_POLL_FRAME: Duration = Duration::from_millis(50);

pub struct RunningOptimization {
    pub cancel: Arc<AtomicBool>,
    pub receiver: Receiver<OptimizationWorkerMessage>,
    pub latest_report: Option<OptimizationReport>,
}

pub enum OptimizationWorkerMessage {
    Progress {
        structure: Structure,
        report: OptimizationReport,
    },
    Finished {
        structure: Structure,
        report: OptimizationReport,
    },
    Failed(String),
}

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

/// Streaming messages produced by an external-engine worker.
pub enum EngineWorkerMessage {
    Stage(String),
    Log(String),
    Finished(Box<EngineSuccess>),
    Failed(String),
}

/// Aggregated information about a successful engine run that should be
/// surfaced to the UI / project state.
#[allow(dead_code)]
pub struct EngineSuccess {
    pub engine: &'static str,
    pub job_kind: &'static str,
    pub structure: Structure,
    pub summary: String,
    pub working_dir: PathBuf,
    /// Trajectory file produced by the run (the production stage's `.xtc`), if
    /// any. Used to mark the resulting entry as an MD-run output that can be
    /// played back; `None` for build jobs.
    pub trajectory: Option<PathBuf>,
}

pub struct GromacsPipelineRequest {
    pub structure: Structure,
    pub topology: TopologySource,
    pub stages: Vec<StageSpec>,
    pub working_dir: PathBuf,
    /// How to launch `gmx` and where it runs (local or remote over SSH).
    pub compute: Compute,
    pub max_duration_per_stage: Duration,
    /// Atoms to freeze (a rigid framework's sheet); `None` for an ordinary run.
    pub freeze: Option<crate::engines::gromacs::FreezeSelection>,
}

/// A background engine job that the UI is currently polling.
pub struct RunningEngineJob {
    pub engine: &'static str,
    pub job_kind: &'static str,
    pub cancel: Arc<AtomicBool>,
    pub receiver: Receiver<EngineWorkerMessage>,
    pub latest_stage: Option<String>,
    pub log_tail: Vec<String>,
}

impl RunningEngineJob {
    pub fn append_log(&mut self, line: String) {
        self.log_tail.push(line);
        if self.log_tail.len() > 200 {
            let drop = self.log_tail.len() - 200;
            self.log_tail.drain(0..drop);
        }
    }
}

/// What a Remote Hosts settings probe is checking on a host.
#[derive(Debug, Clone, Copy)]
pub enum RemoteProbeKind {
    /// Whether passwordless key login already works.
    Passwordless,
    /// Detect a GROMACS executable + version on the host.
    DetectGromacs,
}

/// Result of a remote-host probe (sent back from the worker thread).
pub enum RemoteProbeOutcome {
    Passwordless(bool),
    /// `(program, version)` when GROMACS was found, else `None`.
    Detected(Option<(String, String)>),
}

/// An in-flight Remote Hosts probe. Runs the blocking `ssh` off the UI thread so
/// a slow or dead host never freezes rendering; the dispatcher drains it each
/// frame (like [`RunningUpdateCheck`]).
pub struct RunningRemoteProbe {
    pub host_id: String,
    pub receiver: Receiver<RemoteProbeOutcome>,
}

/// Spawn a remote-host probe on a worker thread. The host is cloned in; only its
/// connection fields matter (the probe uses a throwaway run anchor).
pub fn spawn_remote_probe(
    host: crate::backend::config::RemoteHost,
    kind: RemoteProbeKind,
) -> RunningRemoteProbe {
    use crate::engines::remote;
    let (sender, receiver) = std::sync::mpsc::channel();
    let host_id = host.id.clone();
    std::thread::spawn(move || {
        let target = remote::RemoteTarget::for_run(&host, "probe");
        let outcome = match kind {
            RemoteProbeKind::Passwordless => {
                RemoteProbeOutcome::Passwordless(remote::check_passwordless(&target))
            }
            RemoteProbeKind::DetectGromacs => RemoteProbeOutcome::Detected(
                remote::detect_remote_engine(&target, remote::GMX_REMOTE_CANDIDATES),
            ),
        };
        let _ = sender.send(outcome);
    });
    RunningRemoteProbe { host_id, receiver }
}

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
            AgentHeavyJob::Qm(job) => job.cancel.store(true, Ordering::Relaxed),
            AgentHeavyJob::Engine(job) => job.cancel.store(true, Ordering::Relaxed),
            AgentHeavyJob::Docking(job) => job.cancel.store(true, Ordering::Relaxed),
        }
    }
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

/// The once-per-launch background query of GitHub Releases. No cancel flag:
/// the single HTTP request either answers or times out on its own, and the
/// result is ignored if the handle was dropped.
pub struct RunningUpdateCheck {
    pub receiver: Receiver<anyhow::Result<Option<crate::io::update_check::AvailableUpdate>>>,
}

/// Spawn the update check on a worker thread and return the polling handle.
pub fn spawn_update_check() -> RunningUpdateCheck {
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = sender.send(crate::io::update_check::check_for_update());
    });
    RunningUpdateCheck { receiver }
}

/// An in-flight one-click self-update: the worker downloads the matching
/// release asset and replaces the running executable, then sends the installed
/// version (or the failure). Like [`RunningUpdateCheck`] there is no cancel —
/// the replace is a single blocking operation and the result is ignored if the
/// handle was dropped.
pub struct RunningSelfUpdate {
    pub receiver: Receiver<anyhow::Result<String>>,
}

/// Spawn the download-and-replace on a worker thread and return the polling
/// handle. The blocking work lives entirely in [`crate::io::self_update`].
pub fn spawn_self_update() -> RunningSelfUpdate {
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = sender.send(crate::io::self_update::perform_update());
    });
    RunningSelfUpdate { receiver }
}

/// Model-list fetch tuning. The list is tiny, so a tight cap and a short
/// timeout keep a slow or wrong endpoint from hanging the Refresh button.
const MODEL_FETCH_TIMEOUT: Duration = Duration::from_secs(30);
const MODEL_FETCH_MAX_BYTES: u64 = 4 * 1024 * 1024;

/// An in-flight live model-list fetch for one provider's `/models` endpoint.
/// Like [`RunningUpdateCheck`] there is no cancel flag: it is a single bounded
/// HTTP request that answers or times out on its own, and a late result lands on
/// a closed channel if the handle was dropped. `provider_id` tags which provider
/// the resulting ids belong to so a stale answer for a since-switched provider
/// can be ignored.
pub struct RunningModelFetch {
    pub provider_id: String,
    pub receiver: Receiver<Result<Vec<String>, String>>,
}

/// Spawn a one-off `/models` query on a worker thread and return the polling
/// handle. The blocking HTTP lives here (network takes a moment); the driver
/// drains it in `poll_model_fetch`. OpenAI-compatible providers (incl. Gemini)
/// read `GET {base_url}/models` with a Bearer token; native Anthropic reads
/// `GET https://api.anthropic.com/v1/models` with `x-api-key` +
/// `anthropic-version`. Both list ids under `data[].id`.
pub fn spawn_model_fetch(
    provider_id: String,
    kind: crate::frontend::agent::registry::ProviderKind,
    base_url: String,
    api_key: String,
) -> RunningModelFetch {
    let (sender, receiver) = std::sync::mpsc::channel();
    let handle_id = provider_id.clone();
    std::thread::spawn(move || {
        let _ = sender.send(fetch_model_ids(kind, &base_url, &api_key));
    });
    RunningModelFetch {
        provider_id: handle_id,
        receiver,
    }
}

/// Blocking `/models` GET shared by both transports. Returns the parsed ids, or
/// a short user-facing error string on a transport / HTTP / parse failure.
fn fetch_model_ids(
    kind: crate::frontend::agent::registry::ProviderKind,
    base_url: &str,
    api_key: &str,
) -> Result<Vec<String>, String> {
    use crate::frontend::agent::registry::ProviderKind;

    let config = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(MODEL_FETCH_TIMEOUT))
        .build();
    let agent = ureq::Agent::new_with_config(config);

    let response = match kind {
        ProviderKind::OpenAiCompat => agent
            .get(format!("{}/models", base_url.trim_end_matches('/')))
            .header("authorization", &format!("Bearer {api_key}"))
            .call(),
        // The Anthropic models list lives at the fixed API root; its version
        // header matches the completions adapter (`anthropic.rs`).
        ProviderKind::Native => agent
            .get("https://api.anthropic.com/v1/models")
            .header("x-api-key", api_key)
            .header("anthropic-version", "2023-06-01")
            .call(),
    };

    let mut response = response.map_err(|error| error.to_string())?;
    let status = response.status().as_u16();
    let text = response
        .body_mut()
        .with_config()
        .limit(MODEL_FETCH_MAX_BYTES)
        .read_to_string()
        .map_err(|error| error.to_string())?;
    interpret_models_response(status, &text)
}

/// Turn a `/models` HTTP response into model ids, or a readable error. A
/// non-JSON body (HTML error page, empty, a relay's SPA index) almost always
/// means the Base URL is wrong, so it gets the same "points at the API root
/// (… /v1)" hint the completion path gives — regardless of status, since a wrong
/// URL can 404 to a page as readily as 200 to one. A valid JSON body with a
/// non-200 status is a real API error, surfaced as the status.
fn interpret_models_response(status: u16, body: &str) -> Result<Vec<String>, String> {
    let Ok(json) = serde_json::from_str::<Value>(body) else {
        return Err(crate::io::llm::openai_compat::non_json_response_message(
            body,
        ));
    };
    if status != 200 {
        let message = crate::io::llm::openai_compat::extract_error_message(body);
        if message.trim().is_empty() {
            return Err(format!("provider returned HTTP {status}"));
        }
        return Err(format!("provider returned HTTP {status}: {message}"));
    }
    Ok(parse_model_ids(&json))
}

/// Extract model ids from a `/models` response. Both the OpenAI-compatible and
/// Anthropic list endpoints return `{"data":[{"id":"…"}, …]}`; anything that
/// doesn't match yields an empty list (the caller keeps its static models).
pub fn parse_model_ids(json: &Value) -> Vec<String> {
    json.get("data")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.get("id").and_then(Value::as_str))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// One utilization sample: global CPU load plus a live per-GPU snapshot (one
/// entry per GPU the sampler could read). `gpus` is empty when no live backend
/// is available — the common case, since live GPU stats need the optional NVML
/// feature; the gauges then read N/A.
pub struct Metrics {
    pub cpu_pct: f32,
    pub gpus: Vec<crate::frontend::gpu_monitor::GpuSample>,
}

/// Handle to the live utilization sampler. Dropping it ends the thread: the next
/// `send` fails once the receiver is gone, and the loop exits.
pub struct RunningMetricsSampler {
    pub receiver: std::sync::mpsc::Receiver<Metrics>,
}

/// Spawn a ~500 ms CPU/GPU sampler. The first CPU reading is meaningless (needs
/// two refreshes >= MINIMUM_CPU_UPDATE_INTERVAL apart), so we prime once before
/// the loop. The GPU probe is built once on the sampler thread (NVML init is not
/// cheap) and reused each tick.
pub fn spawn_metrics_sampler() -> RunningMetricsSampler {
    use std::time::Duration;
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut sys = sysinfo::System::new();
        sys.refresh_cpu_usage();
        let mut gpu_sampler = crate::frontend::gpu_monitor::GpuSampler::new();
        std::thread::sleep(Duration::from_millis(500));
        loop {
            sys.refresh_cpu_usage();
            let sample = Metrics {
                cpu_pct: sys.global_cpu_usage(),
                gpus: gpu_sampler.sample(),
            };
            if sender.send(sample).is_err() {
                break; // receiver dropped (toggle turned off / app closing)
            }
            std::thread::sleep(Duration::from_millis(500));
        }
    });
    RunningMetricsSampler { receiver }
}

/// Start or stop the live metrics sampler on `jobs` to match `on`. Idempotent:
/// turning on when already running does not spawn a second sampler. Separated
/// from the settings handler so the lifecycle is testable without touching disk.
pub(crate) fn apply_metrics_sampler(jobs: &mut JobManager, on: bool) {
    if on {
        if jobs.metrics.is_none() {
            jobs.metrics = Some(spawn_metrics_sampler());
        }
    } else {
        jobs.metrics = None;
    }
}

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
    /// In-flight Remote Hosts settings probe (passwordless check / engine detect).
    pub remote_probe: Option<RunningRemoteProbe>,
    /// In-flight assistant model turn (one `provider.complete()` POST). One
    /// `RunningAgentTurn` == one model turn; the agent loop drives the next.
    pub agent: Option<RunningAgentTurn>,
    /// In-flight heavy compute job (md/qm) the agent is awaiting before it
    /// continues its turn.
    pub agent_heavy: Option<AgentHeavyJob>,
    /// In-flight live model-list fetch for the active provider's `/models`
    /// endpoint, started by the "Refresh models" button in settings.
    pub model_fetch: Option<RunningModelFetch>,
    /// Live CPU/GPU utilization sampler, running while `show_utilization_bars` is
    /// on. Dropping this handle stops the background thread.
    pub metrics: Option<RunningMetricsSampler>,
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

    pub fn cancel_optimization(&mut self) {
        if let Some(running) = self.optimizer.take() {
            running.cancel.store(true, Ordering::Relaxed);
        }
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

    pub fn cancel_disorder(&mut self) {
        if let Some(running) = self.disorder.take() {
            running.cancel.store(true, Ordering::Relaxed);
        }
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

    pub fn cancel_qm(&mut self) {
        if let Some(running) = self.qm.take() {
            running.cancel.store(true, Ordering::Relaxed);
        }
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

    pub fn cancel_docking(&mut self) {
        if let Some(running) = self.docking.take() {
            running.cancel.store(true, Ordering::Relaxed);
        }
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

    pub fn cancel_engine(&mut self) {
        if let Some(running) = self.engine.take() {
            running.cancel.store(true, Ordering::Relaxed);
        }
    }
}

pub fn spawn_optimization_job(
    structure: Structure,
    options: OptimizationOptions,
) -> anyhow::Result<RunningOptimization> {
    let (sender, receiver) = std::sync::mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_for_worker = Arc::clone(&cancel);

    std::thread::spawn(move || {
        let result = run_geometry_optimization(
            GeometryOptimizationRequest { structure, options },
            cancel_for_worker,
            |GeometryOptimizationProgress { structure, report }| {
                sender
                    .send(OptimizationWorkerMessage::Progress { structure, report })
                    .map_err(|error| anyhow::anyhow!(error.to_string()))
            },
        );
        match result {
            Ok(result) => {
                let _ = sender.send(OptimizationWorkerMessage::Finished {
                    structure: result.structure,
                    report: result.report,
                });
            }
            Err(error) => {
                let _ = sender.send(OptimizationWorkerMessage::Failed(error.to_string()));
            }
        }
    });

    Ok(RunningOptimization {
        cancel,
        receiver,
        latest_report: None,
    })
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

/// Spawn a quantum-chemistry calculation (molecular or periodic) on a worker
/// thread and return the live handle. The worker streams coarse stage updates,
/// then a `Finished` outcome or `Failed` error. Caller stores the handle in
/// [`JobManager`].
pub fn spawn_qm_job(job: QmJob, threads: Option<usize>) -> RunningQmJob {
    let (sender, receiver) = std::sync::mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_for_worker = Arc::clone(&cancel);

    std::thread::spawn(move || {
        let progress_sender = sender.clone();
        let result = run_qm_calculation(
            job,
            threads,
            cancel_for_worker,
            move |QmCalculationProgress { stage }| {
                let _ = progress_sender.send(QmWorkerMessage::Progress { stage });
            },
        );
        match result {
            Ok(result) => {
                let _ = sender.send(QmWorkerMessage::Finished(Box::new(result.outcome)));
            }
            Err(error) => {
                let _ = sender.send(QmWorkerMessage::Failed(error.to_string()));
            }
        }
    });

    RunningQmJob { cancel, receiver }
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

pub fn optimization_finished_message(report: OptimizationReport) -> String {
    if report.timed_out {
        return format!(
            "forcefield optimization timed out: energy {:.3} -> {:.3} in {} steps",
            report.initial_energy, report.final_energy, report.steps
        );
    }
    if report.stopped {
        return format!(
            "forcefield optimization stopped: energy {:.3} -> {:.3} in {} steps",
            report.initial_energy, report.final_energy, report.steps
        );
    }

    format!(
        "forcefield optimized: energy {:.3} -> {:.3} in {} steps{}",
        report.initial_energy,
        report.final_energy,
        report.steps,
        if report.converged { " (converged)" } else { "" }
    )
}

pub fn request_next_optimization_poll(ctx: &egui::Context) {
    ctx.request_repaint_after(OPTIMIZATION_POLL_FRAME);
}

/// Spawn a multi-step GROMACS pipeline as a background engine job and return
/// the live handle. Caller is responsible for storing it in [`JobManager`].
pub fn spawn_gromacs_pipeline_job(request: GromacsPipelineRequest) -> RunningEngineJob {
    let (sender, receiver) = std::sync::mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_for_worker = Arc::clone(&cancel);

    std::thread::spawn(move || {
        let report_sender = sender.clone();
        if let Some(target) = request.compute.remote_target() {
            crate::engines::remote::write_run_record(target, &request.working_dir);
        }
        let system = prepare_system(crate::engines::gromacs::PrepareSystemRequest {
            structure: request.structure,
            topology: request.topology,
            working_dir: request.working_dir,
            freeze: request.freeze,
        });
        let outcome = system.and_then(|system| {
            run_pipeline(
                system,
                request.stages,
                request.compute,
                request.max_duration_per_stage,
                cancel_for_worker,
                move |progress| match progress {
                    GromacsProgress::Stage(stage) => {
                        let _ = report_sender.send(EngineWorkerMessage::Stage(stage));
                    }
                    GromacsProgress::Log(line) => {
                        let _ = report_sender.send(EngineWorkerMessage::Log(line));
                    }
                },
            )
        });

        match outcome {
            Ok(results) => {
                let _ = sender.send(EngineWorkerMessage::Finished(Box::new(
                    engine_success_from_gromacs_pipeline(results),
                )));
            }
            Err(error) => {
                let _ = sender.send(EngineWorkerMessage::Failed(error.to_string()));
            }
        }
    });

    RunningEngineJob {
        engine: "gromacs",
        job_kind: "run-md",
        cancel,
        receiver,
        latest_stage: None,
        log_tail: Vec::new(),
    }
}

/// Spawn the GROMACS system-build pipeline (pdb2gmx → editconf → solvate →
/// genion) as a background engine job. The build writes `topol.top` into
/// `request.working_dir` (the build task's run directory), which a later MD run
/// reuses as its force-field topology. Caller stores the handle in
/// [`JobManager`].
pub fn spawn_gromacs_build_job(request: BuildRequest) -> RunningEngineJob {
    let (sender, receiver) = std::sync::mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_for_worker = Arc::clone(&cancel);

    // Capture the build inputs the run-MD recommendation later inherits, before
    // `request` is consumed by the build. The solute (not the solvated output)
    // carries the residue metadata system-type detection reads.
    let force_field_token = request.force_field.clone();
    let water_token = request
        .solvate
        .then(|| request.water.db_token().to_string());
    let solute = request.structure.clone();

    std::thread::spawn(move || {
        let report_sender = sender.clone();
        if let Some(target) = request.compute.remote_target() {
            crate::engines::remote::write_run_record(target, &request.working_dir);
        }
        let outcome = build_system(request, cancel_for_worker, move |progress| match progress {
            GromacsProgress::Stage(stage) => {
                let _ = report_sender.send(EngineWorkerMessage::Stage(stage));
            }
            GromacsProgress::Log(line) => {
                let _ = report_sender.send(EngineWorkerMessage::Log(line));
            }
        });

        match outcome {
            Ok(outcome) => {
                // pdb2gmx writes posre.itp, giving the run a "solute" restraint
                // group; record it so restrained equilibration validates.
                let restraint_groups = if outcome.working_dir.join("posre.itp").exists() {
                    vec!["solute".to_string()]
                } else {
                    Vec::new()
                };
                // A successful build with genion neutralization is net-neutral; the
                // exact charge is not parsed back from topol.top here.
                write_md_system_context(
                    &outcome.working_dir,
                    &solute,
                    outcome.structure.atoms.len(),
                    &force_field_token,
                    water_token.as_deref(),
                    false,
                    0.0,
                    false,
                    restraint_groups,
                );
                let _ = sender.send(EngineWorkerMessage::Finished(Box::new(EngineSuccess {
                    engine: "gromacs",
                    job_kind: "build-md",
                    structure: outcome.structure,
                    summary: outcome.summary,
                    working_dir: outcome.working_dir,
                    trajectory: None,
                })));
            }
            Err(error) => {
                let _ = sender.send(EngineWorkerMessage::Failed(error.to_string()));
            }
        }
    });

    RunningEngineJob {
        engine: "gromacs",
        job_kind: "build-md",
        cancel,
        receiver,
        latest_stage: None,
        log_tail: Vec::new(),
    }
}

/// Spawn the framework (nanosheet) build as a background engine job: it
/// generates the topology directly from the structure's bonds and optionally
/// solvates, writing `topol.top` and `framework_run.json` into
/// `request.working_dir` so a later MD run reuses both. Reported as a `build-md`
/// success, so the same completion handling adds the boxed entry.
pub fn spawn_material_build_job(request: MaterialBuildRequest) -> RunningEngineJob {
    let (sender, receiver) = std::sync::mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_for_worker = Arc::clone(&cancel);

    // A framework is not a biomolecule: it has no biomolecular force-field
    // convention (token classifies to the generic family) and uses freeze groups
    // rather than position restraints. Capture the solvent model and solute before
    // the request is consumed.
    let water_token = request
        .solvation
        .as_ref()
        .map(|solvation| solvation.water.db_token().to_string());
    let solute = request.structure.clone();

    std::thread::spawn(move || {
        let report_sender = sender.clone();
        if let Some(target) = request.compute.remote_target() {
            crate::engines::remote::write_run_record(target, &request.working_dir);
        }
        let outcome =
            build_material_system(request, cancel_for_worker, move |progress| match progress {
                GromacsProgress::Stage(stage) => {
                    let _ = report_sender.send(EngineWorkerMessage::Stage(stage));
                }
                GromacsProgress::Log(line) => {
                    let _ = report_sender.send(EngineWorkerMessage::Log(line));
                }
            });

        match outcome {
            Ok(outcome) => {
                // Record the run hints so the MD run applies periodic-molecules
                // / freeze settings; a write failure is non-fatal (the run falls
                // back to plain settings).
                let meta = FrameworkRunMetadata {
                    periodic_molecules: outcome.hints.periodic_molecules,
                    freeze_group: outcome.hints.freeze_group.clone(),
                    framework_atom_count: outcome.framework_atom_count,
                };
                let _ = meta.save(&outcome.working_dir.join(MD_FRAMEWORK_FILE));
                write_md_system_context(
                    &outcome.working_dir,
                    &solute,
                    outcome.structure.atoms.len(),
                    "framework",
                    water_token.as_deref(),
                    true,
                    0.0,
                    false,
                    Vec::new(),
                );
                let _ = sender.send(EngineWorkerMessage::Finished(Box::new(EngineSuccess {
                    engine: "gromacs",
                    job_kind: "build-md",
                    structure: outcome.structure,
                    summary: outcome.summary,
                    working_dir: outcome.working_dir,
                    trajectory: None,
                })));
            }
            Err(error) => {
                let _ = sender.send(EngineWorkerMessage::Failed(error.to_string()));
            }
        }
    });

    RunningEngineJob {
        engine: "gromacs",
        job_kind: "build-md",
        cancel,
        receiver,
        latest_stage: None,
        log_tail: Vec::new(),
    }
}

fn engine_success_from_gromacs_pipeline(results: Vec<StageResult>) -> EngineSuccess {
    let stage_count = results.len();
    let final_result = results
        .last()
        .expect("successful GROMACS pipeline must yield at least one stage");
    let stage = final_result.stage_name.clone();
    let summary = match final_result.final_potential_energy {
        Some(energy) => format!(
            "GROMACS MD complete: {stage_count} steps, final stage {stage}, E = {energy:.3} kJ/mol in {:.2?}",
            final_result.wall_time
        ),
        None => format!(
            "GROMACS MD complete: {stage_count} steps, final stage {stage} in {:.2?}",
            final_result.wall_time
        ),
    };
    // The production stage writes the compressed `.xtc`; take the last stage
    // that produced one so playback follows the actual MD trajectory.
    let trajectory = results
        .iter()
        .rev()
        .find_map(|stage| stage.trajectory.clone());
    EngineSuccess {
        engine: "gromacs",
        job_kind: "run-md",
        structure: final_result.structure.clone(),
        summary,
        working_dir: final_result.working_dir.clone(),
        trajectory,
    }
}

pub fn engine_poll_frame() -> Duration {
    OPTIMIZATION_POLL_FRAME
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn apply_metrics_sampler_starts_and_stops() {
        let mut jobs = JobManager::default();
        apply_metrics_sampler(&mut jobs, true);
        assert!(
            jobs.metrics.is_some(),
            "turning on should spawn the sampler"
        );
        apply_metrics_sampler(&mut jobs, true); // idempotent — no second sampler
        assert!(jobs.metrics.is_some());
        apply_metrics_sampler(&mut jobs, false);
        assert!(
            jobs.metrics.is_none(),
            "turning off should drop the sampler"
        );
    }

    #[test]
    fn parse_model_ids_reads_data_id_list() {
        let json = json!({ "data": [{ "id": "x" }, { "id": "y" }] });
        assert_eq!(parse_model_ids(&json), vec!["x", "y"]);
    }

    #[test]
    fn parse_model_ids_ignores_garbage() {
        // Wrong shape, missing `data`, or non-object items all yield nothing.
        assert!(parse_model_ids(&json!({ "models": ["x"] })).is_empty());
        assert!(parse_model_ids(&json!([1, 2, 3])).is_empty());
        assert!(parse_model_ids(&json!("nope")).is_empty());
        // Items without a string `id` are skipped, not faked.
        assert_eq!(
            parse_model_ids(&json!({ "data": [{ "id": "ok" }, { "name": "no-id" }] })),
            vec!["ok"]
        );
    }

    #[test]
    fn interpret_models_response_reads_ids_on_ok() {
        assert_eq!(
            interpret_models_response(200, r#"{"data":[{"id":"x"},{"id":"y"}]}"#),
            Ok(vec!["x".to_string(), "y".to_string()])
        );
    }

    #[test]
    fn interpret_models_response_html_points_at_base_url() {
        // The exact symptom the user hit: Base URL without `/v1` returns the
        // relay's web page, not JSON. The error must read like the assistant path —
        // name the HTML page and point at the `/v1` API root, not raw serde.
        let err = interpret_models_response(200, "<!doctype html><html></html>").unwrap_err();
        assert!(err.contains("HTML"), "got: {err}");
        assert!(err.contains("/v1"), "got: {err}");
        assert!(!err.contains("malformed"), "leaks serde wording: {err}");
    }

    #[test]
    fn interpret_models_response_empty_body_flags_base_url() {
        let err = interpret_models_response(200, "   ").unwrap_err();
        assert!(err.contains("empty"), "got: {err}");
    }

    #[test]
    fn interpret_models_response_non_json_error_page_hints_url_regardless_of_status() {
        // A wrong Base URL can 404 to an HTML page too; that is still a
        // wrong-URL signal, so it gets the same hint rather than a bare status.
        let err = interpret_models_response(404, "<html>not found</html>").unwrap_err();
        assert!(err.contains("HTML"), "got: {err}");
    }

    #[test]
    fn interpret_models_response_json_error_reports_status() {
        // A valid JSON body with a non-200 status is a real API error, not a
        // wrong URL — surface the status.
        let err = interpret_models_response(503, r#"{"error":"nope"}"#).unwrap_err();
        assert!(err.contains("503"), "got: {err}");
    }

    #[test]
    fn interpret_models_response_json_error_reports_message() {
        let err = interpret_models_response(
            401,
            r#"{"code":"API_KEY_REQUIRED","message":"API key is required"}"#,
        )
        .unwrap_err();
        assert!(err.contains("401"), "got: {err}");
        assert!(err.contains("API key is required"), "got: {err}");
    }
}
