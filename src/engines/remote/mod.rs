//! Remote (SSH) execution transport for external-process engines.
//!
//! This module owns all SSH/SCP mechanics as **plain-data value types** — no async
//! runtime, no live-connection objects threaded through the engine layer. Every
//! `ssh`/`scp`/`ssh-keygen` invocation is built as a [`ProcessConfig`] and run
//! through the existing [`crate::engines::process`] layer (reusing its timeout,
//! cancel, and shell-free spawn).
//!
//! ## How it plugs into the engine layer
//!
//! A GROMACS launch and the transport it runs over are always paired, so instead
//! of threading a separate executor through a dozen signatures, the two travel
//! together as a [`Compute`]: `{ launch, transport }`. `Compute` replaces the
//! `gmx_launch` field wherever it flowed, and the **only** behavioral branch is in
//! `gromacs::runner::run_subprocess`, which calls [`run_remote`] for the
//! [`Transport::Remote`] case.
//!
//! ## Per-command remote model
//!
//! For one `gmx` command in local run dir `D` ↔ remote `R = <work_root>/runs/<uuid>`:
//!
//! 1. **Stage up** (incremental, [`sync_up`]): upload files in `D` that are new or
//!    changed vs a local manifest (`path → size+mtime`), recorded on both up and
//!    down so a pulled-back file is never re-uploaded.
//! 2. **Launch detached**: one SSH call starts the command under `setsid` in its
//!    own process group, writing its PGID to `<cmd>.pgid`, its exit code to
//!    `<cmd>.exit` (the authoritative done-signal), and merged stdout/stderr to
//!    `<cmd>.console`.
//! 3. **Poll**: short SSH calls read the console (streamed to the UI line-by-line)
//!    and check `<cmd>.exit`; a transient failure is retried, not fatal. The cancel
//!    flag and the wall-clock timeout both `kill -- -<PGID>` the remote group.
//! 4. **Stage down** is **orchestrator-driven** ([`sync_down`]) — the run-stage /
//!    build pipeline pulls exactly the files it reads locally — so large
//!    intermediate artifacts never round-trip.

pub mod bootstrap;
pub mod hardware;

use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::backend::config::RemoteHost;
use crate::engines::process::{self, ProcessConfig, ProcessResult};
use crate::engines::registry::EngineLaunch;

/// Where an engine command runs. Plain data; cheaply `Clone` (so the per-stage
/// clone in `run_pipeline` is just a refcount-free copy of small fields).
#[derive(Debug, Clone)]
pub enum Transport {
    /// On this machine, via `std::process::Command` (the historical path).
    Local,
    /// On a remote host over SSH. Boxed so the `Local` variant stays small (a
    /// `RemoteTarget` is ~200 bytes and `Compute` is cloned per pipeline stage).
    Remote(Box<RemoteTarget>),
}

/// An engine launch paired with the transport it runs over. Replaces the bare
/// `gmx_launch` field everywhere it flowed; the launch stays the pure command
/// descriptor, the transport says *where* it runs.
#[derive(Debug, Clone)]
pub struct Compute {
    pub launch: EngineLaunch,
    pub transport: Transport,
}

impl Compute {
    /// A local launch (the default everywhere).
    pub fn local(launch: EngineLaunch) -> Self {
        Self {
            launch,
            transport: Transport::Local,
        }
    }

    /// A launch bound to a remote host.
    pub fn remote(launch: EngineLaunch, target: RemoteTarget) -> Self {
        Self {
            launch,
            transport: Transport::Remote(Box::new(target)),
        }
    }

    pub fn is_remote(&self) -> bool {
        matches!(self.transport, Transport::Remote(_))
    }

    /// The remote target, if this is a remote launch.
    pub fn remote_target(&self) -> Option<&RemoteTarget> {
        match &self.transport {
            Transport::Remote(target) => Some(target.as_ref()),
            Transport::Local => None,
        }
    }
}

impl From<EngineLaunch> for Compute {
    /// A bare launch is a local launch — keeps existing call sites terse
    /// (`launch.into()`).
    fn from(launch: EngineLaunch) -> Self {
        Self::local(launch)
    }
}

