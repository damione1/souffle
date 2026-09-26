//! ONNX Runtime initialization shared by every ort consumer (Silero VAD,
//! Parakeet engine).
//!
//! ort uses the `load-dynamic` feature so ONNX Runtime is dlopen'ed from the
//! bundled dylib instead of statically linked. This keeps its protobuf
//! symbols isolated from sentencepiece's statically-linked copy — two static
//! protobufs in one process corrupt each other's descriptor pools (SIGABRT in
//! TrainerSpec::SharedDtor). Never link onnxruntime statically in this app.

use std::path::PathBuf;

const ORT_DYLIB_FILENAME: &str = "libonnxruntime.dylib";

/// Search for a bundled resource file in standard locations
/// (next to the binary in dev, in the .app bundle when packaged).
/// The ONNX Runtime dylib ships in Contents/Frameworks rather than
/// Resources so the bundler code-signs it; notarization rejects unsigned
/// Mach-O files anywhere in the bundle.
pub fn resolve_resource(filename: &str) -> Option<PathBuf> {
    let candidates: Vec<PathBuf> = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.to_path_buf()))
        .map(|bin_dir| {
            vec![
                bin_dir.join("../Frameworks").join(filename),
                bin_dir.join("resources").join(filename),
                bin_dir.join("../Resources/resources").join(filename),
                PathBuf::from("resources").join(filename),
            ]
        })
        .unwrap_or_default();

    candidates.into_iter().find(|path| path.exists())
}

/// Initialize ONNX Runtime with the bundled dylib (load-dynamic mode).
/// Must be called before creating any ort session (VAD filter, Parakeet).
/// Safe to call multiple times — only the first call does anything.
pub fn ensure_ort_initialized() {
    use std::sync::Once;
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        if let Some(dylib_path) = resolve_resource(ORT_DYLIB_FILENAME) {
            tracing::info!(path = %dylib_path.display(), "Loading ONNX Runtime dylib");
            match ort::init_from(&dylib_path) {
                Ok(builder) => {
                    builder.commit();
                }
                Err(e) => tracing::warn!("Failed to init ort with bundled dylib: {e}"),
            }
        } else {
            tracing::warn!(
                "ONNX Runtime dylib ({ORT_DYLIB_FILENAME}) not found — VAD and ONNX engines will be unavailable"
            );
        }
    });
}

/// How long a test waits for [`ensure_ort_initialized`] before failing. A
/// healthy load takes well under a second; the CI hang (SOU-132) never
/// returns, so anything past this is reported as a stalled initialization.
#[cfg(test)]
pub(crate) const TEST_ORT_INIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Test-only entry point: every test that builds an ort session goes through
/// here, so the bundled dylib is loaded by the single `Once` above and never
/// by ort's own lazy init racing it on another test thread (SOU-132).
///
/// Panics, naming the step, when the initialization does not return within
/// [`TEST_ORT_INIT_TIMEOUT`], instead of holding the test runner forever.
#[cfg(test)]
pub(crate) fn ensure_ort_initialized_for_test() {
    if let Err(stalled) = run_within(
        "ort_runtime::ensure_ort_initialized (ort::init_from bundled libonnxruntime.dylib)",
        TEST_ORT_INIT_TIMEOUT,
        ensure_ort_initialized,
    ) {
        panic!("{stalled}");
    }
}

/// A step that did not return within its deadline.
#[cfg(test)]
#[derive(Debug)]
pub(crate) struct StepTimedOut {
    pub step: &'static str,
    pub timeout: std::time::Duration,
}

#[cfg(test)]
impl std::fmt::Display for StepTimedOut {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} did not return within {:?} (stalled initialization, see SOU-132)",
            self.step, self.timeout
        )
    }
}

/// Run `f` on a helper thread and wait at most `timeout` for it. On timeout
/// the helper thread is left detached (it cannot be cancelled); the caller
/// gets an error naming `step`.
#[cfg(test)]
pub(crate) fn run_within<T, F>(
    step: &'static str,
    timeout: std::time::Duration,
    f: F,
) -> Result<T, StepTimedOut>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name(format!("bounded: {step}"))
        .spawn(move || {
            // The receiver is gone once the caller timed out; nothing to report.
            let _ = tx.send(f());
        })
        .expect("spawn bounded step thread");
    match rx.recv_timeout(timeout) {
        Ok(value) => Ok(value),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Err(StepTimedOut { step, timeout }),
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            panic!("{step} panicked before returning")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn run_within_returns_the_value_of_a_step_that_finishes() {
        let value = run_within("quick step", Duration::from_secs(5), || 42)
            .expect("quick step must finish");
        assert_eq!(value, 42);
    }

    #[test]
    fn run_within_names_the_step_that_stalls() {
        // The step blocks until `release` is dropped, i.e. after the deadline.
        let (release, blocked) = std::sync::mpsc::channel::<()>();
        let stalled = run_within("stuck init", Duration::from_millis(50), move || {
            let _ = blocked.recv();
        })
        .expect_err("a step that never returns must time out");
        drop(release);
        assert_eq!(stalled.step, "stuck init");
        assert!(stalled.to_string().contains("stuck init did not return"));
    }
}
