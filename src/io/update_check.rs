//! Checking GitHub Releases for a newer published SilicoLab version.
//!
//! A single anonymous request to the GitHub REST API fetches the latest
//! release; its `tag_name` is compared against the compiled-in version. Only a
//! strictly newer, parseable `vMAJOR.MINOR.PATCH` tag counts as an update, so a
//! malformed or pre-release-only tag never produces a false prompt.

use anyhow::{Context, Result};

/// The releases page offered to the user when an update is found (also the
/// fallback when the API response carries no per-release URL).
pub const RELEASES_URL: &str = "https://github.com/silicolab/silicolab/releases";

const LATEST_RELEASE_API: &str = "https://api.github.com/repos/silicolab/silicolab/releases/latest";

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
    let body = ureq::get(LATEST_RELEASE_API)
        .header("Accept", "application/vnd.github+json")
        .header(
            "User-Agent",
            concat!("silicolab/", env!("CARGO_PKG_VERSION")),
        )
        .call()
        .context("failed to query GitHub for the latest release")?
        .body_mut()
        .with_config()
        .limit(MAX_RESPONSE_BYTES)
        .read_to_string()
        .context("failed to read the GitHub release response")?;
    update_from_response(&body, env!("CARGO_PKG_VERSION"))
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
}
