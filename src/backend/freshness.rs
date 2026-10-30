//! Whether a completed task still describes the project's current entries.
//!
//! A task records the revision of each input entry when it was bound, and the
//! revision of its anchor entry when it completed. Comparing those with the
//! entries' current revisions tells whether the result is out of date. Staleness
//! of an upstream task propagates to tasks that consumed the entry it produced,
//! but only when the upstream result itself derives from outdated inputs: an
//! upstream whose own result entry was merely edited afterwards does not taint a
//! consumer that bound the edited revision.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::backend::{
    entries::EntryStore,
    tasks::{TaskManager, TaskOutcome, TaskRun, TaskStatus},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Freshness {
    /// Every recorded input and the result still match the current entries.
    Current,
    Stale(Vec<StaleReason>),
    /// Completed, but recorded before inputs or result revisions were tracked.
    Unknown,
    /// Not completed, so there is no result to judge.
    NotApplicable,
}

impl Freshness {
    pub fn token(&self) -> &'static str {
        match self {
            Self::Current => "current",
            Self::Stale(_) => "stale",
            Self::Unknown => "unknown",
            Self::NotApplicable => "not_applicable",
        }
    }

    pub fn reasons(&self) -> &[StaleReason] {
        match self {
            Self::Stale(reasons) => reasons,
            _ => &[],
        }
    }

    fn taints_consumers(&self) -> bool {
        self.reasons().iter().any(StaleReason::propagates)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StaleReason {
    InputEdited {
        role: String,
        entry_id: u64,
        recorded_revision: u64,
        current_revision: u64,
    },
    InputDeleted {
        role: String,
        entry_id: u64,
    },
    ResultEdited {
        entry_id: u64,
        recorded_revision: u64,
        current_revision: u64,
    },
    ResultDeleted {
        entry_id: u64,
    },
    UpstreamStale {
        task_id: u64,
        entry_id: u64,
    },
}

impl StaleReason {
    fn propagates(&self) -> bool {
        matches!(
            self,
            Self::InputEdited { .. } | Self::InputDeleted { .. } | Self::UpstreamStale { .. }
        )
    }
}

/// Assess every task, keyed by task id.
pub fn assess(tasks: &TaskManager, entries: &EntryStore) -> BTreeMap<u64, Freshness> {
    let mut ordered: Vec<&TaskRun> = tasks.tasks.iter().collect();
    ordered.sort_by_key(|task| task.id);
    let mut assessed = BTreeMap::new();
    for task in ordered {
        let freshness = assess_task(task, tasks, entries, &assessed);
        assessed.insert(task.id, freshness);
    }
    assessed
}

fn assess_task(
    task: &TaskRun,
    tasks: &TaskManager,
    entries: &EntryStore,
    earlier: &BTreeMap<u64, Freshness>,
) -> Freshness {
    if task.status != TaskStatus::Completed {
        return Freshness::NotApplicable;
    }
    let Some(inputs) = task.inputs.as_deref() else {
        return Freshness::Unknown;
    };
    // Without a result revision, an edit-in-place run's own edit is
    // indistinguishable from a later one.
    if task.result_revision.is_none() && task.outcome == TaskOutcome::EditInPlace {
        return Freshness::Unknown;
    }

    let anchor = task.anchor_entry_id();
    let mut reasons = Vec::new();
    for input in inputs {
        // An input the run anchors to (a report, or an in-place edit) is compared
        // against its revision at completion, which includes the run's own edit.
        let expected = match task.result_revision {
            Some(revision) if anchor == Some(input.entry_id) => revision,
            _ => input.revision,
        };
        match entries.entry(input.entry_id) {
            None => reasons.push(StaleReason::InputDeleted {
                role: input.role.clone(),
                entry_id: input.entry_id,
            }),
            Some(entry) if entry.revision != expected => reasons.push(StaleReason::InputEdited {
                role: input.role.clone(),
                entry_id: input.entry_id,
                recorded_revision: expected,
                current_revision: entry.revision,
            }),
            Some(_) => {}
        }
        if let Some(upstream) = producer_before(tasks, input.entry_id, task.id)
            && earlier
                .get(&upstream)
                .is_some_and(Freshness::taints_consumers)
        {
            reasons.push(StaleReason::UpstreamStale {
                task_id: upstream,
                entry_id: input.entry_id,
            });
        }
    }
    let anchor_is_input = inputs.iter().any(|input| Some(input.entry_id) == anchor);
    if let (Some(entry_id), Some(recorded), false) = (anchor, task.result_revision, anchor_is_input)
    {
        match entries.entry(entry_id) {
            None => reasons.push(StaleReason::ResultDeleted { entry_id }),
            Some(entry) if entry.revision != recorded => reasons.push(StaleReason::ResultEdited {
                entry_id,
                recorded_revision: recorded,
                current_revision: entry.revision,
            }),
            Some(_) => {}
        }
    }

    if reasons.is_empty() {
        Freshness::Current
    } else {
        Freshness::Stale(reasons)
    }
}

/// The latest completed task created before `before` that produced `entry_id`.
fn producer_before(tasks: &TaskManager, entry_id: u64, before: u64) -> Option<u64> {
    tasks
        .tasks
        .iter()
        .filter(|task| {
            task.id < before
                && task.status == TaskStatus::Completed
                && task.result_entry_id == Some(entry_id)
        })
        .map(|task| task.id)
        .max()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::backend::tasks::{TaskInput, task_controller_by_id};
    use crate::domain::Structure;

    fn entry(entries: &mut EntryStore, name: &str) -> u64 {
        entries.add_entry(
            Structure::new(name, Vec::new()),
            None,
            PathBuf::from(format!("{name}.xyz")),
        )
    }

    fn edit(entries: &mut EntryStore, id: u64) {
        let entry = entries.entry_mut(id).unwrap();
        entry.revision += 1;
    }

    fn complete(
        tasks: &mut TaskManager,
        entries: &EntryStore,
        controller: &str,
        input: u64,
        result: Option<u64>,
    ) -> u64 {
        let id = tasks.create_task_run(*task_controller_by_id(controller).unwrap());
        let task = tasks.task_run_mut(id).unwrap();
        task.inputs = Some(vec![TaskInput {
            role: "primary".into(),
            entry_id: input,
            revision: entries.entry(input).unwrap().revision,
        }]);
        task.source_entry_id = Some(input);
        task.result_entry_id = result;
        task.status = TaskStatus::Completed;
        let anchor = task.anchor_entry_id().unwrap();
        task.result_revision = Some(entries.entry(anchor).unwrap().revision);
        id
    }

    #[test]
    fn untouched_chain_is_current() {
        let mut entries = EntryStore::new_empty();
        let mut tasks = TaskManager::default();
        let a = entry(&mut entries, "A");
        let b = entry(&mut entries, "B");
        let build = complete(&mut tasks, &entries, "build-md-system", a, Some(b));
        let energy = complete(&mut tasks, &entries, "qm-energy", b, None);
        let assessed = assess(&tasks, &entries);
        assert_eq!(assessed[&build], Freshness::Current);
        assert_eq!(assessed[&energy], Freshness::Current);
    }

    #[test]
    fn editing_an_input_marks_the_chain_stale() {
        let mut entries = EntryStore::new_empty();
        let mut tasks = TaskManager::default();
        let a = entry(&mut entries, "A");
        let b = entry(&mut entries, "B");
        let build = complete(&mut tasks, &entries, "build-md-system", a, Some(b));
        let energy = complete(&mut tasks, &entries, "qm-energy", b, None);
        edit(&mut entries, a);
        let assessed = assess(&tasks, &entries);
        assert!(matches!(
            assessed[&build].reasons(),
            [StaleReason::InputEdited { entry_id, .. }] if *entry_id == a
        ));
        assert!(matches!(
            assessed[&energy].reasons(),
            [StaleReason::UpstreamStale { task_id, .. }] if *task_id == build
        ));
    }

    #[test]
    fn editing_a_result_stales_its_producer_but_not_later_consumers() {
        let mut entries = EntryStore::new_empty();
        let mut tasks = TaskManager::default();
        let a = entry(&mut entries, "A");
        let b = entry(&mut entries, "B");
        let build = complete(&mut tasks, &entries, "build-md-system", a, Some(b));
        edit(&mut entries, b);
        let energy = complete(&mut tasks, &entries, "qm-energy", b, None);
        let assessed = assess(&tasks, &entries);
        assert!(matches!(
            assessed[&build].reasons(),
            [StaleReason::ResultEdited { entry_id, .. }] if *entry_id == b
        ));
        assert_eq!(assessed[&energy], Freshness::Current);
    }

    #[test]
    fn edit_in_place_is_judged_by_its_result_revision() {
        let mut entries = EntryStore::new_empty();
        let mut tasks = TaskManager::default();
        let a = entry(&mut entries, "A");
        let hydrogens = tasks.create_task_run(*task_controller_by_id("add-hydrogens").unwrap());
        let task = tasks.task_run_mut(hydrogens).unwrap();
        task.inputs = Some(vec![TaskInput {
            role: "primary".into(),
            entry_id: a,
            revision: 0,
        }]);
        task.source_entry_id = Some(a);
        task.status = TaskStatus::Completed;
        edit(&mut entries, a);
        tasks.task_run_mut(hydrogens).unwrap().result_revision = Some(1);
        assert_eq!(assess(&tasks, &entries)[&hydrogens], Freshness::Current);

        tasks.task_run_mut(hydrogens).unwrap().result_revision = None;
        assert_eq!(assess(&tasks, &entries)[&hydrogens], Freshness::Unknown);
    }

    #[test]
    fn deleted_inputs_and_unfinished_runs() {
        let mut entries = EntryStore::new_empty();
        let mut tasks = TaskManager::default();
        let a = entry(&mut entries, "A");
        let b = entry(&mut entries, "B");
        let build = complete(&mut tasks, &entries, "build-md-system", a, Some(b));
        let pending = tasks.create_task_run(*task_controller_by_id("qm-energy").unwrap());
        entries.delete_entry(a);
        let assessed = assess(&tasks, &entries);
        assert!(matches!(
            assessed[&build].reasons(),
            [StaleReason::InputDeleted { entry_id, .. }] if *entry_id == a
        ));
        assert_eq!(assessed[&pending], Freshness::NotApplicable);
    }
}
