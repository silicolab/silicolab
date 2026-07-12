//! Read-side of the session log: the Output source/job facets and the immutable
//! query UI code runs against the store. No query allocates or clones retained
//! text; it yields references in chronological order.

use crate::job::JobId;

use super::{LogScope, SessionLogEntry, SessionLogStore, SessionSeq};

/// The Output toolbar's non-command source facet. Placement is orthogonal: a
/// remote job's stdout is a [`OutputSource::Jobs`] entry, not [`OutputSource::Remote`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OutputSource {
    System,
    Agent,
    Remote,
    Jobs,
}

impl OutputSource {
    pub fn all() -> [Self; 4] {
        [Self::System, Self::Agent, Self::Remote, Self::Jobs]
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::System => "System",
            Self::Agent => "Agent",
            Self::Remote => "Remote",
            Self::Jobs => "Jobs",
        }
    }
}

/// What a "show Output" navigation points at. Job targets carry the exact
/// [`JobId`], never only a kind, so a deep link resolves one execution.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum OutputTarget {
    All,
    Source(OutputSource),
    Job(JobId),
}

/// A short, stable rendering of a job id for selectors and labels.
pub fn short_job(job_id: JobId) -> String {
    let text = job_id.to_string();
    text.get(..8).unwrap_or(&text).to_string()
}

/// Which scopes a query admits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogFilter {
    /// The command transcript (Console).
    Command,
    /// Every non-command scope (Output's `All Output`).
    OutputAll,
    /// One Output source.
    Source(OutputSource),
    /// One exact job.
    Job(JobId),
}

impl LogFilter {
    pub fn from_output_target(target: &OutputTarget) -> Self {
        match target {
            OutputTarget::All => Self::OutputAll,
            OutputTarget::Source(source) => Self::Source(*source),
            OutputTarget::Job(job_id) => Self::Job(*job_id),
        }
    }

    fn admits(&self, scope: &LogScope) -> bool {
        match self {
            Self::Command => matches!(scope, LogScope::Command { .. }),
            Self::OutputAll => !matches!(scope, LogScope::Command { .. }),
            Self::Source(OutputSource::System) => matches!(scope, LogScope::System { .. }),
            Self::Source(OutputSource::Agent) => matches!(scope, LogScope::Agent { .. }),
            Self::Source(OutputSource::Remote) => matches!(scope, LogScope::RemoteControl { .. }),
            Self::Source(OutputSource::Jobs) => matches!(scope, LogScope::Job { .. }),
            Self::Job(job_id) => matches!(scope, LogScope::Job { job_id: id } if id == job_id),
        }
    }
}

/// An immutable query: a scope filter, a clear-view cursor, and optional
/// case-insensitive substring search. Built by view code and passed to
/// [`SessionLogStore::query`].
#[derive(Debug, Clone)]
pub struct LogQuery {
    filter: LogFilter,
    /// Entries whose latest occurrence is before this sequence are hidden by the
    /// active clear-view cursor. Zero shows everything.
    min_seq: SessionSeq,
    search_lower: Option<String>,
}

impl LogQuery {
    pub fn new(filter: LogFilter) -> Self {
        Self {
            filter,
            min_seq: 0,
            search_lower: None,
        }
    }

    pub fn cleared_before(mut self, cursor: SessionSeq) -> Self {
        self.min_seq = cursor;
        self
    }

    /// Set a case-insensitive substring search. A blank term clears it.
    pub fn search(mut self, term: &str) -> Self {
        let term = term.trim();
        self.search_lower = (!term.is_empty()).then(|| term.to_lowercase());
        self
    }

    fn matches(&self, entry: &SessionLogEntry) -> bool {
        if entry.last_seq < self.min_seq {
            return false;
        }
        if !self.filter.admits(&entry.scope) {
            return false;
        }
        if let Some(term) = &self.search_lower {
            return entry.text.to_lowercase().contains(term);
        }
        true
    }
}

impl SessionLogStore {
    /// Iterate the entries matching `query`, oldest-first. Yields references, so a
    /// per-frame render never clones the retained strings.
    pub fn query<'a>(&'a self, query: &'a LogQuery) -> impl Iterator<Item = &'a SessionLogEntry> {
        self.entries
            .values()
            .filter(move |entry| query.matches(entry))
    }

    /// Whether any entry matches `query` — the Output empty-state decision.
    pub fn any(&self, query: &LogQuery) -> bool {
        self.entries.values().any(|entry| query.matches(entry))
    }

    /// The highest sequence among entries matching `query`, for a view's unread
    /// cursor. `None` when the query is empty.
    pub fn latest_matching_seq(&self, query: &LogQuery) -> Option<SessionSeq> {
        self.entries
            .values()
            .filter(|entry| query.matches(entry))
            .map(|entry| entry.last_seq)
            .max()
    }

    /// The exact jobs that currently own at least one retained entry — the base
    /// of the Output job selector, unioned by the caller with the live-job
    /// projection so a job whose oldest text was evicted still appears.
    pub fn logged_jobs(&self) -> impl Iterator<Item = JobId> + '_ {
        self.job_bytes.keys().copied()
    }
}
