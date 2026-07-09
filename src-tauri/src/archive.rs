//! Full data archive export: writes every meeting (transcript + JSON), all
//! dictation history, and a manifest into a dated folder. Called from
//! `commands::data::export_archive` on a background thread, since walking the
//! whole meeting history and writing many small files can take a while.

use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::db::Database;
use crate::export::{self, ExportFormat};

/// Settings > Data stats line: database size, row counts, and recorded
/// meeting audio size.
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct DataStats {
    pub db_size_bytes: u64,
    pub meeting_count: u32,
    pub dictation_count: u32,
    pub recordings_size_bytes: u64,
}

/// `manifest.json` written at the root of every archive.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchiveManifest {
    pub app_version: String,
    pub schema_version: i64,
    /// Total meetings in the source database at export time, including any
    /// that failed to export (see `errors`).
    pub meeting_count: u32,
    pub dictation_count: u32,
    /// Meetings that failed to load/render and were skipped. The archive is
    /// still written for every other meeting; this is not a fatal error.
    pub errors: u32,
    pub exported_at: DateTime<Utc>,
}

/// Where the archive landed and what it contains, returned to the caller so
/// it can build the final progress event.
pub struct ArchiveExportOutcome {
    pub archive_dir: PathBuf,
    pub manifest: ArchiveManifest,
}

/// Write a full archive under a fresh `souffle-export-YYYY-MM-DD[-N]`
/// directory inside `dest_parent`.
///
/// `on_progress(done, total)` is called once per meeting processed
/// (success or failure) and once more after the dictation/manifest step, so
/// `done == total` on the last call. A single meeting that fails to load or
/// render is logged and counted in the manifest's `errors` field rather than
/// aborting the whole export.
pub fn run_archive_export(
    db: &Database,
    dest_parent: &Path,
    now: DateTime<Utc>,
    mut on_progress: impl FnMut(u32, u32),
) -> Result<ArchiveExportOutcome, String> {
    let archive_dir = export::unique_dir(dest_parent, &export::archive_folder_name(now));
    fs::create_dir_all(&archive_dir).map_err(|e| format!("Create archive directory: {e}"))?;

    let meetings = db.list_meetings()?;
    // +1 for the dictations/manifest step, so 100% is only reported once the
    // whole archive (not just the meeting folders) is on disk.
    let total = meetings.len() as u32 + 1;
    let mut errors = 0u32;

    for (index, item) in meetings.iter().enumerate() {
        if let Err(e) = write_meeting_folder(db, &archive_dir, &item.id) {
            tracing::warn!(
                meeting_id = %item.id,
                error = %e,
                "Archive export: skipping meeting that failed to export"
            );
            errors += 1;
        }
        on_progress(index as u32 + 1, total);
    }

    let dictation_entries = db.list_dictation_entries(i64::MAX)?;
    let dictations_json = serde_json::to_string_pretty(&dictation_entries)
        .map_err(|e| format!("Serialize dictations: {e}"))?;
    fs::write(archive_dir.join("dictations.json"), dictations_json)
        .map_err(|e| format!("Write dictations.json: {e}"))?;

    let manifest = ArchiveManifest {
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        schema_version: crate::db::schema::SCHEMA_VERSION,
        meeting_count: meetings.len() as u32,
        dictation_count: dictation_entries.len() as u32,
        errors,
        exported_at: now,
    };
    let manifest_json =
        serde_json::to_string_pretty(&manifest).map_err(|e| format!("Serialize manifest: {e}"))?;
    fs::write(archive_dir.join("manifest.json"), manifest_json)
        .map_err(|e| format!("Write manifest.json: {e}"))?;

    on_progress(total, total);

    Ok(ArchiveExportOutcome {
        archive_dir,
        manifest,
    })
}

