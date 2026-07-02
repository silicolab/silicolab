use std::sync::atomic::Ordering;

use crate::backend::storage::jobs as registry;
use crate::backend::tasks::TaskStatus;
use crate::frontend::state::AppState;

use super::agent::AgentHeavyJob;
use super::manager::JobManager;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum JobControlId {
    Local(LocalJobSlot),
    Agent(u64),
    Remote(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LocalJobSlot {
    Optimizer,
    Disorder,
    Qm,
    Docking,
    Engine,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobBackend {
    LocalLive,
    AgentLive,
    RemoteRegistry,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobKind {
    Optimizer,
    Disorder,
    Qm,
    Docking,
    Engine,
    AssistantQm,
    AssistantDocking,
    AssistantEngine,
    RemoteEngine,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobStatus {
    Queued,
    Running,
    Cancelling,
    Done,
    Failed,
    Lost,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelCapability {
    Cooperative,
    RemoteLauncher,
    None,
}

impl JobControlId {
    pub fn token(&self) -> String {
        match self {
            Self::Local(slot) => format!("local:{}", slot.token()),
            Self::Agent(id) => format!("agent:{id}"),
            Self::Remote(run_uuid) => format!("remote:{run_uuid}"),
        }
    }
}

impl LocalJobSlot {
    pub fn token(self) -> &'static str {
        match self {
            Self::Optimizer => "optimizer",
            Self::Disorder => "disorder",
            Self::Qm => "qm",
            Self::Docking => "docking",
            Self::Engine => "engine",
        }
    }
}

impl JobBackend {
    pub fn label(self) -> &'static str {
        match self {
            Self::LocalLive => "local",
            Self::AgentLive => "assistant",
            Self::RemoteRegistry => "remote",
        }
    }
}

impl JobKind {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Optimizer => "optimizer",
            Self::Disorder => "disorder",
            Self::Qm => "qm",
            Self::Docking => "docking",
            Self::Engine => "engine",
            Self::AssistantQm => "assistant-qm",
            Self::AssistantDocking => "assistant-docking",
            Self::AssistantEngine => "assistant-engine",
            Self::RemoteEngine => "remote-engine",
        }
    }

    pub fn is_qm(&self) -> bool {
        matches!(self, Self::Qm | Self::AssistantQm)
    }
}

impl JobStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Cancelling => "cancelling",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Lost => "lost",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn is_running(self) -> bool {
        matches!(self, Self::Queued | Self::Running | Self::Cancelling)
    }
}

impl CancelCapability {
    pub fn label(self) -> &'static str {
        match self {
            Self::Cooperative => "cooperative",
            Self::RemoteLauncher => "remote-launcher",
            Self::None => "none",
        }
    }

    pub fn can_cancel(self) -> bool {
        !matches!(self, Self::None)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveJobSnapshot {
    pub id: JobControlId,
    pub backend: JobBackend,
    pub kind: JobKind,
    pub status: JobStatus,
    pub cancel: CancelCapability,
    pub label: String,
    pub engine_id: Option<String>,
    pub job_kind: Option<String>,
    pub stage: Option<String>,
    pub task_run_id: Option<u64>,
    pub run_uuid: Option<String>,
    pub host_label: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CancelOutcome {
    Requested {
        id: JobControlId,
        task_run_id: Option<u64>,
    },
    RequestFailed {
        id: JobControlId,
        reason: String,
    },
    NotFound {
        id: JobControlId,
    },
    NotCancellable {
        id: JobControlId,
        reason: String,
    },
}

impl CancelOutcome {
    pub fn task_run_id(&self) -> Option<u64> {
        match self {
            Self::Requested { task_run_id, .. } => *task_run_id,
            Self::RequestFailed { .. } | Self::NotFound { .. } | Self::NotCancellable { .. } => {
                None
            }
        }
    }
}

impl JobManager {
    pub fn list_live_snapshots(&self, active_task_run: Option<u64>) -> Vec<LiveJobSnapshot> {
        let mut jobs = Vec::new();
        if let Some(running) = self.optimizer.as_ref() {
            jobs.push(local_snapshot(
                LocalJobSlot::Optimizer,
                JobKind::Optimizer,
                "forcefield optimization",
                active_task_run,
                running
                    .latest_report
                    .map(|report| format!("step {}", report.steps)),
            ));
        }
        if let Some(running) = self.disorder.as_ref() {
            jobs.push(local_snapshot(
                LocalJobSlot::Disorder,
                JobKind::Disorder,
                "build disordered system",
                active_task_run,
                running.latest_report.as_ref().map(|report| {
                    format!(
                        "{} / {} placed",
                        report.total_placed(),
                        report.total_requested()
                    )
                }),
            ));
        }
        if let Some(running) = self.qm.as_ref() {
            let mut snapshot = local_snapshot(
                LocalJobSlot::Qm,
                JobKind::Qm,
                "QM calculation",
                active_task_run,
                running.latest_stage.clone(),
            );
            if running.cancel_requested {
                snapshot.status = JobStatus::Cancelling;
                snapshot.cancel = CancelCapability::None;
            }
            jobs.push(snapshot);
        }
        if self.docking.is_some() {
            jobs.push(local_snapshot(
                LocalJobSlot::Docking,
                JobKind::Docking,
                "molecular docking",
                active_task_run,
                None,
            ));
        }
        if let Some(running) = self.engine.as_ref() {
            let mut snapshot = local_snapshot(
                LocalJobSlot::Engine,
                JobKind::Engine,
                format!("{} {}", running.engine, running.job_kind),
                active_task_run,
                running.latest_stage.clone(),
            );
            snapshot.engine_id = Some(running.engine.to_string());
            snapshot.job_kind = Some(running.job_kind.to_string());
            jobs.push(snapshot);
        }
        for tracked in &self.agent_jobs {
            jobs.push(agent_snapshot(tracked));
        }
        jobs
    }

    pub fn cancel_controlled_job(&mut self, id: &JobControlId) -> anyhow::Result<CancelOutcome> {
        Ok(match id {
            JobControlId::Local(slot) => {
                if self.cancel_local_slot(*slot) {
                    CancelOutcome::Requested {
                        id: id.clone(),
                        task_run_id: None,
                    }
                } else {
                    CancelOutcome::NotFound { id: id.clone() }
                }
            }
            JobControlId::Agent(agent_id) => match self.cancel_agent_slot(*agent_id) {
                Some(task_run_id) => CancelOutcome::Requested {
                    id: id.clone(),
                    task_run_id: Some(task_run_id),
                },
                None => CancelOutcome::NotFound { id: id.clone() },
            },
            JobControlId::Remote(_) => CancelOutcome::NotCancellable {
                id: id.clone(),
                reason: "remote jobs require registry and host context".to_string(),
            },
        })
    }

    fn cancel_local_slot(&mut self, slot: LocalJobSlot) -> bool {
        match slot {
            LocalJobSlot::Optimizer => self
                .optimizer
                .as_ref()
                .map(|running| running.cancel.store(true, Ordering::Relaxed)),
            LocalJobSlot::Disorder => self
                .disorder
                .as_ref()
                .map(|running| running.cancel.store(true, Ordering::Relaxed)),
            LocalJobSlot::Qm => self.qm.as_mut().map(|running| {
                running.cancel_requested = true;
                running.cancel.cancel();
            }),
            LocalJobSlot::Docking => self
                .docking
                .as_ref()
                .map(|running| running.cancel.store(true, Ordering::Relaxed)),
            LocalJobSlot::Engine => self
                .engine
                .as_ref()
                .map(|running| running.cancel.store(true, Ordering::Relaxed)),
        }
        .is_some()
    }

    fn cancel_agent_slot(&mut self, id: u64) -> Option<u64> {
        let tracked = self.agent_jobs.iter_mut().find(|job| job.id == id)?;
        tracked.job.cancel();
        tracked.task_cancelling();
        Some(tracked.task_run_id)
    }
}

trait TrackedAgentJobCancel {
    fn task_cancelling(&mut self);
}

impl TrackedAgentJobCancel for super::agent::TrackedAgentJob {
    fn task_cancelling(&mut self) {
        if let AgentHeavyJob::Qm(job) = &mut self.job {
            job.cancel_requested = true;
        }
    }
}

pub fn list_controlled_jobs(state: &AppState) -> Vec<LiveJobSnapshot> {
    let mut jobs = state.jobs.list_live_snapshots(state.active_task_run);
    jobs.extend(list_remote_snapshots(state));
    jobs
}

pub fn cancel_controlled_job(
    state: &mut AppState,
    id: &JobControlId,
) -> anyhow::Result<CancelOutcome> {
    match id {
        JobControlId::Remote(run_uuid) => cancel_remote_controlled_job(state, run_uuid),
        JobControlId::Local(_) | JobControlId::Agent(_) => {
            let active_task_run = state.active_task_run;
            let outcome = state.jobs.cancel_controlled_job(id)?;
            if let Some(task_run_id) = outcome.task_run_id()
                && task_run_id != 0
            {
                mark_task_cancelling(state, task_run_id);
            }
            if matches!(id, JobControlId::Local(_))
                && matches!(outcome, CancelOutcome::Requested { .. })
                && let Some(task_run_id) = active_task_run
            {
                mark_task_cancelling(state, task_run_id);
                return Ok(CancelOutcome::Requested {
                    id: id.clone(),
                    task_run_id: Some(task_run_id),
                });
            }
            Ok(outcome)
        }
    }
}

pub fn format_jobs_status(state: &AppState) -> String {
    let jobs = list_controlled_jobs(state);
    if jobs.is_empty() {
        return "No running or tracked jobs.".to_string();
    }
    format_job_table(&jobs)
}

pub fn cancel_job_by_token(state: &mut AppState, token: &str) -> anyhow::Result<String> {
    let jobs = list_controlled_jobs(state);
    let id = parse_job_control_id(token, &jobs)?;
    let job = jobs.iter().find(|job| job.id == id).cloned();
    let outcome = cancel_controlled_job(state, &id)?;
    Ok(format_cancel_outcome_for_job(&outcome, job.as_ref()))
}

pub fn qm_jobs_status(state: &AppState) -> String {
    let jobs = running_qm_jobs(state);
    if jobs.is_empty() {
        return "No running QM jobs.".to_string();
    }
    format_job_table(&jobs)
}

pub fn cancel_qm_job_alias(state: &mut AppState) -> anyhow::Result<String> {
    let jobs = running_qm_jobs(state);
    match jobs.as_slice() {
        [] => Ok("No running QM job to cancel.".to_string()),
        [job] => {
            let id = job.id.clone();
            let job = job.clone();
            let outcome = cancel_controlled_job(state, &id)?;
            Ok(format_cancel_outcome_for_job(&outcome, Some(&job)))
        }
        _ => {
            let ids = jobs
                .iter()
                .map(|job| job.id.token())
                .collect::<Vec<_>>()
                .join(", ");
            Ok(format!(
                "Multiple QM jobs are running ({ids}); use `jobs cancel <id>`."
            ))
        }
    }
}

pub fn format_job_table(jobs: &[LiveJobSnapshot]) -> String {
    let mut out = String::from("id\tkind\tlabel\tstatus\tstage\tbackend\tcancel\n");
    for job in jobs {
        out.push_str(&format_job_row(job));
        out.push('\n');
    }
    out.trim_end().to_string()
}

pub fn format_job_row(job: &LiveJobSnapshot) -> String {
    format!(
        "{}\t{}\t{}\t{}\t{}\t{}\t{}",
        job.id.token(),
        job.kind.label(),
        job.label,
        job_status_display(job),
        job.stage.as_deref().unwrap_or("-"),
        job.backend.label(),
        job.cancel.label(),
    )
}

pub fn format_cancel_outcome_for_job(
    outcome: &CancelOutcome,
    job: Option<&LiveJobSnapshot>,
) -> String {
    match outcome {
        CancelOutcome::Requested { id, .. } => match job {
            Some(job) if job_is_qm(job) => format!(
                "Cancel requested for {}. The current stage may finish before stopping.",
                id.token()
            ),
            _ => format!("Cancel requested for {}.", id.token()),
        },
        CancelOutcome::RequestFailed { id, reason } => {
            format!("Could not cancel {}: {reason}", id.token())
        }
        CancelOutcome::NotFound { id } => format!("Job not found: {}", id.token()),
        CancelOutcome::NotCancellable { id, reason } => {
            format!("Job {} is not cancellable: {reason}", id.token())
        }
    }
}

pub fn job_status_display(job: &LiveJobSnapshot) -> String {
    if job.backend == JobBackend::RemoteRegistry {
        format!("last-known:{}", job.status.label())
    } else {
        job.status.label().to_string()
    }
}

fn job_is_qm(job: &LiveJobSnapshot) -> bool {
    job.kind.is_qm()
        || job
            .job_kind
            .as_deref()
            .is_some_and(|kind| kind.starts_with("qm-"))
}

pub fn parse_job_control_id(token: &str, jobs: &[LiveJobSnapshot]) -> anyhow::Result<JobControlId> {
    let token = token.trim();
    if token.is_empty() {
        anyhow::bail!("job id is empty");
    }
    let token_lower = token.to_ascii_lowercase();
    for job in jobs {
        if job.id.token().eq_ignore_ascii_case(token) {
            return Ok(job.id.clone());
        }
        if matches!(job.id, JobControlId::Remote(_))
            && job.run_uuid.as_deref().is_some_and(|uuid| uuid == token)
        {
            return Ok(job.id.clone());
        }
    }
    if let Some(rest) = token_lower.strip_prefix("agent:")
        && let Ok(id) = rest.parse::<u64>()
    {
        return Ok(JobControlId::Agent(id));
    }
    if let Some(rest) = token_lower.strip_prefix("remote:") {
        let Some(job) = jobs.iter().find(|job| {
            matches!(job.id, JobControlId::Remote(_))
                && job.run_uuid.as_deref().is_some_and(|uuid| uuid == rest)
        }) else {
            return Ok(JobControlId::Remote(token["remote:".len()..].to_string()));
        };
        return Ok(job.id.clone());
    }
    let local = token_lower
        .strip_prefix("local:")
        .unwrap_or(token_lower.as_str());
    match local {
        "optimizer" | "optimization" | "opt" => Ok(JobControlId::Local(LocalJobSlot::Optimizer)),
        "disorder" | "pack" => Ok(JobControlId::Local(LocalJobSlot::Disorder)),
        "qm" => Ok(JobControlId::Local(LocalJobSlot::Qm)),
        "docking" | "dock" => Ok(JobControlId::Local(LocalJobSlot::Docking)),
        "engine" | "md" | "gromacs" => Ok(JobControlId::Local(LocalJobSlot::Engine)),
        _ => anyhow::bail!("unknown job id `{token}`; run `jobs status` to list jobs"),
    }
}

fn running_qm_jobs(state: &AppState) -> Vec<LiveJobSnapshot> {
    list_controlled_jobs(state)
        .into_iter()
        .filter(|job| {
            job.status.is_running()
                && (job.kind.is_qm()
                    || job
                        .job_kind
                        .as_deref()
                        .is_some_and(|kind| kind.starts_with("qm-")))
        })
        .collect()
}

pub fn remote_job_snapshot(row: &registry::RemoteJob) -> LiveJobSnapshot {
    LiveJobSnapshot {
        id: JobControlId::Remote(row.run_uuid.clone()),
        backend: JobBackend::RemoteRegistry,
        kind: JobKind::RemoteEngine,
        status: remote_status(row.status),
        cancel: if row.status.is_terminal() {
            CancelCapability::None
        } else {
            CancelCapability::RemoteLauncher
        },
        label: format!("remote {} on {}", row.job_kind, row.host_label),
        engine_id: Some(row.engine_id.clone()),
        job_kind: Some(row.job_kind.clone()),
        stage: None,
        task_run_id: None,
        run_uuid: Some(row.run_uuid.clone()),
        host_label: Some(row.host_label.clone()),
    }
}

pub fn record_remote_cancel_success(
    conn: &rusqlite::Connection,
    run_uuid: &str,
    now_ms: i64,
) -> anyhow::Result<()> {
    registry::record_poll(
        conn,
        run_uuid,
        registry::RemoteJobStatus::Cancelled,
        None,
        now_ms,
    )
}

fn list_remote_snapshots(state: &AppState) -> Vec<LiveJobSnapshot> {
    let rows = (|| -> anyhow::Result<Vec<registry::RemoteJob>> {
        let conn = registry::open()?;
        match state.workspace.project() {
            Some(project) => registry::list_for_project(&conn, &project.root.to_string_lossy()),
            None => registry::list_non_terminal(&conn),
        }
    })()
    .unwrap_or_default();
    rows.iter().map(remote_job_snapshot).collect()
}

fn cancel_remote_controlled_job(
    state: &mut AppState,
    run_uuid: &str,
) -> anyhow::Result<CancelOutcome> {
    let conn = registry::open()?;
    let Some(row) = registry::get(&conn, run_uuid)? else {
        return Ok(CancelOutcome::NotFound {
            id: JobControlId::Remote(run_uuid.to_string()),
        });
    };
    if row.status.is_terminal() {
        return Ok(CancelOutcome::NotCancellable {
            id: JobControlId::Remote(run_uuid.to_string()),
            reason: format!("remote job is already {}", row.status.token()),
        });
    }
    let id = JobControlId::Remote(row.run_uuid.clone());
    let Some(host) = state.config.remote_hosts.get(&row.host_id).cloned() else {
        return Ok(CancelOutcome::NotCancellable {
            id,
            reason: format!("remote host {} is no longer configured", row.host_id),
        });
    };
    let Some(launcher) = launcher_from_token(&row.scheduler) else {
        return Ok(CancelOutcome::NotCancellable {
            id,
            reason: format!("unknown remote scheduler {}", row.scheduler),
        });
    };
    let target = crate::engines::remote::RemoteTarget::for_run(&host, &row.run_uuid);
    let handle = crate::engines::remote::launcher::LaunchHandle(row.launch_handle.clone());
    if let Err(error) = launcher.cancel(&target, &handle) {
        return Ok(CancelOutcome::RequestFailed {
            id,
            reason: error.to_string(),
        });
    }
    record_remote_cancel_success(&conn, &row.run_uuid, registry::now_ms())?;
    sync_remote_job_ui_row(state, &row.run_uuid, registry::RemoteJobStatus::Cancelled);
    let task_run_id = state
        .tasks
        .task_run_by_uuid(&row.run_uuid)
        .map(|task| task.id);
    if let Some(task_run_id) = task_run_id {
        mark_task_cancelled(state, task_run_id);
    }
    Ok(CancelOutcome::Requested {
        id: JobControlId::Remote(row.run_uuid),
        task_run_id,
    })
}

fn launcher_from_token(token: &str) -> Option<crate::engines::remote::launcher::Launcher> {
    match token {
        "direct" => Some(crate::engines::remote::launcher::Launcher::Direct),
        _ => None,
    }
}

fn mark_task_cancelled(state: &mut AppState, task_run_id: u64) {
    if let Err(error) =
        crate::frontend::task_executor::mark_task_status(state, task_run_id, TaskStatus::Cancelled)
    {
        state.set_message(format!("failed to update task status: {error}"));
    }
}

fn mark_task_cancelling(state: &mut AppState, task_run_id: u64) {
    if let Err(error) =
        crate::frontend::task_executor::mark_task_status(state, task_run_id, TaskStatus::Cancelling)
    {
        state.set_message(format!("failed to update task status: {error}"));
    }
}

fn sync_remote_job_ui_row(state: &mut AppState, run_uuid: &str, status: registry::RemoteJobStatus) {
    if let Some(row) = state
        .ui
        .remote_jobs
        .iter_mut()
        .find(|row| row.run_uuid == run_uuid)
    {
        row.status = status;
        row.last_polled_at_ms = Some(registry::now_ms());
        row.exit_code = None;
    }
}

fn remote_status(status: registry::RemoteJobStatus) -> JobStatus {
    match status {
        registry::RemoteJobStatus::Queued => JobStatus::Queued,
        registry::RemoteJobStatus::Running => JobStatus::Running,
        registry::RemoteJobStatus::Done => JobStatus::Done,
        registry::RemoteJobStatus::Failed => JobStatus::Failed,
        registry::RemoteJobStatus::Lost => JobStatus::Lost,
        registry::RemoteJobStatus::Cancelled => JobStatus::Cancelled,
    }
}

fn local_snapshot(
    slot: LocalJobSlot,
    kind: JobKind,
    label: impl Into<String>,
    task_run_id: Option<u64>,
    stage: Option<String>,
) -> LiveJobSnapshot {
    LiveJobSnapshot {
        id: JobControlId::Local(slot),
        backend: JobBackend::LocalLive,
        kind,
        status: JobStatus::Running,
        cancel: CancelCapability::Cooperative,
        label: label.into(),
        engine_id: None,
        job_kind: None,
        stage,
        task_run_id,
        run_uuid: None,
        host_label: None,
    }
}

fn agent_snapshot(tracked: &super::agent::TrackedAgentJob) -> LiveJobSnapshot {
    let (kind, stage, engine_id, job_kind, cancel_requested) = match &tracked.job {
        AgentHeavyJob::Qm(job) => (
            JobKind::AssistantQm,
            job.latest_stage.clone(),
            None,
            None,
            job.cancel_requested,
        ),
        AgentHeavyJob::Docking(_) => (JobKind::AssistantDocking, None, None, None, false),
        AgentHeavyJob::Engine(job) => (
            JobKind::AssistantEngine,
            job.latest_stage.clone(),
            Some(job.engine.to_string()),
            Some(job.job_kind.to_string()),
            false,
        ),
    };
    LiveJobSnapshot {
        id: JobControlId::Agent(tracked.id),
        backend: JobBackend::AgentLive,
        kind,
        status: if cancel_requested {
            JobStatus::Cancelling
        } else {
            JobStatus::Running
        },
        cancel: if cancel_requested {
            CancelCapability::None
        } else {
            CancelCapability::Cooperative
        },
        label: tracked.label.clone(),
        engine_id,
        job_kind,
        stage,
        task_run_id: Some(tracked.task_run_id),
        run_uuid: None,
        host_label: None,
    }
}
