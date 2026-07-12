//! Byte-bounded, scope-fair retention for [`SessionLogStore`]. A noisy job can
//! never evict the command transcript or the control-plane diagnostics below
//! their reserves, per-job output is capped, and every eviction leaves one
//! visible marker in the scope it thinned.

use std::num::NonZeroU32;

use super::{LogLevel, LogScope, SessionLogEntry, SessionLogStore, SessionSeq};

/// Total retained text across every scope.
pub(super) const TOTAL_MAX_BYTES: usize = 8 * 1024 * 1024;
/// Retained text ceiling for one exact job scope.
pub(super) const JOB_SCOPE_MAX_BYTES: usize = 1024 * 1024;
/// Command-transcript text a job flood can never evict.
pub(super) const COMMAND_RESERVE_BYTES: usize = 256 * 1024;
/// System/Agent/Remote-control text a job flood can never evict.
pub(super) const CONTROL_RESERVE_BYTES: usize = 256 * 1024;

const EVICTION_MESSAGE: &str = "Earlier output was discarded to limit memory usage.";

impl SessionLogStore {
    /// Bring every budget back within limits after an append: cap per-job scopes
    /// first, then evict global-oldest eligible entries until the total fits.
    /// Each thinned scope gets one folded eviction marker.
    pub(super) fn enforce_retention(&mut self) {
        let mut thinned: Vec<LogScope> = Vec::new();
        self.evict_over_cap_jobs(&mut thinned);
        while self.retained_bytes > TOTAL_MAX_BYTES {
            match self.evict_one_global() {
                Some(scope) => push_unique(&mut thinned, scope),
                None => break,
            }
        }
        for scope in thinned {
            self.note_eviction(scope);
        }
        self.trim_marker_overflow();
    }

    /// Evict the oldest entries of any job scope over its per-job cap.
    fn evict_over_cap_jobs(&mut self, thinned: &mut Vec<LogScope>) {
        let over_cap: Vec<crate::job::JobId> = self
            .job_bytes
            .iter()
            .filter(|&(_, &bytes)| bytes > JOB_SCOPE_MAX_BYTES)
            .map(|(job_id, _)| *job_id)
            .collect();
        for job_id in over_cap {
            let scope = LogScope::Job { job_id };
            while self.job_bytes.get(&job_id).copied().unwrap_or(0) > JOB_SCOPE_MAX_BYTES {
                let Some(oldest) = self
                    .scope_index
                    .get(&scope)
                    .and_then(|set| set.iter().next().copied())
                else {
                    break;
                };
                if self.remove_entry(oldest).is_some() {
                    push_unique(thinned, scope.clone());
                }
            }
        }
    }

    /// Evict the globally-oldest entry eligible under the reserves: job entries are
    /// always eligible; command and control entries only once their category
    /// exceeds its reserve. Returns the scope thinned, or `None` when nothing is
    /// eligible.
    fn evict_one_global(&mut self) -> Option<LogScope> {
        let mut victim: Option<SessionSeq> = None;
        let mut consider = |seq: Option<SessionSeq>| {
            if let Some(seq) = seq {
                victim = Some(match victim {
                    Some(current) => current.min(seq),
                    None => seq,
                });
            }
        };
        consider(self.job_seqs.iter().next().copied());
        if self.command_bytes > COMMAND_RESERVE_BYTES {
            consider(self.command_seqs.iter().next().copied());
        }
        if self.control_bytes > CONTROL_RESERVE_BYTES {
            consider(self.control_seqs.iter().next().copied());
        }
        let victim = victim?;
        self.remove_entry(victim).map(|entry| entry.scope)
    }

    /// Remove one entry by key, updating every byte counter and index. Returns the
    /// removed entry, or `None` if it was already gone. A removed eviction marker
    /// is forgotten so its scope can grow a fresh one later.
    fn remove_entry(&mut self, seq: SessionSeq) -> Option<SessionLogEntry> {
        let entry = self.entries.remove(&seq)?;
        let bytes = entry.text.len();
        self.retained_bytes -= bytes;
        match &entry.scope {
            scope if scope.is_command() => {
                self.command_bytes -= bytes;
                self.command_seqs.remove(&seq);
            }
            scope if scope.is_control() => {
                self.control_bytes -= bytes;
                self.control_seqs.remove(&seq);
            }
            LogScope::Job { job_id } => {
                if let Some(job_bytes) = self.job_bytes.get_mut(job_id) {
                    *job_bytes -= bytes;
                    if *job_bytes == 0 {
                        self.job_bytes.remove(job_id);
                    }
                }
                self.job_seqs.remove(&seq);
            }
            _ => {}
        }
        if let Some(set) = self.scope_index.get_mut(&entry.scope) {
            set.remove(&seq);
            if set.is_empty() {
                self.scope_index.remove(&entry.scope);
            }
        }
        if self.eviction_markers.get(&entry.scope) == Some(&seq) {
            self.eviction_markers.remove(&entry.scope);
        }
        if self.last_touched == Some(seq) {
            self.last_touched = None;
        }
        Some(entry)
    }

    /// Insert or fold the one eviction marker for a thinned scope. The marker is
    /// itself bounded (a removed marker generates no new marker) so warnings can
    /// never recurse.
    fn note_eviction(&mut self, scope: LogScope) {
        if let Some(&key) = self.eviction_markers.get(&scope)
            && let Some(marker) = self.entries.get_mut(&key)
        {
            marker.repeat = marker.repeat.saturating_add(1);
            marker.last_seq = self.next_seq;
            self.next_seq += 1;
            return;
        }
        let seq = self.next_seq;
        self.next_seq += 1;
        let marker = SessionLogEntry {
            first_seq: seq,
            last_seq: seq,
            scope: scope.clone(),
            level: LogLevel::Warn,
            text: EVICTION_MESSAGE.to_string(),
            repeat: NonZeroU32::MIN,
        };
        // Markers never anchor folding of the next real event and are inserted
        // without re-running retention, keeping the warning bounded.
        self.insert_entry(marker, false);
        self.eviction_markers.insert(scope, seq);
    }

    /// Markers are accounted like every other row, so inserting them may push a
    /// scope or the store back over its hard ceiling. Trim that overflow without
    /// producing markers for markers.
    fn trim_marker_overflow(&mut self) {
        let over_cap: Vec<_> = self
            .job_bytes
            .iter()
            .filter(|&(_, &bytes)| bytes > JOB_SCOPE_MAX_BYTES)
            .map(|(&job_id, _)| job_id)
            .collect();
        for job_id in over_cap {
            let scope = LogScope::Job { job_id };
            while self.job_bytes.get(&job_id).copied().unwrap_or(0) > JOB_SCOPE_MAX_BYTES {
                let Some(oldest) = self
                    .scope_index
                    .get(&scope)
                    .and_then(|set| set.iter().next().copied())
                else {
                    break;
                };
                self.remove_entry(oldest);
            }
        }
        while self.retained_bytes > TOTAL_MAX_BYTES {
            if self.evict_one_global().is_none() {
                break;
            }
        }
    }
}

fn push_unique(scopes: &mut Vec<LogScope>, scope: LogScope) {
    if !scopes.contains(&scope) {
        scopes.push(scope);
    }
}
