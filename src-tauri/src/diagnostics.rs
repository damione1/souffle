use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::logging::log_dir;
use crate::state::AppState;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, specta::Type)]
pub struct DiagnosticsBundle {
    pub app_version: String,
    pub data_dir: String,
    pub log_dir: String,
    pub log_file: Option<String>,
    pub db_path: String,
    pub models_dir: String,
    pub machine_state: String,
    pub log_level: String,
    pub debug_transcription: bool,
}

/// Resolve the most recently modified rolling log file under the logs directory.
pub fn resolve_active_log_file() -> Option<PathBuf> {
    let dir = log_dir();
    let entries = fs::read_dir(&dir).ok()?;
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if !name.starts_with("souffle.log") {
            continue;
        }
        let modified = entry.metadata().ok().and_then(|m| m.modified().ok())?;
        match &best {
            Some((best_time, _)) if modified <= *best_time => {}
            _ => best = Some((modified, path)),
        }
    }
    best.map(|(_, path)| path)
}

/// Return the last `max_lines` non-empty lines from the active log file.
pub fn tail_log_file(max_lines: usize) -> Result<String, String> {
    if max_lines == 0 {
        return Ok(String::new());
    }

    let path = resolve_active_log_file().ok_or_else(|| "No log file found yet".to_string())?;
    tail_file(&path, max_lines)
}

fn tail_file(path: &Path, max_lines: usize) -> Result<String, String> {
    let mut file = fs::File::open(path).map_err(|e| format!("Open log file: {e}"))?;
    let len = file
        .metadata()
        .map_err(|e| format!("Log metadata: {e}"))?
        .len();
    if len == 0 {
        return Ok(String::new());
    }

    // Read at most the last 256 KiB — enough for a few hundred log lines.
    const MAX_BYTES: u64 = 256 * 1024;
    let start = len.saturating_sub(MAX_BYTES);
    file.seek(SeekFrom::Start(start))
        .map_err(|e| format!("Seek log file: {e}"))?;

    let mut buf = Vec::new();
    file.read_to_end(&mut buf)
        .map_err(|e| format!("Read log file: {e}"))?;
    let text = String::from_utf8_lossy(&buf);
    let lines: Vec<&str> = text.lines().filter(|line| !line.is_empty()).collect();
    let tail: Vec<&str> = lines.into_iter().rev().take(max_lines).collect::<Vec<_>>();
    let mut ordered = tail;
    ordered.reverse();
    Ok(ordered.join("\n"))
}

pub fn collect_bundle(state: &AppState, settings: &crate::settings::AppSettings) -> DiagnosticsBundle {
    let data_dir = crate::constants::app_data_dir();
    let log_path = resolve_active_log_file();
    let machine_state = state
        .current_machine_state()
        .map(|m| format!("{m:?}"))
        .unwrap_or_else(|e| format!("error: {e}"));

    DiagnosticsBundle {
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        data_dir: data_dir.display().to_string(),
        log_dir: log_dir().display().to_string(),
        log_file: log_path.map(|p| p.display().to_string()),
        db_path: data_dir.join("souffle.db").display().to_string(),
        models_dir: data_dir.join("models").display().to_string(),
        machine_state,
        log_level: settings.log_level.as_str().to_string(),
        debug_transcription: settings.debug_transcription,
    }
}

/// Replace the user's home directory with `~` so a pasted diagnostic does not
/// carry their account name. Every path in the bundle sits under it.
pub fn redact_home(text: &str) -> String {
    match std::env::var("HOME") {
        Ok(home) if !home.is_empty() && home != "/" => {
            text.replace(home.trim_end_matches('/'), "~")
        }
        _ => text.to_string(),
    }
}

/// Drop every log line emitted on the transcript target.
///
/// The tail is what the README asks people to paste into a public issue, and
/// with "Detailed transcription logs" on those lines hold what the user said.
/// Returns the surviving text and how many lines were removed, so the caller
/// can say so rather than silently shortening the log. The user's own file on
/// disk is untouched.
pub fn strip_transcript_lines(tail: &str) -> (String, usize) {
    let marker = format!(" {}", crate::logging::TRANSCRIPT_TARGET);
    let mut kept = Vec::new();
    let mut dropped = 0usize;
    for line in tail.lines() {
        if line.contains(&marker) {
            dropped += 1;
        } else {
            kept.push(line);
        }
    }
    let mut out = kept.join("\n");
    if tail.ends_with('\n') && !out.is_empty() {
        out.push('\n');
    }
    (out, dropped)
}

