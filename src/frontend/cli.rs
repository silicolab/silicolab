use std::{collections::BTreeMap, path::PathBuf};

use anyhow::{Context, Result, anyhow, bail};

use crate::frontend::{console::run_script_file_with_args, state::AppState};

#[derive(Debug, Clone)]
pub struct CliScriptRequest {
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

    let first = args[0].as_str();
    if matches!(first, "help" | "--help" | "-h") {
        return Ok(None);
    }

    let script_path = PathBuf::from(first);
    if script_path
        .extension()
        .and_then(|value| value.to_str())
        .is_none_or(|value| !value.eq_ignore_ascii_case("sls"))
    {
        bail!("CLI mode expects a `.sls` script path as the first argument");
    }

    Ok(Some(CliScriptRequest {
        script_path,
        script_args: parse_named_script_args(&args[1..])?,
    }))
}

pub fn run_cli_script(request: &CliScriptRequest) -> Result<CliScriptResult> {
    let mut state = AppState::scratch(Default::default(), Vec::new());
    let result = run_script_file_with_args(
        &mut state,
        &request.script_path,
        request.script_args.clone(),
    )
    .with_context(|| format!("failed to run {}", request.script_path.display()))?;

    Ok(CliScriptResult {
        stdout_lines: result.stdout_lines,
    })
}

pub fn cli_help_text() -> &'static str {
    "SilicoLab\n\nUsage:\n  silicolab                  Launch the GUI\n  silicolab <script.sls> [--name value | --flag]\n\nScript arguments are passed to ${name} placeholders in the script.\n"
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
    use super::{parse_cli_script_request, parse_named_script_args};

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
}
