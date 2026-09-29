use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::archive::{self, DataStats};
use crate::constants;
use crate::state::AppState;

/// Progress of the export kicked off by `export_archive`, since it runs on a
/// background thread that returns before the walk finishes. `None` until an
/// export has started this process.
#[derive(Debug, Clone)]
pub struct ArchiveExportProgress {
    pub done: u32,
    pub total: u32,
    pub finished: bool,
    pub error: Option<String>,
}

static EXPORT_PROGRESS: Mutex<Option<ArchiveExportProgress>> = Mutex::new(None);

fn record_export_progress(progress: ArchiveExportProgress) {
    if let Ok(mut guard) = EXPORT_PROGRESS.lock() {
        *guard = Some(progress);
    }
}

/// Snapshot of the in-flight (or last finished) archive export.
pub fn get_archive_export_progress() -> Option<ArchiveExportProgress> {
    EXPORT_PROGRESS.lock().ok().and_then(|guard| guard.clone())
}

/// Kick off a full data archive export in a fresh `souffle-export-*` folder
/// under `dest_dir`. Returns as soon as the destination is validated; the
/// actual export runs on a background thread and reports progress via
/// `get_archive_export_progress`, since walking every meeting can take a
/// while for a large history.
pub fn export_archive(state: Arc<AppState>, dest_dir: String) -> Result<(), String> {
    let dest_path = PathBuf::from(&dest_dir);
    if !dest_path.is_dir() {
        return Err(format!("Destination is not a directory: {dest_dir}"));
    }

    // Replace any previous terminal snapshot before spawning so a UI poller
    // cannot mistake the last export's result for this export's result.
    record_export_progress(ArchiveExportProgress {
        done: 0,
        total: 0,
        finished: false,
        error: None,
    });

    let db = Arc::clone(&state.db);
    std::thread::spawn(move || {
        let now = chrono::Utc::now();
        let result = archive::run_archive_export(&db, &dest_path, now, |done, total| {
            record_export_progress(ArchiveExportProgress {
                done,
                total,
                finished: false,
                error: None,
            });
        });

        let final_progress = match result {
            Ok(outcome) => {
                let total = outcome.manifest.meeting_count + 1;
                ArchiveExportProgress {
                    done: total,
                    total,
                    finished: true,
                    error: None,
                }
            }
            Err(e) => {
                tracing::error!(error = %e, "Archive export failed");
                ArchiveExportProgress {
                    done: 0,
                    total: 0,
                    finished: true,
                    error: Some(e),
                }
            }
        };
        record_export_progress(final_progress);
    });

    Ok(())
}

/// Database size on disk plus meeting/dictation counts, for the Settings >
/// Data stats line.
pub fn get_data_stats(state: Arc<AppState>) -> Result<DataStats, String> {
    let db_path = constants::app_data_dir().join("souffle.db");
    let db_size_bytes = std::fs::metadata(&db_path).map(|m| m.len()).unwrap_or(0);

    Ok(DataStats {
        db_size_bytes,
        meeting_count: state.db.count_meetings()?,
        dictation_count: state.db.count_dictation_entries()?,
        recordings_size_bytes: crate::audio::retention::recordings_size_bytes(),
    })
}

/// Reveal the app's data directory in Finder. Uses the `open` CLI (already
/// the pattern for macOS-only shell-outs in this codebase, see
/// `permissions::open_accessibility_settings` and `calendar::mod`) rather
/// than pulling in the Tauri opener plugin, since this app only ships for
/// macOS and `open` is always available there.
pub fn reveal_data_dir() -> Result<(), String> {
    std::process::Command::new("open")
        .arg(constants::app_data_dir())
        .spawn()
        .map_err(|e| format!("Open Finder: {e}"))?;
    Ok(())
}
