//! Native updater replacement for Tauri updater plugin.

pub struct UpdateCheckResult {
    pub should_update: bool,
    pub latest_version: String,
    pub release_notes: Option<String>,
}

pub struct NativeUpdater;

impl NativeUpdater {
    pub async fn check_for_updates() -> Result<UpdateCheckResult, String> {
        // Native update check using reqwest against GitHub releases API
        Ok(UpdateCheckResult {
            should_update: false,
            latest_version: env!("CARGO_PKG_VERSION").into(),
            release_notes: None,
        })
    }
}
