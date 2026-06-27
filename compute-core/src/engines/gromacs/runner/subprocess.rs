//! Subprocess execution and logging shared by every `gmx` invocation: spawning
//! the child, draining its output into the run directory's cumulative log, and
//! turning a failure into a useful error.

use std::{
    fs,
    io::Write as _,
    path::Path,
    sync::{Arc, atomic::AtomicBool},
    time::{Duration, Instant},
};

use anyhow::{Result, anyhow, bail};

use crate::engines::process::{self, ProcessConfig, ProcessEventKind};

use super::{GROMACS_LOG_FILE, GromacsProgress, SubprocessOutcome};

impl SubprocessOutcome {
    pub fn success(&self) -> bool {
        self.result.success()
    }
}

/// Append one `gmx` invocation's full console output to the run directory's
/// cumulative log, prefixed with the command line and exit code. Best-effort:
/// logging must never mask the underlying run error, so write failures are
/// swallowed.
fn append_gromacs_log(log_path: &Path, command_line: &str, exit_code: i32, body: &str) {
    let Ok(mut file) = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)
    else {
        return;
    };
    let _ = write!(
        file,
        "=== {command_line} ===\n{body}\n--- exit code {exit_code} ---\n\n"
    );
}

/// Pull the GROMACS "Fatal error:" block out of a captured log, if present.
/// GROMACS buffers stdout, so on a crash the unbuffered stderr error often
/// lands *before* the trailing buffered progress lines — a plain tail misses
/// it. This finds the real error regardless of where it sits in the stream.
pub(crate) fn extract_fatal_error(log: &str) -> Option<String> {
    let start = log.rfind("Fatal error:")?;
    let rest = &log[start..];
    // The block closes with GROMACS' row-of-dashes banner; stop there.
    let end = rest.find("\n---").unwrap_or(rest.len());
    Some(rest[..end].trim_end().to_string())
}

pub(crate) fn run_subprocess<F>(
    config: ProcessConfig,
    cancel: Arc<AtomicBool>,
    report: &mut F,
) -> Result<SubprocessOutcome>
where
    F: FnMut(GromacsProgress),
{
    // Capture where to log and what command this is before `config` is moved.
    let log_path = config.working_dir.join(GROMACS_LOG_FILE);
    let command_line = format!(
        "{} {}",
        config.executable.to_string_lossy(),
        config.args.join(" ")
    );

    let (result, combined_log) = run_subprocess_local(config, cancel)?;

    // Persist the full output to the run directory so a failed run is always
    // debuggable, even though only the tail is streamed to the UI.
    append_gromacs_log(&log_path, &command_line, result.exit_code, &combined_log);

    // The child's output is not streamed during the run; surface its tail now.
    for line in combined_log
        .lines()
        .rev()
        .take(5)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
    {
        report(GromacsProgress::Log(line.to_string()));
    }

    Ok(SubprocessOutcome {
        result,
        combined_log,
        log_path,
    })
}

/// The local-execution path: spawn the child, drain its streamed stdout/stderr
/// into a combined log, and return the aggregated result. Byte-for-byte the
/// historical behavior.
fn run_subprocess_local(
    config: ProcessConfig,
    cancel: Arc<AtomicBool>,
) -> Result<(process::ProcessResult, String)> {
    let mut handle = process::spawn_with_cancel(config, cancel)?;
    let receiver = handle
        .take_receiver()
        .ok_or_else(|| anyhow!("subprocess handle missing event receiver"))?;

    let log_join = std::thread::spawn(move || {
        let mut combined = String::new();
        while let Ok(event) = receiver.recv() {
            if let ProcessEventKind::Stdout(line) | ProcessEventKind::Stderr(line) = event.kind {
                combined.push_str(&line);
                combined.push('\n');
            }
        }
        combined
    });

    let result = handle.join()?;
    let combined_log = log_join.join().map_err(|_| anyhow!("log drain panicked"))?;
    Ok((result, combined_log))
}

pub(crate) fn subprocess_failure(tool: &str, outcome: &SubprocessOutcome) -> anyhow::Error {
    // Prefer GROMACS' own fatal-error block; fall back to the tail otherwise.
    let snippet = extract_fatal_error(&outcome.combined_log)
        .unwrap_or_else(|| tail(&outcome.combined_log, 20));
    anyhow!(
        "gmx {tool} failed (exit {}). Full log: {}\n{}",
        outcome.result.exit_code,
        outcome.log_path.display(),
        snippet
    )
}

pub(crate) fn tail(log: &str, lines: usize) -> String {
    let collected: Vec<&str> = log.lines().rev().take(lines).collect();
    collected.into_iter().rev().collect::<Vec<_>>().join("\n")
}

pub(super) fn is_cancelled(cancel: &Arc<AtomicBool>) -> bool {
    cancel.load(std::sync::atomic::Ordering::Relaxed)
}

pub(super) fn remaining_budget(total: Duration, started_at: Instant) -> Result<Duration> {
    let elapsed = started_at.elapsed();
    if elapsed >= total {
        bail!("GROMACS wall-clock budget exhausted before next stage");
    }
    Ok(total - elapsed)
}
