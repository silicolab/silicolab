//! The [`AppState`](super::AppState) feedback API: the single sanctioned write
//! path into the session log and the status slot. Producers call these; UI code
//! reads through [`AppState::session_log`]/[`AppState::status_notice`] and emits
//! actions. The methods live beside the state that owns the private
//! `session_log`/`status`/`next_command_id` fields.

use crate::frontend::state::{
    CommandId, DetailTarget, LogLevel, LogScope, NewLogEntry, OutputSource, OutputTarget,
    SessionLogStore, StatusNotice, StatusScope, SystemSubsystem,
};
use crate::job::JobId;

use super::AppState;

impl AppState {
    /// Immutable access to the session log for UI rendering.
    pub fn session_log(&self) -> &SessionLogStore {
        &self.session_log
    }

    /// The visible status notice, or `None` when the slot is empty.
    pub fn status_notice(&self) -> Option<&StatusNotice> {
        self.status.current()
    }

    /// Expire the current ephemeral notice if its lifetime has elapsed. Driven
    /// once per frame from the update loop.
    pub fn tick_status(&mut self, now: std::time::Instant) {
        self.status.tick(now);
    }

    /// Freeze or resume status expiry while a linked notice is hovered/focused.
    pub fn set_status_paused(&mut self, paused: bool, now: std::time::Instant) {
        self.status.set_paused(paused, now);
    }

    /// Dismiss the current status notice. Its canonical log/job record is untouched.
    pub fn acknowledge_status(&mut self) {
        self.status.acknowledge();
    }

    /// Allocate a fresh [`CommandId`] for one console invocation, so its prompt,
    /// result, and error entries can share it.
    pub fn allocate_command_id(&mut self) -> CommandId {
        let id = self.next_command_id;
        self.next_command_id += 1;
        id
    }

    /// Append one session-log entry. The single write path into the store from
    /// state/dispatcher code.
    pub fn append_log(&mut self, entry: NewLogEntry) {
        self.session_log.append(entry);
    }

    pub fn clear_log_fold_boundary(&mut self) -> crate::frontend::state::SessionSeq {
        self.session_log.break_folding()
    }

    /// Append engine/workflow text scoped to one exact execution.
    pub fn append_job_log(&mut self, job_id: JobId, level: LogLevel, text: impl Into<String>) {
        self.append_log(NewLogEntry::new(LogScope::Job { job_id }, level, text));
    }

    /// Append control-plane detail (application/project/settings/file/persistence/
    /// update/rendering).
    pub fn log_system(
        &mut self,
        subsystem: SystemSubsystem,
        level: LogLevel,
        text: impl Into<String>,
    ) {
        self.append_log(NewLogEntry::new(
            LogScope::System { subsystem },
            level,
            text,
        ));
    }

    /// Append remote control-plane detail (SSH/Slurm/transfer/probe). The id is a
    /// stable host configuration id, never a display label.
    pub fn log_remote(
        &mut self,
        host_id: Option<String>,
        level: LogLevel,
        text: impl Into<String>,
    ) {
        self.append_log(NewLogEntry::new(
            LogScope::RemoteControl { host_id },
            level,
            text,
        ));
    }

    /// Append assistant infrastructure/tool audit detail (not narration, not a
    /// `.sls` command).
    pub fn log_agent(&mut self, turn_id: Option<u64>, level: LogLevel, text: impl Into<String>) {
        self.append_log(NewLogEntry::new(LogScope::Agent { turn_id }, level, text));
    }

    /// Post a status notice, subject to the expiry/priority rules.
    pub fn show_status(&mut self, notice: StatusNotice) {
        self.status.post(notice);
    }

    /// A transient neutral confirmation (selection/view/dock changes).
    pub fn status_neutral(&mut self, text: impl Into<String>) {
        self.show_status(StatusNotice::neutral(StatusScope::General, text));
    }

    /// A transient success confirmation for a completed action.
    pub fn status_success(&mut self, text: impl Into<String>) {
        self.show_status(StatusNotice::success(StatusScope::General, text));
    }

