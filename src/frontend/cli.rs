use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value, json};

use crate::backend::{
    config::load_config, freshness, housekeeping, project::read_project_overview,
    storage::task_status_token,
};
use crate::frontend::{
    console::{ScriptPolicy, run_script_file_with_args},
    dispatcher::{close_project_headless, open_project_headless, persist_project},
    state::AppState,
};

#[derive(Debug, Clone)]
pub struct CliScriptRequest {
    /// Run inside this project instead of a throwaway scratch workspace.
    pub project: Option<PathBuf>,
    /// Apply the agent-harness rules; needs a project.
    pub strict: bool,
    pub script_path: PathBuf,
    pub script_args: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default)]
pub struct CliScriptResult {
    pub stdout_lines: Vec<String>,
}

pub fn parse_cli_script_request(args: &[String]) -> Result<Option<CliScriptRequest>> {
    if matches!(
        args.first().map(String::as_str),
        None | Some("help" | "--help" | "-h")
    ) {
        return Ok(None);
    }

    let mut project = None;
    let mut strict = false;
    let mut args = args;
    loop {
        match args.first().map(String::as_str) {
            Some("--project") => {
                let dir = args
                    .get(1)
                    .context("`--project` needs a project directory")?;
                project = Some(PathBuf::from(dir));
                args = &args[2..];
            }
            Some("--strict") => {
                strict = true;
                args = &args[1..];
            }
            _ => break,
        }
    }
    if strict && project.is_none() {
        bail!("`--strict` needs `--project <dir>`");
    }
    let first = args
        .first()
        .context("CLI mode expects a `.sls` script path")?
        .as_str();

    let script_path = PathBuf::from(first);
    if script_path
        .extension()
        .and_then(|value| value.to_str())
        .is_none_or(|value| !value.eq_ignore_ascii_case("sls"))
    {
        bail!("CLI mode expects a `.sls` script path as the first argument");
    }

    Ok(Some(CliScriptRequest {
        project,
        strict,
        script_path,
        script_args: parse_named_script_args(&args[1..])?,
    }))
}

pub fn run_cli_script(request: &CliScriptRequest) -> Result<CliScriptResult> {
    let (config, config_warning) = load_config();
    if let Some(warning) = config_warning {
        eprintln!("warning: {warning}");
    }
    let Some(project) = &request.project else {
        let mut state = AppState::scratch(config, Vec::new());
        return run_script(&mut state, request, ScriptPolicy::default());
    };

    let (mut state, warnings) = open_project_headless(project, config)
        .with_context(|| format!("failed to open project {}", project.display()))?;
    for warning in warnings {
        eprintln!("warning: {warning}");
    }
    // Saving after every line keeps finished steps if a long run is killed.
    let policy = ScriptPolicy {
        checkpoint: Some(checkpoint),
        strict: request.strict,
    };
    let result = run_script(&mut state, request, policy);
    let closed = close_project_headless(&mut state)
        .with_context(|| format!("failed to save project {}", project.display()));
    match (result, closed) {
        (Ok(result), Ok(())) => Ok(result),
        (Err(error), Ok(())) => Err(error),
        (Ok(_), Err(error)) => Err(error),
        (Err(error), Err(save_error)) => Err(error.context(format!("{save_error:#}"))),
    }
}

fn run_script(
    state: &mut AppState,
    request: &CliScriptRequest,
    policy: ScriptPolicy,
) -> Result<CliScriptResult> {
    let result = run_script_file_with_args(
        state,
        &request.script_path,
        request.script_args.clone(),
        policy,
    )
    .with_context(|| format!("failed to run {}", request.script_path.display()))?;

    Ok(CliScriptResult {
        stdout_lines: result.stdout_lines,
    })
}

fn checkpoint(state: &mut AppState) -> Result<()> {
    persist_project(state, false)
}

/// `silicolab status --project <dir>`: a JSON report of the project's entries
/// and tasks, with whether each completed task still matches the current
/// entries. It never writes to the project and takes no lock, so it works while
/// the project is open elsewhere; it reflects the last save.
pub fn run_cli_status(args: &[String]) -> Result<String> {
    let project = match args {
        [flag, dir] if flag == "--project" => PathBuf::from(dir),
        _ => bail!("usage: silicolab status --project <dir>"),
    };
    let report = project_status(&project)
        .with_context(|| format!("failed to read project {}", project.display()))?;
    Ok(serde_json::to_string_pretty(&report)?)
}

