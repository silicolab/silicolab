#[cfg(feature = "dev-worker")]
use std::{
    ffi::OsString,
    fs::File,
    io::Read,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};

#[cfg(all(feature = "network", not(feature = "dev-worker")))]
use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
};

#[cfg(all(feature = "network", not(feature = "dev-worker")))]
use crate::io::update_check;

#[cfg(feature = "dev-worker")]
const DEV_WORKER_ENV: &str = "SILICOLAB_DEV_WORKER";
#[cfg(feature = "dev-worker")]
const DEV_WORKER_TARGET: &str = "target/x86_64-unknown-linux-musl/release/silicolab-compute";
#[cfg(all(feature = "network", not(feature = "dev-worker")))]
const RELEASE_ASSET_NAME: &str = "silicolab-compute-x86_64-unknown-linux-musl";
/// Prefix shared by every worker filename: the release qualifier and the local
/// cache entry (`<prefix><version>-<sha256>`).
#[cfg(all(feature = "network", not(feature = "dev-worker")))]
const CACHE_PREFIX: &str = "silicolab-compute-";

pub(super) const MAX_WORKER_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArtifactSource {
    #[cfg(all(feature = "network", not(feature = "dev-worker")))]
    Release,
    #[cfg(feature = "dev-worker")]
    LocalDev,
}

fn selected_source() -> ArtifactSource {
    #[cfg(feature = "dev-worker")]
    {
        ArtifactSource::LocalDev
    }
    #[cfg(all(feature = "network", not(feature = "dev-worker")))]
    {
        ArtifactSource::Release
    }
}

#[derive(Debug)]
pub(super) enum WorkerArtifact {
    #[cfg(all(feature = "network", not(feature = "dev-worker")))]
    Release { version: String },
    #[cfg(feature = "dev-worker")]
    LocalDev { bytes: Vec<u8>, digest: String },
}

impl WorkerArtifact {
    pub(super) fn selected() -> Result<Self> {
        match selected_source() {
            #[cfg(all(feature = "network", not(feature = "dev-worker")))]
            ArtifactSource::Release => Ok(Self::Release {
                version: env!("CARGO_PKG_VERSION").to_string(),
            }),
            #[cfg(feature = "dev-worker")]
            ArtifactSource::LocalDev => Self::read_local_dev(configured_local_dev_path()?),
        }
    }

    pub(super) fn deployment_id(&self) -> String {
        match self {
            #[cfg(all(feature = "network", not(feature = "dev-worker")))]
            Self::Release { version } => version.clone(),
            #[cfg(feature = "dev-worker")]
            Self::LocalDev { digest, .. } => format!("dev:{digest}"),
        }
    }

    pub(super) fn remote_qualifier(&self) -> String {
        match self {
            #[cfg(all(feature = "network", not(feature = "dev-worker")))]
            Self::Release { version } => version.clone(),
            #[cfg(feature = "dev-worker")]
            Self::LocalDev { digest, .. } => format!("dev-{digest}"),
        }
    }

    pub(super) fn expected_version(&self) -> &'static str {
        env!("CARGO_PKG_VERSION")
    }

    pub(super) fn into_bytes(self) -> Result<Vec<u8>> {
        match self {
            #[cfg(all(feature = "network", not(feature = "dev-worker")))]
            Self::Release { version } => {
                if let Some(bytes) = read_cached_worker(&version) {
                    return Ok(bytes);
                }
                let (bytes, digest) = download_release(&version).with_context(|| {
                    format!(
                        "could not obtain the compute worker for version {version}. If this \
                         machine is offline or rate-limited, run a remote job once while \
                         online to cache the worker, then retry."
                    )
                })?;
                store_cached_worker(&version, &digest, &bytes);
                Ok(bytes)
            }
            #[cfg(feature = "dev-worker")]
            Self::LocalDev { bytes, .. } => Ok(bytes),
        }
    }

    #[cfg(feature = "dev-worker")]
    fn read_local_dev(path: PathBuf) -> Result<Self> {
        let metadata = std::fs::metadata(&path).with_context(|| {
            format!(
                "development worker {} is missing; run `cargo xtask build-dev-worker` first",
                path.display()
            )
        })?;
        if !metadata.is_file() {
            bail!(
                "development worker {} is not a regular file",
                path.display()
            );
        }
        validate_worker_size(metadata.len())?;

        let file = File::open(&path)
            .with_context(|| format!("failed to open development worker {}", path.display()))?;
        let mut reader = file.take(MAX_WORKER_BYTES + 1);
        let mut bytes = Vec::with_capacity(metadata.len() as usize);
        reader
            .read_to_end(&mut bytes)
            .with_context(|| format!("failed to read development worker {}", path.display()))?;
        validate_worker_size(bytes.len() as u64)?;
        validate_linux_x86_64_elf(&bytes)
            .with_context(|| format!("invalid development worker {}", path.display()))?;
        let digest = sha256_hex(&bytes);
        Ok(Self::LocalDev { bytes, digest })
    }
}

