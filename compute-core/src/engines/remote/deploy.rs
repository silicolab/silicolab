//! First-use deployment of the headless compute worker to a remote host.
//!
//! The worker is **not** shipped in a job bundle; it is installed once per host
//! and invoked by absolute path. Deployment is version- and hash-pinned and
//! **fail-closed**: a stale, missing, or checksum-mismatched worker is replaced,
//! never run. The sequence is
//!
//! 1. probe `uname -m` — refuse anything but `x86_64` with an actionable message;
//! 2. fetch the worker asset for this build's exact release tag, verify its
//!    published SHA-256;
//! 3. `scp` into `<work_root>/bin/silicolab-compute-<ver>`, `chmod +x`, then
//!    `ln -sfn` the `silicolab-compute` symlink across atomically — retaining
//!    prior versioned binaries so an in-flight job keeps the executable it
//!    launched against;
//! 4. verify the installed binary self-reports the expected `--version`.
//!
//! The recorded version in [`RemoteHost::engine_versions`] is the fast path: when
//! it already equals this build, deployment is a no-op. The SSH/SCP plumbing is
//! the hardened layer in the parent module, reused verbatim.

use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};

use super::{RemoteTarget, validate_work_root};
use crate::engines::process::{self, ProcessConfig};
use crate::hosts::RemoteHost;
use crate::io::update_check;

/// `engine_versions` key recording the deployed worker's version. A leading
/// underscore can never collide with an `EngineId` string (`uff` / `hartree` /
/// `gromacs` / `docking`, and any future engine, are bare lowercase words).
pub const WORKER_VERSION_KEY: &str = "_worker";

/// Release-asset basename for the worker, matching `release.yml`. Version-less:
/// the release tag already pins the version.
pub const WORKER_ASSET_NAME: &str = "silicolab-compute-x86_64-unknown-linux-musl";

/// Generous cap on the downloaded worker binary.
const MAX_WORKER_BYTES: u64 = 256 * 1024 * 1024;

/// A worker confirmed present on the remote host at a known version.
#[derive(Debug, Clone)]
pub struct DeployedWorker {
    /// The version the deployed binary reports (equals this build).
    pub version: String,
    /// Absolute remote path of the `silicolab-compute` symlink to invoke.
    pub remote_path: String,
}

/// Ensure the worker on `host` matches this build, deploying it on first use and
/// redeploying on any version mismatch. Fail-closed: never returns success while
/// a stale or unverifiable worker would be run.
///
/// When `host.engine_versions[`[`WORKER_VERSION_KEY`]`]` already equals this
/// build, no network or SCP happens. After a fresh deploy the caller records the
/// returned [`DeployedWorker::version`] under that key and persists the config.
pub fn ensure_worker_deployed(host: &RemoteHost, target: &RemoteTarget) -> Result<DeployedWorker> {
    super::ensure_ssh_available()?;
    let current = env!("CARGO_PKG_VERSION");
    let link = worker_link_path(host)?;

    if !needs_redeploy(&host.engine_versions, current) {
        return Ok(DeployedWorker {
            version: current.to_string(),
            remote_path: link,
        });
    }

    let arch = probe_arch(target)?;
    if arch != "x86_64" {
        bail!("{}", arch_refusal_message(&host.label, &arch));
    }

    let tag = format!("v{current}");
    let bin_url = update_check::release_asset_url(&tag, WORKER_ASSET_NAME)?;
    let sum_url = update_check::release_asset_url(&tag, &format!("{WORKER_ASSET_NAME}.sha256"))?;
    let bytes = update_check::download_asset_bytes(&bin_url, MAX_WORKER_BYTES)?;
    let expected = parse_sha256(&update_check::download_asset_text(&sum_url)?)?;
    let actual = sha256_hex(&bytes);
    if !actual.eq_ignore_ascii_case(&expected) {
        bail!(
            "worker checksum mismatch for {tag}: expected {expected}, got {actual}. \
             The download is corrupt or tampered with; deployment aborted."
        );
    }

    install_worker(host, target, current, &bytes)?;

    let reported = run_checked(target, &format!("{link} --version"))?;
    let reported = reported.trim();
    if reported != current {
        bail!(
            "the deployed worker reports version `{reported}` but `{current}` was expected; \
             refusing to run a mismatched worker."
        );
    }

    Ok(DeployedWorker {
        version: current.to_string(),
        remote_path: link,
    })
}

