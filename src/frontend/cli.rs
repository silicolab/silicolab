use std::{collections::BTreeMap, path::PathBuf};

use anyhow::{Context, Result, anyhow, bail};

use crate::backend::config::load_config;
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

pub fn cli_help_text() -> &'static str {
    "SilicoLab\n\nUsage:\n  silicolab                  Launch the GUI\n  silicolab [--project <dir>] <script.sls> [--name value | --flag]\n\nScript arguments are passed to ${name} placeholders in the script.\nWith --project the script runs inside that project and saves it after each\nline; without it the script runs in a throwaway scratch workspace.\n"
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
}
