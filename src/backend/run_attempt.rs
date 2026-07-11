//! The run/execution linkage between a `TaskRun` and the engine invocations it
//! drives: `TaskRun 1─N RunAttempt 1─N JobExecution`. A [`JobExecution`]
//! carries the global [`JobId`] and is the unit attribution resolves through — a
//! finished job is mapped back to its owning task by
//! [`RunGraph::task_run_id_for_job`], not by the ambient "active task run" or by a
//! `run_uuid` string. The MD em/nvt/npt stages of one run, and a run's retries, are
//! already expressible: multiple executions under one attempt, multiple attempts
//! under one task.
//!
//! This lives in the app backend: it references `TaskRun` ids but no UI concept.

use std::collections::HashSet;

use crate::job::{ExecutionState, JobId};

/// Where a job runs — orthogonal to what it computes. Stored so the UI can
/// derive "Local · GROMACS" from a real execution rather than a predicted label.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Placement {
    Local,
    Remote { host: Option<String> },
    Agent,
}

impl Placement {
    pub fn token(&self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Remote { .. } => "remote",
            Self::Agent => "agent",
        }
    }

    pub fn host(&self) -> Option<&str> {
        match self {
            Self::Remote { host } => host.as_deref(),
            _ => None,
        }
    }

    pub fn is_remote(&self) -> bool {
        matches!(self, Self::Remote { .. })
    }

    pub fn from_parts(token: &str, host: Option<String>) -> Self {
        match token {
            "remote" => Self::Remote { host },
            "agent" => Self::Agent,
            _ => Self::Local,
        }
    }
}

/// One user Run (or retry) of a task. `(task_run_id, attempt_no)` is unique.
#[derive(Debug, Clone)]
pub struct RunAttempt {
    pub run_attempt_id: u64,
    pub task_run_id: u64,
    pub attempt_no: u32,
    pub created_at_ms: u64,
    pub finished_at_ms: Option<u64>,
}

/// Whether a job execution's outcome has been imported into the project.
/// Distinct from execution success: a job can succeed remotely yet have its result
/// still `Pending` (or `PendingRecovery` when its downloaded outcome file is gone).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResultImport {
    /// The execution has nothing to import — a local run applies its result inline,
    /// so there is no separate import step to track.
    NotRequired,
    /// Terminal with an outcome that has not yet been applied to this project (a
    /// remote result awaiting a refresh or open-project compensation).
    Pending,
    /// The outcome has been materialized into the project (ledger recorded).
    Applied,
    /// Import was attempted and failed — e.g. the downloaded outcome was unreadable.
    Failed,
    /// Terminal remotely, but the downloaded outcome file is missing: surfaced to
    /// the user for recovery, retried on the next remote refresh.
    PendingRecovery,
}

impl ResultImport {
    pub fn token(self) -> &'static str {
        match self {
            Self::NotRequired => "not_required",
            Self::Pending => "pending",
            Self::Applied => "applied",
            Self::Failed => "failed",
            Self::PendingRecovery => "pending_recovery",
        }
    }

    pub fn from_token(token: &str) -> Option<Self> {
        Some(match token {
            "not_required" => Self::NotRequired,
            "pending" => Self::Pending,
            "applied" => Self::Applied,
            "failed" => Self::Failed,
            "pending_recovery" => Self::PendingRecovery,
            _ => return None,
        })
    }

    /// Whether the import is settled with no outstanding work — the state a Task
    /// must reach (alongside a successful execution) to display as complete.
    pub fn is_complete(self) -> bool {
        matches!(self, Self::NotRequired | Self::Applied)
    }
}

/// One engine invocation. `job_id` is the global execution identity that
/// supersedes `run_uuid`; `(run_attempt_id, ordinal)` orders executions within an
/// attempt.
#[derive(Debug, Clone)]
pub struct JobExecution {
    pub job_id: JobId,
    pub run_attempt_id: u64,
    pub ordinal: u32,
    pub placement: Placement,
    pub job_kind: Option<String>,
    pub execution_state: ExecutionState,
    /// Whether this execution's outcome has been imported — the durable home
    /// for the pending-recovery signal, orthogonal to `execution_state`.
    pub import_state: ResultImport,
    pub created_at_ms: u64,
    pub finished_at_ms: Option<u64>,
}

