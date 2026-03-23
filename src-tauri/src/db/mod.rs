pub mod dictation;
pub mod meetings;
pub mod migrate;
pub mod schema;
pub mod settings;

use std::path::Path;
use std::sync::Mutex;

use rusqlite::Connection;
use tracing::info;

use crate::lock_ext::MutexExt;

/// SQLite database wrapper with interior mutability via Mutex.
pub struct Database {
    conn: Mutex<Connection>,
}

impl Database {
    /// Open (or create) the database at the given path.
    /// Enables WAL mode, foreign keys, and runs schema migrations.
    /// Then migrates any existing JSON data files.
    pub fn open(db_path: &Path) -> Result<Self, String> {
        // Ensure parent directory exists
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("Create db dir: {e}"))?;
        }

        let conn = Connection::open(db_path).map_err(|e| format!("Open database: {e}"))?;

        // WAL mode — journal_mode PRAGMA returns a result row, must use query_row
        let _: String = conn
            .query_row("PRAGMA journal_mode=WAL", [], |row| row.get(0))
            .map_err(|e| format!("Set WAL mode: {e}"))?;

        // Enable foreign key enforcement (no result row)
        conn.execute_batch("PRAGMA foreign_keys=ON;")
            .map_err(|e| format!("Enable foreign keys: {e}"))?;

        let db = Self {
            conn: Mutex::new(conn),
        };

        db.ensure_schema()?;

        info!(path = %db_path.display(), "Database opened");

        // Run data migrations from JSON files
        if let Some(app_data_dir) = db_path.parent() {
            if let Err(e) = db.migrate_json_meetings(app_data_dir) {
                tracing::warn!("JSON meeting migration: {e}");
            }
            if let Err(e) = db.migrate_settings_json(app_data_dir) {
                tracing::warn!("Settings migration: {e}");
            }
            if let Err(e) = db.migrate_dictation_json(app_data_dir) {
                tracing::warn!("Dictation migration: {e}");
            }
        }

        Ok(db)
    }

    /// Ensure the database schema is at the current version.
    fn ensure_schema(&self) -> Result<(), String> {
        let conn = self.conn.acquire()?;

        // Check if schema_version table exists
        let has_version_table: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='schema_version'",
                [],
                |row| row.get(0),
            )
            .map_err(|e| format!("Check schema_version: {e}"))?;

        let current_version = if has_version_table {
            conn.query_row(
                "SELECT COALESCE(MAX(version), 0) FROM schema_version",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap_or(0)
        } else {
            0
        };

        if current_version < schema::SCHEMA_VERSION {
            info!(
                from = current_version,
                to = schema::SCHEMA_VERSION,
                "Applying schema migration"
            );

            for sql in schema::SCHEMA_V1 {
                conn.execute_batch(sql)
                    .map_err(|e| format!("Schema migration: {e}"))?;
            }

            // Record version
            if current_version == 0 {
                conn.execute(
                    "INSERT INTO schema_version (version) VALUES (?1)",
                    rusqlite::params![schema::SCHEMA_VERSION],
                )
                .map_err(|e| format!("Insert version: {e}"))?;
            } else {
                conn.execute(
                    "UPDATE schema_version SET version = ?1",
                    rusqlite::params![schema::SCHEMA_VERSION],
                )
                .map_err(|e| format!("Update version: {e}"))?;
            }

            info!("Schema migration complete");
        }

        Ok(())
    }
}
