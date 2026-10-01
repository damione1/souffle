//! SpeechAnalyzer bridge, exclusively driven by the engine actor.
use super::{AudioInputRequirements, EngineError, TranscriptionEngine, TranscriptionSegment};
use serde::Deserialize;
use std::ffi::{CStr, CString, c_char};
use std::path::Path;
use std::sync::OnceLock;

#[derive(Debug, Clone)]
pub enum Availability {
    Available { locale: String, installed: bool },
    Unavailable { reason: String },
}
impl Availability {
    pub fn locale(&self) -> Option<&str> {
        match self {
            Self::Available { locale, .. } => Some(locale),
            Self::Unavailable { .. } => None,
        }
    }
    pub fn reason(&self) -> Option<&str> {
        match self {
            Self::Available { .. } => None,
            Self::Unavailable { reason } => Some(reason),
        }
    }
    pub fn is_available(&self) -> bool {
        match self {
            Self::Available { .. } => true,
            Self::Unavailable { .. } => false,
        }
    }
    pub fn is_installed_for(&self, selected: &str) -> bool {
        match self {
            Self::Available { locale, installed } => *installed && locale == selected,
            Self::Unavailable { .. } => false,
        }
    }
}
#[derive(Deserialize)]
struct AvailabilityWire {
    available: bool,
    locale: Option<String>,
    #[serde(default)]
    installed: bool,
    reason: Option<String>,
}
fn availability_from_wire(value: serde_json::Value) -> Result<Availability, String> {
    let wire: AvailabilityWire = serde_json::from_value(value).map_err(|e| e.to_string())?;
    if wire.available {
        Ok(Availability::Available {
            locale: wire
                .locale
                .filter(|s| !s.is_empty())
                .ok_or("Available Speech bridge must return a locale")?,
            installed: wire.installed,
        })
    } else {
        Ok(Availability::Unavailable {
            reason: wire
                .reason
                .unwrap_or_else(|| "Apple Speech unavailable".into()),
        })
    }
}
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
unsafe extern "C" {
    fn souffle_speech_status() -> *mut c_char;
    fn souffle_speech_install(locale: *const c_char) -> *mut c_char;
    fn souffle_speech_start(locale: *const c_char) -> *mut c_char;
    fn souffle_speech_push(session: u64, samples: *const f32, count: u32) -> *mut c_char;
    fn souffle_speech_finish(session: u64) -> *mut c_char;
    fn souffle_speech_cancel(session: u64) -> *mut c_char;
    fn souffle_speech_free(pointer: *mut c_char);
}
fn decode(pointer: *mut c_char) -> Result<serde_json::Value, String> {
    if pointer.is_null() {
        return Err("Apple Speech returned no response".into());
    }
    let text = unsafe { CStr::from_ptr(pointer) }
        .to_string_lossy()
        .into_owned();
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    unsafe {
        souffle_speech_free(pointer);
    }
    let value: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("Speech bridge JSON: {e}"))?;
    if let Some(error) = value["error"].as_str() {
        return Err(error.into());
    }
    Ok(value)
}
fn unsupported() -> Availability {
    Availability::Unavailable {
        reason: "Apple Speech bridge unavailable on this platform/build".into(),
    }
}
pub fn status() -> Availability {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    if !cfg!(apple_speech_stub) {
        return decode(unsafe { souffle_speech_status() })
            .and_then(availability_from_wire)
            .unwrap_or_else(|reason| Availability::Unavailable { reason });
    }
    unsupported()
}
/// Locale/hardware support is observed once, without reserving/downloading assets.
/// Installed state is queried afresh for the selected model on worker threads.
pub fn availability() -> &'static Availability {
    static SUPPORT: OnceLock<Availability> = OnceLock::new();
    SUPPORT.get_or_init(status)
}
pub fn install(locale: &str) -> Result<(), String> {
    let locale = CString::new(locale).map_err(|e| e.to_string())?;
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        decode(unsafe { souffle_speech_install(locale.as_ptr()) }).map(|_| ())
    }
    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
    {
        let _ = locale;
        Err("Apple Speech requires Apple Silicon macOS 26+".into())
    }
}

