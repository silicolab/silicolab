//! `silicolab mcp --project <dir>`: a Model Context Protocol server on stdio, so
//! an agent client (Claude Code, Codex) drives a project through tools instead
//! of shell commands. It is a thin layer over the CLI: every call opens the
//! project, runs under the strict rules, saves, and closes it again, so the
//! project stays usable from the GUI between calls and a crashed client never
//! leaves it locked.
//!
//! Messages are newline-delimited JSON-RPC 2.0, the MCP stdio transport. Stdout
//! carries only protocol messages; diagnostics go to stderr.

use std::{
    io::{BufRead, Write},
    path::{Path, PathBuf},
};

use anyhow::{Result, anyhow, bail};
use serde_json::{Value, json};

use crate::frontend::{
    cli::{project_status, with_project},
    console::{command_catalog, run_script_text},
};

/// Protocol revisions this server speaks, newest first.
const PROTOCOL_VERSIONS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];

const INSTRUCTIONS: &str = "SilicoLab project tools. Start every session with \
`project_status`: it lists the entries with their revisions and every task with \
whether its result is still current, so you never rely on memory of earlier work. \
Run work with `run_commands`, which takes `.sls` console lines (see \
`command_catalog`). Each calculation must pin its inputs by putting \
`--entry #id@revision` before the command, using the revisions `project_status` \
reported; add `--request-id <key>` so a retry after an interruption returns the \
earlier run instead of computing again. Inputs from stale or failed upstream \
tasks are refused unless you add `--allow-stale` deliberately. Call \
`project_status` again after each step. Do not edit the project directory by hand.";

pub fn run_mcp_server(args: &[String]) -> Result<()> {
    let project = match args {
        [flag, dir] if flag == "--project" => PathBuf::from(dir),
        _ => bail!("usage: silicolab mcp --project <dir>"),
    };
    let mut stdout = std::io::stdout().lock();
    for line in std::io::stdin().lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(&line) {
            Ok(message) => handle(&project, &message),
            Err(error) => Some(error_reply(
                Value::Null,
                -32700,
                format!("parse error: {error}"),
            )),
        };
        if let Some(reply) = reply {
            serde_json::to_writer(&mut stdout, &reply)?;
            stdout.write_all(b"\n")?;
            stdout.flush()?;
        }
    }
    Ok(())
}

/// The reply to one message, or `None` for notifications and client responses,
/// which get none.
fn handle(project: &Path, message: &Value) -> Option<Value> {
    let method = message.get("method")?.as_str().unwrap_or_default();
    let id = message.get("id")?.clone();
    let params = message.get("params").cloned().unwrap_or(Value::Null);
    let result = match method {
        "initialize" => Ok(initialize(&params)),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tools() })),
        "tools/call" => call_tool(project, &params),
        _ => Err((-32601, format!("method `{method}` is not supported"))),
    };
    Some(match result {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err((code, message)) => error_reply(id, code, message),
    })
}

fn error_reply(id: Value, code: i64, message: String) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn initialize(params: &Value) -> Value {
    let requested = params.get("protocolVersion").and_then(Value::as_str);
    let version = requested
        .filter(|version| PROTOCOL_VERSIONS.contains(version))
        .unwrap_or(PROTOCOL_VERSIONS[0]);
    json!({
        "protocolVersion": version,
        "capabilities": { "tools": {} },
        "serverInfo": { "name": "silicolab", "version": env!("CARGO_PKG_VERSION") },
        "instructions": INSTRUCTIONS,
    })
}

fn tools() -> Value {
    json!([
        {
            "name": "project_status",
            "description": "The project's entries (id, name, revision) and tasks (inputs, \
                result entry, status, and freshness: current, stale with reasons, or unknown), \
                as JSON. Read-only; reflects the last save.",
            "inputSchema": { "type": "object", "properties": {} },
        },
        {
            "name": "run_commands",
            "description": "Run `.sls` console lines in the project, one command per line, \
                under the strict rules: each calculation needs `--entry #id@revision` before \
                the command, e.g. `--entry #3@2 --request-id opt-1 qm optimize --method \
                r2scan-3c`. Lines run in order and each finished line is saved; the first \
                failure stops the call. Calculations run to completion before the call \
                returns.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "commands": {
                        "type": "string",
                        "description": "One or more `.sls` lines separated by newlines.",
                    },
                },
                "required": ["commands"],
            },
        },
        {
            "name": "command_catalog",
            "description": "The `.sls` command reference with examples.",
            "inputSchema": { "type": "object", "properties": {} },
        },
    ])
}

fn call_tool(project: &Path, params: &Value) -> Result<Value, (i64, String)> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let arguments = params.get("arguments").cloned().unwrap_or(Value::Null);
    let outcome = match name {
        "project_status" => {
            project_status(project).and_then(|report| Ok(serde_json::to_string_pretty(&report)?))
        }
        "run_commands" => match arguments.get("commands").and_then(Value::as_str) {
            Some(commands) => run_commands(project, commands),
            None => Err(anyhow!("`commands` must be a string of `.sls` lines")),
        },
        "command_catalog" => Ok(command_catalog()),
        _ => return Err((-32602, format!("unknown tool `{name}`"))),
    };
    let (text, is_error) = match outcome {
        Ok(text) => (text, false),
        Err(error) => (format!("{error:#}"), true),
    };
    Ok(json!({ "content": [{ "type": "text", "text": text }], "isError": is_error }))
}

/// Run the lines one at a time so a failure names its line and the output of
/// the lines before it is still reported.
fn run_commands(project: &Path, commands: &str) -> Result<String> {
    let mut output = Vec::new();
    let failure = with_project(project, true, |state, policy| {
        for line in commands.lines().map(str::trim) {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            match run_script_text(state, line, policy) {
                Ok(result) => output.extend(result.stdout_lines),
                Err(error) => return Ok(Some(format!("`{line}` failed: {error:#}"))),
            }
        }
        Ok(None)
    })?;
    match failure {
        None if output.is_empty() => Ok("done".to_string()),
        None => Ok(output.join("\n")),
        Some(failure) => {
            output.push(failure);
            Err(anyhow!(output.join("\n")))
        }
    }
}

#[cfg(test)]
mod tests;
