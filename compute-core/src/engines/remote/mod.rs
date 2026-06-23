//! Remote (SSH) execution transport for external-process engines.
//!
//! This module owns all SSH/SCP mechanics as **plain-data value types** — no async
//! runtime, no live-connection objects threaded through the engine layer. Every
//! `ssh`/`scp`/`ssh-keygen` invocation is built as a [`ProcessConfig`] and run
//! through the existing [`crate::engines::process`] layer (reusing its timeout,
//! cancel, and shell-free spawn).
//!
//! [`RemoteTarget`], the hardened SSH option block, the incremental
//! [`sync_up`]/[`sync_down`] file transfer, and the host probes
//! ([`detect_remote_engine`], [`run_probe_command`]) are the shared spine the
//! detached [`launcher`] drives to deploy and run the pre-deployed headless worker
//! on a host. A GROMACS launch travels with [`Compute`].

pub mod bootstrap;
#[cfg(feature = "network")]
pub mod deploy;
pub mod hardware;
pub mod launcher;
pub mod run_record;

pub use run_record::*;

use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    time::{Duration, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::engines::process::{self, ProcessConfig};
use crate::engines::registry::EngineLaunch;
use crate::hosts::RemoteHost;

/// CPU/GPU resources a `gmx mdrun` subprocess may use, mapped to mdrun flags by
/// the runner. `0` means "let gmx decide" (its own default — all cores / detected
/// GPUs), preserving the prior behaviour for an untouched run. Serializable so a
/// relayed remote job carries the request to the worker.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct ComputeResources {
    /// CPU threads for the mdrun (`-nt`, or `-ntomp` under a GPU rank); `0` = gmx
    /// default (all available cores).
    pub cores: u32,
    /// GPUs to offload to (`-ntmpi`/`-nb gpu`…); `0` = none / gmx auto-detect.
    pub gpu: u32,
}

/// How to invoke `gmx`: the launch descriptor (and the resource envelope) threaded
/// through the GROMACS pipeline so a run and its launch travel together. `gmx`
/// always runs as a local subprocess of whichever host executes the pipeline — the
/// laptop for a local run, the compute node for a relayed remote run — so there is
/// no transport here.
#[derive(Debug, Clone)]
pub struct Compute {
    pub launch: EngineLaunch,
    pub resources: ComputeResources,
}

impl Compute {
    /// Run `gmx` as a local subprocess with this launch, letting gmx pick its own
    /// CPU/GPU defaults.
    pub fn local(launch: EngineLaunch) -> Self {
        Self {
            launch,
            resources: ComputeResources::default(),
        }
    }

    /// Run `gmx` locally with an explicit CPU/GPU resource request.
    pub fn local_with_resources(launch: EngineLaunch, resources: ComputeResources) -> Self {
        Self { launch, resources }
    }
}

impl From<EngineLaunch> for Compute {
    /// Keeps existing call sites terse (`launch.into()`).
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
        || name == REMOTE_RUN_FILE
        || name == "outcome.json"
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
    fn compute_from_launch_carries_it() {
        let compute: Compute = EngineLaunch::native("gmx").into();
        assert_eq!(compute.launch.program, "gmx");
    }

    #[test]
    fn remote_target_anchors_dir_at_run_uuid() {
        use crate::hosts::RemoteHost;
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
            resources: Default::default(),
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
