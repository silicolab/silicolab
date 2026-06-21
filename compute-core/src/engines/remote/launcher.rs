//! The job bundle and the pluggable launcher for a remote compute run.
//!
//! A remote run is a single per-run work directory holding two client-written
//! files — `request.json` (the serialized engine job) and a self-contained
//! `run.sh` — plus the worker's outputs (`.exit` / `.console` / `outcome.json`).
//! The worker binary is **pre-deployed** (see [`super::deploy`]), not shipped in
//! the bundle, and is invoked from `run.sh` by absolute path.
//!
//! All scheduler differences live behind [`Launcher`]'s submit / poll / cancel
//! triplet; the shared stage→submit→poll→retrieve spine is launcher-agnostic.
//! This module is engine-agnostic too: it moves files and reads exit/console
//! state, never the typed `EngineRequest`/`EngineOutcome` (those stay one layer
//! up in `wire`). The SSH/SCP plumbing is the hardened layer in the parent
//! module, reused verbatim.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};

use super::RemoteTarget;
use crate::engines::process;

/// Fixed bundle filenames. Each run owns its own `runs/<uuid>` directory, so a
/// single fixed name per file never collides across runs.
const RUN_SCRIPT: &str = "run.sh";
const PGID_FILE: &str = "run.pgid";
const EXIT_FILE: &str = "run.exit";
const CONSOLE_FILE: &str = "run.console";
/// The request the worker reads and the outcome it writes (relative to the run
/// dir, the worker's working directory).
pub const REQUEST_FILE: &str = "request.json";
pub const OUTCOME_FILE: &str = "outcome.json";

/// Everything `wire::run_job(Executor::Remote(_))` needs to drive a remote run:
/// the connection target, the launcher (scheduler adapter), the local run
/// directory holding `request.json`/`run.sh`, and the absolute remote path of the
/// pre-deployed worker. Engine-agnostic — it carries no typed job.
#[derive(Debug, Clone)]
pub struct RemoteExecution {
    pub target: RemoteTarget,
    pub launcher: Launcher,
    /// Local directory the bundle is staged from and outputs retrieve to.
    pub working_dir: PathBuf,
    /// Absolute remote path of the `silicolab-compute` symlink to invoke
    /// (`DeployedWorker::remote_path`). Safe to emit unquoted in `run.sh`: it is
    /// `<work_root>/bin/silicolab-compute`, and `work_root` is metacharacter-free.
    pub worker_path: String,
}

/// The scheduler adapter. Only the submit / poll / cancel triplet varies; the
/// staging, run-script, and retrieval are shared. Direct (bare-node) execution is
/// the only variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Launcher {
    /// Bare-node execution: `setsid sh run.sh`, handle = process-group id,
    /// cancel = `kill -- -<PGID>`.
    Direct,
}

impl Launcher {
    /// The scheduler token recorded with a run (matches the `jobs.db` `scheduler`
    /// column).
    pub fn token(self) -> &'static str {
        match self {
            Launcher::Direct => "direct",
        }
    }
}

/// An opaque, durable launch handle: the PGID for [`Launcher::Direct`] (a JobID
/// for a scheduler). May be empty if the id could not be read back at submit;
/// liveness and cancel still work, reading `run.pgid` on the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchHandle(pub String);

/// Whether a submitted job is still on the system.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Liveness {
    /// Running.
    Alive,
    /// Finished with this exit code (from `.exit`).
    Done(i32),
    /// Gone without an exit code — OOM, admin kill, node crash, or a purged dir.
    Lost,
}

impl Launcher {
    /// Stage the bundle (write `run.sh`, upload it and `request.json`) and launch
    /// the job, returning its durable handle. `request.json` must already be in
    /// `working_dir`.
    pub fn submit(
        self,
        target: &RemoteTarget,
        working_dir: &Path,
        worker_path: &str,
    ) -> Result<LaunchHandle> {
        write_run_sh(working_dir, worker_path, &target.prelude)?;
        super::sync_up(target, working_dir)?;
        match self {
            Launcher::Direct => direct_submit(target),
        }
    }

    /// One refresh: the authoritative `.exit` read **and** a liveness probe,
    /// plus the full console (progress is a log read over SSH). Combining both is
    /// what turns a job that died without writing `.exit` into [`Liveness::Lost`]
    /// rather than one shown running forever.
    pub fn poll(self, target: &RemoteTarget, _handle: &LaunchHandle) -> Result<(Liveness, String)> {
        match self {
            Launcher::Direct => direct_poll(target),
        }
    }