/// The in-memory attempts + executions for the open project, and the index that
/// resolves a `JobId` back to its owning task. Persisted alongside the task rows.
#[derive(Debug, Clone, Default)]
pub struct RunGraph {
    attempts: Vec<RunAttempt>,
    executions: Vec<JobExecution>,
    next_run_attempt_id: u64,
    /// Set when an attempt/execution changed since the last write, so the row can
    /// be flushed to `project.db` promptly (like the task-run dirty flag), without
    /// waiting for a full-project save.
    dirty: bool,
}

impl RunGraph {
    pub fn from_rows(attempts: Vec<RunAttempt>, executions: Vec<JobExecution>) -> Self {
        let next_run_attempt_id = attempts
            .iter()
            .map(|attempt| attempt.run_attempt_id + 1)
            .max()
            .unwrap_or(1);
        Self {
            attempts,
            executions,
            next_run_attempt_id,
            dirty: false,
        }
    }

    pub fn attempts(&self) -> &[RunAttempt] {
        &self.attempts
    }

    pub fn executions(&self) -> &[JobExecution] {
        &self.executions
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn mark_saved(&mut self) {
        self.dirty = false;
    }

    /// The task's current (latest) attempt, creating attempt 1 lazily on the first
    /// job. Retries — a new attempt only after the current one is terminal — arrive
    /// with the event runtime; today one attempt covers a task's single run.
    fn ensure_current_attempt(&mut self, task_run_id: u64, now_ms: u64) -> u64 {
        if let Some(attempt) = self
            .attempts
            .iter()
            .filter(|attempt| attempt.task_run_id == task_run_id)
            .max_by_key(|attempt| attempt.attempt_no)
        {
            return attempt.run_attempt_id;
        }
        let run_attempt_id = self.next_run_attempt_id;
        self.next_run_attempt_id += 1;
        self.attempts.push(RunAttempt {
            run_attempt_id,
            task_run_id,
            attempt_no: 1,
            created_at_ms: now_ms,
            finished_at_ms: None,
        });
        self.dirty = true;
        run_attempt_id
    }

    /// Begin a job execution under the task's current attempt and return its freshly
    /// minted [`JobId`] — the durable execution identity the runtime handle (local)
    /// or registry row (remote) carries from here on.
    pub fn begin_execution(
        &mut self,
        task_run_id: u64,
        placement: Placement,
        job_kind: Option<String>,
        now_ms: u64,
    ) -> JobId {
        let run_attempt_id = self.ensure_current_attempt(task_run_id, now_ms);
        let ordinal = self
            .executions
            .iter()
            .filter(|execution| execution.run_attempt_id == run_attempt_id)
            .count() as u32;
        let job_id = JobId::new();
        // A remote execution's result is imported separately (on refresh or
        // open-project compensation), so it starts `Pending`; a local run applies
        // its result inline, so it needs no import step.
        let import_state = if placement.is_remote() {
            ResultImport::Pending
        } else {
            ResultImport::NotRequired
        };
        self.executions.push(JobExecution {
            job_id,
            run_attempt_id,
            ordinal,
            placement,
            job_kind,
            execution_state: ExecutionState::Queued,
            import_state,
            created_at_ms: now_ms,
            finished_at_ms: None,
        });
        self.dirty = true;
        job_id
    }

    fn execution(&self, job_id: &str) -> Option<&JobExecution> {
        self.executions
            .iter()
            .find(|execution| execution.job_id.to_string() == job_id)
    }

    /// Resolve a job identity (its `JobId` string, as carried by a runtime handle or
    /// a registry row) to the owning `TaskRun` id, walking execution → attempt.
    pub fn task_run_id_for_job(&self, job_id: &str) -> Option<u64> {
        let run_attempt_id = self.execution(job_id)?.run_attempt_id;
        self.attempts
            .iter()
            .find(|attempt| attempt.run_attempt_id == run_attempt_id)
            .map(|attempt| attempt.task_run_id)
    }

    /// A human label for where a task's current attempt actually ran, derived from
    /// its latest [`JobExecution`] rather than a predicted catalog field.
    /// `None` for a task that has not begun an execution (an inline task, or one not
    /// yet run) — there is no real placement to report yet.
    pub fn placement_label(&self, task_run_id: u64) -> Option<String> {
        let attempt = self
            .attempts
            .iter()
            .filter(|attempt| attempt.task_run_id == task_run_id)
            .max_by_key(|attempt| attempt.attempt_no)?;
        let execution = self
            .executions
            .iter()
            .filter(|execution| execution.run_attempt_id == attempt.run_attempt_id)
            .max_by_key(|execution| execution.ordinal)?;
        Some(match &execution.placement {
            Placement::Local => "Local".to_string(),
            Placement::Remote { host: Some(host) } => format!("Remote · {host}"),
            Placement::Remote { host: None } => "Remote".to_string(),
            Placement::Agent => "Agent".to_string(),
        })
    }

    /// Whether a task has any remote execution — so the crash reconcile leaves it to
    /// the remote reconnect/compensation path instead of marking it `Interrupted`.
    pub fn task_has_remote_execution(&self, task_run_id: u64) -> bool {
        let attempt_ids: HashSet<u64> = self
            .attempts
            .iter()
            .filter(|attempt| attempt.task_run_id == task_run_id)
            .map(|attempt| attempt.run_attempt_id)
            .collect();
        self.executions.iter().any(|execution| {
            attempt_ids.contains(&execution.run_attempt_id) && execution.placement.is_remote()
        })
    }

    /// Advance a job's execution state. A terminal state is irreversible and a later
    /// non-terminal update is ignored; the finish time is stamped once.
    pub fn set_execution_state(&mut self, job_id: &str, state: ExecutionState, now_ms: u64) {
        if let Some(execution) = self
            .executions
            .iter_mut()
            .find(|execution| execution.job_id.to_string() == job_id)
        {
            if execution.execution_state.is_terminal() {
                return;
            }
            execution.execution_state = state;
            if state.is_terminal() {
                execution.finished_at_ms = Some(now_ms);
            }
            self.dirty = true;
        }
    }

    /// Set a job execution's import state. Called when the outcome is
    /// materialized (`Applied`), when an open-project compensation cannot find the
    /// downloaded outcome (`PendingRecovery`), or when it was unreadable (`Failed`).
    /// A no-op when the `JobId` is not in this project's graph.
    pub fn set_import_state(&mut self, job_id: &str, import_state: ResultImport) {
        if let Some(execution) = self
            .executions
            .iter_mut()
            .find(|execution| execution.job_id.to_string() == job_id)
        {
            if execution.import_state == import_state {
                return;
            }
            execution.import_state = import_state;
            self.dirty = true;
        }
    }

    /// Reduce a task's current attempt's required executions to one import state:
    /// `PendingRecovery` dominates, then `Failed`, then `Pending`; only when
    /// none of those remain is the import considered settled (`Applied` when any
    /// entry was imported, else `NotRequired`). All executions are required today.
    pub fn attempt_import_state(&self, task_run_id: u64) -> ResultImport {
        let Some(attempt) = self
            .attempts
            .iter()
            .filter(|attempt| attempt.task_run_id == task_run_id)
            .max_by_key(|attempt| attempt.attempt_no)
        else {
            return ResultImport::NotRequired;
        };
        let states = self
            .executions
            .iter()
            .filter(|execution| execution.run_attempt_id == attempt.run_attempt_id)
            .map(|execution| execution.import_state);
        let mut any_applied = false;
        let mut any_pending = false;
        let mut any_failed = false;
        for state in states {
            match state {
                ResultImport::PendingRecovery => return ResultImport::PendingRecovery,
                ResultImport::Failed => any_failed = true,
                ResultImport::Pending => any_pending = true,
                ResultImport::Applied => any_applied = true,
                ResultImport::NotRequired => {}
            }
        }
        if any_failed {
            ResultImport::Failed
        } else if any_pending {
            ResultImport::Pending
        } else if any_applied {
            ResultImport::Applied
        } else {
            ResultImport::NotRequired
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn begin_execution_links_job_to_task_and_resolves_back() {
        let mut graph = RunGraph::default();
        let job_a = graph.begin_execution(7, Placement::Local, Some("qm-energy".into()), 100);
        let job_b = graph.begin_execution(9, Placement::Remote { host: None }, None, 100);

        // Each job resolves to its own task — no ambient attribution to cross.
        assert_eq!(graph.task_run_id_for_job(&job_a.to_string()), Some(7));
        assert_eq!(graph.task_run_id_for_job(&job_b.to_string()), Some(9));
        assert_eq!(graph.task_run_id_for_job("nope"), None);
        assert!(graph.is_dirty());
    }

    #[test]
    fn a_second_job_for_a_task_shares_its_attempt_with_the_next_ordinal() {
        let mut graph = RunGraph::default();
        let first = graph.begin_execution(1, Placement::Local, None, 0);
        let second = graph.begin_execution(1, Placement::Local, None, 0);
        assert_eq!(
            graph.attempts().len(),
            1,
            "one attempt covers the task's run"
        );
        let ordinals: Vec<u32> = [first, second]
            .iter()
            .map(|job| {
                graph
                    .executions()
                    .iter()
                    .find(|execution| execution.job_id == *job)
                    .unwrap()
                    .ordinal
            })
            .collect();
        assert_eq!(ordinals, vec![0, 1]);
    }

    #[test]
    fn remote_execution_marks_a_task_as_not_a_local_zombie() {
        let mut graph = RunGraph::default();
        graph.begin_execution(1, Placement::Local, None, 0);
        graph.begin_execution(
            2,
            Placement::Remote {
                host: Some("hpc".into()),
            },
            None,
            0,
        );
        assert!(!graph.task_has_remote_execution(1));
        assert!(graph.task_has_remote_execution(2));
    }

    #[test]
    fn remote_executions_start_pending_import_and_settle_to_applied() {
        let mut graph = RunGraph::default();
        let local = graph.begin_execution(1, Placement::Local, None, 0);
        let remote = graph.begin_execution(2, Placement::Remote { host: None }, None, 0);
        let import_of = |graph: &RunGraph, job: &JobId| {
            graph
                .executions()
                .iter()
                .find(|execution| &execution.job_id == job)
                .unwrap()
                .import_state
        };
        assert_eq!(import_of(&graph, &local), ResultImport::NotRequired);
        assert_eq!(import_of(&graph, &remote), ResultImport::Pending);

        graph.set_import_state(&remote.to_string(), ResultImport::Applied);
        assert_eq!(import_of(&graph, &remote), ResultImport::Applied);
    }

    #[test]
    fn attempt_import_state_reduces_by_severity() {
        let mut graph = RunGraph::default();
        // Two executions under one task's attempt.
        graph.begin_execution(1, Placement::Remote { host: None }, None, 0);
        let second = graph.begin_execution(1, Placement::Remote { host: None }, None, 0);
        // Both Pending → the attempt is Pending.
        assert_eq!(graph.attempt_import_state(1), ResultImport::Pending);
        // One Applied, one Pending → still Pending (not all settled).
        graph.set_import_state(&second.to_string(), ResultImport::Applied);
        assert_eq!(graph.attempt_import_state(1), ResultImport::Pending);
        // PendingRecovery dominates everything.
        graph.set_import_state(&second.to_string(), ResultImport::PendingRecovery);
        assert_eq!(graph.attempt_import_state(1), ResultImport::PendingRecovery);
        assert!(!graph.attempt_import_state(1).is_complete());
        // A task with no attempt has nothing to import.
        assert_eq!(graph.attempt_import_state(99), ResultImport::NotRequired);
        assert!(ResultImport::Applied.is_complete());
    }

    #[test]
    fn result_import_tokens_round_trip() {
        for state in [
            ResultImport::NotRequired,
            ResultImport::Pending,
            ResultImport::Applied,
            ResultImport::Failed,
            ResultImport::PendingRecovery,
        ] {
            assert_eq!(ResultImport::from_token(state.token()), Some(state));
        }
        assert_eq!(ResultImport::from_token("bogus"), None);
    }

    #[test]
    fn terminal_execution_state_is_irreversible() {
        let mut graph = RunGraph::default();
        let job = graph.begin_execution(1, Placement::Local, None, 0);
        let id = job.to_string();
        graph.set_execution_state(&id, ExecutionState::Succeeded, 10);
        graph.set_execution_state(&id, ExecutionState::Running, 20);
        let execution = graph.executions().iter().find(|e| e.job_id == job).unwrap();
        assert_eq!(execution.execution_state, ExecutionState::Succeeded);
        assert_eq!(execution.finished_at_ms, Some(10));
    }
}