/// Plain-data SSH connection target plus the per-run remote scratch dir. Built
/// fresh per job by [`RemoteTarget::for_run`]; cheaply `Clone`.
#[derive(Debug, Clone)]
pub struct RemoteTarget {
    /// [`RemoteHost::id`] — for the run record and the cleanup affordance.
    pub host_id: String,
    /// Human label, for messages.
    pub host_label: String,
    pub user: String,
    pub host: String,
    pub port: u16,
    /// Dedicated private key path (local).
    pub key_path: PathBuf,
    /// App-owned `known_hosts` (local), shared with bootstrap.
    pub known_hosts_path: PathBuf,
    /// Shell lines run before the engine (`module load …`, `source GMXRC`).
    pub prelude: Vec<String>,
    /// The per-run remote scratch dir, e.g. `~/.silicolab/runs/<uuid>`. Built from
    /// a validated `work_root` (no shell metacharacters) + the run UUID, so it is
    /// safe to emit **unquoted** (the leading `~` must expand).
    pub remote_dir: String,
    pub connect_timeout_secs: u32,
}

impl RemoteTarget {
    /// Build the target for a specific run. `run_uuid` is the task's durable UUID
    /// (hex + hyphens — safe to concatenate unquoted).
    pub fn for_run(host: &RemoteHost, run_uuid: &str) -> Self {
        let work_root = host.work_root.trim_end_matches('/');
        Self {
            host_id: host.id.clone(),
            host_label: host.label.clone(),
            user: host.username.clone(),
            host: host.hostname.clone(),
            port: host.port,
            key_path: bootstrap::private_key_path(),
            known_hosts_path: bootstrap::known_hosts_path(),
            prelude: host.prelude.clone(),
            remote_dir: format!("{work_root}/runs/{run_uuid}"),
            connect_timeout_secs: 15,
        }
    }

    fn user_host(&self) -> String {
        format!("{}@{}", self.user, self.host)
    }

    fn prelude_prefix(&self) -> String {
        if self.prelude.is_empty() {
            String::new()
        } else {
            format!("{} && ", self.prelude.join(" && "))
        }
    }
}

/// How often the poll loop checks the remote console/exit state.
const POLL_INTERVAL: Duration = Duration::from_secs(4);
/// Granularity at which the poll sleep wakes to check the cancel flag.
const CANCEL_TICK: Duration = Duration::from_millis(250);
/// Consecutive failed polls (≈ this × `POLL_INTERVAL` of unreachability) before a
/// remote run is declared lost.
const MAX_POLL_FAILURES: u32 = 30;
/// Filename of the local incremental-sync manifest (excluded from upload).
const MANIFEST_FILE: &str = ".silicolab_remote_manifest.json";

/// GROMACS executables to probe on a remote host, in priority order — bare names
/// (resolved via the host's prelude/PATH) then the conventional install path.
pub const GMX_REMOTE_CANDIDATES: &[&str] =
    &["gmx", "/usr/local/gromacs/bin/gmx", "gmx_mpi", "gmx_d"];

// ---------------------------------------------------------------------------
// SSH/SCP command construction (pure; unit-tested without a network).
// ---------------------------------------------------------------------------

/// The shared `-i/-o…` options for `ssh`/`scp`. `port_flag` is `-p` for `ssh`,
/// `-P` for `scp`. All paths are passed as distinct argv elements (the spawn layer
/// is shell-free), so local paths with spaces need no quoting.
fn common_opts(target: &RemoteTarget, port_flag: &str) -> Vec<String> {
    vec![
        "-i".to_string(),
        target.key_path.to_string_lossy().into_owned(),
        "-o".to_string(),
        "IdentitiesOnly=yes".to_string(),
        "-o".to_string(),
        format!(
            "UserKnownHostsFile={}",
            target.known_hosts_path.to_string_lossy()
        ),
        "-o".to_string(),
        "StrictHostKeyChecking=accept-new".to_string(),
        "-o".to_string(),
        "BatchMode=yes".to_string(),
        "-o".to_string(),
        format!("ConnectTimeout={}", target.connect_timeout_secs),
        port_flag.to_string(),
        target.port.to_string(),
    ]
}

/// Build an `ssh` [`ProcessConfig`] that runs `remote_script` (a single argv
/// element handed verbatim to the remote shell).
fn ssh_config(
    target: &RemoteTarget,
    remote_script: &str,
    timeout: Option<Duration>,
) -> ProcessConfig {
    let mut args = common_opts(target, "-p");
    args.push(target.user_host());
    args.push(remote_script.to_string());
    let mut config = ProcessConfig::new("ssh", std::env::temp_dir()).args(args);
    if let Some(timeout) = timeout {
        config = config.timeout(timeout);
    }
    config
}

/// `scp` a local file up to `<remote_dir>/<remote_name>`.
fn scp_up_config(target: &RemoteTarget, local: &Path, remote_name: &str) -> ProcessConfig {
    let mut args = common_opts(target, "-P");
    args.push(local.to_string_lossy().into_owned());
    args.push(format!(
        "{}:{}/{}",
        target.user_host(),
        target.remote_dir,
        remote_name
    ));
    ProcessConfig::new("scp", std::env::temp_dir())
        .args(args)
        .timeout(Duration::from_secs(300))
}

