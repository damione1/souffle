use rusqlite::params;

use crate::lock_ext::MutexExt;

use super::Database;

impl Database {
    /// Get a setting value by key. Returns None if not found.
    pub fn get_setting(&self, key: &str) -> Result<Option<String>, String> {
        let conn = self.conn.acquire()?;

        let result = conn.query_row(
            "SELECT value FROM settings WHERE key = ?1",
            params![key],
            |row| row.get::<_, String>(0),
        );

        match result {
            Ok(value) => Ok(Some(value)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(format!("Query setting: {e}")),
        }
    }

    /// Set a setting value (upsert).
    pub fn set_setting(&self, key: &str, value: &str) -> Result<(), String> {
        let conn = self.conn.acquire()?;

        conn.execute(
            "INSERT OR REPLACE INTO settings (key, value) VALUES (?1, ?2)",
            params![key, value],
        )
        .map_err(|e| format!("Set setting: {e}"))?;

        Ok(())
    }

    /// Get all settings as key-value pairs.
    pub fn get_all_settings(&self) -> Result<Vec<(String, String)>, String> {
        let conn = self.conn.acquire()?;

        let mut stmt = conn
            .prepare("SELECT key, value FROM settings")
            .map_err(|e| format!("Prepare: {e}"))?;

        let pairs = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|e| format!("Query: {e}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Collect: {e}"))?;

        Ok(pairs)
    }
}

#[cfg(test)]
mod tests {
    use crate::db::Database;
    use tempfile::TempDir;

    fn test_db() -> (Database, TempDir) {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("test.db");
        let db = Database::open(&db_path).unwrap();
        (db, dir)
    }

    #[test]
    fn get_set_round_trip() {
        let (db, _dir) = test_db();
        db.set_setting("key1", "value1").unwrap();
        assert_eq!(db.get_setting("key1").unwrap(), Some("value1".to_string()));
    }

    #[test]
    fn upsert_overwrites() {
        let (db, _dir) = test_db();
        db.set_setting("key1", "old").unwrap();
        db.set_setting("key1", "new").unwrap();
        assert_eq!(db.get_setting("key1").unwrap(), Some("new".to_string()));
    }

    #[test]
    fn missing_key_returns_none() {
        let (db, _dir) = test_db();
        assert_eq!(db.get_setting("nonexistent").unwrap(), None);
    }

    #[test]
    fn get_all_settings() {
        let (db, _dir) = test_db();
        db.set_setting("a", "1").unwrap();
        db.set_setting("b", "2").unwrap();
        let all = db.get_all_settings().unwrap();
        assert_eq!(all.len(), 2);
        assert!(all.contains(&("a".to_string(), "1".to_string())));
        assert!(all.contains(&("b".to_string(), "2".to_string())));
    }
}