    /// Terminate the job (best-effort).
    pub fn cancel(self, target: &RemoteTarget, _handle: &LaunchHandle) -> Result<()> {
        match self {
            Launcher::Direct => direct_cancel(target),
        }
    }
}

/// Retrieve the worker's outputs into `working_dir` and return the raw
/// `outcome.json` bytes. `.console`/`.exit` come along when present.
pub fn retrieve_outcome(target: &RemoteTarget, working_dir: &Path) -> Result<Vec<u8>> {
    super::sync_down(
        target,
        working_dir,
        &[OUTCOME_FILE],
        &[CONSOLE_FILE, EXIT_FILE],
    )?;
    let path = working_dir.join(OUTCOME_FILE);
    std::fs::read(&path).with_context(|| format!("read {}", path.display()))
}

/// Render and write the self-contained `run.sh` into `working_dir`.
///
/// Structure: shebang, `set -e`, the host prelude (a non-interactive SSH shell
/// does not source the login environment), then the one-line worker launch. The
/// optional work↔scratch copy is a passthrough when no scratch dir is configured.
/// `worker_path` is emitted unquoted so a leading `~` expands; it is
/// metacharacter-free by construction.
fn write_run_sh(working_dir: &Path, worker_path: &str, prelude: &[String]) -> Result<()> {
    let script = render_run_sh(worker_path, prelude);
    let path = working_dir.join(RUN_SCRIPT);
    std::fs::write(&path, script).with_context(|| format!("write {}", path.display()))
}

/// Pure renderer for `run.sh`, split out so it is unit-testable without a disk.
fn render_run_sh(worker_path: &str, prelude: &[String]) -> String {
    let mut script = String::from("#!/bin/sh\nset -e\n");
    for line in prelude {
        script.push_str(line);
        script.push('\n');
    }
    // `exec` so the worker's exit status becomes the script's, captured into
    // `run.exit` by the launcher wrapper.
    script.push_str(&format!(
        "exec {worker_path} exec {REQUEST_FILE} {OUTCOME_FILE}\n"
    ));
    script
}

/// `setsid sh run.sh` detached into its own process group, capturing the PGID,
/// merged console, and authoritative exit code — mirroring the per-command
/// launch the GROMACS transport already proves.
fn direct_submit(target: &RemoteTarget) -> Result<LaunchHandle> {
    let body = format!(
        "echo $$ > {PGID_FILE}; ( sh {RUN_SCRIPT} ) >> {CONSOLE_FILE} 2>&1; echo $? > {EXIT_FILE}"
    );
    let launch = format!(
        "mkdir -p {dir} && cd {dir} && : > {CONSOLE_FILE} && {{ setsid sh -c {body} </dev/null >/dev/null 2>&1 & }}",
        dir = target.remote_dir,
        body = super::single_quote_wrap(&body),
    );
    let result = process::run(super::ssh_config(
        target,
        &launch,
        Some(Duration::from_secs(30)),
    ))
    .context("failed to launch the remote job over SSH")?;
    if !result.success() {
        bail!(
            "remote launch failed (exit {}): {}",
            result.exit_code,
            result.stderr.trim()
        );
    }
    Ok(LaunchHandle(read_pgid(target).unwrap_or_default()))
}

/// Read back the PGID the detached launch wrote, with a brief retry for the
/// sub-second window before `run.pgid` lands.
fn read_pgid(target: &RemoteTarget) -> Option<String> {
    let script = format!(
        "cd {dir} 2>/dev/null || exit 0; i=0; while [ $i -lt 10 ] && [ ! -s {PGID_FILE} ]; do sleep 0.2; i=$((i+1)); done; cat {PGID_FILE} 2>/dev/null",
        dir = target.remote_dir,
    );
    let result = process::run(super::ssh_config(
        target,
        &script,
        Some(Duration::from_secs(10)),
    ))
    .ok()?;
    let pgid = result.stdout.trim();
    (!pgid.is_empty()).then(|| pgid.to_string())
}

/// `.exit` read + PGID liveness probe + console, in one round trip.
fn direct_poll(target: &RemoteTarget) -> Result<(Liveness, String)> {
    let script = format!(
        "cd {dir} 2>/dev/null || {{ echo SL_LOST; exit 0; }}; \
         if [ -f {EXIT_FILE} ]; then printf 'SL_DONE %s\\n' \"$(cat {EXIT_FILE} 2>/dev/null)\"; \
         else PGID=$(cat {PGID_FILE} 2>/dev/null); \
         if [ -n \"$PGID\" ] && kill -0 -\"$PGID\" 2>/dev/null; then echo SL_ALIVE; else echo SL_LOST; fi; fi; \
         cat {CONSOLE_FILE} 2>/dev/null",
        dir = target.remote_dir,
    );
    let result = process::run(super::ssh_config(
        target,
        &script,
        Some(Duration::from_secs(30)),
    ))
    .context("failed to poll the remote job over SSH")?;
    if !result.success() {
        bail!("remote poll failed (exit {})", result.exit_code);
    }
    let mut lines = result.stdout.lines();
    let status = parse_liveness(lines.next().unwrap_or(""));
    let console = lines.collect::<Vec<_>>().join("\n");
    Ok((status, console))
}

