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

/// Background daily check.
///
/// Deliberately the same request the About button makes: ask GitHub for the
/// latest release, compare versions, show a dialog when ours is older. It
/// never downloads or installs anything.
pub mod scheduler {
    use std::time::Duration;

    use tauri::Manager;
    use tauri_specta::Event;
    use tracing::{info, warn};

    use crate::app_events::UpdateAvailable;
    use crate::settings::{AppSettings, LAST_UPDATE_CHECK_AT_KEY};
    use crate::state::AppState;

    /// How often the task wakes to decide whether a check is due.
    const TICK: Duration = Duration::from_secs(60 * 60);

    /// Minimum gap between two checks. Under a day on purpose: at exactly 24h
    /// a machine woken at a slightly earlier hour each day would skip a day.
    const MIN_GAP_SECONDS: i64 = 20 * 60 * 60;

    /// Grace period after launch, so the check never competes with startup.
    const STARTUP_DELAY: Duration = Duration::from_secs(90);

    pub fn spawn(app: tauri::AppHandle) {
        tauri::async_runtime::spawn(run(app));
    }

    /// Whether a check is due, given the last recorded time. `None` means
    /// never checked, which is due.
    fn check_is_due(now: i64, last: Option<i64>) -> bool {
        match last {
            // A clock moved backwards leaves a future timestamp; check anyway
            // rather than going quiet until it catches up.
            Some(last) => now - last >= MIN_GAP_SECONDS || last > now,
            None => true,
        }
    }

    async fn run(app: tauri::AppHandle) {
        tokio::time::sleep(STARTUP_DELAY).await;
        let mut interval = tokio::time::interval(TICK);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        // One dialog per launch per version: a user who dismissed it should not
        // meet it again an hour later.
        let mut announced: Option<String> = None;

        loop {
            interval.tick().await;

            let db = app.state::<AppState>().db.clone();
            let settings = match AppSettings::load(&db) {
                Ok(settings) => settings,
                Err(e) => {
                    warn!("Update check: settings load failed: {e}");
                    continue;
                }
            };
            if !settings.auto_update_check_enabled {
                continue;
            }

            let now = chrono::Utc::now().timestamp();
            let last = db
                .get_setting(LAST_UPDATE_CHECK_AT_KEY)
                .ok()
                .flatten()
                .and_then(|raw| raw.trim().parse::<i64>().ok());
            if !check_is_due(now, last) {
                continue;
            }

            let result = match tauri::async_runtime::spawn_blocking(super::check_for_updates).await
            {
                Ok(result) => result,
                Err(e) => {
                    warn!("Update check: task failed: {e}");
                    continue;
                }
            };
            // Recorded even when the request failed, so a machine offline for a
            // week does not retry every hour.
            if let Err(e) = db.set_setting(LAST_UPDATE_CHECK_AT_KEY, &now.to_string()) {
                warn!("Update check: could not record the check time: {e}");
            }
            if let Some(error) = &result.check_error {
                info!("Update check: {error}");
                continue;
            }
            let Some(latest) = result.latest_version.clone() else {
                continue;
            };
            if !result.update_available || announced.as_deref() == Some(latest.as_str()) {
                continue;
            }

            info!(latest = %latest, "Update check: newer release available");
            announced = Some(latest.clone());
            if let Err(e) = (UpdateAvailable {
                latest_version: latest,
                release_notes: result.release_notes,
                release_url: result.release_url,
            })
            .emit(&app)
            {
                warn!("Update check: emit failed: {e}");
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::{MIN_GAP_SECONDS, check_is_due};

        #[test]
        fn a_first_run_is_due() {
            assert!(check_is_due(1_000_000, None));
        }

        #[test]
        fn a_recent_check_is_not_due() {
            let now = 1_000_000;
            assert!(!check_is_due(now, Some(now - MIN_GAP_SECONDS + 1)));
            assert!(check_is_due(now, Some(now - MIN_GAP_SECONDS)));
        }

        /// A timestamp in the future means the clock moved, not that we checked
        /// tomorrow. Going quiet until it catches up would be worse.
        #[test]
        fn a_future_timestamp_still_checks() {
            let now = 1_000_000;
            assert!(check_is_due(now, Some(now + 86_400)));
        }
    }
}
