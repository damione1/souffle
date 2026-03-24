use std::sync::atomic::{AtomicBool, Ordering};

use crate::db::Database;

static DEBUG_TRANSCRIPTION: AtomicBool = AtomicBool::new(false);

fn parse_bool(value: &str) -> Option<bool> {
    serde_json::from_str::<bool>(value)
        .ok()
        .or_else(|| match value.trim() {
            "1" | "true" | "TRUE" | "on" | "ON" => Some(true),
            "0" | "false" | "FALSE" | "off" | "OFF" => Some(false),
            _ => None,
        })
}

pub fn init_from_db(db: &Database) {
    if let Ok(env_value) = std::env::var("SOUFFLE_DEBUG_TRANSCRIPTION")
        && let Some(enabled) = parse_bool(&env_value)
    {
        set_transcription_debug(enabled);
        return;
    }

    if let Ok(Some(value)) = db.get_setting("debug_transcription")
        && let Some(enabled) = parse_bool(&value)
    {
        set_transcription_debug(enabled);
    }
}

pub fn set_transcription_debug(enabled: bool) {
    DEBUG_TRANSCRIPTION.store(enabled, Ordering::Relaxed);
}

pub fn transcription_debug_enabled() -> bool {
    DEBUG_TRANSCRIPTION.load(Ordering::Relaxed)
}
