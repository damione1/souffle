use serde::{Deserialize, Serialize};

const GITHUB_REPO: &str = "damione1/souffle";
const CHECK_TIMEOUT_SECS: u64 = 10;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, specta::Type)]
pub struct UpdateCheckResult {
    pub current_version: String,
    pub latest_version: Option<String>,
    pub update_available: bool,
    pub release_notes: Option<String>,
    pub release_url: Option<String>,
    pub check_error: Option<String>,
}

#[derive(Deserialize)]
struct GitHubRelease {
    tag_name: String,
    html_url: String,
    body: Option<String>,
}

pub fn current_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// Lightweight check against the latest GitHub release. Network failures are
/// surfaced in `check_error` rather than failing the command.
pub fn check_for_updates() -> UpdateCheckResult {
    let current = current_version();
    match fetch_latest_release() {
        Ok(release) => {
            let latest = normalize_version_tag(&release.tag_name);
            let update_available = version_gt(&latest, &current);
            UpdateCheckResult {
                current_version: current,
                latest_version: Some(latest),
                update_available,
                release_notes: release.body.filter(|b| !b.trim().is_empty()),
                release_url: Some(release.html_url),
                check_error: None,
            }
        }
        Err(e) => UpdateCheckResult {
            current_version: current,
            latest_version: None,
            update_available: false,
            release_notes: None,
            release_url: None,
            check_error: Some(e),
        },
    }
}

/// Release notes for the installed version tag (What's New). Network failures
/// return `None` so callers can keep a local fallback string.
pub fn release_notes_for_version(version: &str) -> Option<String> {
    let tag = version_tag_for_api(version);
    let url = format!("https://api.github.com/repos/{GITHUB_REPO}/releases/tags/{tag}");
    fetch_release(&url)
        .ok()
        .and_then(|release| release.body.filter(|b| !b.trim().is_empty()))
}

fn fetch_latest_release() -> Result<GitHubRelease, String> {
    let url = format!("https://api.github.com/repos/{GITHUB_REPO}/releases/latest");
    fetch_release(&url)
}

fn fetch_release(url: &str) -> Result<GitHubRelease, String> {
    let client = github_client()?;
    let response = client
        .get(url)
        .send()
        .map_err(|e| format!("GitHub request failed: {e}"))?;

    if !response.status().is_success() {
        return Err(format!("GitHub returned HTTP {}", response.status()));
    }

    response
        .json::<GitHubRelease>()
        .map_err(|e| format!("Parse GitHub release: {e}"))
}

fn github_client() -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(CHECK_TIMEOUT_SECS))
        .user_agent(format!("souffle/{}", current_version()))
        .build()
        .map_err(|e| format!("HTTP client: {e}"))
}

fn version_tag_for_api(version: &str) -> String {
    format!("v{}", normalize_version_tag(version))
}

fn normalize_version_tag(tag: &str) -> String {
    tag.trim().trim_start_matches('v').to_string()
}

/// Compare dotted numeric version strings (`0.1.0` style).
pub fn version_gt(left: &str, right: &str) -> bool {
    parse_version_parts(left) > parse_version_parts(right)
}

fn parse_version_parts(version: &str) -> Vec<u64> {
    version
        .split('.')
        .map(|part| part.parse::<u64>().unwrap_or(0))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_gt_orders_semver_parts() {
        assert!(version_gt("0.2.0", "0.1.9"));
        assert!(version_gt("1.0.0", "0.9.9"));
        assert!(!version_gt("0.1.0", "0.1.0"));
        assert!(!version_gt("0.1.0", "0.2.0"));
    }

    #[test]
    fn normalize_strips_v_prefix() {
        assert_eq!(normalize_version_tag("v0.1.0"), "0.1.0");
        assert_eq!(normalize_version_tag("0.1.0"), "0.1.0");
    }

    #[test]
    fn version_tag_for_api_adds_v_prefix() {
        assert_eq!(version_tag_for_api("0.1.1"), "v0.1.1");
        assert_eq!(version_tag_for_api("v0.1.1"), "v0.1.1");
    }
}
