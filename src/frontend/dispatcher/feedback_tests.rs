//! Integration tests for the typed feedback architecture: command-transcript
//! grouping, exact-`JobId` log isolation, typed Output reveal, and the composite
//! persistence-failure route. Store-level folding/retention live with
//! [`SessionLogStore`](crate::frontend::state::SessionLogStore); these exercise
//! the dispatcher/state seams.

use crate::frontend::console::record_console_command;
use crate::frontend::state::{
    AppState, CommandActor, DockTab, LogFilter, LogLevel, LogQuery, LogScope, OutputSource,
    OutputTarget, StaticView, StatusSeverity, SystemSubsystem,
};
use crate::job::JobId;

fn scratch() -> AppState {
    AppState::scratch(Default::default(), Vec::new())
}

fn command_entries(state: &AppState) -> Vec<(u64, CommandActor, LogLevel, String)> {
    let query = LogQuery::new(LogFilter::Command);
    state
        .session_log()
        .query(&query)
        .filter_map(|entry| match entry.scope {
            LogScope::Command { command_id, actor } => {
                Some((command_id, actor, entry.level, entry.text.clone()))
            }
            _ => None,
        })
        .collect()
}

#[test]
fn console_command_groups_prompt_and_result_under_one_command_id() {
    let mut state = scratch();
    let _ = record_console_command(&mut state, "help", CommandActor::User);

    let entries = command_entries(&state);
    assert!(entries.len() >= 2, "prompt plus at least one result line");
    let command_id = entries[0].0;
    assert!(
        entries.iter().all(|(id, ..)| *id == command_id),
        "every entry of one invocation shares its CommandId"
    );
    assert_eq!(entries[0].1, CommandActor::User);
    assert_eq!(entries[0].3, "help", "the prompt records the raw command");
}

#[test]
fn concurrent_commands_do_not_cross_associate() {
    let mut state = scratch();
    let _ = record_console_command(&mut state, "help", CommandActor::User);
    let _ = record_console_command(&mut state, "help", CommandActor::User);

    let ids: std::collections::BTreeSet<u64> = command_entries(&state)
        .into_iter()
        .map(|(id, ..)| id)
        .collect();
    assert_eq!(ids.len(), 2, "each invocation gets a distinct CommandId");
}

#[test]
fn assistant_issued_command_is_command_scoped_with_agent_actor() {
    let mut state = scratch();
    let _ = record_console_command(&mut state, "help", CommandActor::Agent);

    let entries = command_entries(&state);
    assert!(!entries.is_empty());
    assert!(
        entries
            .iter()
            .all(|(_, actor, ..)| *actor == CommandActor::Agent),
        "an assistant command stays Command-scoped with the Agent actor"
    );
    // It never leaks into the Output (non-command) surface.
    let output = LogQuery::new(LogFilter::OutputAll);
    assert_eq!(state.session_log().query(&output).count(), 0);
}

#[test]
fn console_error_is_recorded_as_a_command_error() {
    let mut state = scratch();
    let result = record_console_command(&mut state, "definitely-not-a-command", CommandActor::User);
    assert!(result.is_err());
    let entries = command_entries(&state);
    assert!(
        entries
            .iter()
            .any(|(_, _, level, _)| *level == LogLevel::Error),
        "a failed command records an error line in the transcript"
    );
}

#[test]
fn two_concurrent_same_kind_jobs_never_share_log_scope() {
    let mut state = scratch();
    let a = JobId::new();
    let b = JobId::new();
    state.append_job_log(a, LogLevel::Info, "a: scf step 1");
    state.append_job_log(b, LogLevel::Info, "b: scf step 1");
    state.append_job_log(a, LogLevel::Info, "a: scf step 2");

    let a_lines: Vec<_> = state
        .session_log()
        .tail_for_job(a, 10)
        .map(|entry| entry.text.clone())
        .collect();
    let b_lines: Vec<_> = state
        .session_log()
        .tail_for_job(b, 10)
        .map(|entry| entry.text.clone())
        .collect();
    assert_eq!(a_lines, vec!["a: scf step 1", "a: scf step 2"]);
    assert_eq!(b_lines, vec!["b: scf step 1"]);
}

#[test]
fn reveal_output_reveals_the_tab_and_applies_the_job_filter() {
    let mut state = scratch();
    let job_id = JobId::new();
    // Collapse the bottom dock so reveal must restore/activate the tab.
    state.ui.layout.dock.bottom.collapsed = true;
    super::reveal_output(&mut state, OutputTarget::Job(job_id));

    assert_eq!(state.ui.output.target, OutputTarget::Job(job_id));
    assert!(
        state
            .ui
            .layout
            .dock
            .is_visible(crate::frontend::state::DockArea::Bottom),
        "revealing Output uncollapses its dock area"
    );
    assert_eq!(
        state.ui.layout.dock.bottom.active,
        Some(DockTab::Static(StaticView::Output)),
    );
}

#[test]
fn reveal_output_to_a_job_with_no_text_keeps_it_selected() {
    let mut state = scratch();
    let job_id = JobId::new();
    super::reveal_output(&mut state, OutputTarget::Job(job_id));
    // No captured text yet, but the exact job remains the active filter.
    assert_eq!(state.ui.output.target, OutputTarget::Job(job_id));
    let query = LogQuery::new(LogFilter::Job(job_id));
    assert!(!state.session_log().any(&query));
}

#[test]
fn persistence_failure_yields_one_system_log_and_one_linked_sticky_status() {
    let mut state = scratch();
    state.report_system_error(SystemSubsystem::Storage, "disk full");

    // Exactly one canonical System entry (not two differently-worded rows).
    let query = LogQuery::new(LogFilter::Source(OutputSource::System));
    let system: Vec<_> = state
        .session_log()
        .query(&query)
        .map(|entry| (entry.level, entry.text.clone()))
        .collect();
    assert_eq!(system, vec![(LogLevel::Error, "disk full".to_string())]);

    // Plus one linked sticky error status carrying the same text.
    let notice = state.status_notice().expect("a status is posted");
    assert_eq!(notice.severity, StatusSeverity::Error);
    assert_eq!(notice.text, "disk full");
    assert!(notice.target.is_some(), "the status links to the detail");
}

#[test]
fn status_link_navigates_and_acknowledge_clears() {
    let mut state = scratch();
    state.report_system_error(SystemSubsystem::Settings, "bad config");
    assert!(state.status_notice().is_some());
    state.acknowledge_status();
    assert!(
        state.status_notice().is_none(),
        "acknowledging clears the slot; the System log record persists"
    );
    // The canonical log entry survives acknowledgement.
    let query = LogQuery::new(LogFilter::OutputAll);
    assert_eq!(state.session_log().query(&query).count(), 1);
}

#[test]
fn dock_default_bottom_order_keeps_the_five_tokens() {
    let dock = crate::frontend::state::DockModel::default();
    let tokens: Vec<&str> = dock
        .bottom
        .tabs
        .iter()
        .filter_map(|tab| match tab {
            DockTab::Static(view) => Some(view.token()),
            DockTab::Task(_) => None,
        })
        .collect();
    assert_eq!(
        tokens,
        vec!["console", "sequence", "task_monitor", "output", "plot"],
    );
}
