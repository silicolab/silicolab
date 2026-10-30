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
    console::run_script_file_with_args,
    dispatcher::{close_project_headless, open_project_headless, persist_project},
    state::AppState,
};

#[derive(Debug, Clone)]
pub struct CliScriptRequest {
    /// Run inside this project instead of a throwaway scratch workspace.
    pub project: Option<PathBuf>,
    pub script_path: PathBuf,
    pub script_args: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default)]
pub struct CliScriptResult {
    pub stdout_lines: Vec<String>,
}

pub fn parse_cli_script_request(args: &[String]) -> Result<Option<CliScriptRequest>> {
    if args.is_empty() {
        return Ok(None);
    }

    let (project, args) = match args[0].as_str() {
        "help" | "--help" | "-h" => return Ok(None),
        "--project" => {
            let dir = args
                .get(1)
                .context("`--project` needs a project directory")?;
            (Some(PathBuf::from(dir)), &args[2..])
        }
        _ => (None, args),
    };
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
        return run_script(&mut state, request, None);
    };

    let (mut state, warnings) = open_project_headless(project, config)
        .with_context(|| format!("failed to open project {}", project.display()))?;
    for warning in warnings {
        eprintln!("warning: {warning}");
    }
    // Saving after every line keeps finished steps if a long run is killed.
    let result = run_script(&mut state, request, Some(checkpoint));
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
    checkpoint: Option<crate::frontend::console::ScriptCheckpoint>,
) -> Result<CliScriptResult> {
    let result = run_script_file_with_args(
        state,
        &request.script_path,
        request.script_args.clone(),
        checkpoint,
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
    "SilicoLab\n\nUsage:\n  silicolab                  Launch the GUI\n  silicolab [--project <dir>] <script.sls> [--name value | --flag]\n  silicolab status --project <dir>\n\nScript arguments are passed to ${name} placeholders in the script.\nWith --project the script runs inside that project and saves it after each\nline; without it the script runs in a throwaway scratch workspace.\n\n`status` prints the project's entries and tasks as JSON, marking each completed\ntask current or stale against the entries' latest revisions.\n"
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
mod tests {
    use std::{collections::BTreeMap, fs, path::PathBuf};

    use super::{
        CliScriptRequest, parse_cli_script_request, parse_named_script_args, run_cli_script,
    };
    use crate::backend::project::{create_project, open_project};

    #[test]
    fn parses_script_request_with_named_args() {
        let args = vec![
            "examples/demo.sls".to_string(),
            "--output".to_string(),
            "out/demo.png".to_string(),
            "--dry-run".to_string(),
            "--width=1200".to_string(),
        ];
        let request = parse_cli_script_request(&args)
            .unwrap()
            .expect("script request should parse");

        assert_eq!(request.script_path.to_string_lossy(), "examples/demo.sls");
        assert_eq!(request.script_args.get("output").unwrap(), "out/demo.png");
        assert_eq!(request.script_args.get("dry_run").unwrap(), "true");
        assert_eq!(request.script_args.get("width").unwrap(), "1200");
    }

    #[test]
    fn rejects_unexpected_positional_script_args() {
        let error = parse_named_script_args(&["plain".to_string()]).expect_err("should fail");
        assert!(error.to_string().contains("unexpected positional argument"));
    }

    #[test]
    fn parses_project_flag_before_script() {
        let args = ["--project", "proj", "run.sls", "--basis", "sto-3g"].map(String::from);
        let request = parse_cli_script_request(&args)
            .unwrap()
            .expect("script request should parse");
        assert_eq!(request.project, Some(PathBuf::from("proj")));
        assert_eq!(request.script_path, PathBuf::from("run.sls"));
        assert_eq!(request.script_args.get("basis").unwrap(), "sto-3g");

        let error = parse_cli_script_request(&["--project".to_string()]).unwrap_err();
        assert!(error.to_string().contains("project directory"), "{error}");
    }

    #[test]
    fn project_runs_persist_across_invocations() {
        let root = PathBuf::from("target/test-cli-project");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let project = create_project(&root, "demo").unwrap();
        fs::write(root.join("h2.xyz"), "2\nh2\nH 0 0 0\nH 0 0 0.74\n").unwrap();
        let script = root.join("step.sls");
        fs::write(
            &script,
            format!(
                "open {}\nqm energy --method rhf --basis sto-3g\n",
                root.join("h2.xyz").display()
            ),
        )
        .unwrap();
        let request = CliScriptRequest {
            project: Some(project.root.clone()),
            script_path: script,
            script_args: BTreeMap::new(),
        };

        run_cli_script(&request).unwrap();
        run_cli_script(&request).unwrap();

        let (_, snapshot) = open_project(&project.root).unwrap();
        assert_eq!(snapshot.entries.records.len(), 2);
        assert_eq!(snapshot.tasks.tasks.len(), 2);
        assert!(snapshot.warnings.is_empty(), "{:?}", snapshot.warnings);
    }

    #[test]
    fn project_open_in_another_session_is_refused() {
        let root = PathBuf::from("target/test-cli-locked");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let project = create_project(&root, "demo").unwrap();
        let script = root.join("noop.sls");
        fs::write(&script, "# nothing\n").unwrap();
        let (held, _) = crate::backend::housekeeping::acquire_lock(&project).unwrap();
        let request = CliScriptRequest {
            project: Some(project.root.clone()),
            script_path: script,
            script_args: BTreeMap::new(),
        };

        let error = run_cli_script(&request).unwrap_err();
        assert!(format!("{error:#}").contains("already open"), "{error:#}");
        held.release();
        run_cli_script(&request).unwrap();
    }

    #[test]
    fn status_reports_results_made_stale_by_a_later_edit() {
        let root = PathBuf::from("target/test-cli-status");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let project = create_project(&root, "demo").unwrap();
        fs::write(root.join("o.xyz"), "1\no\nO 0 0 0\n").unwrap();
        let run = |name: &str, body: String| {
            let script = root.join(name);
            fs::write(&script, body).unwrap();
            run_cli_script(&CliScriptRequest {
                project: Some(project.root.clone()),
                script_path: script,
                script_args: BTreeMap::new(),
            })
            .unwrap();
        };
        let status = || -> serde_json::Value {
            let args = ["--project".to_string(), project.root.display().to_string()];
            serde_json::from_str(&super::run_cli_status(&args).unwrap()).unwrap()
        };

        run(
            "energy.sls",
            format!(
                "open {}\nhydrogen add\nqm energy --method rhf --basis sto-3g\n",
                root.join("o.xyz").display()
            ),
        );
        let report = status();
        assert_eq!(report["summary"]["stale"], 0, "{report:#}");
        assert_eq!(report["project"]["open_elsewhere"], false);
        let energy = report["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|task| task["task"] == "qm-energy")
            .unwrap();
        assert_eq!(energy["freshness"], "current", "{report:#}");

        run("edit.sls", "delete chain A\nhydrogen add\n".to_string());
        let report = status();
        let energy = report["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|task| task["task"] == "qm-energy")
            .unwrap();
        assert_eq!(energy["freshness"], "stale", "{report:#}");
        assert_eq!(energy["stale_reasons"][0]["kind"], "input_edited");
    }

    #[test]
    fn status_never_writes_even_to_an_unmigrated_project() {
        let root = PathBuf::from("target/test-cli-status-readonly");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let project = create_project(&root, "demo").unwrap();
        fs::write(root.join("o.xyz"), "1\no\nO 0 0 0\n").unwrap();
        let script = root.join("energy.sls");
        fs::write(
            &script,
            format!(
                "open {}\nqm energy --method rhf --basis sto-3g\n",
                root.join("o.xyz").display()
            ),
        )
        .unwrap();
        run_cli_script(&CliScriptRequest {
            project: Some(project.root.clone()),
            script_path: script,
            script_args: BTreeMap::new(),
        })
        .unwrap();
        // Simulate a project last saved before result revisions existed.
        rusqlite::Connection::open(&project.project_db)
            .unwrap()
            .execute("alter table task_runs drop column result_revision", [])
            .unwrap();
        let before = fs::read(&project.project_db).unwrap();

        let args = ["--project".to_string(), project.root.display().to_string()];
        let report: serde_json::Value =
            serde_json::from_str(&super::run_cli_status(&args).unwrap()).unwrap();

        assert_eq!(fs::read(&project.project_db).unwrap(), before);
        assert_eq!(report["tasks"][0]["freshness"], "current", "{report:#}");
        assert!(report["tasks"][0]["result_revision"].is_null());
    }
}
