use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, fmt, reload};

static LOG_GUARD: OnceLock<WorkerGuard> = OnceLock::new();
static FILTER_HANDLE: OnceLock<reload::Handle<EnvFilter, tracing_subscriber::Registry>> =
    OnceLock::new();

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default, specta::Type)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Error,
    Warn,
    #[default]
    Info,
    Debug,
    Trace,
}

impl LogLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warn => "warn",
            Self::Info => "info",
            Self::Debug => "debug",
            Self::Trace => "trace",
        }
    }

    /// Global filter for the `souffle` crate; third-party crates stay at `warn`.
    pub fn filter_directive(self) -> String {
        format!("souffle={},warn", self.as_str())
    }

    pub fn from_str_lossy(value: &str) -> Self {
        match value.trim().to_lowercase().as_str() {
            "error" => Self::Error,
            "warn" | "warning" => Self::Warn,
            "debug" => Self::Debug,
            "trace" => Self::Trace,
            _ => Self::Info,
        }
    }
}

/// Tracing target for events that carry transcribed speech. It is a child of
/// `souffle`, so the level filter reaches it, and naming it apart lets the
/// diagnostics tail drop those lines before anyone pastes them into an issue.
/// Emitters must still check [`crate::debug::transcription_debug_enabled`]:
/// this target marks the lines, it does not gate them.
pub const TRANSCRIPT_TARGET: &str = "souffle::transcript";

/// Daily log files kept on disk. Rotation had no cap, so files accumulated for
/// the life of the install.
const MAX_LOG_FILES: usize = 14;

pub fn log_dir() -> std::path::PathBuf {
    crate::constants::app_data_dir().join("logs")
}

/// Initialize logging to a rolling file under the app data dir (and stderr for
/// terminal runs). `default_level` is applied unless `SOUFFLE_LOG` overrides it.
pub fn init(default_level: LogLevel) {
    let filter = EnvFilter::try_from_env("SOUFFLE_LOG")
        .unwrap_or_else(|_| EnvFilter::new(default_level.filter_directive()));

    let (filter_layer, handle) = reload::Layer::new(filter);
    let _ = FILTER_HANDLE.set(handle);

    let log_dir = log_dir();
    let appender = if std::fs::create_dir_all(&log_dir).is_ok() {
        tracing_appender::rolling::Builder::new()
            .rotation(tracing_appender::rolling::Rotation::DAILY)
            .filename_prefix("souffle.log")
            .max_log_files(MAX_LOG_FILES)
            .build(&log_dir)
            .ok()
    } else {
        None
    };
    let file_layer = appender.map(|appender| {
        let (writer, guard) = tracing_appender::non_blocking(appender);
        let _ = LOG_GUARD.set(guard);
        fmt::layer().with_ansi(false).with_writer(writer)
    });

    let _ = tracing_subscriber::registry()
        .with(filter_layer)
        .with(fmt::layer().with_writer(std::io::stderr))
        .with(file_layer)
        .try_init();
}

pub fn set_level(level: LogLevel) -> Result<(), String> {
    let handle = FILTER_HANDLE
        .get()
        .ok_or_else(|| "Logging not initialized".to_string())?;
    handle
        .modify(|filter| {
            *filter = EnvFilter::new(level.filter_directive());
        })
        .map_err(|e| format!("Update log level: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_directive_scopes_souffle_only() {
        assert_eq!(LogLevel::Info.filter_directive(), "souffle=info,warn");
        assert_eq!(LogLevel::Debug.filter_directive(), "souffle=debug,warn");
    }

    #[test]
    fn from_str_lossy_defaults_unknown() {
        assert_eq!(LogLevel::from_str_lossy("info"), LogLevel::Info);
        assert_eq!(LogLevel::from_str_lossy("bogus"), LogLevel::Info);
    }
}