/// Write one meeting's `<date>-<slug>/transcript.md` + `meeting.json`.
/// Folder name collisions (two meetings sharing a date and title) are
/// disambiguated the same way as the archive root, by probing the
/// filesystem via [`export::unique_dir`].
fn write_meeting_folder(db: &Database, archive_dir: &Path, meeting_id: &str) -> Result<(), String> {
    let meeting = db.load_meeting(meeting_id)?;

    let base = format!(
        "{}-{}",
        meeting.started_at.format("%Y-%m-%d"),
        export::slugify(&meeting.title)
    );
    let meeting_dir = export::unique_dir(archive_dir, &base);
    fs::create_dir_all(&meeting_dir).map_err(|e| format!("Create meeting directory: {e}"))?;

    let markdown = export::render_meeting(&meeting, ExportFormat::Markdown)?;
    fs::write(meeting_dir.join("transcript.md"), markdown)
        .map_err(|e| format!("Write transcript.md: {e}"))?;

    let json = export::render_meeting(&meeting, ExportFormat::Json)?;
    fs::write(meeting_dir.join("meeting.json"), json).map_err(|e| format!("Write meeting.json: {e}"))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::fixtures::{sample_meeting, test_db};
    use tempfile::TempDir;

    fn fixed_now() -> DateTime<Utc> {
        "2026-07-09T10:00:00Z".parse().unwrap()
    }

    #[test]
    fn empty_db_still_writes_dictations_and_manifest() {
        let (db, _db_dir) = test_db();
        let dest = TempDir::new().unwrap();

        let mut progress_calls = Vec::new();
        let outcome = run_archive_export(&db, dest.path(), fixed_now(), |done, total| {
            progress_calls.push((done, total));
        })
        .unwrap();

        assert_eq!(outcome.manifest.meeting_count, 0);
        assert_eq!(outcome.manifest.dictation_count, 0);
        assert_eq!(outcome.manifest.errors, 0);
        assert_eq!(outcome.manifest.schema_version, crate::db::schema::SCHEMA_VERSION);
        assert_eq!(outcome.archive_dir.file_name().unwrap(), "souffle-export-2026-07-09");

        let dictations = fs::read_to_string(outcome.archive_dir.join("dictations.json")).unwrap();
        assert_eq!(dictations.trim(), "[]");

        assert!(outcome.archive_dir.join("manifest.json").exists());
        // Only the final (total, total) progress call: no meetings to loop over.
        assert_eq!(progress_calls, vec![(1, 1)]);
    }

    #[test]
    fn writes_meeting_folders_dictations_and_manifest() {
        let (db, _db_dir) = test_db();
        let mut m1 = sample_meeting("m1");
        m1.title = "Weekly Sync".to_string();
        m1.started_at = "2026-07-01T09:00:00Z".parse().unwrap();
        db.save_meeting(&m1).unwrap();

        let mut m2 = sample_meeting("m2");
        m2.title = "Budget Review".to_string();
        m2.started_at = "2026-07-03T14:00:00Z".parse().unwrap();
        db.save_meeting(&m2).unwrap();

        db.add_dictation_entry("d1", "Hello there", "2026-07-01T08:00:00Z")
            .unwrap();
        db.add_dictation_entry("d2", "General Kenobi", "2026-07-02T08:00:00Z")
            .unwrap();

        let dest = TempDir::new().unwrap();
        let mut progress_calls = Vec::new();
        let outcome = run_archive_export(&db, dest.path(), fixed_now(), |done, total| {
            progress_calls.push((done, total));
        })
        .unwrap();

        assert_eq!(outcome.manifest.meeting_count, 2);
        assert_eq!(outcome.manifest.dictation_count, 2);
        assert_eq!(outcome.manifest.errors, 0);
        // One call per meeting, then a final call for the dictations/manifest step.
        assert_eq!(progress_calls, vec![(1, 3), (2, 3), (3, 3)]);

        let sync_dir = outcome.archive_dir.join("2026-07-01-weekly-sync");
        assert!(sync_dir.join("transcript.md").exists());
        let transcript = fs::read_to_string(sync_dir.join("transcript.md")).unwrap();
        assert!(transcript.starts_with("# Weekly Sync"));

        let meeting_json = fs::read_to_string(sync_dir.join("meeting.json")).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&meeting_json).unwrap();
        assert_eq!(parsed["id"], "m1");

        assert!(
            outcome
                .archive_dir
                .join("2026-07-03-budget-review")
                .join("transcript.md")
                .exists()
        );

        let dictations_json = fs::read_to_string(outcome.archive_dir.join("dictations.json")).unwrap();
        let dictations: Vec<crate::db::dictation::DictationEntry> =
            serde_json::from_str(&dictations_json).unwrap();
        assert_eq!(dictations.len(), 2);
        assert!(dictations.iter().any(|d| d.text == "Hello there"));

        let manifest_json = fs::read_to_string(outcome.archive_dir.join("manifest.json")).unwrap();
        let manifest: ArchiveManifest = serde_json::from_str(&manifest_json).unwrap();
        assert_eq!(manifest.meeting_count, 2);
        assert_eq!(manifest.app_version, env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn disambiguates_meeting_folders_sharing_date_and_title() {
        let (db, _db_dir) = test_db();
        let mut m1 = sample_meeting("m1");
        m1.title = "Standup".to_string();
        m1.started_at = "2026-07-01T09:00:00Z".parse().unwrap();
        db.save_meeting(&m1).unwrap();

        let mut m2 = sample_meeting("m2");
        m2.title = "Standup".to_string();
        m2.started_at = "2026-07-01T09:00:00Z".parse().unwrap();
        db.save_meeting(&m2).unwrap();

        let dest = TempDir::new().unwrap();
        let outcome = run_archive_export(&db, dest.path(), fixed_now(), |_, _| {}).unwrap();

        assert!(outcome.archive_dir.join("2026-07-01-standup").exists());
        assert!(outcome.archive_dir.join("2026-07-01-standup-2").exists());
        assert_eq!(outcome.manifest.errors, 0);
    }

    #[test]
    fn corrupt_meeting_is_skipped_and_counted_without_aborting() {
        let (db, _db_dir) = test_db();
        let mut good = sample_meeting("good");
        good.title = "Good Meeting".to_string();
        good.started_at = "2026-07-01T09:00:00Z".parse().unwrap();
        db.save_meeting(&good).unwrap();

        db.insert_corrupt_meeting_for_test("bad", "2026-07-02T09:00:00Z")
            .unwrap();

        let dest = TempDir::new().unwrap();
        let mut progress_calls = Vec::new();
        let outcome = run_archive_export(&db, dest.path(), fixed_now(), |done, total| {
            progress_calls.push((done, total));
        })
        .unwrap();

        assert_eq!(outcome.manifest.meeting_count, 2, "counts the attempted total");
        assert_eq!(outcome.manifest.errors, 1);
        assert!(outcome.archive_dir.join("2026-07-01-good-meeting").exists());
        // Progress still reaches (total, total) despite the mid-run failure.
        assert_eq!(progress_calls.last(), Some(&(3, 3)));
    }

    #[test]
    fn archive_folder_disambiguated_on_repeat_export_same_day() {
        let (db, _db_dir) = test_db();
        let dest = TempDir::new().unwrap();

        let first = run_archive_export(&db, dest.path(), fixed_now(), |_, _| {}).unwrap();
        let second = run_archive_export(&db, dest.path(), fixed_now(), |_, _| {}).unwrap();

        assert_eq!(first.archive_dir.file_name().unwrap(), "souffle-export-2026-07-09");
        assert_eq!(second.archive_dir.file_name().unwrap(), "souffle-export-2026-07-09-2");
    }
}
