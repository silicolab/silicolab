//! Fail-closed deployment of the headless compute worker to a remote host.
//!
//! The worker is **not** shipped in a job bundle; it is installed once per host
//! and invoked by absolute path. Production deployment is release/checksum-pinned;
//! `dev-worker` deployment is pinned to a validated local binary's SHA-256. Both are
//! **fail-closed**: a stale, missing, or checksum-mismatched worker is replaced,
//! never run. The sequence is
//!
//! 1. probe `uname -m` — refuse anything but `x86_64` with an actionable message;
//! 2. resolve the selected release or local-development artifact;
//! 3. `scp` into an identity-qualified path and `chmod +x`;
//! 4. verify that binary self-reports the expected `--version`;
//! 5. atomically replace the stable symlink, retaining prior binaries so an
//!    in-flight job keeps the executable it launched against.
//!
//! The recorded deployment identity in [`RemoteHost::engine_versions`] is a cache
//! hint. Even on a match, the remote binary must run and report this package
//! version before it is reused.

use std::path::Path;
use std::time::Duration;

use super::{RemoteTarget, artifact::WorkerArtifact, validate_work_root};
use crate::engines::process::{self, ProcessConfig};
use crate::hosts::RemoteHost;
use anyhow::{Context, Result, bail};

/// `engine_versions` key recording the worker deployment identity. A leading
/// underscore can never collide with an `EngineId` string (`uff` / `hartree` /
/// `gromacs` / `docking`, and any future engine, are bare lowercase words).
pub const WORKER_DEPLOYMENT_KEY: &str = "_worker";

/// A worker confirmed present on the remote host for a deployment identity.
#[derive(Debug, Clone)]
pub struct DeployedWorker {
    /// Release semver or `dev:<sha256>` for the exact local binary.
    pub deployment_id: String,
    /// Absolute identity-qualified remote executable path to invoke.
    pub remote_path: String,
}

/// Ensure the selected worker is present, executable, and reports this package
/// version. A stale, missing, or unverifiable cached worker is redeployed.
pub fn ensure_worker_deployed(host: &RemoteHost, target: &RemoteTarget) -> Result<DeployedWorker> {
    super::ensure_ssh_available()?;
    let artifact = WorkerArtifact::selected()?;
    let deployment_id = artifact.deployment_id();
    let expected_version = artifact.expected_version();
    let qualifier = artifact.remote_qualifier();
    let qualified = worker_qualified_path(host, &qualifier)?;

    if recorded_identity(&host.engine_versions) == Some(deployment_id.as_str()) {
        let reported = run_checked(target, &format!("{qualified} --version")).ok();
        if cache_hit_is_usable(
            Some(deployment_id.as_str()),
            &deployment_id,
            reported.as_deref(),
            expected_version,
        ) {
            publish_worker_link(host, target, &qualified)?;
            return Ok(DeployedWorker {
                deployment_id,
                remote_path: qualified,
            });
        }
    }

    let arch = probe_arch(target)?;
    if arch != "x86_64" {
        bail!("{}", arch_refusal_message(&host.label, &arch));
    }

    let bytes = artifact.into_bytes()?;
    install_worker(host, target, &qualifier, &bytes)?;

    let reported = run_checked(target, &format!("{qualified} --version"))?;
    let reported = reported.trim();
    if reported != expected_version {
        bail!(
            "the deployed worker reports version `{reported}` but `{expected_version}` was expected; \
             refusing to run a mismatched worker."
        );
    }
    publish_worker_link(host, target, &qualified)?;

    Ok(DeployedWorker {
        deployment_id,
        remote_path: qualified,
    })
}

fn recorded_identity(engine_versions: &std::collections::HashMap<String, String>) -> Option<&str> {
    engine_versions
        .get(WORKER_DEPLOYMENT_KEY)
        .map(String::as_str)
}

fn cache_hit_is_usable(
    recorded: Option<&str>,
    selected: &str,
    reported: Option<&str>,
    expected_version: &str,
) -> bool {
    recorded == Some(selected) && reported.map(str::trim) == Some(expected_version)
}

/// The actionable refusal for a non-x86_64 host. Names the offending arch and
/// states the supported one.
fn arch_refusal_message(host_label: &str, arch: &str) -> String {
    format!(
        "remote host {host_label} reports architecture `{arch}`, but the SilicoLab compute worker \
         currently ships only for x86_64 Linux. Run the job on an x86_64 host, or run it locally."
    )
}