pub fn format_bundle_text(bundle: &DiagnosticsBundle, log_tail: &str) -> String {
    let mut out = String::new();
    out.push_str("Soufflé diagnostics\n");
    out.push_str("===================\n");
    out.push_str(&format!("App version: {}\n", bundle.app_version));
    out.push_str(&format!("Machine state: {}\n", bundle.machine_state));
    out.push_str(&format!("Log level: {}\n", bundle.log_level));
    out.push_str(&format!(
        "Detailed transcription logs: {}\n",
        bundle.debug_transcription
    ));
    out.push_str(&format!("Data dir: {}\n", bundle.data_dir));
    out.push_str(&format!("Database: {}\n", bundle.db_path));
    out.push_str(&format!("Models dir: {}\n", bundle.models_dir));
    out.push_str(&format!("Log dir: {}\n", bundle.log_dir));
    if let Some(log_file) = &bundle.log_file {
        out.push_str(&format!("Log file: {log_file}\n"));
    }
    let (tail, dropped) = strip_transcript_lines(log_tail);
    if dropped > 0 {
        out.push_str(&format!(
            "Transcript lines removed from the tail below: {dropped}\n"
        ));
    }
    if !tail.is_empty() {
        out.push_str("\n--- Recent log tail ---\n");
        out.push_str(&tail);
        if !tail.ends_with('\n') {
            out.push('\n');
        }
    }
    redact_home(&out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    use tempfile::tempdir;

    #[test]
    fn tail_file_returns_last_lines() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("souffle.log");
        let mut file = fs::File::create(&path).expect("create");
        for i in 0..10 {
            writeln!(file, "line {i}").expect("write");
        }

        let tail = tail_file(&path, 3).expect("tail");
        assert_eq!(tail, "line 7\nline 8\nline 9");
    }

    #[test]
    fn tail_file_empty_returns_empty() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("souffle.log");
        fs::File::create(&path).expect("create");
        assert_eq!(tail_file(&path, 5).expect("tail"), "");
    }

    /// The stripper keys on the target as the `fmt` layer prints it, so this
    /// emits a real event through a real layer rather than trusting a
    /// hand-written fixture to match the format.
    #[test]
    fn strip_removes_a_genuinely_emitted_transcript_line() {
        use std::sync::{Arc, Mutex};
        use tracing_subscriber::layer::SubscriberExt;

        #[derive(Clone)]
        struct Buffer(Arc<Mutex<Vec<u8>>>);
        impl std::io::Write for Buffer {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.0.lock().expect("buffer lock").extend_from_slice(buf);
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        let sink = Arc::new(Mutex::new(Vec::new()));
        let buffer = Buffer(Arc::clone(&sink));
        let subscriber = tracing_subscriber::registry().with(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_writer(move || buffer.clone()),
        );
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(target: crate::logging::TRANSCRIPT_TARGET, text = "my private sentence", "WORD emitted");
            tracing::info!("Database opened");
        });

        let captured = String::from_utf8(sink.lock().expect("sink lock").clone()).expect("utf8");
        assert!(
            captured.contains("my private sentence"),
            "the event must reach the writer for this test to mean anything"
        );

        let (cleaned, dropped) = strip_transcript_lines(&captured);
        assert_eq!(dropped, 1);
        assert!(
            !cleaned.contains("my private sentence"),
            "speech survived the strip: {cleaned}"
        );
        assert!(
            cleaned.contains("Database opened"),
            "ordinary lines must stay"
        );
    }

    #[test]
    fn strip_keeps_everything_when_no_transcript_lines() {
        let tail = "line one\nline two\n";
        let (cleaned, dropped) = strip_transcript_lines(tail);
        assert_eq!(dropped, 0);
        assert_eq!(cleaned, tail);
    }

    #[test]
    fn redact_home_replaces_the_account_path() {
        let home = std::env::var("HOME").expect("HOME set in tests");
        let text = format!("Log dir: {home}/Library/Logs/souffle");
        let redacted = redact_home(&text);
        assert_eq!(redacted, "Log dir: ~/Library/Logs/souffle");
        assert!(!redacted.contains(&home));
    }
}
