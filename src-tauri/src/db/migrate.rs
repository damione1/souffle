use std::path::Path;

use tracing::info;

use crate::transcript::MeetingTranscript;

use super::Database;

impl Database {
    /// Migrate existing JSON meeting files into SQLite.
    /// Idempotent: skips meetings that already exist in the database.
    /// On success, renames the meetings directory to meetings.json_backup.
    pub fn migrate_json_meetings(&self, app_data_dir: &Path) -> Result<(), String> {
        let meetings_dir = app_data_dir.join("meetings");
        if !meetings_dir.exists() {
            return Ok(());
        }

        let entries =
            std::fs::read_dir(&meetings_dir).map_err(|e| format!("Read meetings dir: {e}"))?;

        let mut migrated = 0;
        let mut skipped = 0;

        for entry in entries {
            let entry = entry.map_err(|e| format!("Dir entry: {e}"))?;
            let path = entry.path();

            if path.extension().is_none_or(|ext| ext != "json") {
                continue;
            }

            let json = match std::fs::read_to_string(&path) {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!(path = %path.display(), "Skip unreadable file: {e}");
                    continue;
                }
            };

            let transcript: MeetingTranscript = match serde_json::from_str(&json) {
                Ok(t) => t,
                Err(e) => {
                    tracing::warn!(path = %path.display(), "Skip unparseable file: {e}");
                    continue;
                }
            };

            // Skip if already in DB
            if self.meeting_exists(&transcript.id)? {
                skipped += 1;
                continue;
            }

            self.save_meeting(&transcript)?;
            migrated += 1;
        }

        if migrated > 0 {
            // Rename directory to backup
            let backup_dir = app_data_dir.join("meetings.json_backup");
            if !backup_dir.exists() {
                std::fs::rename(&meetings_dir, &backup_dir)
                    .map_err(|e| format!("Rename meetings dir: {e}"))?;
            }
            info!(migrated, skipped, "JSON meeting migration complete");
        } else if skipped > 0 {
            info!(skipped, "All meetings already migrated");
        }

        Ok(())
    }

    /// Migrate settings from tauri-plugin-store's settings.json to SQLite.
    /// Reads existing settings.json, inserts into settings table, renames to .bak.
    pub fn migrate_settings_json(&self, app_data_dir: &Path) -> Result<(), String> {
        let settings_path = app_data_dir.join("settings.json");
        if !settings_path.exists() {
            return Ok(());
        }

        let json = std::fs::read_to_string(&settings_path)
            .map_err(|e| format!("Read settings.json: {e}"))?;

        let values: serde_json::Value =
            serde_json::from_str(&json).map_err(|e| format!("Parse settings.json: {e}"))?;

        if let Some(obj) = values.as_object() {
            for (key, value) in obj {
                // Store each value as JSON string
                let value_str = serde_json::to_string(value)
                    .map_err(|e| format!("Serialize setting value: {e}"))?;
                self.set_setting(key, &value_str)?;
            }
            info!(count = obj.len(), "Settings migrated from settings.json");
        }

        // Rename to backup
        let backup_path = app_data_dir.join("settings.json.bak");
        if !backup_path.exists() {
            std::fs::rename(&settings_path, &backup_path)
                .map_err(|e| format!("Rename settings.json: {e}"))?;
        }

        Ok(())
    }

    /// Migrate dictation history from tauri-plugin-store's dictation_history.json.
    pub fn migrate_dictation_json(&self, app_data_dir: &Path) -> Result<(), String> {
        let path = app_data_dir.join("dictation_history.json");
        if !path.exists() {
            return Ok(());
        }

        let json = std::fs::read_to_string(&path)
            .map_err(|e| format!("Read dictation_history.json: {e}"))?;

        let values: serde_json::Value = serde_json::from_str(&json)
            .map_err(|e| format!("Parse dictation_history.json: {e}"))?;

        // tauri-plugin-store wraps the data — look for "entries" key
        if let Some(entries) = values.get("entries").and_then(|v| v.as_array()) {
            let mut count = 0;
            for entry in entries {
                let id = entry.get("id").and_then(|v| v.as_str()).unwrap_or_default();
                let text = entry
                    .get("text")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                let timestamp = entry
                    .get("timestamp")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();

                if id.is_empty() || text.is_empty() {
                    continue;
                }

                // Skip if already exists (idempotent)
                let conn = self.conn.lock().map_err(|e| format!("Lock: {e}"))?;
                let exists: bool = conn
                    .query_row(
                        "SELECT COUNT(*) > 0 FROM dictation_entries WHERE id = ?1",
                        rusqlite::params![id],
                        |row| row.get(0),
                    )
                    .map_err(|e| format!("Query: {e}"))?;
                drop(conn);

                if !exists {
                    self.add_dictation_entry(id, text, timestamp)?;
                    count += 1;
                }
            }
            if count > 0 {
                info!(
                    count,
                    "Dictation entries migrated from dictation_history.json"
                );
            }
        }

        // Rename to backup
        let backup_path = app_data_dir.join("dictation_history.json.bak");
        if !backup_path.exists() {
            std::fs::rename(&path, &backup_path)
                .map_err(|e| format!("Rename dictation_history.json: {e}"))?;
        }

        Ok(())
    }
}