/// `scp` `<remote_dir>/<remote_name>` down to a local path.
fn scp_down_config(target: &RemoteTarget, remote_name: &str, local: &Path) -> ProcessConfig {
    let mut args = common_opts(target, "-P");
    args.push(format!(
        "{}:{}/{}",
        target.user_host(),
        target.remote_dir,
        remote_name
    ));
    args.push(local.to_string_lossy().into_owned());
    ProcessConfig::new("scp", std::env::temp_dir())
        .args(args)
        .timeout(Duration::from_secs(300))
}

/// POSIX single-quote `s` so it survives the remote shell as one literal token
/// (handles spaces, `$`, quotes; an embedded `'` becomes `'\''`).
fn sh_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for ch in s.chars() {
        if ch == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}

/// Wrap `body` so it can be embedded as `sh -c <result>`: surround with single
/// quotes, escaping any interior single quote. `$`/`&&`/`"` inside `body` are
/// preserved for the inner shell to evaluate.
fn single_quote_wrap(body: &str) -> String {
    format!("'{}'", body.replace('\'', "'\\''"))
}

/// The remote script that launches one engine command detached. `gmx_tokens` is
/// `[program, args…]`; each is `sh_quote`d. `cmd_id` is the per-command basename
/// for the `.console`/`.exit`/`.pgid`/`.stdin` files.
fn launch_script(
    target: &RemoteTarget,
    cmd_id: &str,
    gmx_tokens: &[String],
    has_stdin: bool,
) -> String {
    let mut gmx = gmx_tokens
        .iter()
        .map(|t| sh_quote(t))
        .collect::<Vec<_>>()
        .join(" ");
    if has_stdin {
        gmx.push_str(&format!(" < {cmd_id}.stdin"));
    }
    // The pgid/console/exit names are safe tokens (cmd-<hex>.<ext>). `$$` (set by
    // setsid as the session/group leader) is the PGID; the subshell groups the
    // prelude + engine so prelude failures are captured in the console too.
    let body = format!(
        "echo $$ > {cmd_id}.pgid; ( {prelude}{gmx} ) >> {cmd_id}.console 2>&1; echo $? > {cmd_id}.exit",
        prelude = target.prelude_prefix(),
    );
    format!(
        "mkdir -p {dir} && cd {dir} && : > {cmd_id}.console && {{ setsid sh -c {body} </dev/null >/dev/null 2>&1 & }}",
        dir = target.remote_dir,
        body = single_quote_wrap(&body),
    )
}

/// The remote script for one poll: a status line (`SILICOLAB_DONE <code>` /
/// `SILICOLAB_RUNNING` / `SILICOLAB_NODIR`) followed by the full console.
fn poll_script(target: &RemoteTarget, cmd_id: &str) -> String {
    format!(
        "cd {dir} 2>/dev/null || {{ echo SILICOLAB_NODIR; exit 0; }}; \
         if [ -f {cmd_id}.exit ]; then printf 'SILICOLAB_DONE %s\\n' \"$(cat {cmd_id}.exit 2>/dev/null)\"; \
         else echo SILICOLAB_RUNNING; fi; \
         cat {cmd_id}.console 2>/dev/null",
        dir = target.remote_dir,
    )
}

/// The remote script that kills the command's process group (TERM, then KILL).
fn cancel_script(target: &RemoteTarget, cmd_id: &str) -> String {
    format!(
        "cd {dir} 2>/dev/null || exit 0; \
         PGID=$(cat {cmd_id}.pgid 2>/dev/null); \
         if [ -n \"$PGID\" ]; then kill -TERM -- -$PGID 2>/dev/null; sleep 1; kill -KILL -- -$PGID 2>/dev/null; fi; true",
        dir = target.remote_dir,
    )
}

// ---------------------------------------------------------------------------
// Availability / probing.
// ---------------------------------------------------------------------------

/// Error out with actionable guidance if the OS `ssh`/`scp` client is missing
/// (Windows 11 OpenSSH is an *optional* feature and may be absent).
pub fn ensure_ssh_available() -> Result<()> {
    if process::find_on_path("ssh").is_none() || process::find_on_path("scp").is_none() {
        bail!(
            "the OpenSSH client (`ssh`/`scp`) was not found on PATH. \
             On Windows, enable it via Settings → Apps → Optional features → OpenSSH Client, \
             or install Git for Windows; on macOS/Linux it ships with the OS."
        );
    }
    Ok(())
}