/// Whether the worker must be (re)deployed: true when the recorded version is
/// missing or differs from `current`. The pin is strict equality — an exact match
/// is the only no-op; a newer recorded version still redeploys (fail-closed).
fn needs_redeploy(
    engine_versions: &std::collections::HashMap<String, String>,
    current: &str,
) -> bool {
    engine_versions.get(WORKER_VERSION_KEY).map(String::as_str) != Some(current)
}

/// The actionable refusal for a non-x86_64 host. Names the offending arch and
/// states the supported one.
fn arch_refusal_message(host_label: &str, arch: &str) -> String {
    format!(
        "remote host {host_label} reports architecture `{arch}`, but the SilicoLab compute worker \
         currently ships only for x86_64 Linux. Run the job on an x86_64 host, or run it locally."
    )
}

/// `<work_root>/bin` — validated for shell-safety so the concatenated paths carry
/// no metacharacters (the version and asset names are numeric/constant).
fn worker_bin_dir(host: &RemoteHost) -> Result<String> {
    validate_work_root(&host.work_root)?;
    Ok(format!("{}/bin", host.work_root.trim_end_matches('/')))
}

fn worker_link_path(host: &RemoteHost) -> Result<String> {
    Ok(format!("{}/silicolab-compute", worker_bin_dir(host)?))
}

fn worker_versioned_path(host: &RemoteHost, version: &str) -> Result<String> {
    Ok(format!(
        "{}/silicolab-compute-{version}",
        worker_bin_dir(host)?
    ))
}

fn probe_arch(target: &RemoteTarget) -> Result<String> {
    let out = super::run_probe_command(target, "uname -m", Duration::from_secs(20))?;
    Ok(out.trim().to_string())
}

/// `mkdir -p` the bin dir, `scp` the bytes to the versioned path, then
/// `chmod +x` and swap the symlink atomically (`ln -sfn`), retaining prior
/// versioned binaries.
fn install_worker(
    host: &RemoteHost,
    target: &RemoteTarget,
    version: &str,
    bytes: &[u8],
) -> Result<()> {
    let bin_dir = worker_bin_dir(host)?;
    let versioned = worker_versioned_path(host, version)?;
    let link = worker_link_path(host)?;

    run_checked(target, &format!("mkdir -p {bin_dir}"))?;

    let tmp = std::env::temp_dir().join(format!(
        "silicolab-compute-{version}-{}",
        uuid::Uuid::new_v4().simple()
    ));
    std::fs::write(&tmp, bytes)
        .with_context(|| format!("failed to stage the worker at {}", tmp.display()))?;
    let scp = scp_up(target, &tmp, &versioned);
    let result = process::run(scp);
    let _ = std::fs::remove_file(&tmp);
    let result = result.context("failed to upload the worker binary over SSH")?;
    if !result.success() {
        bail!(
            "uploading the worker failed (exit {}): {}",
            result.exit_code,
            result.stderr.trim()
        );
    }

    run_checked(
        target,
        &format!("chmod +x {versioned} && ln -sfn {versioned} {link}"),
    )?;
    Ok(())
}

/// `scp` a local file up to an absolute `remote_path`, reusing the hardened
/// `-i/-o` option block from the parent module.
fn scp_up(target: &RemoteTarget, local: &Path, remote_path: &str) -> ProcessConfig {
    let mut args = super::common_opts(target, "-P");
    args.push(local.to_string_lossy().into_owned());
    args.push(format!("{}:{remote_path}", target.user_host()));
    ProcessConfig::new("scp", std::env::temp_dir())
        .args(args)
        .timeout(Duration::from_secs(300))
}

