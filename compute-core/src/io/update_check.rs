//! Checking GitHub Releases for a newer published SilicoLab version.
//!
//! A single anonymous request to the GitHub REST API fetches the latest
//! release; its `tag_name` is compared against the compiled-in version. Only a
//! strictly newer, parseable `vMAJOR.MINOR.PATCH` tag counts as an update, so a
//! malformed or pre-release-only tag never produces a false prompt.

use std::time::Duration;

use anyhow::{Context, Result};

/// The releases page offered to the user when an update is found (also the
/// fallback when the API response carries no per-release URL).
pub const RELEASES_URL: &str = "https://github.com/silicolab/silicolab/releases";

/// Timeout for the small JSON/text GitHub API calls (release metadata, `.sha256`).
const API_TIMEOUT: Duration = Duration::from_secs(30);

/// Timeout for the (large) worker-binary download. Generous: the musl binary is
/// tens of MB and a remote host may be on a slow link.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(300);

/// A ureq agent with a global timeout. Without one a hung connection wedges a
/// deploy indefinitely; non-2xx still surfaces as an error (fail-closed).
fn http_agent(timeout: Duration) -> ureq::Agent {
    ureq::Agent::new_with_config(
        ureq::Agent::config_builder()
            .timeout_global(Some(timeout))
            .build(),
    )
}

/// GitHub REST base for this repo's releases. `releases/latest` (the update
/// check) and `releases/tags/<tag>` (deploy's exact-version asset resolution)
/// both hang off it.
const RELEASES_API_BASE: &str = "https://api.github.com/repos/silicolab/silicolab/releases";

const LATEST_RELEASE_API: &str = "https://api.github.com/repos/silicolab/silicolab/releases/latest";

/// The User-Agent GitHub's API requires; carries the running version.
const API_USER_AGENT: &str = concat!("silicolab/", env!("CARGO_PKG_VERSION"));

/// Generous cap on the API response body; a release JSON is a few KB.
const MAX_RESPONSE_BYTES: u64 = 1024 * 1024;

/// A published release newer than the running build.
#[derive(Debug, Clone)]
pub struct AvailableUpdate {
    /// The release's version, without the `v` tag prefix (e.g. `"0.2.0"`).
    pub version: String,
    /// Web page of the release (its notes and downloadable assets).
    pub url: String,
}

/// Query GitHub for the latest release and compare it to the running version.
/// `Ok(None)` means up to date (or the repository has no releases yet —
/// GitHub answers 404 then, which surfaces as an `Err` the caller reports
/// quietly).
pub fn check_for_update() -> Result<Option<AvailableUpdate>> {
    let body = http_agent(API_TIMEOUT)
        .get(LATEST_RELEASE_API)
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", API_USER_AGENT)
        .call()
        .context("failed to query GitHub for the latest release")?
        .body_mut()
        .with_config()
        .limit(MAX_RESPONSE_BYTES)
        .read_to_string()
        .context("failed to read the GitHub release response")?;
    update_from_response(&body, env!("CARGO_PKG_VERSION"))
}

/// Resolve the public download URL of the asset named `asset_name` on the
/// release tagged `tag` (e.g. `v0.1.1`). Unlike [`check_for_update`], which reads
/// `releases/latest`, this pins an exact tag so the deployed worker always
/// matches the running build. Fails closed: a missing release or asset is an
/// `Err`, so a deploy never proceeds against an absent binary.
pub fn release_asset_url(tag: &str, asset_name: &str) -> Result<String> {
    let url = format!("{RELEASES_API_BASE}/tags/{tag}");
    let body = http_agent(API_TIMEOUT)
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", API_USER_AGENT)
        .call()
        .with_context(|| {
            format!(
                "could not fetch GitHub release {tag}. Confirm the release is published \
                 (not a draft) and that the repository is reachable."
            )
        })?
        .body_mut()
        .with_config()
        .limit(MAX_RESPONSE_BYTES)
        .read_to_string()
        .context("failed to read the GitHub release response")?;
    asset_url_from_release(&body, asset_name).with_context(|| {
        format!(
            "release {tag} has no asset named {asset_name}; the worker binary may not have been \
             built for this version (the release's CI worker job is required)."
        )
    })
}

/// Find a named asset's `browser_download_url` in a release JSON body. Split out
/// from the network call so it is unit-testable.
fn asset_url_from_release(body: &str, asset_name: &str) -> Result<String> {
    let json: serde_json::Value =
        serde_json::from_str(body).context("malformed GitHub release response")?;
    let assets = json["assets"]
        .as_array()
        .context("GitHub release response has no assets array")?;
    assets
        .iter()
        .find(|asset| asset["name"].as_str() == Some(asset_name))
        .and_then(|asset| asset["browser_download_url"].as_str())
        .map(str::to_string)
        .with_context(|| format!("the release has no asset named {asset_name}"))
}

/// Download a release asset's raw bytes from its (public) download URL, capped at
/// `max_bytes`. Used by the worker deploy to fetch the musl binary.
pub fn download_asset_bytes(url: &str, max_bytes: u64) -> Result<Vec<u8>> {
    http_agent(DOWNLOAD_TIMEOUT)
        .get(url)
        .header("User-Agent", API_USER_AGENT)
        .call()
        .with_context(|| format!("failed to download {url}"))?
        .body_mut()
        .with_config()
        .limit(max_bytes)
        .read_to_vec()
        .with_context(|| format!("failed to read {url}"))
}

