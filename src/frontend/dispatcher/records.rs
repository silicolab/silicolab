use super::*;
use crate::backend::records::{
    Category, Content, Record, Scope, Source, artifacts::Artifact, qm::QmFacts,
};

fn execution_record(
    state: &AppState,
    job: &str,
    id: String,
    content: Content,
    brief: String,
) -> anyhow::Result<Record> {
    let task_id = state
        .tasks
        .runs
        .task_run_id_for_job(job)
        .ok_or_else(|| anyhow!("execution not found: {job}"))?;
    let task = state
        .tasks
        .task_run(task_id)
        .ok_or_else(|| anyhow!("task not found"))?;
    let execution = state
        .tasks
        .runs
        .execution(job)
        .ok_or_else(|| anyhow!("execution not found"))?;
    Ok(Record {
        id,
        category: Category::Evidence,
        storage_version: 1,
        content_version: 1,
        revision: 1,
        source: Source::Program { job: job.into() },
        scope: Scope {
            task: Some(task_id),
            session: None,
            run_uuid: Some(task.run_uuid.clone()),
        },
        created_at_ms: execution.created_at_ms,
        supersedes: None,
        invalidated: None,
        brief,
        input_entries: task
            .inputs
            .as_ref()
            .map(|inputs| inputs.iter().map(|i| i.entry_id).collect())
            .unwrap_or_else(|| task.source_entry_id.into_iter().collect()),
        result_entries: state
            .materializations
            .get(job)
            .map(|m| m.entries.iter().map(|e| e.entry_id).collect())
            .unwrap_or_default(),
        artifacts: vec![],
        content,
    })
}

pub(crate) fn capture_qm_input(
    state: &mut AppState,
    job: &str,
    request: crate::engines::qm::QmJob,
) {
    let result = execution_record(
        state,
        job,
        format!("input:{job}"),
        Content::QmInput(Box::new(request)),
        "Immutable QM request, including input geometry and calculation conditions".into(),
    )
    .and_then(|r| state.tasks.runs.records.insert(r));
    if let Err(error) = result {
        state.report_system_error(
            crate::frontend::state::SystemSubsystem::Storage,
            format!("QM input evidence unavailable: {error}"),
        );
    }
    flush_dirty_run_graph(state);
}

pub(crate) fn save_qm_execution(
    state: &mut AppState,
    job: &str,
    outcome: &crate::engines::qm::QmOutcome,
) -> anyhow::Result<crate::backend::run_attempt::QmResult> {
    use crate::backend::run_attempt::{ArtifactStatus, QmResult};
    let id = format!("qm:{job}");
    anyhow::ensure!(
        !state.tasks.runs.records.unavailable.contains_key(&id),
        "QM evidence {id} is unavailable; refusing to overwrite its preserved artifacts"
    );
    let previous_result = state
        .tasks
        .runs
        .execution(job)
        .ok_or_else(|| anyhow!("execution not found: {job}"))?
        .qm_result
        .clone();
    anyhow::ensure!(
        previous_result
            .as_ref()
            .is_none_or(|r| r.converged == outcome.converged),
        "Conflicting QM convergence for {job}; retained original evidence"
    );
    let existing = state.tasks.runs.records.get(&id).cloned();
    if let Some(record) = &existing {
        let input = match &record.content {
            Content::Qm(facts) => facts.input_record.clone(),
            _ => None,
        };
        let facts = QmFacts::from_outcome(outcome, input);
        anyhow::ensure!(
            matches!(&record.content, Content::Qm(previous) if previous.as_ref() == &facts),
            "Conflicting QM outcome for {job}; retained original evidence"
        );
        if let Some(result) = previous_result
            && result.artifacts_complete()
        {
            return Ok(result);
        }
    }
    let task = state
        .tasks
        .runs
        .task_run_id_for_job(job)
        .and_then(|id| state.tasks.task_run(id));
    let dir = task
        .and_then(|t| t.run_dir.clone())
        .map(|p| p.join("jobs").join(job));
    let result = match &dir {
        Some(dir) => save_qm_artifacts(state, dir, outcome),
        None => QmResult {
            converged: outcome.converged,
            report: ArtifactStatus::Failed("missing owning run directory".into()),
            series: ArtifactStatus::Failed("missing owning run directory".into()),
        },
    };
    state.tasks.runs.set_qm_result(job, result.clone());
    let input_id = format!("input:{job}");
    let input = state.tasks.runs.records.get(&input_id).cloned();
    let facts = QmFacts::from_outcome(outcome, input.as_ref().map(|r| r.id.clone()));
    let record = execution_record(
        state,
        job,
        id,
        Content::Qm(Box::new(facts)),
        "QM numerical evidence; diagnostics coverage partial, inspect raw report".into(),
    )
    .and_then(|mut r| {
        if let Some(input) = input {
            r.scope = input.scope;
            r.input_entries = input.input_entries;
        }
        if let Some(task) = r.scope.task {
            if result.report == ArtifactStatus::Saved {
                r.artifacts.push(Artifact {
                    name: "report".into(),
                    task,
                    relative: PathBuf::from("jobs").join(job).join(QM_OUTPUT_FILE),
                });
            }
            if result.series == ArtifactStatus::Saved {
                r.artifacts.push(Artifact {
                    name: "series".into(),
                    task,
                    relative: PathBuf::from("jobs")
                        .join(job)
                        .join(crate::backend::runs::SERIES_FILE),
                });
            }
        }
        if let Some(existing) = &existing {
            state
                .tasks
                .runs
                .records
                .register_artifacts(&existing.id, r.artifacts)?;
            Ok(false)
        } else {
            state.tasks.runs.records.insert(r)
        }
    });
    if let Err(error) = record {
        state.report_system_error(
            crate::frontend::state::SystemSubsystem::Storage,
            format!("QM execution succeeded, evidence index unavailable: {error}"),
        );
    }
    Ok(result)
}

pub(crate) fn evidence_notice(state: &AppState, job: &str) -> String {
    let available = state.tasks.runs.records.get(&format!("qm:{job}")).is_some();
    let persistence = if state.workspace.project().is_none() {
        "memory only"
    } else if state.tasks.runs.is_dirty()
        || state.materializations.is_dirty()
        || state.project_save_error().is_some()
    {
        "project commit pending or failed"
    } else {
        "project committed"
    };
    format!(
        "evidence qm:{job}; index {}; {persistence}",
        if available {
            "available"
        } else {
            "unavailable; inspect for recovery limits"
        }
    )
}

#[cfg(test)]
mod tests;
