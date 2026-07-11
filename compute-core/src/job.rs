//! Engine-neutral execution identity and lifecycle vocabulary, shared by the app
//! backend (persistence) and the headless worker. Carries no app concepts — no
//! `TaskId`, no `ProjectId`.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Global, path- and project-independent identity of one engine invocation (one
/// `JobExecution`). The single durable execution identity that supersedes the
/// former `run_uuid` string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct JobId(Uuid);

impl JobId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    pub fn as_uuid(&self) -> Uuid {
        self.0
    }
}

impl Default for JobId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for JobId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.as_hyphenated().fmt(f)
    }
}

impl FromStr for JobId {
    type Err = uuid::Error;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(text).map(Self)
    }
}

/// Execution truth for one `JobExecution` — what the work is doing. Orthogonal to
/// [`ObservationState`]: a remote job made unreachable by a pure timeout keeps its
/// last execution state and never advances here on timeout alone.
///
/// Terminal = `Succeeded | Failed | Cancelled | Interrupted`, and a terminal state
/// is irreversible; a later `Progress`/`Log` never moves it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExecutionState {
    Queued,
    Running,
    Cancelling,
    Succeeded,
    Failed,
    Cancelled,
    /// A local run whose worker/channel was lost, or one left non-terminal by a
    /// crash and reconciled at startup. Never reached by a remote timeout.
    Interrupted,
}

impl ExecutionState {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Succeeded | Self::Failed | Self::Cancelled | Self::Interrupted
        )
    }

    pub fn token(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Cancelling => "cancelling",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Interrupted => "interrupted",
        }
    }

    pub fn from_token(token: &str) -> Option<Self> {
        Some(match token {
            "queued" => Self::Queued,
            "running" => Self::Running,
            "cancelling" => Self::Cancelling,
            "succeeded" => Self::Succeeded,
            "failed" => Self::Failed,
            "cancelled" => Self::Cancelled,
            "interrupted" => Self::Interrupted,
            _ => return None,
        })
    }
}

/// Whether a (remote) execution can currently be observed — orthogonal to
/// [`ExecutionState`]. `Unreachable` records only that observation failed; it
/// never changes the execution state. Local jobs are always `Observed`. Freshness
/// timestamps and the last error live on the persisted row, not here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ObservationState {
    Observed,
    Unreachable,
}

impl ObservationState {
    pub fn token(self) -> &'static str {
        match self {
            Self::Observed => "observed",
            Self::Unreachable => "unreachable",
        }
    }

    pub fn from_token(token: &str) -> Option<Self> {
        Some(match token {
            "observed" => Self::Observed,
            "unreachable" => Self::Unreachable,
            _ => return None,
        })
    }
}

/// Static, conservative promise about whether a job can be cancelled, exposed
/// with the job so the UI can show cancel availability before the click. It must
/// hold for the whole execution lifecycle; a single opaque blocking engine that
/// cannot be interrupted mid-run is `Unsupported`, not `Cooperative`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CancelCapability {
    Cooperative,
    Preemptive,
    Unsupported,
}

impl CancelCapability {
    pub fn token(self) -> &'static str {
        match self {
            Self::Cooperative => "cooperative",
            Self::Preemptive => "preemptive",
            Self::Unsupported => "unsupported",
        }
    }

    pub fn from_token(token: &str) -> Option<Self> {
        Some(match token {
            "cooperative" => Self::Cooperative,
            "preemptive" => Self::Preemptive,
            "unsupported" => Self::Unsupported,
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn job_id_round_trips_through_string() {
        let id = JobId::new();
        let parsed: JobId = id.to_string().parse().expect("parse");
        assert_eq!(id, parsed);
        assert!(JobId::from_str("not-a-uuid").is_err());
    }

    #[test]
    fn execution_state_tokens_round_trip_and_classify_terminal() {
        for state in [
            ExecutionState::Queued,
            ExecutionState::Running,
            ExecutionState::Cancelling,
            ExecutionState::Succeeded,
            ExecutionState::Failed,
            ExecutionState::Cancelled,
            ExecutionState::Interrupted,
        ] {
            assert_eq!(ExecutionState::from_token(state.token()), Some(state));
        }
        assert_eq!(ExecutionState::from_token("bogus"), None);
        for terminal in [
            ExecutionState::Succeeded,
            ExecutionState::Failed,
            ExecutionState::Cancelled,
            ExecutionState::Interrupted,
        ] {
            assert!(terminal.is_terminal());
        }
        for live in [
            ExecutionState::Queued,
            ExecutionState::Running,
            ExecutionState::Cancelling,
        ] {
            assert!(!live.is_terminal());
        }
    }

    #[test]
    fn observation_and_cancel_tokens_round_trip() {
        for state in [ObservationState::Observed, ObservationState::Unreachable] {
            assert_eq!(ObservationState::from_token(state.token()), Some(state));
        }
        assert_eq!(ObservationState::from_token("bogus"), None);
        for capability in [
            CancelCapability::Cooperative,
            CancelCapability::Preemptive,
            CancelCapability::Unsupported,
        ] {
            assert_eq!(
                CancelCapability::from_token(capability.token()),
                Some(capability)
            );
        }
        assert_eq!(CancelCapability::from_token("bogus"), None);
    }
}