#[derive(Default)]
pub struct AppleSpeechEngine {
    locale: Option<String>,
    session: Option<u64>,
    sample_rate: u32,
    timeline_offset: f64,
    consumed_samples: u64,
}
impl AppleSpeechEngine {
    fn begin(&mut self) -> Result<(), EngineError> {
        self.cancel()?;
        let locale = CString::new(self.locale.as_deref().ok_or(EngineError::NotInitialized)?)
            .map_err(|e| EngineError::InferenceError(e.to_string()))?;
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        {
            let value = decode(unsafe { souffle_speech_start(locale.as_ptr()) })
                .map_err(EngineError::InferenceError)?;
            self.session = Some(
                value["session"]
                    .as_u64()
                    .ok_or_else(|| EngineError::InferenceError("missing Speech session".into()))?,
            );
            self.sample_rate = u32::try_from(value["sample_rate"].as_u64().unwrap_or(0))
                .map_err(|e| EngineError::InferenceError(e.to_string()))?;
            if self.sample_rate == 0 {
                self.cancel()?;
                return Err(EngineError::InferenceError(
                    "invalid Speech audio rate".into(),
                ));
            }
            self.consumed_samples = 0;
            Ok(())
        }
        #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
        {
            let _ = locale;
            Err(EngineError::InferenceError(
                "Apple Speech unavailable".into(),
            ))
        }
    }
    fn cancel(&mut self) -> Result<(), EngineError> {
        if let Some(session) = self.session.take() {
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            decode(unsafe { souffle_speech_cancel(session) })
                .map_err(EngineError::InferenceError)?;
            #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
            let _ = session;
        }
        Ok(())
    }
    fn segments(&self, value: serde_json::Value) -> Result<Vec<TranscriptionSegment>, EngineError> {
        #[derive(Deserialize)]
        struct Segment {
            text: String,
            start_time: f64,
            end_time: f64,
        }
        let values: Vec<Segment> = serde_json::from_value(value["segments"].clone())
            .map_err(|e| EngineError::InferenceError(e.to_string()))?;
        values
            .into_iter()
            .filter(|s| !s.text.trim().is_empty())
            .map(|s| {
                if !s.start_time.is_finite()
                    || !s.end_time.is_finite()
                    || s.start_time < 0.0
                    || s.end_time < s.start_time
                {
                    return Err(EngineError::InferenceError(
                        "invalid Speech timeline".into(),
                    ));
                }
                Ok(TranscriptionSegment {
                    text: s.text,
                    start_time: s.start_time + self.timeline_offset,
                    end_time: s.end_time + self.timeline_offset,
                    is_final: true,
                    language: self
                        .locale
                        .as_ref()
                        .map(|s| s.split(['-', '_']).next().unwrap_or(s).to_string()),
                    confidence: None,
                    speaker: None,
                })
            })
            .collect()
    }
}
impl TranscriptionEngine for AppleSpeechEngine {
    fn load_model(&mut self, _path: &Path) -> Result<(), EngineError> {
        Err(EngineError::InferenceError(
            "Apple Speech loads system assets, not model files".into(),
        ))
    }
    fn load_system_assets(&mut self, locale: &str) -> Result<(), EngineError> {
        self.locale = Some(locale.into());
        self.timeline_offset = 0.0;
        self.begin()
    }
    fn unload_model(&mut self) -> Result<(), EngineError> {
        self.cancel()?;
        self.locale = None;
        Ok(())
    }
    fn transcribe(
        &mut self,
        audio: &[f32],
        _language: Option<&str>,
    ) -> Result<Vec<TranscriptionSegment>, EngineError> {
        let session = self.session.ok_or(EngineError::NotInitialized)?;
        let count =
            u32::try_from(audio.len()).map_err(|e| EngineError::InferenceError(e.to_string()))?;
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        {
            let value = decode(unsafe { souffle_speech_push(session, audio.as_ptr(), count) })
                .map_err(EngineError::InferenceError)?;
            self.consumed_samples += u64::from(count);
            self.segments(value)
        }
        #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
        {
            let _ = (session, count);
            Err(EngineError::NotInitialized)
        }
    }
    fn flush(&mut self) -> Result<Vec<TranscriptionSegment>, EngineError> {
        let session = self.session.ok_or(EngineError::NotInitialized)?;
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        {
            let result = decode(unsafe { souffle_speech_finish(session) });
            if result.is_ok() {
                self.session = None;
            }
            self.segments(result.map_err(EngineError::InferenceError)?)
        }
        #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
        {
            let _ = session;
            Err(EngineError::NotInitialized)
        }
    }
    fn reset_state(&mut self) -> Result<(), EngineError> {
        self.timeline_offset = 0.0;
        self.begin()
    }
    fn reset_state_preserving_timeline(&mut self) -> Result<(), EngineError> {
        self.timeline_offset += self.consumed_samples as f64 / f64::from(self.sample_rate.max(1));
        self.begin()
    }
    fn audio_requirements(&self) -> AudioInputRequirements {
        AudioInputRequirements {
            sample_rate_hz: self.sample_rate,
            channels: 1,
            chunk_size_samples: self.sample_rate / 10,
        }
    }
    fn emission_delay_seconds(&self) -> f64 {
        2.0
    }
}
impl Drop for AppleSpeechEngine {
    fn drop(&mut self) {
        let _ = self.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn system_assets_never_load_from_app_files() {
        let mut engine = AppleSpeechEngine::default();
        assert!(
            engine
                .load_model(Path::new("/nonexistent/apple-speech"))
                .is_err()
        );
        assert!(engine.transcribe(&[], None).is_err());
        assert!(!engine.supports_diarization());
    }
    #[test]
    #[ignore = "explicitly opt-in; reports real system availability without installing assets"]
    fn real_bridge_status() {
        let status = status();
        println!("{status:?}");
        assert!(status.is_available() && status.locale().is_some());
    }
}

#[cfg(test)]
mod contract_tests {
    use super::*;
    #[test]
    fn availability_never_accepts_ready_without_a_locale() {
        assert!(
            availability_from_wire(serde_json::json!({"available":true,"installed":true})).is_err()
        );
        let missing = availability_from_wire(
            serde_json::json!({"available":true,"locale":"fr_FR","installed":false}),
        )
        .unwrap();
        assert!(missing.is_available());
        assert!(!missing.is_installed_for("fr_FR"));
        let ready = availability_from_wire(
            serde_json::json!({"available":true,"locale":"fr_FR","installed":true}),
        )
        .unwrap();
        assert!(ready.is_installed_for("fr_FR"));
        assert!(!ready.is_installed_for("en_US"));
        let unsupported = availability_from_wire(
            serde_json::json!({"available":false,"reason":"locale unsupported"}),
        )
        .unwrap();
        assert!(!unsupported.is_available());
        assert!(unsupported.locale().is_none());
    }
    #[test]
    fn speech_segments_keep_timestamps_and_never_invent_a_speaker() {
        let engine = AppleSpeechEngine {
            locale: Some("fr_FR".into()),
            timeline_offset: 12.0,
            ..Default::default()
        };
        let segments = engine.segments(serde_json::json!({"segments":[{"text":"Bonjour","start_time":2.0,"end_time":3.0}]})).unwrap();
        assert_eq!((segments[0].start_time, segments[0].end_time), (14.0, 15.0));
        assert_eq!(segments[0].language.as_deref(), Some("fr"));
        assert!(segments[0].speaker.is_none());
        assert!(
            engine
                .segments(
                    serde_json::json!({"segments":[{"text":"Bad","start_time":3.0,"end_time":2.0}]})
                )
                .is_err()
        );
    }
}