/// Fetch and verify the release worker, returning its bytes and their verified
/// lowercase SHA-256 so the caller can name the cache entry without re-hashing.
#[cfg(all(feature = "network", not(feature = "dev-worker")))]
fn download_release(version: &str) -> Result<(Vec<u8>, String)> {
    let tag = format!("v{version}");
    let bin_url = update_check::release_asset_url(&tag, RELEASE_ASSET_NAME)?;
    let sum_url = update_check::release_asset_url(&tag, &format!("{RELEASE_ASSET_NAME}.sha256"))?;
    let bytes = update_check::download_asset_bytes(&bin_url, MAX_WORKER_BYTES)?;
    let expected = parse_sha256(&update_check::download_asset_text(&sum_url)?)?;
    let actual = sha256_hex(&bytes);
    if !actual.eq_ignore_ascii_case(&expected) {
        bail!(
            "worker checksum mismatch for {tag}: expected {expected}, got {actual}. \
             The download is corrupt or tampered with; deployment aborted."
        );
    }
    Ok((bytes, actual))
}

/// Per-user cache of verified worker binaries, one file per version. It survives
/// GitHub outages, rate limits, and proxies once a version has been fetched once.
#[cfg(all(feature = "network", not(feature = "dev-worker")))]
fn worker_cache_dir() -> PathBuf {
    crate::hosts::config_dir().join("workers")
}

#[cfg(all(feature = "network", not(feature = "dev-worker")))]
fn cache_entry_name(version: &str, digest: &str) -> String {
    format!("{CACHE_PREFIX}{version}-{digest}")
}

#[cfg(all(feature = "network", not(feature = "dev-worker")))]
fn read_cached_worker(version: &str) -> Option<Vec<u8>> {
    read_cached_worker_in(&worker_cache_dir(), version)
}