/// Download a small text asset (e.g. a published `.sha256`) from its download URL.
pub fn download_asset_text(url: &str) -> Result<String> {
    http_agent(API_TIMEOUT)
        .get(url)
        .header("User-Agent", API_USER_AGENT)
        .call()
        .with_context(|| format!("failed to download {url}"))?
        .body_mut()
        .with_config()
        .limit(MAX_RESPONSE_BYTES)
        .read_to_string()
        .with_context(|| format!("failed to read {url}"))
}

/// Parse a `releases/latest` response and decide whether it is newer than
/// `current`. Split out from the network call so it is unit-testable.
fn update_from_response(body: &str, current: &str) -> Result<Option<AvailableUpdate>> {
    let json: serde_json::Value =
        serde_json::from_str(body).context("malformed GitHub release response")?;
    let tag = json["tag_name"]
        .as_str()
        .context("GitHub release response has no tag_name")?;
    if !is_newer(tag, current) {
        return Ok(None);
    }
    Ok(Some(AvailableUpdate {
        version: tag.trim_start_matches('v').to_string(),
        url: json["html_url"]
            .as_str()
            .unwrap_or(RELEASES_URL)
            .to_string(),
    }))
}

/// Whether `remote` (a release tag, `v` prefix tolerated) is strictly newer
/// than `current`. Unparseable versions compare as "not newer" — failing
/// closed means a renamed tag scheme can never nag the user.
fn is_newer(remote: &str, current: &str) -> bool {
    match (parse_version(remote), parse_version(current)) {
        (Some(remote), Some(current)) => remote > current,
        _ => false,
    }
}

/// Parse `MAJOR.MINOR.PATCH` (optionally `v`-prefixed; any `-prerelease` or
/// `+build` suffix on the last component is ignored).
fn parse_version(version: &str) -> Option<(u64, u64, u64)> {
    let version = version.trim().trim_start_matches('v');
    let version = version
        .split_once(['-', '+'])
        .map_or(version, |(numeric, _)| numeric);
    let mut parts = version.split('.');
    let mut next = || parts.next()?.parse::<u64>().ok();
    let parsed = (next()?, next()?, next()?);
    parts.next().is_none().then_some(parsed)
}

#[cfg(test)]
mod tests {
    use super::{RELEASES_URL, is_newer, parse_version, update_from_response};

    #[test]
    fn parses_plain_and_prefixed_versions() {
        assert_eq!(parse_version("0.1.0"), Some((0, 1, 0)));
        assert_eq!(parse_version("v1.12.3"), Some((1, 12, 3)));
        assert_eq!(parse_version("v1.2.3-rc.1"), Some((1, 2, 3)));
        assert_eq!(parse_version("nightly"), None);
        assert_eq!(parse_version("1.2"), None);
        assert_eq!(parse_version("1.2.3.4"), None);
    }

    #[test]
    fn newer_comparison_orders_numerically() {
        assert!(is_newer("v0.2.0", "0.1.0"));
        assert!(is_newer("v0.1.10", "0.1.9"));
        assert!(!is_newer("v0.1.0", "0.1.0"));
        assert!(!is_newer("v0.0.9", "0.1.0"));
        assert!(!is_newer("not-a-version", "0.1.0"));
    }

    #[test]
    fn response_with_newer_tag_yields_update() {
        let body = r#"{"tag_name": "v0.2.0", "html_url": "https://example.org/rel/v0.2.0"}"#;
        let update = update_from_response(body, "0.1.0").unwrap().unwrap();
        assert_eq!(update.version, "0.2.0");
        assert_eq!(update.url, "https://example.org/rel/v0.2.0");
    }

    #[test]
    fn response_with_same_tag_yields_none() {
        let body = r#"{"tag_name": "v0.1.0", "html_url": "https://example.org"}"#;
        assert!(update_from_response(body, "0.1.0").unwrap().is_none());
    }

    #[test]
    fn missing_html_url_falls_back_to_releases_page() {
        let body = r#"{"tag_name": "v9.9.9"}"#;
        let update = update_from_response(body, "0.1.0").unwrap().unwrap();
        assert_eq!(update.url, RELEASES_URL);
    }

    #[test]
    fn malformed_response_is_an_error() {
        assert!(update_from_response("not json", "0.1.0").is_err());
        assert!(update_from_response("{}", "0.1.0").is_err());
    }

    #[test]
    fn asset_url_resolves_by_exact_name() {
        let body = r#"{
            "tag_name": "v0.1.1",
            "assets": [
                {"name": "silicolab-v0.1.1-x86_64-unknown-linux-gnu.tar.gz", "browser_download_url": "https://example.org/gui"},
                {"name": "silicolab-compute-x86_64-unknown-linux-musl", "browser_download_url": "https://example.org/worker"},
                {"name": "silicolab-compute-x86_64-unknown-linux-musl.sha256", "browser_download_url": "https://example.org/sum"}
            ]
        }"#;
        assert_eq!(
            super::asset_url_from_release(body, "silicolab-compute-x86_64-unknown-linux-musl")
                .unwrap(),
            "https://example.org/worker"
        );
        assert_eq!(
            super::asset_url_from_release(
                body,
                "silicolab-compute-x86_64-unknown-linux-musl.sha256"
            )
            .unwrap(),
            "https://example.org/sum"
        );
    }

    #[test]
    fn asset_url_missing_asset_fails_closed() {
        let body = r#"{"tag_name": "v0.1.1", "assets": []}"#;
        assert!(
            super::asset_url_from_release(body, "silicolab-compute-x86_64-unknown-linux-musl")
                .is_err()
        );
        // A release JSON without an assets array is also an error, not a silent miss.
        assert!(super::asset_url_from_release(r#"{"tag_name":"v0.1.1"}"#, "x").is_err());
    }
}
