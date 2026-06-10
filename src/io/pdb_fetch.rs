//! Fetching crystallographic structures from a PDB repository by identifier.
//!
//! Files are downloaded over HTTPS and cached under a [`DOWNLOAD_SUBDIR`]
//! directory inside the active project so a structure is only pulled once.

use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};

/// Default base URL for downloading PDB files. The normalized id and a `.pdb`
/// suffix are appended to this prefix (see [`download_url`]).
pub const RCSB_DEFAULT_BASE_URL: &str = "https://files.rcsb.org/download/";

/// Subdirectory, relative to the project root, where fetched structures are
/// stored. Keeping downloads together makes them easy to find and clean up.
pub const DOWNLOAD_SUBDIR: &str = "structures";

/// Upper bound on a downloaded body. PDB files cap at 99,999 atoms, so even the
/// largest single deposition stays well under this; the limit just guards
/// against a runaway or mistaken response.
const MAX_DOWNLOAD_BYTES: u64 = 128 * 1024 * 1024;

/// Result of a fetch: the local file path and whether the file was freshly
/// downloaded (`false` means an existing cached copy was reused).
#[derive(Debug, Clone)]
pub struct FetchedPdb {
    pub path: PathBuf,
    pub downloaded: bool,
}

/// Validate a PDB identifier and return its canonical uppercase form.
///
/// Classic PDB ids are exactly four alphanumeric characters whose first
/// character is a digit.
pub fn normalize_pdb_id(raw: &str) -> Result<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        bail!("PDB id is required");
    }
    if trimmed.len() != 4 || !trimmed.chars().all(|ch| ch.is_ascii_alphanumeric()) {
        bail!("`{trimmed}` is not a valid PDB id (expected four letters or digits)");
    }
    if !trimmed.chars().next().is_some_and(|ch| ch.is_ascii_digit()) {
        bail!("`{trimmed}` is not a valid PDB id (the first character must be a digit)");
    }
    Ok(trimmed.to_ascii_uppercase())
}

/// Build the download URL for a normalized id against the given base URL,
/// tolerating a base that does or does not end in a slash.
pub fn download_url(base_url: &str, normalized_id: &str) -> String {
    let separator = if base_url.ends_with('/') { "" } else { "/" };
    format!("{base_url}{separator}{normalized_id}.pdb")
}

/// Local path a normalized id is cached at inside `dir`.
pub fn target_path(dir: &Path, normalized_id: &str) -> PathBuf {
    dir.join(format!("{normalized_id}.pdb"))
}

/// Download `raw_id` from `base_url` into `dir`, returning the cached path.
///
/// If the file already exists locally it is reused without contacting the
/// network. A successful download writes the file only after the body has been
/// received in full, so an interrupted transfer never leaves a partial file.
pub fn fetch_pdb(raw_id: &str, base_url: &str, dir: &Path) -> Result<FetchedPdb> {
    let id = normalize_pdb_id(raw_id)?;
    let path = target_path(dir, &id);
    if path.is_file() {
        return Ok(FetchedPdb {
            path,
            downloaded: false,
        });
    }

    let url = download_url(base_url, &id);
    let body = download_pdb_text(&url)?;
    if body.trim().is_empty() {
        bail!("the PDB repository returned an empty file for {id}");
    }

    fs::create_dir_all(dir).with_context(|| format!("failed to create {}", dir.display()))?;
    fs::write(&path, body).with_context(|| format!("failed to write {}", path.display()))?;
    Ok(FetchedPdb {
        path,
        downloaded: true,
    })
}

fn download_pdb_text(url: &str) -> Result<String> {
    ureq::get(url)
        .call()
        .with_context(|| format!("failed to download {url}"))?
        .body_mut()
        .with_config()
        .limit(MAX_DOWNLOAD_BYTES)
        .read_to_string()
        .with_context(|| format!("failed to read the response body from {url}"))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{RCSB_DEFAULT_BASE_URL, download_url, normalize_pdb_id, target_path};

    #[test]
    fn normalizes_valid_ids_to_uppercase() {
        assert_eq!(normalize_pdb_id("1mxe").unwrap(), "1MXE");
        assert_eq!(normalize_pdb_id("  4hhb  ").unwrap(), "4HHB");
    }

    #[test]
    fn rejects_malformed_ids() {
        assert!(normalize_pdb_id("").is_err());
        assert!(normalize_pdb_id("12").is_err());
        assert!(normalize_pdb_id("12345").is_err());
        assert!(normalize_pdb_id("abcd").is_err());
        assert!(normalize_pdb_id("1a!c").is_err());
    }

    #[test]
    fn builds_download_url_regardless_of_trailing_slash() {
        assert_eq!(
            download_url(RCSB_DEFAULT_BASE_URL, "1MXE"),
            "https://files.rcsb.org/download/1MXE.pdb"
        );
        assert_eq!(
            download_url("https://example.org/pdb", "1MXE"),
            "https://example.org/pdb/1MXE.pdb"
        );
    }

    #[test]
    fn target_path_appends_pdb_extension() {
        assert_eq!(
            target_path(Path::new("proj/structures"), "1MXE"),
            Path::new("proj/structures/1MXE.pdb")
        );
    }
}
