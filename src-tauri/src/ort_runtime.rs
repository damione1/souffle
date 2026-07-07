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
