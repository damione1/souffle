use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, fmt, reload};

static LOG_GUARD: OnceLock<WorkerGuard> = OnceLock::new();
static FILTER_HANDLE: OnceLock<reload::Handle<EnvFilter, tracing_subscriber::Registry>> =
    OnceLock::new();

/// Set when `SOUFFLE_LOG` supplied the filter at startup. That filter then
/// stays in place: the detailed logging setting no longer replaces it.
static ENV_FILTER_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Global filter for the `souffle` crate; third-party crates stay at `warn`.
fn filter_directive(detailed: bool) -> &'static str {
    if detailed {
        "souffle=debug,warn"
    } else {
        "souffle=info,warn"
    }
}

/// The filter `SOUFFLE_LOG` asks for, or `None` when it is unset or invalid.
fn env_filter(value: Option<&str>) -> Option<EnvFilter> {
    value.and_then(|directives| EnvFilter::try_new(directives).ok())
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

/// Whether a stderr layer is worth installing: an attached terminal, or an
/// explicit `SOUFFLE_LOG` run. The bundled app's stderr is `/dev/null`, where a
/// second fmt layer only formats every event twice for nobody.
fn stderr_logging_wanted(stderr_is_terminal: bool, souffle_log_set: bool) -> bool {
    stderr_is_terminal || souffle_log_set
}

/// Initialize logging to a rolling file under the app data dir (and stderr for
/// terminal or `SOUFFLE_LOG` runs). The level is Info unless `SOUFFLE_LOG`
/// overrides it.
pub fn init() {
    let from_env = env_filter(std::env::var("SOUFFLE_LOG").ok().as_deref());
    ENV_FILTER_ACTIVE.store(from_env.is_some(), Ordering::Relaxed);
    let filter = from_env.unwrap_or_else(|| EnvFilter::new(filter_directive(false)));

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

    let to_stderr = stderr_logging_wanted(
        std::io::IsTerminal::is_terminal(&std::io::stderr()),
        std::env::var_os("SOUFFLE_LOG").is_some(),
    );

    let _ = tracing_subscriber::registry()
        .with(filter_layer)
        .with(to_stderr.then(|| fmt::layer().with_writer(std::io::stderr)))
        .with(file_layer)
        .try_init();
}

/// Apply the detailed logging setting: Debug for the `souffle` crate when on,
/// Info otherwise. A no-op while a `SOUFFLE_LOG` filter is in place.
pub fn set_detailed(detailed: bool) -> Result<(), String> {
    if ENV_FILTER_ACTIVE.load(Ordering::Relaxed) {
        return Ok(());
    }
    let handle = FILTER_HANDLE
        .get()
        .ok_or_else(|| "Logging not initialized".to_string())?;
    handle
        .modify(|filter| {
            *filter = EnvFilter::new(filter_directive(detailed));
        })
        .map_err(|e| format!("Update log level: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_directive_scopes_souffle_only() {
        assert_eq!(filter_directive(false), "souffle=info,warn");
        assert_eq!(filter_directive(true), "souffle=debug,warn");
    }

    #[test]
    fn stderr_layer_only_for_terminal_or_souffle_log() {
        assert!(!stderr_logging_wanted(false, false));
        assert!(stderr_logging_wanted(true, false));
        assert!(stderr_logging_wanted(false, true));
        assert!(stderr_logging_wanted(true, true));
    }

    #[test]
    fn souffle_log_counts_only_when_it_parses() {
        assert!(env_filter(None).is_none());
        assert!(env_filter(Some("souffle=trace,warn")).is_some());
        assert!(env_filter(Some("souffle=not-a-level")).is_none());
    }
}
