//! Guard options a console line can put before its command, and the checks they
//! drive: `--entry #id[@revision]` pins the entries a run may take as input,
//! `--allow-stale` accepts inputs from out-of-date upstream results, and
//! `--request-id` makes a retried command return the earlier run instead of
//! starting another. A strict script (an agent harness run) installs a guard on
//! every line, so a run whose inputs were not pinned is refused.
//!
//! The pins are checked before the command runs; the inputs are checked where a
//! run binds them ([`crate::frontend::dispatcher::bind_task_inputs`]), which every
//! compute path goes through.

use std::collections::BTreeSet;

use anyhow::{Result, anyhow, bail};
use clap::Args;

use super::{ScriptContext, grammar::Command};
use crate::backend::{
    freshness::{self, UpstreamProblem},
    storage::task_status_token,
    tasks::{TaskInput, TaskStatus},
};
use crate::frontend::state::AppState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EntryPin {
    pub(crate) entry_id: u64,
    pub(crate) revision: Option<u64>,
}

pub(crate) fn parse_entry_pin(value: &str) -> Result<EntryPin, String> {
    let body = value.strip_prefix('#').unwrap_or(value);
    let (id, revision) = match body.split_once('@') {
        Some((id, revision)) => (id, Some(revision)),
        None => (body, None),
    };
    let invalid = || format!("expected `#id` or `#id@revision`, got `{value}`");
    Ok(EntryPin {
        entry_id: id.parse().map_err(|_| invalid())?,
        revision: revision
            .map(|revision| revision.parse().map_err(|_| invalid()))
            .transpose()?,
    })
}

/// Options accepted before any command, e.g.
/// `--entry #3@2 --request-id opt-1 qm optimize`.
#[derive(Debug, Default, Args)]
pub(crate) struct GuardArgs {
    /// Pin an input entry, optionally at the revision you expect (`#3@2`). The
    /// first pinned entry becomes active. Repeat for runs with several inputs.
    #[arg(long = "entry", value_parser = parse_entry_pin)]
    pub(crate) entries: Vec<EntryPin>,
    /// Accept inputs whose upstream result is stale or did not complete; the run
    /// records that it did.
    #[arg(long)]
    pub(crate) allow_stale: bool,
    /// Retry key: if a run with this id completed or is running, report it
    /// instead of starting another.
    #[arg(long)]
    pub(crate) request_id: Option<String>,
}

impl GuardArgs {
    fn is_empty(&self) -> bool {
        self.entries.is_empty() && !self.allow_stale && self.request_id.is_none()
    }
}

#[derive(Debug, Clone, Default)]
pub struct CommandGuard {
    pins: Vec<EntryPin>,
    allow_stale: bool,
    request_id: Option<String>,
    strict: bool,
}

impl CommandGuard {
    /// Whether the line runs under a strict script's rules.
    pub(crate) fn strict(&self) -> bool {
        self.strict
    }

    pub(crate) fn request_id(&self) -> Option<&str> {
        self.request_id.as_deref()
    }

    /// Check the inputs a run is about to bind. Returns whether a stale input was
    /// accepted because of `--allow-stale`.
    pub(crate) fn admit(&self, state: &AppState, inputs: &[TaskInput]) -> Result<bool> {
        if self.strict
            && let Some(input) = inputs
                .iter()
                .find(|input| !self.pins.iter().any(|pin| pin.entry_id == input.entry_id))
        {
            bail!(
                "the {} input #{} was not pinned; put `--entry #{}@{}` before the command \
                 (`silicolab status` lists entry revisions)",
                input.role,
                input.entry_id,
                input.entry_id,
                input.revision,
            );
        }
        let assessed = freshness::assess(&state.tasks, &state.entries);
        let mut accepted = false;
        for input in inputs {
            let Some(problem) =
                freshness::upstream_problem(&state.tasks, &assessed, input.entry_id)
            else {
                continue;
            };
            if !self.allow_stale {
                bail!(
                    "the {} input #{} {}; rerun that step, or put `--allow-stale` before the \
                     command to use it anyway",
                    input.role,
                    input.entry_id,
                    describe(&problem),
                );
            }
            accepted = true;
        }
        Ok(accepted)
    }
}

