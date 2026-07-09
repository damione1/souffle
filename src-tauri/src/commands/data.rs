use std::path::PathBuf;
use std::sync::Arc;

use tauri::{AppHandle, State};
use tauri_specta::Event;

use crate::app_events::ArchiveExportProgress;
use crate::archive::{self, DataStats};
use crate::constants;
use crate::state::AppState;

/// Kick off a full data archive export in a fresh `souffle-export-*` folder
/// under `dest_dir`. Returns as soon as the destination is validated; the
/// actual export runs on a background thread and reports progress through
/// `ArchiveExportProgress` events, since walking every meeting can take a
/// while for a large history.
#[tauri::command]
#[specta::specta]
pub fn export_archive(app: AppHandle, state: State<'_, AppState>, dest_dir: String) -> Result<(), String> {
    let dest_path = PathBuf::from(&dest_dir);
    if !dest_path.is_dir() {
        return Err(format!("Destination is not a directory: {dest_dir}"));
    }

    let db = Arc::clone(&state.db);
    std::thread::spawn(move || {
        let now = chrono::Utc::now();
        let result = archive::run_archive_export(&db, &dest_path, now, |done, total| {
            let _ = ArchiveExportProgress {
                done,
                total,
                finished: false,
                error: None,
            }
            .emit(&app);
        });

        let final_event = match result {
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
        let _ = final_event.emit(&app);
    });

    Ok(())
}

/// Database size on disk plus meeting/dictation counts, for the Settings >
/// Data stats line.
#[tauri::command]
#[specta::specta]
pub fn get_data_stats(state: State<'_, AppState>) -> Result<DataStats, String> {
    let db_path = constants::app_data_dir().join("souffle.db");
    let db_size_bytes = std::fs::metadata(&db_path).map(|m| m.len()).unwrap_or(0);

    Ok(DataStats {
        db_size_bytes,
        meeting_count: state.db.count_meetings()?,
        dictation_count: state.db.count_dictation_entries()?,
    })
}

/// Reveal the app's data directory in Finder. Uses the `open` CLI (already
/// the pattern for macOS-only shell-outs in this codebase, see
/// `permissions::open_accessibility_settings` and `calendar::mod`) rather
/// than pulling in the Tauri opener plugin, since this app only ships for
/// macOS and `open` is always available there.
#[tauri::command]
#[specta::specta]
pub fn reveal_data_dir() -> Result<(), String> {
    std::process::Command::new("open")
        .arg(constants::app_data_dir())
        .spawn()
        .map_err(|e| format!("Open Finder: {e}"))?;
    Ok(())
}