/// Return the cached bytes for `version` if a self-consistent entry exists. The
/// filename records the digest the bytes were verified against at download time;
/// a read recomputes it, so a corrupted or planted file is rejected — and dropped
/// so the next deploy repopulates cleanly.
#[cfg(all(feature = "network", not(feature = "dev-worker")))]
fn read_cached_worker_in(dir: &Path, version: &str) -> Option<Vec<u8>> {
    let prefix = format!("{CACHE_PREFIX}{version}-");
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let file_name = entry.file_name();
        let name = file_name.to_string_lossy();
        let Some(digest) = name.strip_prefix(prefix.as_str()) else {
            continue;
        };
        if !is_sha256_hex(digest) {
            continue;
        }
        match load_and_verify_cache_entry(&entry.path(), digest) {
            Ok(bytes) => return Some(bytes),
            Err(_) => {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
    None
}

#[cfg(all(feature = "network", not(feature = "dev-worker")))]
fn load_and_verify_cache_entry(path: &Path, expected_digest: &str) -> Result<Vec<u8>> {
    let metadata = std::fs::metadata(path)
        .with_context(|| format!("failed to stat cached worker {}", path.display()))?;
    validate_worker_size(metadata.len())?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    File::open(path)
        .with_context(|| format!("failed to open cached worker {}", path.display()))?
        .take(MAX_WORKER_BYTES + 1)
        .read_to_end(&mut bytes)
        .with_context(|| format!("failed to read cached worker {}", path.display()))?;
    validate_worker_size(bytes.len() as u64)?;
    validate_linux_x86_64_elf(&bytes)?;
    let actual = sha256_hex(&bytes);
    if !actual.eq_ignore_ascii_case(expected_digest) {
        bail!(
            "cached worker {} does not match its recorded digest",
            path.display()
        );
    }
    Ok(bytes)
}

/// Cache the verified worker. Best-effort: a write failure only costs a future
/// re-download, so it never fails the deploy.
#[cfg(all(feature = "network", not(feature = "dev-worker")))]
fn store_cached_worker(version: &str, digest: &str, bytes: &[u8]) {
    if let Err(error) = store_cached_worker_in(&worker_cache_dir(), version, digest, bytes) {
        eprintln!("failed to cache the worker binary: {error}");
    }
}

#[cfg(all(feature = "network", not(feature = "dev-worker")))]
fn store_cached_worker_in(dir: &Path, version: &str, digest: &str, bytes: &[u8]) -> Result<()> {
    std::fs::create_dir_all(dir)
        .with_context(|| format!("failed to create worker cache {}", dir.display()))?;
    let final_path = dir.join(cache_entry_name(version, digest));
    let nonce = uuid::Uuid::new_v4().simple();
    let tmp = dir.join(format!("{}.tmp-{nonce}", cache_entry_name(version, digest)));
    std::fs::write(&tmp, bytes)
        .with_context(|| format!("failed to stage cached worker {}", tmp.display()))?;
    std::fs::rename(&tmp, &final_path)
        .with_context(|| format!("failed to publish cached worker {}", final_path.display()))?;
    prune_other_versions(dir, version);
    Ok(())
}

/// Drop cache entries for versions other than the running build. A client only
/// ever needs its own compile-time version, and in-flight jobs reference the
/// remote binary, not this cache, so keeping one version is safe.
#[cfg(all(feature = "network", not(feature = "dev-worker")))]
fn prune_other_versions(dir: &Path, keep_version: &str) {
    let keep = format!("{CACHE_PREFIX}{keep_version}-");
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let file_name = entry.file_name();
        let name = file_name.to_string_lossy();
        if name.starts_with(CACHE_PREFIX) && !name.starts_with(keep.as_str()) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

#[cfg(all(feature = "network", not(feature = "dev-worker")))]
fn is_sha256_hex(token: &str) -> bool {
    token.len() == 64 && token.bytes().all(|b| b.is_ascii_hexdigit())
}

#[cfg(feature = "dev-worker")]
fn configured_local_dev_path() -> Result<PathBuf> {
    local_dev_path(
        std::env::var_os(DEV_WORKER_ENV),
        Path::new(env!("CARGO_MANIFEST_DIR")),
    )
}

#[cfg(feature = "dev-worker")]
fn local_dev_path(override_path: Option<OsString>, manifest_dir: &Path) -> Result<PathBuf> {
    if let Some(path) = override_path {
        if path.is_empty() {
            bail!("{DEV_WORKER_ENV} is set but empty");
        }
        return Ok(PathBuf::from(path));
    }
    let workspace = manifest_dir
        .parent()
        .context("compute-core manifest has no workspace parent")?;
    Ok(workspace.join(DEV_WORKER_TARGET))
}

#[cfg(any(feature = "network", feature = "dev-worker"))]
fn validate_worker_size(size: u64) -> Result<()> {
    if size == 0 {
        bail!("development worker is empty");
    }
    if size > MAX_WORKER_BYTES {
        bail!("development worker is {size} bytes, exceeding the {MAX_WORKER_BYTES}-byte limit");
    }
    Ok(())
}

#[cfg(any(feature = "network", feature = "dev-worker"))]
fn validate_linux_x86_64_elf(bytes: &[u8]) -> Result<()> {
    if bytes.len() < 64 {
        bail!("file is too short to contain an ELF64 header");
    }
    if &bytes[..4] != b"\x7fELF" {
        bail!("file is not an ELF executable");
    }
    if bytes[4] != 2 {
        bail!("ELF class is not 64-bit");
    }
    if bytes[5] != 1 {
        bail!("ELF byte order is not little-endian");
    }
    let elf_type = u16::from_le_bytes([bytes[16], bytes[17]]);
    if !matches!(elf_type, 2 | 3) {
        bail!("ELF type is not executable or position-independent executable");
    }
    let machine = u16::from_le_bytes([bytes[18], bytes[19]]);
    if machine != 62 {
        bail!("ELF machine is not x86-64");
    }
    Ok(())
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

#[cfg(all(feature = "network", not(feature = "dev-worker")))]
fn parse_sha256(text: &str) -> Result<String> {
    let token = text
        .split_whitespace()
        .next()
        .context("checksum file is empty")?
        .to_ascii_lowercase();
    if !is_sha256_hex(&token) {
        bail!("checksum file does not contain a SHA-256 digest");
    }
    Ok(token)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_matches_known_vector() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn artifact_source_selection_is_feature_isolated() {
        #[cfg(feature = "dev-worker")]
        assert_eq!(selected_source(), ArtifactSource::LocalDev);
        #[cfg(all(feature = "network", not(feature = "dev-worker")))]
        assert_eq!(selected_source(), ArtifactSource::Release);
    }

    #[cfg(all(feature = "network", not(feature = "dev-worker")))]
    #[test]
    fn release_checksum_parser_accepts_sum_format_and_rejects_invalid_text() {
        let digest = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        assert_eq!(parse_sha256(digest).unwrap(), digest);
        assert_eq!(parse_sha256(&format!("{digest}  worker")).unwrap(), digest);
        assert_eq!(parse_sha256(&digest.to_uppercase()).unwrap(), digest);
        assert!(parse_sha256("").is_err());
        assert!(parse_sha256("not-a-hash").is_err());
    }

    #[cfg(any(feature = "network", feature = "dev-worker"))]
    fn elf_header() -> Vec<u8> {
        let mut bytes = vec![0; 64];
        bytes[..4].copy_from_slice(b"\x7fELF");
        bytes[4] = 2;
        bytes[5] = 1;
        bytes[16..18].copy_from_slice(&2_u16.to_le_bytes());
        bytes[18..20].copy_from_slice(&62_u16.to_le_bytes());
        bytes
    }

    #[cfg(all(feature = "network", not(feature = "dev-worker")))]
    fn scratch_cache_dir() -> PathBuf {
        std::env::temp_dir().join(format!(
            "silicolab-worker-cache-{}",
            uuid::Uuid::new_v4().simple()
        ))
    }

    #[cfg(all(feature = "network", not(feature = "dev-worker")))]
    #[test]
    fn cache_round_trips_and_self_heals_on_tamper() {
        let dir = scratch_cache_dir();
        let bytes = elf_header();
        let digest = sha256_hex(&bytes);
        let version = "0.9.9";

        assert!(read_cached_worker_in(&dir, version).is_none());
        store_cached_worker_in(&dir, version, &digest, &bytes).unwrap();
        assert_eq!(read_cached_worker_in(&dir, version), Some(bytes.clone()));

        // Overwrite the entry with different bytes: its recomputed digest no
        // longer matches the filename, so it is rejected and removed.
        let entry = dir.join(cache_entry_name(version, &digest));
        let mut tampered = bytes.clone();
        tampered.push(0xff);
        std::fs::write(&entry, &tampered).unwrap();
        assert!(read_cached_worker_in(&dir, version).is_none());
        assert!(!entry.exists());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(all(feature = "network", not(feature = "dev-worker")))]
    #[test]
    fn cache_store_prunes_other_versions() {
        let dir = scratch_cache_dir();
        let bytes = elf_header();
        let digest = sha256_hex(&bytes);

        store_cached_worker_in(&dir, "0.1.0", &digest, &bytes).unwrap();
        store_cached_worker_in(&dir, "0.2.0", &digest, &bytes).unwrap();

        assert!(read_cached_worker_in(&dir, "0.1.0").is_none());
        assert_eq!(read_cached_worker_in(&dir, "0.2.0"), Some(bytes));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(all(feature = "network", not(feature = "dev-worker")))]
    #[test]
    fn cache_does_not_confuse_a_prerelease_with_its_stable_version() {
        let dir = scratch_cache_dir();
        let bytes = elf_header();
        let digest = sha256_hex(&bytes);

        store_cached_worker_in(&dir, "0.2.0-beta.1", &digest, &bytes).unwrap();
        assert!(read_cached_worker_in(&dir, "0.2.0").is_none());
        assert_eq!(read_cached_worker_in(&dir, "0.2.0-beta.1"), Some(bytes));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(feature = "dev-worker")]
    #[test]
    fn local_path_prefers_nonempty_environment_override() {
        let manifest = Path::new("/checkout/compute-core");
        assert_eq!(
            local_dev_path(Some(OsString::from("/tmp/custom-worker")), manifest).unwrap(),
            PathBuf::from("/tmp/custom-worker")
        );
        assert!(local_dev_path(Some(OsString::new()), manifest).is_err());
        assert_eq!(
            local_dev_path(None, manifest).unwrap(),
            PathBuf::from("/checkout").join(DEV_WORKER_TARGET)
        );
    }

    #[cfg(feature = "dev-worker")]
    #[test]
    fn local_worker_rejects_missing_path_and_size_limits() {
        let missing = std::env::temp_dir().join(format!(
            "silicolab-missing-worker-{}",
            uuid::Uuid::new_v4().simple()
        ));
        assert!(WorkerArtifact::read_local_dev(missing).is_err());
        assert!(validate_worker_size(0).is_err());
        assert!(validate_worker_size(MAX_WORKER_BYTES).is_ok());
        assert!(validate_worker_size(MAX_WORKER_BYTES + 1).is_err());
    }

    #[cfg(feature = "dev-worker")]
    #[test]
    fn elf_validation_checks_class_endianness_architecture_and_type() {
        let valid = elf_header();
        assert!(validate_linux_x86_64_elf(&valid).is_ok());

        let mut wrong = valid.clone();
        wrong[4] = 1;
        assert!(validate_linux_x86_64_elf(&wrong).is_err());
        wrong = valid.clone();
        wrong[5] = 2;
        assert!(validate_linux_x86_64_elf(&wrong).is_err());
        wrong = valid.clone();
        wrong[18..20].copy_from_slice(&183_u16.to_le_bytes());
        assert!(validate_linux_x86_64_elf(&wrong).is_err());
        wrong = valid;
        wrong[16..18].copy_from_slice(&1_u16.to_le_bytes());
        assert!(validate_linux_x86_64_elf(&wrong).is_err());
    }

    #[cfg(feature = "dev-worker")]
    #[test]
    fn local_artifact_identity_changes_with_binary() {
        let suffix = uuid::Uuid::new_v4().simple();
        let path_a = std::env::temp_dir().join(format!("silicolab-worker-a-{suffix}"));
        let path_b = std::env::temp_dir().join(format!("silicolab-worker-b-{suffix}"));
        let first = elf_header();
        let mut changed = first.clone();
        changed.push(1);
        std::fs::write(&path_a, &first).unwrap();
        std::fs::write(&path_b, &changed).unwrap();

        let artifact_a = WorkerArtifact::read_local_dev(path_a.clone()).unwrap();
        let artifact_a_again = WorkerArtifact::read_local_dev(path_a.clone()).unwrap();
        let artifact_b = WorkerArtifact::read_local_dev(path_b.clone()).unwrap();
        assert_eq!(artifact_a.deployment_id(), artifact_a_again.deployment_id());
        assert_ne!(artifact_a.deployment_id(), artifact_b.deployment_id());
        assert!(artifact_a.deployment_id().starts_with("dev:"));

        std::fs::remove_file(path_a).unwrap();
        std::fs::remove_file(path_b).unwrap();
    }
}