/// Whether the command now running is a line of a strict script.
pub(crate) fn in_strict_command(state: &AppState) -> bool {
    state
        .command_guard
        .as_ref()
        .is_some_and(CommandGuard::strict)
}

fn describe(problem: &UpstreamProblem) -> String {
    match problem {
        UpstreamProblem::Stale { task_id } => {
            format!("comes from task #{task_id}, whose own inputs have changed since")
        }
        UpstreamProblem::NotCompleted { task_id, status } => format!(
            "comes from task #{task_id}, which did not complete ({})",
            task_status_token(*status)
        ),
    }
}

/// Run `command` under the guard options given before it.
pub(crate) fn run(
    state: &mut AppState,
    context: &mut ScriptContext,
    args: GuardArgs,
    command: Command,
) -> Result<String> {
    if args.is_empty() && !context.strict() {
        return command.dispatch(state, context);
    }
    if let Some(request_id) = &args.request_id
        && let Some(previous) = previous_request(state, request_id)?
    {
        return Ok(previous);
    }
    check_pins(state, &args.entries)?;
    if let Some(first) = args.entries.first() {
        state.show_entry(first.entry_id);
    }
    let guard = CommandGuard {
        pins: args.entries,
        allow_stale: args.allow_stale,
        request_id: args.request_id,
        strict: context.strict(),
    };
    let outer = state.command_guard.replace(guard);
    let result = command.dispatch(state, context);
    state.command_guard = outer;
    result
}

fn check_pins(state: &AppState, pins: &[EntryPin]) -> Result<()> {
    let mut seen = BTreeSet::new();
    for pin in pins {
        if !seen.insert(pin.entry_id) {
            bail!("entry #{} is pinned twice", pin.entry_id);
        }
        let entry = state
            .entries
            .entry(pin.entry_id)
            .ok_or_else(|| anyhow!("pinned entry #{} does not exist", pin.entry_id))?;
        if let Some(expected) = pin.revision
            && entry.revision != expected
        {
            bail!(
                "pinned entry #{} is at revision {}, not {expected}; it changed since you \
                 last looked, so check `silicolab status` before running on it",
                pin.entry_id,
                entry.revision,
            );
        }
    }
    Ok(())
}

/// The report for an earlier run with this request id, if one completed or is
/// still running. A run that failed, was cancelled, or was interrupted may be
/// retried under the same id.
fn previous_request(state: &AppState, request_id: &str) -> Result<Option<String>> {
    let Some(task) = state
        .tasks
        .tasks
        .iter()
        .filter(|task| task.request_id.as_deref() == Some(request_id))
        .max_by_key(|task| task.id)
    else {
        return Ok(None);
    };
    match task.status {
        TaskStatus::Completed => Ok(Some(format!(
            "request `{request_id}` already completed as task #{} ({}); not run again",
            task.id, task.controller_id
        ))),
        TaskStatus::Running | TaskStatus::Cancelling => bail!(
            "request `{request_id}` is still running as task #{} ({})",
            task.id,
            task.controller_id
        ),
        TaskStatus::Ready
        | TaskStatus::WaitingInput
        | TaskStatus::Failed
        | TaskStatus::Cancelled
        | TaskStatus::Interrupted => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_pins_with_and_without_revision() {
        assert_eq!(
            parse_entry_pin("#3@2"),
            Ok(EntryPin {
                entry_id: 3,
                revision: Some(2)
            })
        );
        assert_eq!(
            parse_entry_pin("7"),
            Ok(EntryPin {
                entry_id: 7,
                revision: None
            })
        );
        assert!(parse_entry_pin("#a@1").is_err());
        assert!(parse_entry_pin("#3@").is_err());
        assert!(parse_entry_pin("water").is_err());
    }
}