/// `<work_root>/bin` — validated for shell-safety so concatenated paths carry no
/// metacharacters. Artifact qualifiers are generated internally.
fn worker_bin_dir(host: &RemoteHost) -> Result<String> {
    validate_work_root(&host.work_root)?;
    Ok(format!("{}/bin", host.work_root.trim_end_matches('/')))
}

fn worker_link_path(host: &RemoteHost) -> Result<String> {
    Ok(format!("{}/silicolab-compute", worker_bin_dir(host)?))
}

fn worker_qualified_path(host: &RemoteHost, qualifier: &str) -> Result<String> {
    Ok(format!(
        "{}/silicolab-compute-{qualifier}",
        worker_bin_dir(host)?
    ))
}

fn probe_arch(target: &RemoteTarget) -> Result<String> {
    let out = super::run_probe_command(target, "uname -m", Duration::from_secs(20))?;
    Ok(out.trim().to_string())
}

/// Stage an identity-qualified binary and mark it executable. It is verified by
/// the caller before [`publish_worker_link`] exposes it through the stable link.
fn install_worker(
    host: &RemoteHost,
    target: &RemoteTarget,
    qualifier: &str,
    bytes: &[u8],
) -> Result<()> {
    let bin_dir = worker_bin_dir(host)?;
    let qualified = worker_qualified_path(host, qualifier)?;

    run_checked(target, &format!("mkdir -p {bin_dir}"))?;

    let nonce = uuid::Uuid::new_v4().simple();
    let tmp = std::env::temp_dir().join(format!("silicolab-compute-{qualifier}-{nonce}"));
    std::fs::write(&tmp, bytes)
        .with_context(|| format!("failed to stage the worker at {}", tmp.display()))?;
    let remote_stage = format!("{qualified}.upload-{nonce}");
    let scp = scp_up(target, &tmp, &remote_stage);
    let result = process::run(scp);
    if let Err(error) = std::fs::remove_file(&tmp) {
        eprintln!("failed to remove staged worker {}: {error}", tmp.display());
    }
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
        &format!("chmod +x {remote_stage} && mv -f {remote_stage} {qualified}"),
    )?;
    Ok(())
}

fn publish_worker_link(host: &RemoteHost, target: &RemoteTarget, qualified: &str) -> Result<()> {
    let link = worker_link_path(host)?;
    let qualified_name = qualified
        .rsplit('/')
        .next()
        .context("qualified worker path has no filename")?;
    let next_link = format!("{link}.next-{}", uuid::Uuid::new_v4().simple());
    run_checked(
        target,
        &format!("ln -s {qualified_name} {next_link} && mv -Tf {next_link} {link}"),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_deployment_key_cannot_collide_with_engine_ids() {
        // The reserved key must never be a real EngineId string.
        for id in ["uff", "hartree", "gromacs", "docking"] {
            assert_ne!(WORKER_DEPLOYMENT_KEY, id);
        }
        assert!(WORKER_DEPLOYMENT_KEY.starts_with('_'));
    }

    #[test]
    fn arch_refusal_names_the_bad_arch_and_requires_x86_64() {
        let message = arch_refusal_message("Cluster", "aarch64");
        assert!(message.contains("aarch64"));
        assert!(message.contains("x86_64"));
        assert!(message.contains("Cluster"));
    }

    #[test]
    fn cache_requires_matching_identity_and_successful_remote_verification() {
        let version = "0.1.1";
        let dev = "dev:0123456789abcdef";

        assert!(cache_hit_is_usable(
            Some(version),
            version,
            Some(version),
            version
        ));
        assert!(cache_hit_is_usable(Some(dev), dev, Some(version), version));
        assert!(!cache_hit_is_usable(None, version, Some(version), version));
        assert!(!cache_hit_is_usable(
            Some(version),
            dev,
            Some(version),
            version
        ));
        assert!(!cache_hit_is_usable(
            Some(dev),
            version,
            Some(version),
            version
        ));
        assert!(!cache_hit_is_usable(Some(version), version, None, version));
        assert!(!cache_hit_is_usable(
            Some(version),
            version,
            Some("wrong"),
            version
        ));
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
            resources: Default::default(),
        };
        assert_eq!(worker_bin_dir(&host).unwrap(), "~/.silicolab/bin");
        assert_eq!(
            worker_link_path(&host).unwrap(),
            "~/.silicolab/bin/silicolab-compute"
        );
        assert_eq!(
            worker_qualified_path(&host, "0.1.1").unwrap(),
            "~/.silicolab/bin/silicolab-compute-0.1.1"
        );
        assert_eq!(
            worker_qualified_path(&host, "dev-0123456789abcdef").unwrap(),
            "~/.silicolab/bin/silicolab-compute-dev-0123456789abcdef"
        );
    }
}
