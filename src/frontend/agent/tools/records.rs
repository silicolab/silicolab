use crate::backend::records::{Category, Content, Record, Scope, Source};
use crate::frontend::state::AppState;
use crate::io::llm::types::ToolCall;
use anyhow::{Result, ensure};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Query {
    view: Option<String>,
    query: Option<String>,
    id: Option<String>,
    job: Option<String>,
    run_uuid: Option<String>,
    input_entry: Option<u64>,
    result_entry: Option<u64>,
    category: Option<Category>,
    content_type: Option<String>,
    task: Option<u64>,
    session: Option<u64>,
    lifecycle: Option<String>,
    offset: usize,
    limit: Option<usize>,
    artifact: Option<String>,
    byte_offset: u64,
    detail_offset: usize,
}

pub(super) fn inspect(state: &AppState, input: &Value) -> Result<String> {
    let q: Query = serde_json::from_value(input.clone())?;
    let filtered = q.id.is_some()
        || q.job.is_some()
        || q.run_uuid.is_some()
        || q.input_entry.is_some()
        || q.result_entry.is_some()
        || q.category.is_some()
        || q.content_type.is_some()
        || q.task.is_some()
        || q.session.is_some()
        || q.lifecycle.is_some();
    let view = q
        .view
        .as_deref()
        .unwrap_or(if filtered { "catalog" } else { "workspace" });
    if view == "intent" {
        ensure!(
            !filtered
                && q.query.is_none()
                && q.artifact.is_none()
                && q.offset == 0
                && q.byte_offset == 0,
            "intent accepts only detail_offset and limit for the active conversation"
        );
        let text = state
            .ui
            .agent
            .transcript
            .iter()
            .rev()
            .find_map(|entry| match entry {
                crate::frontend::agent::TranscriptEntry::User(text) => Some(text),
                _ => None,
            })
            .ok_or_else(|| anyhow::anyhow!("no user instruction in this conversation"))?;
        let count = text.chars().count();
        let limit = q.limit.unwrap_or(400);
        ensure!(
            (1..=2400).contains(&limit) && q.detail_offset <= count,
            "invalid intent limit or detail_offset"
        );
        let segment: String = text
            .chars()
            .skip(q.detail_offset)
            .take(limit.min(400))
            .collect();
        let next = q.detail_offset + segment.chars().count();
        return Ok(json!({"source":"latest user instruction; not a confirmed constraint", "session":state.ui.agent.active_conversation.raw(),
            "text":segment,"next_detail_offset":next,"truncated":next < count}).to_string());
    }
    if view == "workspace" {
        ensure!(
            !filtered
                && q.artifact.is_none()
                && q.offset == 0
                && q.byte_offset == 0
                && q.detail_offset == 0
                && q.limit.is_none(),
            "workspace view cannot use record filters or pagination"
        );
        if let Some(job) = &q.query {
            ensure!(
                state.tasks.runs.execution(job).is_some(),
                "job not found: {job}"
            );
        }
        return Ok(super::inspection::inspect(state, q.query.as_deref()));
    }
    ensure!(
        q.query.is_none(),
        "query is a workspace job selector; use job for records"
    );
    ensure!(
        ["catalog", "summary", "details", "raw", "unavailable"].contains(&view),
        "unknown inspect view"
    );
    ensure!(
        q.lifecycle
            .as_deref()
            .is_none_or(|s| ["active", "superseded", "invalidated", "stale"].contains(&s)),
        "unknown lifecycle filter"
    );
    let store = &state.tasks.runs.records;
    if view == "unavailable" {
        ensure!(
            !filtered && q.artifact.is_none() && q.byte_offset == 0 && q.detail_offset == 0,
            "unavailable view accepts only offset and limit"
        );
        let limit = q.limit.unwrap_or(20);
        ensure!((1..=20).contains(&limit), "limit must be 1..20");
        ensure!(
            q.offset <= store.unavailable.len(),
            "offset exceeds unavailable count"
        );
        let mut records = Vec::new();
        let mut used = 0;
        for (id, (_, reason)) in store.unavailable.iter().skip(q.offset).take(limit) {
            let row = json!({"id":id,"index":"unavailable","reason":reason.chars().take(300).collect::<String>()});
            let size = row.to_string().chars().count();
            if used + size > 3000 {
                break;
            }
            used += size;
            records.push(row);
        }
        let next = q.offset + records.len();
        return Ok(json!({"records":records,"total":store.unavailable.len(),"next_offset":next,"truncated":next < store.unavailable.len()}).to_string());
    }
    let matches = |r: &&Record| {
        q.id.as_ref().is_none_or(|id| id == &r.id)
            && q.job.as_deref().is_none_or(|job| r.job() == Some(job))
            && q.run_uuid
                .as_ref()
                .is_none_or(|id| r.scope.run_uuid.as_ref() == Some(id))
            && q.input_entry.is_none_or(|id| r.input_entries.contains(&id))
            && q.result_entry
                .is_none_or(|id| r.result_entries.contains(&id))
            && q.category.as_ref().is_none_or(|c| &r.category == c)
            && q.content_type
                .as_deref()
                .is_none_or(|c| r.content.kind() == c)
            && q.task.is_none_or(|t| r.scope.task == Some(t))
            && q.session.is_none_or(|s| r.scope.session == Some(s))
            && q.lifecycle.as_deref().is_none_or(|s| match s {
                "active" => store.active(r) && !store.stale(r),
                "superseded" => !store.active(r) && r.invalidated.is_none(),
                "invalidated" => r.invalidated.is_some(),
                "stale" => store.stale(r),
                _ => false,
            })
    };
    if let Some(id) = &q.id {
        ensure!(
            matches(&store.require(id)?),
            "record exists but conflicts with supplied filters"
        );
    }
    if let Some(job) = &q.job {
        ensure!(
            state.tasks.runs.execution(job).is_some(),
            "job not found: {job}"
        );
    }
    if view == "raw" || view == "details" {
        ensure!(
            q.offset == 0,
            "use byte_offset for raw or detail_offset for details"
        );
        let id =
            q.id.as_deref()
                .ok_or_else(|| anyhow::anyhow!("{view} requires exact record id"))?;
        let r = store.require(id)?;
        if view == "raw" {
            ensure!(q.detail_offset == 0, "raw does not accept detail_offset");
            let name = q
                .artifact
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("raw requires artifact name"))?;
            let artifact = r
                .artifacts
                .iter()
                .find(|a| a.name == name)
                .ok_or_else(|| anyhow::anyhow!("registered artifact not found"))?;
            let root = state
                .tasks
                .task_run(artifact.task)
                .and_then(|t| t.run_dir.as_deref())
                .ok_or_else(|| anyhow::anyhow!("owning run directory unavailable"))?;
            let page = crate::backend::records::artifacts::read_page(
                root,
                artifact,
                q.byte_offset,
                q.limit.unwrap_or(480).min(480),
            )?;
            return Ok(serde_json::to_string(&page)?);
        }
        ensure!(
            q.artifact.is_none() && q.byte_offset == 0,
            "details does not accept raw parameters"
        );
        let text = serde_json::to_string(r)?;
        let count = text.chars().count();
        ensure!(
            q.detail_offset <= count,
            "detail_offset exceeds content length"
        );
        let limit = q.limit.unwrap_or(400);
        ensure!(
            (1..=2400).contains(&limit),
            "details limit must be 1..2400 characters"
        );
        let segment: String = text
            .chars()
            .skip(q.detail_offset)
            .take(limit.min(400))
            .collect();
        let next = q.detail_offset + segment.chars().count();
        return Ok(json!({"id": id, "text": segment, "next_detail_offset": next, "truncated": next < count}).to_string());
    }
    ensure!(
        q.artifact.is_none() && q.byte_offset == 0 && q.detail_offset == 0,
        "raw/detail parameters require their explicit view"
    );
    let limit = q.limit.unwrap_or(20);
    ensure!(
        (1..=20).contains(&limit),
        "catalog/summary limit must be 1..20"
    );
    let mut rows: Vec<_> = store.all().filter(matches).collect();
    rows.sort_by(|a, b| (a.created_at_ms, &a.id).cmp(&(b.created_at_ms, &b.id)));
    ensure!(q.offset <= rows.len(), "offset exceeds result count");
    let mut output = Vec::new();
    for r in rows.iter().skip(q.offset).take(limit) {
        let execution = r.job().and_then(|j| state.tasks.runs.execution(j));
        let mut item = json!({"id":r.id, "category":r.category, "content_type":r.content.kind(), "brief":r.brief,
            "active":store.active(r), "stale":store.stale(r), "scope":r.scope, "source":r.source,
            "execution":execution.map(|e| e.execution_state.token()), "qm_status":execution.and_then(|e| e.qm_result.as_ref()),
            "status_unavailable":r.job().is_some_and(|j| state.tasks.runs.unavailable_qm_results.contains_key(j)),
            "index":"available", "artifacts":r.artifacts.iter().map(|a| &a.name).collect::<Vec<_>>()});
        if view == "summary" && !matches!(r.content, Content::QmInput(_)) {
            item["content"] = serde_json::to_value(&r.content)?;
        }
        if item.to_string().chars().count() > 2800 {
            item = json!({"id":r.id,"brief":r.brief,"details_required":true});
        }
        let used: usize = output
            .iter()
            .map(|v: &Value| v.to_string().chars().count())
            .sum();
        if used + item.to_string().chars().count() > 3000 {
            break;
        }
        output.push(item);
    }
    let next = q.offset + output.len();
    Ok(json!({"records":output,"total":rows.len(),"next_offset":next,"truncated":next < rows.len(),
        "unavailable_count":store.unavailable.len(), "unavailable_view":"unavailable",
        "persistence":if state.workspace.project().is_none() {"memory only; save project"} else if state.tasks.runs.is_dirty() || state.project_save_error().is_some() {"pending project commit"} else {"project commit completed"},
        "missing_evidence": if rows.is_empty() && q.job.is_some() {Some("No structured record: legacy or unavailable. Cannot rebuild numerical facts from output.txt; workspace view locates legacy task report without claiming exact job attribution.")} else {None}}).to_string())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Constraint {
    text: String,
    task: Option<u64>,
    replaces: Option<String>,
}

