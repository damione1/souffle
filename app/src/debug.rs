use std::sync::atomic::{AtomicBool, Ordering};

static DEBUG_TRANSCRIPTION: AtomicBool = AtomicBool::new(false);

pub fn set_transcription_debug(enabled: bool) {
    DEBUG_TRANSCRIPTION.store(enabled, Ordering::Relaxed);
}

pub fn transcription_debug_enabled() -> bool {
    DEBUG_TRANSCRIPTION.load(Ordering::Relaxed)
}
