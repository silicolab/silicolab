//! The status-bar feedback slot: one transient [`StatusNotice`] with tested
//! expiry, severity priority, sticky-error, same-scope recovery, and typed-link
//! behavior. Status is presentation state — it is never the sole record of a
//! failure, which always keeps a canonical session-log or structured-job owner.

use std::time::{Duration, Instant};

use crate::job::JobId;

use super::{OutputTarget, SystemSubsystem};

/// How long a neutral confirmation lingers before any newer notice may replace it.
const NEUTRAL_LIFETIME: Duration = Duration::from_secs(4);
/// How long a success confirmation lingers before it expires on its own.
const SUCCESS_LIFETIME: Duration = Duration::from_secs(5);

/// Visual and behavioral weight of a status notice. The renderer maps this to an
/// icon and color; state never depends on a UI tone type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusSeverity {
    Neutral,
    Success,
    Warning,
    Error,
}

impl StatusSeverity {
    /// Replacement priority: a newer notice replaces the current one when its
    /// priority is at least as high (or it shares the current scope).
    fn priority(self) -> u8 {
        match self {
            Self::Neutral => 0,
            Self::Success => 1,
            Self::Warning => 2,
            Self::Error => 3,
        }
    }

    pub fn is_sticky(self) -> bool {
        matches!(self, Self::Warning | Self::Error)
    }
}

/// Whether a notice expires on a timer or stays until acknowledged / recovered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusLifetime {
    Ephemeral(Duration),
    Sticky,
}

/// Identifies the producer well enough for same-scope replacement and recovery.
/// A later Storage success recovers a Storage error; a job's completion notice
/// replaces its own running notice; unrelated confirmations share `General`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StatusScope {
    General,
    Subsystem(SystemSubsystem),
    Job(JobId),
    Remote(String),
}

/// Where a status link navigates. Typed so a link never addresses a job by kind
/// or a panel by string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DetailTarget {
    Output(OutputTarget),
    ActivityJob(JobId),
    Settings,
}

/// One status-bar notice. `created_at` anchors expiry; pause bookkeeping lives on
/// [`StatusState`], not here.
#[derive(Debug, Clone)]
pub struct StatusNotice {
    pub scope: StatusScope,
    pub severity: StatusSeverity,
    pub text: String,
    pub target: Option<DetailTarget>,
    pub lifetime: StatusLifetime,
    pub created_at: Instant,
}

impl StatusNotice {
    fn new(scope: StatusScope, severity: StatusSeverity, text: impl Into<String>) -> Self {
        let lifetime = match severity {
            StatusSeverity::Neutral => StatusLifetime::Ephemeral(NEUTRAL_LIFETIME),
            StatusSeverity::Success => StatusLifetime::Ephemeral(SUCCESS_LIFETIME),
            StatusSeverity::Warning | StatusSeverity::Error => StatusLifetime::Sticky,
        };
        Self {
            scope,
            severity,
            text: text.into(),
            target: None,
            lifetime,
            created_at: Instant::now(),
        }
    }

    pub fn neutral(scope: StatusScope, text: impl Into<String>) -> Self {
        Self::new(scope, StatusSeverity::Neutral, text)
    }

    pub fn success(scope: StatusScope, text: impl Into<String>) -> Self {
        Self::new(scope, StatusSeverity::Success, text)
    }

    pub fn warning(scope: StatusScope, text: impl Into<String>) -> Self {
        Self::new(scope, StatusSeverity::Warning, text)
    }

    pub fn error(scope: StatusScope, text: impl Into<String>) -> Self {
        Self::new(scope, StatusSeverity::Error, text)
    }

    pub fn with_target(mut self, target: DetailTarget) -> Self {
        self.target = Some(target);
        self
    }
}

/// The single-slot status state: the visible notice plus the pause bookkeeping
/// that freezes expiry while the user hovers or keyboard-focuses a linked notice.
#[derive(Debug, Default)]
pub struct StatusState {
    current: Option<StatusNotice>,
    paused: bool,
    /// Time already spent paused for the current notice, excluded from expiry.
    accumulated_pause: Duration,
    paused_since: Option<Instant>,
}

impl StatusState {
    pub fn current(&self) -> Option<&StatusNotice> {
        self.current.as_ref()
    }

    /// Post a notice, honoring severity priority and same-scope recovery: a lower
    /// priority notice with a different scope is dropped rather than queued, so
    /// there is no unbounded status history and a failure is never masked by a
    /// routine confirmation.
    pub fn post(&mut self, notice: StatusNotice) {
        if let Some(current) = &self.current
            && !should_replace(current, &notice)
        {
            return;
        }
        self.current = Some(notice);
        self.paused = false;
        self.accumulated_pause = Duration::ZERO;
        self.paused_since = None;
    }

    /// Dismiss the current notice (user acknowledgement). Its canonical log/job
    /// record is untouched — that lives elsewhere.
    pub fn acknowledge(&mut self) {
        self.current = None;
        self.paused = false;
        self.accumulated_pause = Duration::ZERO;
        self.paused_since = None;
    }

    /// Freeze or resume expiry (hover / keyboard focus on a linked notice).
    pub fn set_paused(&mut self, paused: bool, now: Instant) {
        if paused == self.paused {
            return;
        }
        self.paused = paused;
        match paused {
            true => self.paused_since = Some(now),
            false => {
                if let Some(since) = self.paused_since.take() {
                    self.accumulated_pause += now.saturating_duration_since(since);
                }
            }
        }
    }