fn project_status(path: &Path) -> Result<Value> {
    let (session, snapshot) = read_project_overview(path)?;
    let assessed = freshness::assess(&snapshot.tasks, &snapshot.entries);
    let entries: Vec<Value> = snapshot
        .entries
        .records
        .iter()
        .map(|entry| json!({"id": entry.id, "name": entry.name, "revision": entry.revision}))
        .collect();
    let tasks: Vec<Value> = snapshot
        .tasks
        .tasks
        .iter()
        .map(|task| {
            let freshness = assessed
                .get(&task.id)
                .unwrap_or(&freshness::Freshness::NotApplicable);
            json!({
                "id": task.id,
                "run_uuid": task.run_uuid,
                "task": task.controller_id,
                "title": task.title,
                "status": task_status_token(task.status),
                "inputs": task.inputs,
                "result_entry": task.result_entry_id,
                "result_revision": task.result_revision,
                "request_id": task.request_id,
                "stale_inputs_accepted": task.stale_inputs_accepted,
                "freshness": freshness.token(),
                "stale_reasons": freshness.reasons(),
            })
        })
        .collect();
    let count = |token: &str| {
        assessed
            .values()
            .filter(|freshness| freshness.token() == token)
            .count()
    };
    Ok(json!({
        "project": {
            "name": session.name,
            "root": session.root,
            "open_elsewhere": housekeeping::is_locked(&session),
        },
        "active_entry": snapshot.entries.active_entry_id(),
        "entries": entries,
        "tasks": tasks,
        "summary": {
            "current": count("current"),
            "stale": count("stale"),
            "unknown": count("unknown"),
            "not_completed": count("not_applicable"),
        },
    }))
}

pub fn cli_help_text() -> &'static str {
    "SilicoLab\n\nUsage:\n  silicolab                  Launch the GUI\n  silicolab [--project <dir> [--strict]] <script.sls> [--name value | --flag]\n  silicolab status --project <dir>\n\nScript arguments are passed to ${name} placeholders in the script.\nWith --project the script runs inside that project and saves it after each\nline; without it the script runs in a throwaway scratch workspace.\n\n--strict applies the rules for an AI agent: each calculation must pin its\ninputs with `--entry #id@revision` before the command, stale inputs are\nrefused unless `--allow-stale` is given, `--request-id` makes retries safe,\nunconverged QM steps fail, MD needs a build of the current revision, and\nfiles are written only inside the project.\n\n`status` prints the project's entries and tasks as JSON, marking each completed\ntask current or stale against the entries' latest revisions.\n"
}

fn parse_named_script_args(args: &[String]) -> Result<BTreeMap<String, String>> {
    let mut parsed = BTreeMap::new();
    let mut index = 0;
    while index < args.len() {
        let current = &args[index];
        if !current.starts_with("--") || current.len() <= 2 {
            bail!(
                "unexpected positional argument `{current}`; use `--name value` or `--name=value`"
            );
        }

        if let Some((name, value)) = current[2..].split_once('=') {
            parsed.insert(normalize_script_arg_name(name)?, value.to_string());
            index += 1;
            continue;
        }

        let name = normalize_script_arg_name(&current[2..])?;
        if index + 1 < args.len() && !args[index + 1].starts_with("--") {
            parsed.insert(name, args[index + 1].clone());
            index += 2;
        } else {
            parsed.insert(name, "true".to_string());
            index += 1;
        }
    }
    Ok(parsed)
}

fn normalize_script_arg_name(name: &str) -> Result<String> {
    if name.is_empty() {
        bail!("script argument names cannot be empty");
    }
    let normalized = name.replace('-', "_");
    if !normalized
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
    {
        return Err(anyhow!(
            "script argument `{name}` contains unsupported characters"
        ));
    }
    Ok(normalized)
}

#[cfg(test)]
mod tests;