pub(super) fn save_constraint(state: &mut AppState, call: &ToolCall) -> Result<String> {
    ensure!(
        state.ui.agent.approved_ids.remove(&call.id),
        "constraint requires explicit approval of this exact tool call"
    );
    let request: Constraint = serde_json::from_value(call.input.clone())?;
    if let Some(task) = request.task {
        ensure!(state.tasks.task_run(task).is_some(), "task not found");
    }
    let session = state.ui.agent.active_conversation.raw();
    let id = format!("constraint:{}", uuid::Uuid::new_v4());
    let record = Record {
        id: id.clone(),
        category: Category::Memory,
        storage_version: 1,
        content_version: 1,
        revision: 1,
        source: Source::UserApproval {
            session,
            call: call.id.clone(),
        },
        scope: Scope {
            task: request.task,
            session: Some(session),
            run_uuid: request
                .task
                .and_then(|t| state.tasks.task_run(t).map(|t| t.run_uuid.clone())),
        },
        created_at_ms: crate::backend::storage::jobs::now_ms().max(0) as u64,
        supersedes: request.replaces,
        invalidated: None,
        brief: request.text.chars().take(240).collect(),
        input_entries: vec![],
        result_entries: vec![],
        artifacts: vec![],
        content: Content::Constraint { text: request.text },
    };
    state.tasks.runs.records.insert(record)?;
    if state.workspace.project().is_some()
        && let Err(error) = crate::frontend::dispatcher::persist_project(state, true)
    {
        return Ok(format!(
            "Constraint {id} recorded in memory, but project commit failed: {error}; not persisted. Save project to retry."
        ));
    }
    Ok(format!(
        "User-confirmed constraint {id} recorded in this project/session. Persistence: {}",
        if state.workspace.project().is_some() && !state.tasks.runs.is_dirty() {
            "committed"
        } else {
            "memory only or pending commit; save project"
        }
    ))
}

#[cfg(test)]
mod tests;