fn parse_liveness(status_line: &str) -> Liveness {
    let line = status_line.trim();
    if line == "SL_ALIVE" {
        Liveness::Alive
    } else if let Some(code) = line.strip_prefix("SL_DONE ") {
        Liveness::Done(code.trim().parse().unwrap_or(-1))
    } else {
        // SL_LOST or anything unrecognized → lost (never alive).
        Liveness::Lost
    }
}

/// Kill the run's process group (TERM, then KILL), reading the PGID on the host.
fn direct_cancel(target: &RemoteTarget) -> Result<()> {
    let script = format!(
        "cd {dir} 2>/dev/null || exit 0; \
         PGID=$(cat {PGID_FILE} 2>/dev/null); \
         if [ -n \"$PGID\" ]; then kill -TERM -- -$PGID 2>/dev/null; sleep 1; kill -KILL -- -$PGID 2>/dev/null; fi; true",
        dir = target.remote_dir,
    );
    let result = process::run(super::ssh_config(
        target,
        &script,
        Some(Duration::from_secs(20)),
    ))
    .context("failed to cancel the remote job over SSH")?;
    if !result.success() {
        bail!("remote cancel failed (exit {})", result.exit_code);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_sh_launches_worker_with_prelude_and_set_e() {
        let script = render_run_sh(
            "~/.silicolab/bin/silicolab-compute",
            &["module load foo".to_string(), "source bar".to_string()],
        );
        assert!(script.starts_with("#!/bin/sh\nset -e\n"));
        assert!(script.contains("module load foo\n"));
        assert!(script.contains("source bar\n"));
        // Worker path is unquoted (so the leading ~ expands) and given the
        // positional `exec` subcommand with the fixed request/outcome names.
        assert!(
            script
                .contains("exec ~/.silicolab/bin/silicolab-compute exec request.json outcome.json")
        );
    }

    #[test]
    fn run_sh_with_no_prelude_is_still_well_formed() {
        let script = render_run_sh("/opt/silicolab-compute", &[]);
        assert_eq!(
            script,
            "#!/bin/sh\nset -e\nexec /opt/silicolab-compute exec request.json outcome.json\n"
        );
    }

    #[test]
    fn write_run_sh_emits_the_host_prelude() {
        // Guards the wired path (not just `render_run_sh` in isolation): a host
        // prelude threaded through `write_run_sh` must reach the on-disk script,
        // after `set -e` and before the worker launch.
        let dir = std::env::temp_dir().join(format!(
            "silicolab-runsh-test-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        write_run_sh(
            &dir,
            "~/.silicolab/bin/silicolab-compute",
            &["module load gromacs".to_string()],
        )
        .unwrap();
        let written = std::fs::read_to_string(dir.join(RUN_SCRIPT)).unwrap();
        assert!(written.starts_with("#!/bin/sh\nset -e\n"));
        assert!(written.contains("module load gromacs\n"));
        let prelude_at = written.find("module load gromacs").unwrap();
        let exec_at = written
            .find("exec ~/.silicolab/bin/silicolab-compute")
            .unwrap();
        assert!(
            prelude_at < exec_at,
            "prelude must precede the worker launch"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn liveness_parses_each_status() {
        assert_eq!(parse_liveness("SL_ALIVE"), Liveness::Alive);
        assert_eq!(parse_liveness("SL_DONE 0"), Liveness::Done(0));
        assert_eq!(parse_liveness("SL_DONE 137"), Liveness::Done(137));
        assert_eq!(parse_liveness("SL_LOST"), Liveness::Lost);
        // A garbled exit code degrades to Done(-1), not a panic.
        assert_eq!(parse_liveness("SL_DONE x"), Liveness::Done(-1));
        // Anything unrecognized is treated as lost, never alive.
        assert_eq!(parse_liveness("???"), Liveness::Lost);
    }

    #[test]
    fn launcher_token_is_stable() {
        assert_eq!(Launcher::Direct.token(), "direct");
    }
}
