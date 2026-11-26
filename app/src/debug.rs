use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

static DEBUG_TRANSCRIPTION: AtomicBool = AtomicBool::new(false);

pub fn set_transcription_debug(enabled: bool) {
    DEBUG_TRANSCRIPTION.store(enabled, Ordering::Relaxed);
}

pub fn transcription_debug_enabled() -> bool {
    DEBUG_TRANSCRIPTION.load(Ordering::Relaxed)
}

/// Where the engine writes the first seconds of a session's input while
/// detailed logging is on. The file only lives for the length of the session.
pub fn engine_input_capture_path() -> PathBuf {
    crate::constants::app_data_dir().join("debug_engine_input.wav")
}

/// Delete the engine input capture at `path` and drop the samples still
/// buffered for it. Runs whatever the debug flag says: the flag may have been
/// turned off since the file was written.
pub fn remove_engine_input_capture(path: &Path) {
    crate::engine::kyutai::discard_debug_capture();
    match std::fs::remove_file(path) {
        Ok(()) => tracing::debug!("Removed the engine input capture"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => tracing::warn!("Failed to remove the engine input capture: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::remove_engine_input_capture;

    #[test]
    fn removing_the_capture_deletes_the_file_and_tolerates_its_absence() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("capture.wav");
        std::fs::write(&path, b"audio").expect("plant capture");

        remove_engine_input_capture(&path);
        assert!(!path.exists());
        remove_engine_input_capture(&path);
    }
}