/// Reject a user-entered `work_root` that could break or inject into the remote
/// command. Allows `~`, alphanumerics, `/`, `.`, `-`, `_`; rejects spaces and
/// shell metacharacters. The run UUID we append is always safe.
pub fn validate_work_root(work_root: &str) -> Result<()> {
    let trimmed = work_root.trim();
    if trimmed.is_empty() {
        bail!("remote work directory cannot be empty");
    }
    let ok = trimmed
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '~' | '/' | '.' | '-' | '_'));
    if !ok {
        bail!(
            "remote work directory may only contain letters, digits, and ~ / . - _ \
             (no spaces or shell metacharacters)"
        );
    }
    Ok(())
}

/// Whether passwordless key login to `target` works right now: a fast
/// `ssh -o BatchMode=yes … true`.
pub fn check_passwordless(target: &RemoteTarget) -> bool {
    let config = ssh_config(target, "true", Some(Duration::from_secs(20)));
    matches!(process::run(config), Ok(result) if result.success())
}

/// SSH-run each candidate `<prelude && candidate> --version` and return the first
/// that identifies as a working engine, with its parsed version. Used by the
/// settings "Detect" button (always off the UI thread).
pub fn detect_remote_engine(
    target: &RemoteTarget,
    candidates: &[&str],
) -> Option<(String, String)> {
    for candidate in candidates {
        let script = format!(
            "{}{} --version",
            target.prelude_prefix(),
            sh_quote(candidate)
        );
        let config = ssh_config(target, &script, Some(Duration::from_secs(30)));
        let Ok(result) = process::run(config) else {
            continue;
        };
        let blob = format!("{}{}", result.stdout, result.stderr);
        if result.success()
            && blob.contains("GROMACS")
            && let Some(version) = crate::engines::registry::extract_version(&blob)
        {
            return Some((candidate.to_string(), version));
        }
    }
    None
}

/// SSH-run `script` on `target` and return its stdout. Errors only when the SSH
/// transport itself failed (connection/auth/shell): make the remote `script` end
/// in `; true` so a missing optional tool isn't mistaken for a failure. The SSH
/// blocks, so always call this off the UI thread. Used by settings probes such as
/// the remote hardware inventory.
pub fn run_probe_command(target: &RemoteTarget, script: &str, timeout: Duration) -> Result<String> {
    // Run the host's prelude (module loads / PATH setup) first, so tools a
    // non-login SSH shell doesn't have on PATH (e.g. `nvidia-smi` behind
    // `module load cuda`) become reachable. Joined with `;` rather than `&&` so a
    // prelude that errors still lets the probe — and its section markers — run;
    // the inventory is best-effort.
    let script = if target.prelude.is_empty() {
        script.to_string()
    } else {
        format!("{}; {script}", target.prelude.join("; "))
    };
    let config = ssh_config(target, &script, Some(timeout));
    let result = process::run(config)?;
    if result.success() {
        Ok(result.stdout)
    } else {
        let detail = result.stderr.trim();
        bail!(
            "remote command failed: {}",
            if detail.is_empty() {
                "connection or shell error"
            } else {
                detail
            }
        )
    }
}

/// Write a small `remote_run.json` into the local run dir at launch, recording
/// where the detached remote job lives. Because the remote command is detached
/// (`nohup`/`setsid`), closing the app leaves it running; this record is what a
/// later session (or the user) needs to find and clean up the remote scratch dir.
/// Best-effort — a write failure must never fail the run.
pub fn write_run_record(target: &RemoteTarget, working_dir: &Path) {
    let started_at = std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let record = serde_json::json!({
        "host_id": target.host_id,
        "host_label": target.host_label,
        "user_host": target.user_host(),
        "remote_dir": target.remote_dir,
        "started_at_unix": started_at,
    });
    if let Ok(text) = serde_json::to_string_pretty(&record) {
        let _ = fs::write(working_dir.join("remote_run.json"), text);
    }
}

