//! Typed, byte-bounded session log: the single owner of the application's
//! free-text feedback (command transcript, system/agent/remote-control detail,
//! and per-job engine output), read by Console and Output.
//!
//! Every entry carries an exact [`LogScope`], a [`LogLevel`], and a stable
//! chronological sequence. Job output is keyed by the task-monitor [`JobId`], so
//! two concurrent jobs of the same kind never share a log scope. Retention is
//! bounded by bytes with per-scope fairness (see [`retention`]), and adjacent
//! repeats fold into a single row with a count.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::num::NonZeroU32;

use crate::job::JobId;

mod query;
mod retention;

#[cfg(test)]
mod tests;

pub use query::{LogFilter, LogQuery, OutputSource, OutputTarget, short_job};

/// Monotonic, session-local order of a log *event*. Every append allocates one,
/// including a folded repeat (its sequence advances `last_seq`), so ordering is
/// total and gap-tolerant. Not persisted.
pub type SessionSeq = u64;

/// Session-local identity of one `.sls` command invocation. Prompt, result, and
/// error entries of the same invocation share it, so concurrent or nested command
/// execution never cross-associates its output.
pub type CommandId = u64;

/// Severity of a log entry. Independent of scope, source, and placement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LogLevel {
    Trace,
    Info,
    Warn,
    Error,
}

/// Who issued a console command. The command transcript is the same question —
/// "what command ran?" — regardless of actor, so assistant-issued `.sls` commands
/// are Command-scoped with [`CommandActor::Agent`], not routed to Output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CommandActor {
    User,
    Agent,
}

/// The stable, closed vocabulary of application subsystems that own System-scoped
/// detail. Kept typed so call sites can never drift subsystem names as ad-hoc
/// strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SystemSubsystem {
    Application,
    Project,
    File,
    Settings,
    Update,
    Storage,
}

impl SystemSubsystem {
    pub fn label(self) -> &'static str {
        match self {
            Self::Application => "Application",
            Self::Project => "Project",
            Self::File => "File",
            Self::Settings => "Settings",
            Self::Update => "Update",
            Self::Storage => "Storage",
        }
    }
}

/// The exact owner of a log entry. Job kind, placement, and engine label are not
/// encoded here: they are display/filter facets resolved from the live-job
/// projection by [`JobId`], not a second identity.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum LogScope {
    /// A `.sls` command transcript line (prompt, result, or error).
    Command {
        command_id: CommandId,
        actor: CommandActor,
    },
    /// Application/project/settings/file/persistence/update/rendering detail.
    System { subsystem: SystemSubsystem },
    /// Assistant infrastructure/tool audit detail — not assistant narration and
    /// not assistant-issued `.sls` commands.
    Agent { turn_id: Option<u64> },
    /// SSH/Slurm/transfer/probe and other remote control-plane detail. The id is
    /// the stable host configuration id, never a display label.
    RemoteControl { host_id: Option<String> },
    /// Engine/workflow text scoped to one exact execution, local or remote.
    Job { job_id: JobId },
}

impl LogScope {
    fn is_command(&self) -> bool {
        matches!(self, Self::Command { .. })
    }

    /// Whether this scope counts against the reserved control-plane budget
    /// (System, Agent, and Remote-control detail share one reserve).
    fn is_control(&self) -> bool {
        matches!(
            self,
            Self::System { .. } | Self::Agent { .. } | Self::RemoteControl { .. }
        )
    }
}

/// One retained log row. Adjacent identical events fold into it: `repeat` counts
/// the occurrences and `last_seq` tracks the latest, while `first_seq` and list
/// position are immutable so chronology never reorders. Wall-clock timestamps are
/// deliberately absent.
#[derive(Debug, Clone)]
pub struct SessionLogEntry {
    pub first_seq: SessionSeq,
    pub last_seq: SessionSeq,
    pub scope: LogScope,
    pub level: LogLevel,
    pub text: String,
    pub repeat: NonZeroU32,
}

impl SessionLogEntry {
    pub fn repeat_count(&self) -> u32 {
        self.repeat.get()
    }
}

/// A caller's request to append one log event. The store assigns sequence and
/// timestamps and decides folding, truncation, and retention.
#[derive(Debug, Clone)]
pub struct NewLogEntry {
    pub scope: LogScope,
    pub level: LogLevel,
    pub text: String,
}

impl NewLogEntry {
    pub fn new(scope: LogScope, level: LogLevel, text: impl Into<String>) -> Self {
        Self {
            scope,
            level,
            text: text.into(),
        }
    }
}

/// The byte-bounded, scope-fair, chronology-preserving session log store. Only
/// dispatcher/state-application code appends; UI code receives immutable queries.
///
/// Entries are keyed by their immutable `first_seq`, so iteration in key order is
/// chronological and survives front eviction. Per-category ordered key sets and a
/// per-scope index make retention and the Activity job-tail lookup logarithmic,
/// never a full-store scan.
#[derive(Default)]
pub struct SessionLogStore {
    entries: BTreeMap<SessionSeq, SessionLogEntry>,
    next_seq: SessionSeq,
    /// The entry that received the most recent sequence, for global-adjacency
    /// folding. `None` after its entry is evicted (only possible when the store
    /// is emptied, since eviction removes the oldest).
    last_touched: Option<SessionSeq>,
    retained_bytes: usize,
    command_bytes: usize,
    control_bytes: usize,
    job_bytes: HashMap<JobId, usize>,
    /// `first_seq` keys grouped by exact scope (for the job tail and per-job cap).
    scope_index: HashMap<LogScope, BTreeSet<SessionSeq>>,
    command_seqs: BTreeSet<SessionSeq>,
    control_seqs: BTreeSet<SessionSeq>,
    job_seqs: BTreeSet<SessionSeq>,
    /// The live eviction-warning row per scope, so repeated eviction updates one
    /// marker instead of flooding the scope with duplicates.
    eviction_markers: HashMap<LogScope, SessionSeq>,
}