    /// Expire the current notice if its ephemeral lifetime has elapsed, excluding
    /// paused time. Sticky notices never expire here.
    pub fn tick(&mut self, now: Instant) {
        let Some(current) = &self.current else {
            return;
        };
        let StatusLifetime::Ephemeral(lifetime) = current.lifetime else {
            return;
        };
        let mut paused = self.accumulated_pause;
        if let Some(since) = self.paused_since {
            paused += now.saturating_duration_since(since);
        }
        let elapsed = now
            .saturating_duration_since(current.created_at)
            .saturating_sub(paused);
        if elapsed >= lifetime {
            self.acknowledge();
        }
    }
}

/// Whether `next` should take the slot from `current`. A notice sharing the
/// current scope always replaces it (same-scope recovery/refresh); otherwise the
/// newer notice wins only when its severity priority is at least as high, so
/// Neutral never masks a sticky Warning/Error.
fn should_replace(current: &StatusNotice, next: &StatusNotice) -> bool {
    next.scope == current.scope || next.severity.priority() >= current.severity.priority()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(base: Instant, secs: u64) -> Instant {
        base + Duration::from_secs(secs)
    }

    #[test]
    fn neutral_expires_after_its_lifetime() {
        let mut state = StatusState::default();
        let mut notice = StatusNotice::neutral(StatusScope::General, "selection changed");
        let base = Instant::now();
        notice.created_at = base;
        state.post(notice);
        state.tick(at(base, 3));
        assert!(state.current().is_some());
        state.tick(at(base, 4));
        assert!(state.current().is_none());
    }

    #[test]
    fn sticky_error_does_not_expire() {
        let mut state = StatusState::default();
        let mut notice =
            StatusNotice::error(StatusScope::Subsystem(SystemSubsystem::Storage), "boom");
        let base = Instant::now();
        notice.created_at = base;
        state.post(notice);
        state.tick(at(base, 3600));
        assert!(state.current().is_some());
    }

    #[test]
    fn neutral_cannot_replace_sticky_error() {
        let mut state = StatusState::default();
        state.post(StatusNotice::error(
            StatusScope::Subsystem(SystemSubsystem::Storage),
            "save failed",
        ));
        state.post(StatusNotice::neutral(
            StatusScope::General,
            "selection changed",
        ));
        assert_eq!(state.current().unwrap().severity, StatusSeverity::Error);
    }

    #[test]
    fn warning_or_error_replaces_success() {
        let mut state = StatusState::default();
        state.post(StatusNotice::success(StatusScope::General, "saved"));
        state.post(StatusNotice::warning(
            StatusScope::Subsystem(SystemSubsystem::Update),
            "low memory",
        ));
        assert_eq!(state.current().unwrap().severity, StatusSeverity::Warning);
    }

    #[test]
    fn same_scope_success_recovers_a_warning() {
        let mut state = StatusState::default();
        let scope = StatusScope::Remote("host-1".to_string());
        state.post(StatusNotice::error(scope.clone(), "probe failed"));
        assert_eq!(state.current().unwrap().severity, StatusSeverity::Error);
        state.post(StatusNotice::success(scope, "probe ok"));
        assert_eq!(state.current().unwrap().severity, StatusSeverity::Success);
    }

    #[test]
    fn acknowledge_clears_the_slot() {
        let mut state = StatusState::default();
        state.post(StatusNotice::error(StatusScope::General, "boom"));
        state.acknowledge();
        assert!(state.current().is_none());
    }

    #[test]
    fn pause_defers_expiry() {
        let mut state = StatusState::default();
        let mut notice = StatusNotice::neutral(StatusScope::General, "hint");
        let base = Instant::now();
        notice.created_at = base;
        state.post(notice);
        // Paused for the whole window: it must not expire.
        state.set_paused(true, at(base, 1));
        state.tick(at(base, 10));
        assert!(state.current().is_some());
        // Resume at +10 with 9s of paused time excluded (effective age 1s), so it
        // lives ~3 more unpaused seconds.
        state.set_paused(false, at(base, 10));
        state.tick(at(base, 12));
        assert!(state.current().is_some());
        state.tick(at(base, 14));
        assert!(state.current().is_none());
    }

    #[test]
    fn replacing_a_paused_notice_resets_pause_bookkeeping() {
        let mut state = StatusState::default();
        let base = Instant::now();
        let mut first = StatusNotice::neutral(StatusScope::General, "first");
        first.created_at = base;
        state.post(first);
        state.set_paused(true, at(base, 1));

        let mut second = StatusNotice::neutral(StatusScope::General, "second");
        second.created_at = at(base, 2);
        state.post(second);
        state.set_paused(true, at(base, 2));
        state.tick(at(base, 20));
        assert_eq!(state.current().unwrap().text, "second");
    }

    #[test]
    fn newer_same_priority_notice_wins_by_recency() {
        let mut state = StatusState::default();
        state.post(StatusNotice::neutral(StatusScope::General, "first"));
        state.post(StatusNotice::neutral(StatusScope::General, "second"));
        assert_eq!(state.current().unwrap().text, "second");
    }
}