/// Remove the run's remote scratch directory (the "Remove remote scratch" button).
/// The dir is our own `runs/<uuid>` path, so this is safe.
pub fn remove_remote_scratch(target: &RemoteTarget) -> Result<()> {
    let script = format!("rm -rf {}", target.remote_dir);
    let config = ssh_config(target, &script, Some(Duration::from_secs(30)));
    let result = process::run(config).context("failed to run ssh for remote cleanup")?;
    if !result.success() {
        bail!(
            "removing remote scratch failed (exit {}): {}",
            result.exit_code,
            result.stderr.trim()
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Incremental file sync (manifest-based, scp transport).
// ---------------------------------------------------------------------------

#[derive(Default, Serialize, Deserialize)]
struct SyncManifest {
    /// Whether the remote run dir has been `mkdir -p`'d this run.
    dir_created: bool,
    /// `name → (size, mtime_secs)` last synced (either direction).
    files: HashMap<String, (u64, i64)>,
}

fn manifest_path(working_dir: &Path) -> PathBuf {
    working_dir.join(MANIFEST_FILE)
}

fn load_manifest(working_dir: &Path) -> SyncManifest {
    fs::read_to_string(manifest_path(working_dir))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn save_manifest(working_dir: &Path, manifest: &SyncManifest) {
    if let Ok(text) = serde_json::to_string(manifest) {
        let _ = fs::write(manifest_path(working_dir), text);
    }
}

fn file_signature(path: &Path) -> Option<(u64, i64)> {
    let md = fs::metadata(path).ok()?;
    let mtime = md
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    Some((md.len(), mtime))
}

/// Whether a file must be (re)uploaded: true when its size+mtime differs from the
/// last-synced signature in the manifest. Because [`sync_down`] records the
/// **post-download** signature, a file pulled back from the host (e.g. a prior
/// stage's `_out.gro` that feeds the next stage) is **not** re-uploaded on the
/// next command — the central efficiency invariant of the incremental sync.
fn needs_upload(manifest: &SyncManifest, name: &str, signature: (u64, i64)) -> bool {
    manifest.files.get(name) != Some(&signature)
}

/// Files in the working dir that are SilicoLab/remote bookkeeping, not engine
/// inputs — never uploaded.
fn is_excluded_from_upload(name: &str) -> bool {
    name == MANIFEST_FILE
        || name == "gromacs.log"
        || name == "remote_run.json"
        || name.ends_with(".console")
        || name.ends_with(".exit")
        || name.ends_with(".pgid")
}

/// Ensure the remote dir exists, then upload every top-level file in `working_dir`
/// that is new or changed vs the manifest. Cheap on repeat calls (only the freshly
/// written `.mdp`/`.stdin` move).
pub fn sync_up(target: &RemoteTarget, working_dir: &Path) -> Result<()> {
    let mut manifest = load_manifest(working_dir);

    if !manifest.dir_created {
        let script = format!("mkdir -p {}", target.remote_dir);
        let config = ssh_config(target, &script, Some(Duration::from_secs(30)));
        let result = process::run(config).context("failed to create remote run directory")?;
        if !result.success() {
            bail!(
                "could not create remote run directory {} (exit {}): {}",
                target.remote_dir,
                result.exit_code,
                result.stderr.trim()
            );
        }
        manifest.dir_created = true;
        save_manifest(working_dir, &manifest);
    }

    let entries = fs::read_dir(working_dir)
        .with_context(|| format!("reading working directory {}", working_dir.display()))?;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if is_excluded_from_upload(name) {
            continue;
        }
        let Some(signature) = file_signature(&path) else {
            continue;
        };
        if !needs_upload(&manifest, name, signature) {
            continue;
        }
        let config = scp_up_config(target, &path, name);
        let result =
            process::run(config).with_context(|| format!("uploading {name} to remote host"))?;
        if !result.success() {
            bail!(
                "failed to upload {name} (exit {}): {}",
                result.exit_code,
                result.stderr.trim()
            );
        }
        manifest.files.insert(name.to_string(), signature);
    }
    save_manifest(working_dir, &manifest);
    Ok(())
}

/// Pull named files from the remote dir into `working_dir`, recording each
/// downloaded file's local signature so the next [`sync_up`] does not re-upload it.
/// `required` files missing remotely is an error; `optional` files missing is a
/// silent skip (e.g. a stage that wrote no `.cpt`/`.xtc`).
pub fn sync_down(
    target: &RemoteTarget,
    working_dir: &Path,
    required: &[&str],
    optional: &[&str],
) -> Result<()> {
    let mut manifest = load_manifest(working_dir);
    for (name, is_required) in required
        .iter()
        .map(|n| (n, true))
        .chain(optional.iter().map(|n| (n, false)))
    {
        let local = working_dir.join(name);
        let config = scp_down_config(target, name, &local);
        let result =
            process::run(config).with_context(|| format!("downloading {name} from remote host"))?;
        if !result.success() {
            if is_required {
                bail!(
                    "failed to download {name} (exit {}): {}",
                    result.exit_code,
                    result.stderr.trim()
                );
            }
            continue;
        }
        if let Some(signature) = file_signature(&local) {
            manifest.files.insert((*name).to_string(), signature);
        }
    }
    save_manifest(working_dir, &manifest);
    Ok(())
}

// ---------------------------------------------------------------------------
// Detached launch + poll (the run_subprocess remote branch).
// ---------------------------------------------------------------------------

/// Run one engine command on the remote host: incremental stage-up → detached
/// launch → poll until the remote `.exit` appears (streaming console lines via
/// `report_line`), honoring `cancel` and `config.timeout` with a remote group
/// kill. Returns the aggregated [`ProcessResult`] (with `stdout` = the full
/// console) and that console text (for the engine's `gromacs.log`/fatal-error
/// extraction). Stage-**down** is the orchestrator's job — see [`sync_down`].
pub fn run_remote(
    config: &ProcessConfig,
    target: &RemoteTarget,
    cancel: Arc<AtomicBool>,
    report_line: &mut dyn FnMut(String),
) -> Result<(ProcessResult, String)> {
    let started_at = Instant::now();
    let working_dir = &config.working_dir;

    // A unique id per command so concurrent/serial commands in the same run dir
    // don't collide on their .console/.exit/.pgid files.
    let cmd_id = format!("cmd-{}", uuid::Uuid::new_v4().simple());

    // Materialize any stdin payload as a file so the remote command can redirect
    // from it (GROMACS selection prompts: `gmx … < cmd.stdin`).
    let has_stdin = config.stdin.is_some();
    if let Some(payload) = &config.stdin {
        let stdin_path = working_dir.join(format!("{cmd_id}.stdin"));
        fs::write(&stdin_path, payload)
            .with_context(|| format!("writing remote stdin file {}", stdin_path.display()))?;
    }

    // 1. Stage up (incremental). Picks up the freshly written .mdp/.stdin/etc.
    sync_up(target, working_dir)?;

    // 2. Build the [program, args…] token list and launch detached.
    let mut tokens = Vec::with_capacity(config.args.len() + 1);
    tokens.push(config.executable.to_string_lossy().into_owned());
    tokens.extend(config.args.iter().cloned());
    let launch = launch_script(target, &cmd_id, &tokens, has_stdin);
    let launch_config = ssh_config(target, &launch, Some(Duration::from_secs(30)));
    let launch_result =
        process::run(launch_config).context("failed to launch the remote command over SSH")?;
    if !launch_result.success() {
        bail!(
            "remote launch failed (exit {}): {}",
            launch_result.exit_code,
            launch_result.stderr.trim()
        );
    }

    // 3. Poll loop.
    let mut full_console = String::new();
    let mut forwarded_lines = 0usize;
    let mut consecutive_failures = 0u32;
    let timeout = config.timeout;

    loop {
        // Cancel-responsive sleep between polls.
        let mut slept = Duration::ZERO;
        let mut aborted = false;
        while slept < POLL_INTERVAL {
            if cancel.load(Ordering::Relaxed) {
                aborted = true;
                break;
            }
            std::thread::sleep(CANCEL_TICK);
            slept += CANCEL_TICK;
        }

        let cancelled = aborted || cancel.load(Ordering::Relaxed);
        let timed_out = timeout.is_some_and(|limit| started_at.elapsed() >= limit);
        if cancelled || timed_out {
            // Best-effort remote group kill, then a final console fetch.
            let kill = ssh_config(
                target,
                &cancel_script(target, &cmd_id),
                Some(Duration::from_secs(20)),
            );
            let _ = process::run(kill);
            if let Ok((console, _)) = poll_once(target, &cmd_id) {
                full_console = console;
            }
            forward_new_lines(&full_console, &mut forwarded_lines, report_line);
            return Ok((
                ProcessResult {
                    exit_code: -1,
                    stdout: full_console.clone(),
                    stderr: String::new(),
                    wall_time: started_at.elapsed(),
                    timed_out,
                    cancelled,
                },
                full_console,
            ));
        }

        match poll_once(target, &cmd_id) {
            Ok((console, status)) => {
                consecutive_failures = 0;
                full_console = console;
                forward_new_lines(&full_console, &mut forwarded_lines, report_line);
                match status {
                    PollStatus::Done(code) => {
                        return Ok((
                            ProcessResult {
                                exit_code: code,
                                stdout: full_console.clone(),
                                stderr: String::new(),
                                wall_time: started_at.elapsed(),
                                timed_out: false,
                                cancelled: false,
                            },
                            full_console,
                        ));
                    }
                    PollStatus::NoDir => {
                        bail!(
                            "remote run directory {} disappeared (scratch purged?) before the command finished",
                            target.remote_dir
                        );
                    }
                    PollStatus::Running => {}
                }
            }
            Err(_) => {
                // Transient drop: retry on the next tick, fail only after a long
                // run of unreachability.
                consecutive_failures += 1;
                if consecutive_failures >= MAX_POLL_FAILURES {
                    bail!(
                        "lost contact with remote host {} after {} consecutive failed polls",
                        target.host_label,
                        consecutive_failures
                    );
                }
            }
        }
    }
}

enum PollStatus {
    Running,
    Done(i32),
    NoDir,
}

/// One poll round: returns (full console text, status).
fn poll_once(target: &RemoteTarget, cmd_id: &str) -> Result<(String, PollStatus)> {
    let config = ssh_config(
        target,
        &poll_script(target, cmd_id),
        Some(Duration::from_secs(30)),
    );
    let result = process::run(config)?;
    if !result.success() {
        bail!("poll failed (exit {})", result.exit_code);
    }
    let mut lines = result.stdout.lines();
    let status = match lines.next().unwrap_or("") {
        "SILICOLAB_RUNNING" => PollStatus::Running,
        "SILICOLAB_NODIR" => PollStatus::NoDir,
        other => {
            if let Some(code) = other.strip_prefix("SILICOLAB_DONE ") {
                PollStatus::Done(code.trim().parse().unwrap_or(-1))
            } else {
                // Unrecognized first line: treat the whole output as console and
                // keep waiting (defensive).
                PollStatus::Running
            }
        }
    };
    let console = lines.collect::<Vec<_>>().join("\n");
    Ok((console, status))
}

/// Forward console lines that appeared since the last forward (best-effort live
/// streaming; the authoritative full console is captured at completion).
fn forward_new_lines(console: &str, forwarded: &mut usize, report_line: &mut dyn FnMut(String)) {
    let lines: Vec<&str> = console.lines().collect();
    for line in lines.iter().skip(*forwarded) {
        report_line((*line).to_string());
    }
    if lines.len() > *forwarded {
        *forwarded = lines.len();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_target() -> RemoteTarget {
        RemoteTarget {
            host_id: "hpc".to_string(),
            host_label: "Cluster".to_string(),
            user: "alice".to_string(),
            host: "login.example.edu".to_string(),
            port: 2222,
            key_path: PathBuf::from("/home/alice/.silicolab/keys/id_silicolab_ed25519"),
            known_hosts_path: PathBuf::from("/home/alice/.silicolab/keys/known_hosts"),
            prelude: vec!["module load gromacs".to_string()],
            remote_dir: "~/.silicolab/runs/abc-123".to_string(),
            connect_timeout_secs: 15,
        }
    }

    #[test]
    fn common_opts_use_dedicated_key_and_pinned_known_hosts() {
        let opts = common_opts(&test_target(), "-p").join(" ");
        assert!(opts.contains("IdentitiesOnly=yes"));
        assert!(opts.contains("StrictHostKeyChecking=accept-new"));
        assert!(opts.contains("BatchMode=yes"));
        assert!(opts.contains("known_hosts"));
        assert!(opts.contains("-p 2222"));
    }

    #[test]
    fn scp_uses_capital_port_flag_and_remote_dir() {
        let config = scp_up_config(&test_target(), Path::new("/tmp/em.mdp"), "em.mdp");
        let joined = config.args.join(" ");
        assert!(joined.contains("-P 2222"), "scp must use -P: {joined}");
        assert!(joined.contains("alice@login.example.edu:~/.silicolab/runs/abc-123/em.mdp"));
    }

    #[test]
    fn sh_quote_is_injection_safe() {
        assert_eq!(sh_quote("em.mdp"), "'em.mdp'");
        // A space-bearing path stays one token.
        assert_eq!(sh_quote("/my path/gmx"), "'/my path/gmx'");
        // An embedded single quote is closed/escaped/reopened.
        assert_eq!(sh_quote("a'b"), "'a'\\''b'");
        // Shell metacharacters are inert inside single quotes.
        assert_eq!(sh_quote("$(rm -rf ~)"), "'$(rm -rf ~)'");
    }

    #[test]
    fn launch_script_detaches_with_process_group_and_captures_exit() {
        let tokens = vec![
            "/usr/local/gromacs/bin/gmx".to_string(),
            "grompp".to_string(),
            "-f".to_string(),
            "em.mdp".to_string(),
        ];
        let script = launch_script(&test_target(), "cmd-x", &tokens, false);
        // New session/process group + detached.
        assert!(script.contains("setsid sh -c"));
        assert!(script.contains("</dev/null >/dev/null 2>&1 &"));
        // Authoritative done-signal + PGID capture + prelude applied. (The whole
        // `sh -c` body is single-quote-wrapped, so the prelude's spaces survive;
        // the `&&` chaining the prelude to the engine is present.)
        assert!(script.contains("cmd-x.pgid"));
        assert!(script.contains("cmd-x.exit"));
        assert!(script.contains("module load gromacs"));
        // The engine program and its args all reach the script (each token is
        // sh-quoted then the whole body single-quote-wrapped — exact escaping is
        // covered by `sh_quote_is_injection_safe`).
        assert!(script.contains("grompp"));
        assert!(script.contains("em.mdp"));
        assert!(script.contains("/usr/local/gromacs/bin/gmx"));
        // cd into the run dir before launching.
        assert!(script.contains("cd ~/.silicolab/runs/abc-123"));
    }

    #[test]
    fn launch_script_redirects_stdin_file_when_present() {
        let tokens = vec!["gmx".to_string(), "genion".to_string()];
        let script = launch_script(&test_target(), "cmd-y", &tokens, true);
        assert!(script.contains("< cmd-y.stdin"));
    }

    #[test]
    fn poll_script_reports_status_then_console() {
        let script = poll_script(&test_target(), "cmd-z");
        assert!(script.contains("SILICOLAB_DONE"));
        assert!(script.contains("SILICOLAB_RUNNING"));
        assert!(script.contains("SILICOLAB_NODIR"));
        assert!(script.contains("cat cmd-z.console"));
    }

    #[test]
    fn cancel_script_kills_the_process_group() {
        let script = cancel_script(&test_target(), "cmd-z");
        assert!(script.contains("kill -TERM -- -$PGID"));
        assert!(script.contains("kill -KILL -- -$PGID"));
    }

    #[test]
    fn validate_work_root_rejects_metacharacters() {
        assert!(validate_work_root("~/.silicolab").is_ok());
        assert!(validate_work_root("/scratch/alice/sl").is_ok());
        assert!(validate_work_root("").is_err());
        assert!(validate_work_root("/scratch/my proj").is_err());
        assert!(validate_work_root("/scratch; rm -rf ~").is_err());
        assert!(validate_work_root("/scratch/$(whoami)").is_err());
    }

    #[test]
    fn manifest_uploads_changed_skips_unchanged_and_does_not_re_upload_downloads() {
        let mut manifest = SyncManifest::default();
        // A never-seen file must upload.
        assert!(needs_upload(&manifest, "em.mdp", (10, 100)));
        // After recording its signature (an upload OR a download both record it),
        // the same file is skipped — this is the "don't re-upload what we just
        // pulled back" invariant that prevents the build/pipeline thrash.
        manifest.files.insert("em.mdp".to_string(), (10, 100));
        assert!(!needs_upload(&manifest, "em.mdp", (10, 100)));
        // A size change re-uploads.
        assert!(needs_upload(&manifest, "em.mdp", (12, 100)));
        // An mtime change re-uploads.
        assert!(needs_upload(&manifest, "em.mdp", (10, 101)));
    }

    #[test]
    fn compute_from_launch_is_local() {
        let compute: Compute = EngineLaunch::native("gmx").into();
        assert!(!compute.is_remote());
        assert!(compute.remote_target().is_none());
        let remote = Compute::remote(EngineLaunch::native("gmx"), test_target());
        assert!(remote.is_remote());
        assert_eq!(remote.remote_target().unwrap().host_id, "hpc");
    }

    #[test]
    fn remote_target_anchors_dir_at_run_uuid() {
        use crate::backend::config::RemoteHost;
        let host = RemoteHost {
            id: "h".to_string(),
            label: "H".to_string(),
            hostname: "example.edu".to_string(),
            username: "bob".to_string(),
            port: 22,
            work_root: "~/.silicolab/".to_string(), // trailing slash trimmed
            prelude: Vec::new(),
            engines: HashMap::new(),
            engine_versions: HashMap::new(),
        };
        let target = RemoteTarget::for_run(&host, "abc-123");
        assert_eq!(target.remote_dir, "~/.silicolab/runs/abc-123");
    }

    #[test]
    fn upload_exclusions_cover_bookkeeping_files() {
        assert!(is_excluded_from_upload(MANIFEST_FILE));
        assert!(is_excluded_from_upload("gromacs.log"));
        assert!(is_excluded_from_upload("cmd-abc.console"));
        assert!(is_excluded_from_upload("cmd-abc.exit"));
        assert!(is_excluded_from_upload("cmd-abc.pgid"));
        // Genuine engine inputs are uploaded.
        assert!(!is_excluded_from_upload("em.mdp"));
        assert!(!is_excluded_from_upload("conf.gro"));
        assert!(!is_excluded_from_upload("cmd-abc.stdin"));
    }
}