/// Run a remote `script` over SSH and return its stdout, bailing on a non-zero
/// exit — the fail-closed primitive every deploy step uses.
fn run_checked(target: &RemoteTarget, script: &str) -> Result<String> {
    super::run_probe_command(target, script, Duration::from_secs(60))
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Parse the digest from a `sha256sum`-format line (`HEX  filename`) or a bare
/// hex string. Rejects anything that is not a 64-hex-character SHA-256.
fn parse_sha256(text: &str) -> Result<String> {
    let token = text
        .split_whitespace()
        .next()
        .context("checksum file is empty")?
        .to_ascii_lowercase();
    if token.len() != 64 || !token.bytes().all(|b| b.is_ascii_hexdigit()) {
        bail!("checksum file does not contain a SHA-256 digest");
    }
    Ok(token)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_version_key_cannot_collide_with_engine_ids() {
        // The reserved key must never be a real EngineId string.
        for id in ["uff", "hartree", "gromacs", "docking"] {
            assert_ne!(WORKER_VERSION_KEY, id);
        }
        assert!(WORKER_VERSION_KEY.starts_with('_'));
    }

    #[test]
    fn sha256_matches_known_vector() {
        // SHA-256 of the empty input.
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn parse_sha256_accepts_sum_format_and_bare_hex() {
        let digest = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        assert_eq!(parse_sha256(digest).unwrap(), digest);
        assert_eq!(
            parse_sha256(&format!(
                "{digest}  silicolab-compute-x86_64-unknown-linux-musl"
            ))
            .unwrap(),
            digest
        );
        // Uppercase is normalized.
        assert_eq!(parse_sha256(&digest.to_uppercase()).unwrap(), digest);
    }

    #[test]
    fn parse_sha256_rejects_non_digests() {
        assert!(parse_sha256("").is_err());
        assert!(parse_sha256("not-a-hash").is_err());
        assert!(parse_sha256("abc123").is_err());
    }

    #[test]
    fn arch_refusal_names_the_bad_arch_and_requires_x86_64() {
        let message = arch_refusal_message("Cluster", "aarch64");
        assert!(message.contains("aarch64"));
        assert!(message.contains("x86_64"));
        assert!(message.contains("Cluster"));
    }

    #[test]
    fn needs_redeploy_is_false_only_on_exact_match() {
        use std::collections::HashMap;
        let mut versions = HashMap::new();

        // Missing key → redeploy.
        assert!(needs_redeploy(&versions, "0.1.1"));

        // Exact match → no redeploy.
        versions.insert(WORKER_VERSION_KEY.to_string(), "0.1.1".to_string());
        assert!(!needs_redeploy(&versions, "0.1.1"));

        // Differing version (older or newer) → redeploy.
        assert!(needs_redeploy(&versions, "0.1.2"));
        versions.insert(WORKER_VERSION_KEY.to_string(), "0.2.0".to_string());
        assert!(needs_redeploy(&versions, "0.1.1"));
    }

    #[test]
    fn worker_paths_anchor_at_work_root_bin() {
        let host = RemoteHost {
            id: "h".into(),
            label: "H".into(),
            hostname: "example.edu".into(),
            username: "bob".into(),
            port: 22,
            work_root: "~/.silicolab/".into(),
            prelude: Vec::new(),
            engines: std::collections::HashMap::new(),
            engine_versions: std::collections::HashMap::new(),
        };
        assert_eq!(worker_bin_dir(&host).unwrap(), "~/.silicolab/bin");
        assert_eq!(
            worker_link_path(&host).unwrap(),
            "~/.silicolab/bin/silicolab-compute"
        );
        assert_eq!(
            worker_versioned_path(&host, "0.1.1").unwrap(),
            "~/.silicolab/bin/silicolab-compute-0.1.1"
        );
    }
}