impl SessionLogStore {
    /// Append one event: fold it into the previous globally-adjacent entry when
    /// scope, level, and normalized text all match; otherwise truncate to the
    /// single-entry limit, insert a new row, and enforce retention.
    pub fn append(&mut self, entry: NewLogEntry) {
        let NewLogEntry { scope, level, text } = entry;
        let text = normalize_newlines(text);
        let seq = self.alloc_seq();

        if let Some(prev_key) = self.last_touched
            && let Some(prev) = self.entries.get_mut(&prev_key)
            && prev.scope == scope
            && prev.level == level
            && prev.text == text
        {
            prev.repeat = prev.repeat.saturating_add(1);
            prev.last_seq = seq;
            return;
        }

        let (text, _omitted) = truncate_entry_text(text);
        let stored = SessionLogEntry {
            first_seq: seq,
            last_seq: seq,
            scope,
            level,
            text,
            repeat: NonZeroU32::MIN,
        };
        self.insert_entry(stored, true);
        self.enforce_retention();
    }

    /// The last six (by default `limit`) canonical entries scoped to one exact
    /// job, oldest-first — the Activity card's log tail. Indexed, never a scan.
    pub fn tail_for_job(
        &self,
        job_id: JobId,
        limit: usize,
    ) -> impl Iterator<Item = &SessionLogEntry> {
        let scope = LogScope::Job { job_id };
        let keys = self.scope_index.get(&scope);
        let start = keys.map(|set| set.len().saturating_sub(limit)).unwrap_or(0);
        keys.into_iter()
            .flat_map(|set| set.iter())
            .skip(start)
            .filter_map(move |seq| self.entries.get(seq))
    }

    /// The sequence the next event will take — the value a clear-view cursor is
    /// set to so every current entry falls before it, and the high-water mark a
    /// view compares against its last-seen cursor for unread state.
    #[cfg(test)]
    pub fn next_seq(&self) -> SessionSeq {
        self.next_seq
    }

    /// Prevent the next event from folding into a row that a view has just
    /// hidden behind a clear cursor.
    pub fn break_folding(&mut self) -> SessionSeq {
        self.last_touched = None;
        self.next_seq
    }

    #[cfg(test)]
    pub fn retained_bytes(&self) -> usize {
        self.retained_bytes
    }

    fn alloc_seq(&mut self) -> SessionSeq {
        let seq = self.next_seq;
        self.next_seq += 1;
        seq
    }

    /// Insert a fully-formed entry, updating every index and byte counter.
    /// `track_adjacency` is false for synthetic markers so they never become the
    /// fold target of the next real event.
    fn insert_entry(&mut self, entry: SessionLogEntry, track_adjacency: bool) {
        let key = entry.first_seq;
        let bytes = entry.text.len();
        self.retained_bytes += bytes;
        match &entry.scope {
            scope if scope.is_command() => {
                self.command_bytes += bytes;
                self.command_seqs.insert(key);
            }
            scope if scope.is_control() => {
                self.control_bytes += bytes;
                self.control_seqs.insert(key);
            }
            LogScope::Job { job_id } => {
                *self.job_bytes.entry(*job_id).or_default() += bytes;
                self.job_seqs.insert(key);
            }
            _ => {}
        }
        self.scope_index
            .entry(entry.scope.clone())
            .or_default()
            .insert(key);
        self.entries.insert(key, entry);
        if track_adjacency {
            self.last_touched = Some(key);
        }
    }
}

/// Normalize only CRLF to LF so folding treats otherwise-identical lines from a
/// Windows-native engine and a WSL engine as the same event. No other bytes move.
fn normalize_newlines(text: String) -> String {
    if text.contains('\r') {
        text.replace("\r\n", "\n").replace('\r', "\n")
    } else {
        text
    }
}

/// Longest char boundary at or before `index`, so oversized text truncates
/// without splitting a UTF-8 code point.
fn floor_char_boundary(text: &str, mut index: usize) -> usize {
    if index >= text.len() {
        return text.len();
    }
    while index > 0 && !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

/// Largest text a single entry may retain before it is truncated once, on a valid
/// UTF-8 boundary, with a marker naming the omitted byte count.
const ENTRY_MAX_BYTES: usize = 64 * 1024;

/// Truncate an oversized entry to the single-entry limit, returning the kept text
/// (ending in a visible marker) and the number of omitted bytes. Text within the
/// limit is returned unchanged with zero omitted.
fn truncate_entry_text(text: String) -> (String, usize) {
    if text.len() <= ENTRY_MAX_BYTES {
        return (text, 0);
    }
    // Reserve headroom for the marker so the retained string stays within budget.
    let keep = floor_char_boundary(&text, ENTRY_MAX_BYTES.saturating_sub(64));
    let omitted = text.len() - keep;
    let mut kept = text[..keep].to_string();
    kept.push_str(&format!(
        "\n… [{omitted} bytes truncated to limit memory usage]"
    ));
    (kept, omitted)
}