    /// A sticky warning that needs acknowledgement but no separate log record.
    pub fn status_warning(&mut self, text: impl Into<String>) {
        self.show_status(StatusNotice::warning(StatusScope::General, text));
    }

    /// A sticky error that needs acknowledgement but no separate log record — for
    /// a failure whose full detail the immediate message already carries.
    pub fn status_error(&mut self, text: impl Into<String>) {
        self.show_status(StatusNotice::error(StatusScope::General, text));
    }

    /// Record a subsystem failure the user may revisit: one canonical System error
    /// entry plus one linked sticky error status, from a single message. A Settings
    /// failure links to the Settings panel; the rest link to their System log.
    pub fn report_system_error(&mut self, subsystem: SystemSubsystem, text: impl Into<String>) {
        let text = text.into();
        let target = match subsystem {
            SystemSubsystem::Settings => DetailTarget::Settings,
            _ => DetailTarget::Output(OutputTarget::Source(OutputSource::System)),
        };
        self.apply_log_and_status(
            NewLogEntry::new(
                LogScope::System { subsystem },
                LogLevel::Error,
                text.clone(),
            ),
            StatusNotice::error(StatusScope::Subsystem(subsystem), text).with_target(target),
        );
    }

    /// Record a remote control-plane failure the user may revisit: one canonical
    /// Remote log plus a host-scoped sticky error status, so a later successful
    /// observation of the same host recovers it.
    pub fn report_remote_error(&mut self, host_id: impl Into<String>, text: impl Into<String>) {
        let host_id = host_id.into();
        let text = text.into();
        self.apply_log_and_status(
            NewLogEntry::new(
                LogScope::RemoteControl {
                    host_id: Some(host_id.clone()),
                },
                LogLevel::Error,
                text.clone(),
            ),
            StatusNotice::error(StatusScope::Remote(host_id), text).with_target(
                DetailTarget::Output(OutputTarget::Source(OutputSource::Remote)),
            ),
        );
    }

    pub fn report_unscoped_remote_error(&mut self, text: impl Into<String>) {
        let text = text.into();
        self.apply_log_and_status(
            NewLogEntry::new(
                LogScope::RemoteControl { host_id: None },
                LogLevel::Error,
                text.clone(),
            ),
            StatusNotice::error(StatusScope::General, text).with_target(DetailTarget::Output(
                OutputTarget::Source(OutputSource::Remote),
            )),
        );
    }

    /// A transient neutral notice about one job (e.g. a cancel acknowledgement),
    /// scoped so a later same-job notice supersedes it.
    pub fn job_notice(&mut self, job_id: JobId, text: impl Into<String>) {
        self.show_status(StatusNotice::neutral(StatusScope::Job(job_id), text));
    }

    /// Record a job's successful completion: one lifecycle Job log plus a linked
    /// success status that opens the job in Activity.
    pub fn job_succeeded(&mut self, job_id: JobId, text: impl Into<String>) {
        let text = text.into();
        self.apply_log_and_status(
            NewLogEntry::new(LogScope::Job { job_id }, LogLevel::Info, text.clone()),
            StatusNotice::success(StatusScope::Job(job_id), text)
                .with_target(DetailTarget::ActivityJob(job_id)),
        );
    }

    /// Record a job's failure: one lifecycle Job error log plus a linked sticky
    /// error status that opens the job in Activity.
    pub fn job_failed(&mut self, job_id: JobId, text: impl Into<String>) {
        let text = text.into();
        self.apply_log_and_status(
            NewLogEntry::new(LogScope::Job { job_id }, LogLevel::Error, text.clone()),
            StatusNotice::error(StatusScope::Job(job_id), text)
                .with_target(DetailTarget::ActivityJob(job_id)),
        );
    }

    fn apply_log_and_status(&mut self, log: NewLogEntry, status: StatusNotice) {
        self.append_log(log);
        self.show_status(status);
    }
}
