//! SpeechAnalyzer bridge, exclusively driven by the engine actor.
use super::{
    AudioInputRequirements, EngineError, Speaker, TranscriptionEngine, TranscriptionSegment,
    TranscriptionUnavailableReason,
};
use serde::Deserialize;
use std::ffi::{CStr, CString, c_char};
use std::path::Path;
use std::sync::OnceLock;

#[derive(Debug, Clone)]
pub enum Availability {
    Available {
        locale: String,
        locale_names: std::collections::BTreeMap<String, String>,
        installed: bool,
    },
    Unavailable {
        reason: TranscriptionUnavailableReason,
    },
}
impl Availability {
    pub fn locale(&self) -> Option<&str> {
        match self {
            Self::Available { locale, .. } => Some(locale),
            Self::Unavailable { .. } => None,
        }
    }
    pub fn locale_names(&self) -> Option<&std::collections::BTreeMap<String, String>> {
        match self {
            Self::Available { locale_names, .. } => Some(locale_names),
            Self::Unavailable { .. } => None,
        }
    }
    pub fn reason(&self) -> Option<TranscriptionUnavailableReason> {
        match self {
            Self::Available { .. } => None,
            Self::Unavailable { reason } => Some(*reason),
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
            Self::Available {
                locale, installed, ..
            } => *installed && locale == selected,
            Self::Unavailable { .. } => false,
        }
    }
}
#[derive(Deserialize)]
struct AvailabilityWire {
    available: bool,
    locale: Option<String>,
    #[serde(default)]
    locale_names: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    installed: bool,
    reason: Option<TranscriptionUnavailableReason>,
}
fn availability_from_wire(value: serde_json::Value) -> Result<Availability, String> {
    let wire: AvailabilityWire = serde_json::from_value(value).map_err(|e| e.to_string())?;
    if wire.available {
        let locale = wire
            .locale
            .filter(|s| !s.is_empty())
            .ok_or("Available Speech bridge must return a locale")?;
        Ok(Availability::Available {
            locale,
            locale_names: wire.locale_names,
            installed: wire.installed,
        })
    } else {
        Ok(Availability::Unavailable {
            reason: wire
                .reason
                .ok_or("Unavailable Speech bridge must return a reason")?,
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
        reason: TranscriptionUnavailableReason::BuildUnsupported,
    }
}
pub fn status() -> Availability {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    if !cfg!(apple_speech_stub) {
        return decode(unsafe { souffle_speech_status() })
            .and_then(availability_from_wire)
            .unwrap_or_else(|error| {
                tracing::warn!(%error, "Apple Speech availability check failed");
                Availability::Unavailable {
                    reason: TranscriptionUnavailableReason::CheckFailed,
                }
            });
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
struct AppleSpeechLane {
    locale: Option<String>,
    session: Option<u64>,
    sample_rate: u32,
    timeline_offset: f64,
    consumed_samples: u64,
    preview: Option<TranscriptionSegment>,
}
impl AppleSpeechLane {
    fn preserve_timeline(&mut self) {
        // Consume the interval before any fallible cancellation/restart. A
        // retry must not carry the same accepted samples into the clock twice.
        let consumed = std::mem::take(&mut self.consumed_samples);
        self.timeline_offset += consumed as f64 / f64::from(self.sample_rate.max(1));
    }
    fn reset_preserving_with(
        &mut self,
        begin: impl FnOnce(&mut Self) -> Result<(), EngineError>,
    ) -> Result<(), EngineError> {
        self.preserve_timeline();
        begin(self)
    }
    fn begin(&mut self) -> Result<(), EngineError> {
        self.cancel()?;
        self.preview = None;
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
    fn segments(
        &mut self,
        value: serde_json::Value,
    ) -> Result<Vec<TranscriptionSegment>, EngineError> {
        #[derive(Deserialize)]
        struct Segment {
            text: String,
            start_time: f64,
            end_time: f64,
            is_final: bool,
        }
        let values: Vec<Segment> = serde_json::from_value(value["segments"].clone())
            .map_err(|e| EngineError::InferenceError(e.to_string()))?;
        let mut out = Vec::new();
        for s in values {
            if !s.start_time.is_finite()
                || !s.end_time.is_finite()
                || s.start_time < 0.0
                || s.end_time < s.start_time
            {
                return Err(EngineError::InferenceError(
                    "invalid Speech timeline".into(),
                ));
            }
            // Apple sends each phrase in order, revising the volatile
            // result until its final replacement. Preserve that order:
            // the shared contract replaces only this capture lane.
            let duration = self.consumed_samples as f64 / f64::from(self.sample_rate.max(1));
            let segment = TranscriptionSegment {
                text: s.text,
                start_time: s.start_time.min(duration) + self.timeline_offset,
                end_time: s.end_time.min(duration) + self.timeline_offset,
                is_final: s.is_final,
                language: self
                    .locale
                    .as_ref()
                    .map(|s| s.split(['-', '_']).next().unwrap_or(s).to_string()),
                confidence: None,
                speaker: None,
            };
            if segment.is_final {
                if segment.text.trim().is_empty() {
                    out.extend(self.withdraw_preview());
                } else {
                    self.preview = None;
                    out.push(segment);
                }
            } else {
                self.preview = Some(segment.clone());
                out.push(segment);
            }
        }
        Ok(out)
    }
    fn withdraw_preview(&mut self) -> Option<TranscriptionSegment> {
        self.preview.take().map(|mut s| {
            s.text.clear();
            s
        })
    }
    fn transcribe_with(
        &mut self,
        audio: &[f32],
        push: impl FnOnce(u32) -> Result<serde_json::Value, EngineError>,
    ) -> Result<Vec<TranscriptionSegment>, EngineError> {
        let count =
            u32::try_from(audio.len()).map_err(|e| EngineError::InferenceError(e.to_string()))?;
        // The actor has already removed this capture interval from its buffer.
        // Account for it even if delivery fails; recovery replaces the old
        // analyzer before another frame can use its shorter accepted clock.
        self.consumed_samples += u64::from(count);
        let value = push(count)?;
        self.segments(value)
    }
}
impl TranscriptionEngine for AppleSpeechLane {
    fn silence_handling(&self) -> super::SilenceHandling {
        super::SilenceHandling::Continuous
    }
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
        let session = self.session;
        self.transcribe_with(audio, |count| {
            let session = session.ok_or(EngineError::NotInitialized)?;
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            {
                decode(unsafe { souffle_speech_push(session, audio.as_ptr(), count) })
                    .map_err(EngineError::InferenceError)
            }
            #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
            {
                let _ = (session, count);
                Err(EngineError::NotInitialized)
            }
        })
    }
    fn flush(&mut self) -> Result<Vec<TranscriptionSegment>, EngineError> {
        let Some(session) = self.session else {
            return Ok(Vec::new());
        };
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        {
            let result = decode(unsafe { souffle_speech_finish(session) });
            if result.is_ok() {
                self.session = None;
            }
            let mut segments = self.segments(result.map_err(EngineError::InferenceError)?)?;
            segments.extend(self.withdraw_preview());
            Ok(segments)
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
        self.reset_preserving_with(Self::begin)
    }
    fn audio_requirements(&self) -> AudioInputRequirements {
        let rate = if self.sample_rate == 0 {
            16000
        } else {
            self.sample_rate
        };
        AudioInputRequirements {
            sample_rate_hz: rate,
            channels: 1,
            chunk_size_samples: rate / 10,
        }
    }
    fn emission_delay_seconds(&self) -> f64 {
        2.0
    }
}
impl Drop for AppleSpeechLane {
    fn drop(&mut self) {
        let _ = self.cancel();
    }
}

/// Independent analyzers preserve capture identity; Apple does not infer speakers.
enum SpeechLanes {
    Mono(AppleSpeechLane),
    Dual {
        me: AppleSpeechLane,
        them: AppleSpeechLane,
    },
}
impl Default for SpeechLanes {
    fn default() -> Self {
        Self::Mono(AppleSpeechLane::default())
    }
}
impl SpeechLanes {
    fn primary(&self) -> &AppleSpeechLane {
        match self {
            Self::Mono(lane) => lane,
            Self::Dual { me, .. } => me,
        }
    }
    fn cancel(&mut self) -> Result<(), EngineError> {
        match self {
            Self::Mono(lane) => lane.cancel(),
            Self::Dual { me, them } => {
                let first = me.cancel();
                let second = them.cancel();
                first.and(second)
            }
        }
    }
}
pub struct AppleSpeechEngine {
    lanes: SpeechLanes,
    dual: bool,
    preview_enabled: bool,
    pending_finals: Vec<TranscriptionSegment>,
}
impl Default for AppleSpeechEngine {
    fn default() -> Self {
        Self {
            lanes: SpeechLanes::default(),
            dual: false,
            preview_enabled: true,
            pending_finals: Vec::new(),
        }
    }
}
impl AppleSpeechEngine {
    fn restart(&mut self, preserve: bool) -> Result<(), EngineError> {
        self.restart_with(preserve, AppleSpeechLane::begin)
    }
    fn restart_with(
        &mut self,
        preserve: bool,
        mut begin: impl FnMut(&mut AppleSpeechLane) -> Result<(), EngineError>,
    ) -> Result<(), EngineError> {
        let locale = self
            .lanes
            .primary()
            .locale
            .clone()
            .ok_or(EngineError::NotInitialized)?;
        let sample_rate = self.lanes.primary().sample_rate;
        let carry = |lane: &mut AppleSpeechLane| {
            lane.preserve_timeline();
            if !preserve {
                lane.timeline_offset = 0.0;
            }
            lane.timeline_offset
        };
        let offset = match &mut self.lanes {
            SpeechLanes::Mono(lane) => carry(lane),
            // Normally both lanes consumed the same padded frame. If only
            // one push succeeded, retain that complete capture interval too.
            SpeechLanes::Dual { me, them } => carry(me).max(carry(them)),
        };
        self.lanes.cancel()?;
        if !preserve {
            self.pending_finals.clear();
        }
        let me = AppleSpeechLane {
            locale: Some(locale.clone()),
            sample_rate,
            timeline_offset: offset,
            preview: None,
            ..Default::default()
        };
        self.lanes = if self.dual {
            let them = AppleSpeechLane {
                locale: Some(locale),
                sample_rate,
                timeline_offset: offset,
                preview: None,
                ..Default::default()
            };
            SpeechLanes::Dual { me, them }
        } else {
            SpeechLanes::Mono(me)
        };
        let result = match &mut self.lanes {
            SpeechLanes::Mono(lane) => begin(lane),
            SpeechLanes::Dual { me, them } => begin(me).and_then(|()| {
                begin(them)?;
                if me.sample_rate == them.sample_rate {
                    Ok(())
                } else {
                    Err(EngineError::InferenceError(
                        "Apple Speech source rates differ".into(),
                    ))
                }
            }),
        };
        let result = result.and_then(|()| {
            if preserve && sample_rate != 0 && self.lanes.primary().sample_rate != sample_rate {
                Err(EngineError::InferenceError(
                    "Apple Speech rate changed during capture recovery".into(),
                ))
            } else {
                Ok(())
            }
        });
        if result.is_err() {
            // A failed second begin must not leave its successful peer alive.
            // The replacement lanes already retain the committed offset.
            let _ = self.lanes.cancel();
            if preserve {
                // Capture/resampling continues at the last agreed rate even
                // when one begin negotiated a new format before its peer failed.
                match &mut self.lanes {
                    SpeechLanes::Mono(lane) => lane.sample_rate = sample_rate,
                    SpeechLanes::Dual { me, them } => {
                        me.sample_rate = sample_rate;
                        them.sample_rate = sample_rate;
                    }
                }
            }
        }
        result
    }
    fn finish_push(
        &mut self,
        result: Result<Vec<TranscriptionSegment>, EngineError>,
        begin: impl FnMut(&mut AppleSpeechLane) -> Result<(), EngineError>,
    ) -> Result<Vec<TranscriptionSegment>, EngineError> {
        if result.is_err()
            && let Err(error) = self.restart_with(true, begin)
        {
            tracing::warn!(%error, "Apple Speech capture recovery failed; clock retained for retry");
        }
        result.map(|mut segments| {
            visible_results(&mut segments, self.preview_enabled);
            segments
        })
    }
    fn transcribe_dual_with(
        &mut self,
        mic: &[f32],
        system: &[f32],
        mut push: impl FnMut(
            &mut AppleSpeechLane,
            &[f32],
        ) -> Result<Vec<TranscriptionSegment>, EngineError>,
        begin: impl FnMut(&mut AppleSpeechLane) -> Result<(), EngineError>,
    ) -> Result<Vec<TranscriptionSegment>, EngineError> {
        let result = match &mut self.lanes {
            SpeechLanes::Mono(_) => {
                return Err(EngineError::InferenceError(
                    "Dual audio supplied to mono Apple Speech session".into(),
                ));
            }
            SpeechLanes::Dual { me, them } => {
                let len = mic.len().max(system.len());
                let first = push(me, &aligned_audio(mic, len));
                let second = push(them, &aligned_audio(system, len));
                combine_lanes(first, second, &mut self.pending_finals)
            }
        };
        self.finish_push(result, begin)
    }
}
fn label(mut segments: Vec<TranscriptionSegment>, speaker: Speaker) -> Vec<TranscriptionSegment> {
    for segment in &mut segments {
        segment.speaker = Some(speaker);
    }
    segments
}
fn aligned_audio(audio: &[f32], len: usize) -> std::borrow::Cow<'_, [f32]> {
    if audio.len() == len {
        return std::borrow::Cow::Borrowed(audio);
    }
    let mut padded = audio.to_vec();
    padded.resize(len, 0.0);
    std::borrow::Cow::Owned(padded)
}
fn visible_results(segments: &mut Vec<TranscriptionSegment>, preview_enabled: bool) {
    segments.retain(|s| preview_enabled || s.is_final || s.text.is_empty());
}
fn combine_lanes(
    me: Result<Vec<TranscriptionSegment>, EngineError>,
    them: Result<Vec<TranscriptionSegment>, EngineError>,
    pending: &mut Vec<TranscriptionSegment>,
) -> Result<Vec<TranscriptionSegment>, EngineError> {
    match (me, them) {
        (Ok(me), Ok(them)) => {
            let mut out = std::mem::take(pending);
            out.extend(label(me, Speaker::Me));
            out.extend(label(them, Speaker::Them));
            Ok(out)
        }
        (Ok(segments), Err(error)) => {
            pending.extend(
                label(segments, Speaker::Me)
                    .into_iter()
                    .filter(|s| s.is_final),
            );
            Err(error)
        }
        (Err(error), Ok(segments)) => {
            pending.extend(
                label(segments, Speaker::Them)
                    .into_iter()
                    .filter(|s| s.is_final),
            );
            Err(error)
        }
        (Err(error), Err(_)) => Err(error),
    }
}
impl TranscriptionEngine for AppleSpeechEngine {
    fn silence_handling(&self) -> super::SilenceHandling {
        super::SilenceHandling::Continuous
    }
    fn load_model(&mut self, _path: &Path) -> Result<(), EngineError> {
        Err(EngineError::InferenceError(
            "Apple Speech loads system assets, not model files".into(),
        ))
    }
    fn load_system_assets(&mut self, locale: &str) -> Result<(), EngineError> {
        self.pending_finals.clear();
        self.lanes.cancel()?;
        self.lanes = SpeechLanes::Mono(AppleSpeechLane {
            locale: Some(locale.into()),
            preview: None,
            ..Default::default()
        });
        self.restart(false)
    }
    fn unload_model(&mut self) -> Result<(), EngineError> {
        self.pending_finals.clear();
        self.lanes.cancel()?;
        self.lanes = SpeechLanes::default();
        Ok(())
    }
    fn transcribe(
        &mut self,
        audio: &[f32],
        language: Option<&str>,
    ) -> Result<Vec<TranscriptionSegment>, EngineError> {
        let result = match &mut self.lanes {
            SpeechLanes::Mono(lane) => lane.transcribe(audio, language),
            SpeechLanes::Dual { .. } => {
                return Err(EngineError::InferenceError(
                    "Mono audio supplied to dual Apple Speech session".into(),
                ));
            }
        };
        self.finish_push(result, AppleSpeechLane::begin)
    }
    fn transcribe_dual(
        &mut self,
        mic: &[f32],
        system: &[f32],
    ) -> Result<Vec<TranscriptionSegment>, EngineError> {
        self.transcribe_dual_with(
            mic,
            system,
            |lane, audio| lane.transcribe(audio, None),
            AppleSpeechLane::begin,
        )
    }
    fn flush(&mut self) -> Result<Vec<TranscriptionSegment>, EngineError> {
        let mut out = match &mut self.lanes {
            SpeechLanes::Mono(lane) => lane.flush(),
            SpeechLanes::Dual { me, them } => {
                // Always finalize both analyzers, even if one reports an error.
                let first = me.flush();
                let second = them.flush();
                combine_lanes(first, second, &mut self.pending_finals)
            }
        }?;
        visible_results(&mut out, self.preview_enabled);
        Ok(out)
    }
    fn reset_state(&mut self) -> Result<(), EngineError> {
        self.restart(false)
    }
    fn reset_state_preserving_timeline(&mut self) -> Result<(), EngineError> {
        self.restart(true)
    }
    fn audio_requirements(&self) -> AudioInputRequirements {
        self.lanes.primary().audio_requirements()
    }
    fn emission_delay_seconds(&self) -> f64 {
        2.0
    }
    fn supports_diarization(&self) -> bool {
        true
    }
    fn set_diarization(&mut self, enabled: bool) {
        self.dual = enabled;
    }
    fn set_preview_enabled(&mut self, enabled: bool) {
        self.preview_enabled = enabled;
    }
    fn salvage_pending_after_flush_error(&mut self) -> Vec<TranscriptionSegment> {
        std::mem::take(&mut self.pending_finals)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn failed_restart_carries_samples_only_once() {
        let mut engine = AppleSpeechLane {
            locale: None,
            session: None,
            sample_rate: 16000,
            consumed_samples: 48000,
            timeline_offset: 7.0,
            preview: None,
        };
        assert!(
            engine
                .reset_preserving_with(|_| Err(EngineError::NotInitialized))
                .is_err()
        );
        assert_eq!(engine.timeline_offset, 10.0);
        assert_eq!(engine.consumed_samples, 0);
        engine.reset_preserving_with(|_| Ok(())).unwrap();
        engine.consumed_samples = 16000;
        let value = serde_json::json!({"segments":[{"text":"after recovery","start_time":0.5,"end_time":1.0,"is_final":true}]});
        let segments = engine.segments(value).unwrap();
        assert_eq!((segments[0].start_time, segments[0].end_time), (10.5, 11.0));
        engine.consumed_samples = 16000;
        engine.reset_preserving_with(|_| Ok(())).unwrap();
        assert_eq!(engine.timeline_offset, 11.0);
    }
    #[test]
    fn system_assets_never_load_from_app_files() {
        let mut engine = AppleSpeechEngine::default();
        assert!(
            engine
                .load_model(Path::new("/nonexistent/apple-speech"))
                .is_err()
        );
        assert!(engine.transcribe(&[], None).is_err());
        assert!(engine.supports_diarization());
    }
    #[test]
    fn continuous_policy_applies_to_mono_and_independent_source_lanes() {
        let mut engine = AppleSpeechEngine::default();
        assert_eq!(
            engine.silence_handling(),
            super::super::SilenceHandling::Continuous
        );
        engine.set_diarization(true);
        assert_eq!(
            engine.silence_handling(),
            super::super::SilenceHandling::Continuous
        );
        assert_eq!(
            AppleSpeechLane::default().silence_handling(),
            super::super::SilenceHandling::Continuous
        );
    }
    #[test]
    #[ignore = "explicitly opt-in; reports real system availability without installing assets"]
    fn real_bridge_status() {
        let status = status();
        println!("{status:?}");
        assert!(status.is_available() && status.locale().is_some());
    }

    #[test]
    #[ignore = "opt-in installed en_US assets and SOUFFLE_SPEECH_TEST_WAV synthetic known phrase; never installs assets"]
    fn real_bridge_known_phrase_and_session_resets() {
        let status = status();
        assert!(status.is_installed_for("en_US"), "{status:?}");
        let path = std::env::var("SOUFFLE_SPEECH_TEST_WAV").expect("synthetic mono int16 WAV");
        let mut reader = hound::WavReader::open(path).unwrap();
        let spec = reader.spec();
        assert_eq!(spec.channels, 1);
        assert_eq!(spec.bits_per_sample, 16);
        let samples: Vec<f32> = reader
            .samples::<i16>()
            .map(|s| f32::from(s.unwrap()) / 32768.0)
            .collect();
        let mut engine = AppleSpeechEngine::default();
        engine.load_system_assets("en_US").unwrap();
        let requirements = engine.audio_requirements();
        let mut resampler =
            crate::audio::Resampler::new(spec.sample_rate, 1, requirements.sample_rate_hz, 1.0);
        let mut pcm = resampler.process(&samples);
        pcm.extend(resampler.flush());
        pcm.resize(pcm.len() + requirements.sample_rate_hz as usize, 0.0);
        let duration = pcm.len() as f64 / f64::from(requirements.sample_rate_hz);
        for run in 0..3 {
            let offset = match run {
                0 => 0.0,
                1 => {
                    engine.reset_state_preserving_timeline().unwrap();
                    duration
                }
                2 => {
                    engine.reset_state().unwrap();
                    0.0
                }
                _ => unreachable!(),
            };
            let mut segments = Vec::new();
            for chunk in pcm.chunks(requirements.chunk_size_samples as usize) {
                segments.extend(engine.transcribe(chunk, None).unwrap());
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            segments.extend(engine.flush().unwrap());
            let finals: Vec<_> = segments.iter().filter(|s| s.is_final).collect();
            let text = finals
                .iter()
                .map(|s| s.text.as_str())
                .collect::<Vec<_>>()
                .join(" ")
                .to_lowercase();
            println!(
                "run={run} rate={} offset={offset:.3} text={text:?} segments={segments:?}",
                requirements.sample_rate_hz
            );
            assert!(text.contains("quick brown fox"), "{text}");
            assert!(!finals.is_empty());
            for segment in finals {
                assert!(segment.speaker.is_none());
                assert!(
                    segment.start_time >= offset && segment.end_time <= offset + duration + 0.1
                );
            }
        }
        engine.unload_model().unwrap();
        assert!(engine.transcribe(&[], None).is_err());
    }

    #[test]
    #[ignore = "opt-in system assets installation and real ASR; requires SOUFFLE_SPEECH_TEST_WAV and SOUFFLE_SPEECH_TEST_TEXT"]
    fn real_bridge_transcribes_pcm_and_restarts() {
        let path = std::env::var("SOUFFLE_SPEECH_TEST_WAV").expect("mono float32 WAV fixture");
        let expected = std::env::var("SOUFFLE_SPEECH_TEST_TEXT").expect("expected phrase");
        assert!(!expected.trim().is_empty());
        let mut reader = hound::WavReader::open(path).unwrap();
        let spec = reader.spec();
        assert_eq!(spec.channels, 1);
        assert_eq!(spec.sample_format, hound::SampleFormat::Float);
        let samples: Vec<f32> = reader.samples::<f32>().collect::<Result<_, _>>().unwrap();
        assert!(!samples.is_empty());
        let observed = status();
        let locale = observed.locale().expect("real supported Speech locale");
        install(locale).unwrap();
        let mut engine = AppleSpeechEngine::default();
        engine.load_system_assets(locale).unwrap();
        let requirements = engine.audio_requirements();
        assert_eq!(spec.sample_rate, requirements.sample_rate_hz);
        let mut segments = Vec::new();
        for chunk in samples.chunks(requirements.chunk_size_samples as usize) {
            segments.extend(engine.transcribe(chunk, None).unwrap());
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        assert!(
            segments.iter().any(|s| !s.is_final && !s.text.is_empty()),
            "no progressive results before Stop"
        );
        segments.extend(engine.flush().unwrap());
        let text = segments
            .iter()
            .filter(|segment| segment.is_final)
            .map(|segment| segment.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        println!("Apple Speech: {text}");
        assert!(
            text.to_lowercase().contains(&expected.to_lowercase()),
            "{text:?}"
        );
        assert!(segments.iter().all(|segment| segment.speaker.is_none()));
        assert!(segments.iter().all(|segment| segment.start_time >= 0.0
            && segment.end_time >= segment.start_time
            && segment.end_time <= samples.len() as f64 / f64::from(spec.sample_rate)));
        engine.reset_state().unwrap();
        engine
            .transcribe(&vec![0.0; requirements.chunk_size_samples as usize], None)
            .unwrap();
        engine.flush().unwrap();
        assert!(engine.flush().unwrap().is_empty());

        // Me and Them are capture lanes, each with an independent analyzer.
        // Unequal pairs must insert silence so both clocks keep moving.
        engine.set_diarization(true);
        engine.reset_state().unwrap();
        let mut dual = Vec::new();
        for speaker in [Speaker::Me, Speaker::Them] {
            for chunk in samples.chunks(requirements.chunk_size_samples as usize) {
                dual.extend(match speaker {
                    Speaker::Me => engine.transcribe_dual(chunk, &[]).unwrap(),
                    Speaker::Them => engine.transcribe_dual(&[], chunk).unwrap(),
                });
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        }
        for speaker in [Speaker::Me, Speaker::Them] {
            assert!(
                dual.iter()
                    .any(|s| s.speaker == Some(speaker) && !s.is_final && !s.text.is_empty()),
                "missing {speaker:?} preview before Stop"
            );
        }
        dual.extend(engine.flush().unwrap());
        let duration = samples.len() as f64 / f64::from(spec.sample_rate);
        for speaker in [Speaker::Me, Speaker::Them] {
            let finals: Vec<_> = dual
                .iter()
                .filter(|s| s.is_final && s.speaker == Some(speaker))
                .collect();
            let text = finals
                .iter()
                .map(|s| s.text.as_str())
                .collect::<Vec<_>>()
                .join(" ");
            println!("Apple Speech {speaker:?}: {text}");
            assert!(
                text.to_lowercase().contains(&expected.to_lowercase()),
                "{speaker:?}: {text:?}"
            );
            println!(
                "Apple Speech {speaker:?} ranges: {:?}",
                finals
                    .iter()
                    .map(|s| (s.start_time, s.end_time))
                    .collect::<Vec<_>>()
            );
            assert!(
                finals
                    .iter()
                    .all(|s| s.start_time >= 0.0 && s.end_time <= 2.0 * duration)
            );
        }
        engine.reset_state_preserving_timeline().unwrap();
        assert_eq!(engine.lanes.primary().timeline_offset, 2.0 * duration);
        engine.set_diarization(false);
        engine.reset_state().unwrap();
        assert_eq!(engine.lanes.primary().timeline_offset, 0.0);
        assert!(engine.lanes.primary().preview.is_none());
        engine.unload_model().unwrap();
    }
}

#[cfg(test)]
mod contract_tests {
    use super::*;
    fn result(text: &str, is_final: bool, start: f64, end: f64) -> serde_json::Value {
        serde_json::json!({"segments":[{"text":text,"start_time":start,"end_time":end,"is_final":is_final}]})
    }
    fn lane() -> AppleSpeechLane {
        AppleSpeechLane {
            locale: Some("en_US".into()),
            sample_rate: 16000,
            consumed_samples: 64000,
            preview: None,
            ..Default::default()
        }
    }
    fn lane_with_capture(timeline_offset: f64, consumed_samples: u64) -> AppleSpeechLane {
        let mut value = lane();
        value.timeline_offset = timeline_offset;
        value.consumed_samples = consumed_samples;
        value
    }
    #[test]
    fn partial_delivery_recovers_both_clocks_and_retains_finals_until_next_success() {
        for failed_push in [1, 2] {
            let mut engine = AppleSpeechEngine {
                dual: true,
                lanes: SpeechLanes::Dual {
                    me: lane_with_capture(7.0, 48000),
                    them: lane_with_capture(7.0, 48000),
                },
                ..Default::default()
            };
            let mut pushes = 0;
            let mut starts = 0;
            assert!(
                engine
                    .transcribe_dual_with(
                        &vec![0.1; 4000],
                        &vec![0.2; 2000],
                        |lane, audio| {
                            pushes += 1;
                            lane.transcribe_with(audio, |count| {
                                assert_eq!(count, 4000);
                                if pushes == failed_push {
                                    Err(EngineError::InferenceError("delivery failed".into()))
                                } else {
                                    Ok(result("Already finalized.", true, 2.0, 2.25))
                                }
                            })
                        },
                        |lane| {
                            starts += 1;
                            assert_eq!(lane.timeline_offset, 10.25);
                            lane.sample_rate = 16000;
                            Ok(())
                        },
                    )
                    .is_err()
            );
            assert_eq!((pushes, starts), (2, 2));
            match &engine.lanes {
                SpeechLanes::Dual { me, them } => {
                    for lane in [me, them] {
                        assert_eq!(lane.timeline_offset, 10.25);
                        assert_eq!(lane.consumed_samples, 0);
                        assert!(lane.preview.is_none());
                    }
                }
                SpeechLanes::Mono(_) => panic!("recovery lost a source"),
            }
            let out = engine
                .transcribe_dual_with(
                    &vec![0.0; 4000],
                    &vec![0.0; 4000],
                    |lane, audio| {
                        lane.transcribe_with(audio, |_| Ok(result("Recovered.", true, 0.0, 0.25)))
                    },
                    |_| panic!("successful delivery must not restart"),
                )
                .unwrap();
            assert_eq!(out.len(), 3);
            assert_eq!(out[0].text, "Already finalized.");
            assert_eq!((out[0].start_time, out[0].end_time), (9.0, 9.25));
            assert_eq!(
                out[0].speaker,
                Some(if failed_push == 1 {
                    Speaker::Them
                } else {
                    Speaker::Me
                })
            );
            for (segment, speaker) in out[1..].iter().zip([Speaker::Me, Speaker::Them]) {
                assert_eq!(segment.speaker, Some(speaker));
                assert_eq!((segment.start_time, segment.end_time), (10.25, 10.5));
            }
            assert!(engine.salvage_pending_after_flush_error().is_empty());
            engine.restart_with(true, |_| Ok(())).unwrap();
            assert_eq!(engine.lanes.primary().timeline_offset, 10.5);
        }
    }
    #[test]
    fn repeated_delivery_and_restart_failures_count_each_frame_once_at_negotiated_rate() {
        let make_lane = || {
            let mut lane = lane_with_capture(5.0, 48000);
            lane.sample_rate = 48000;
            lane
        };
        let mut engine = AppleSpeechEngine {
            dual: true,
            lanes: SpeechLanes::Dual {
                me: make_lane(),
                them: make_lane(),
            },
            ..Default::default()
        };
        for expected in [6.1, 6.2] {
            assert!(
                engine
                    .transcribe_dual_with(
                        &vec![0.0; 4800],
                        &[],
                        |lane, audio| lane
                            .transcribe_with(audio, |_| Err(EngineError::NotInitialized)),
                        |_| Err(EngineError::InferenceError("restart failed".into())),
                    )
                    .is_err()
            );
            match &engine.lanes {
                SpeechLanes::Dual { me, them } => {
                    for lane in [me, them] {
                        assert!((lane.timeline_offset - expected).abs() < 1e-10);
                        assert_eq!(lane.sample_rate, 48000);
                        assert_eq!(lane.consumed_samples, 0);
                        assert!(lane.session.is_none());
                    }
                }
                SpeechLanes::Mono(_) => panic!("recovery lost a source"),
            }
        }
        engine.restart_with(true, |_| Ok(())).unwrap();
        assert!((engine.lanes.primary().timeline_offset - 6.2).abs() < 1e-10);
        let out = engine
            .transcribe_dual_with(
                &vec![0.0; 4800],
                &[],
                |lane, audio| {
                    lane.transcribe_with(audio, |_| Ok(result("After retry.", true, 0.0, 0.1)))
                },
                |_| panic!("successful delivery must not restart"),
            )
            .unwrap();
        for segment in out {
            assert!((segment.start_time - 6.2).abs() < 1e-10);
            assert!((segment.end_time - 6.3).abs() < 1e-10);
        }
    }
    #[test]
    fn recovery_rejects_changed_rates_and_restores_both_lanes_after_partial_begin() {
        for (dual, fail_second) in [(false, false), (true, false), (true, true)] {
            let mut captured = lane_with_capture(5.0, 48000);
            captured.sample_rate = 48000;
            let mut engine = AppleSpeechEngine {
                dual,
                lanes: SpeechLanes::Mono(captured),
                ..Default::default()
            };
            let mut starts = 0;
            assert!(
                engine
                    .restart_with(true, |lane| {
                        starts += 1;
                        lane.sample_rate = 16000;
                        if fail_second && starts == 2 {
                            Err(EngineError::NotInitialized)
                        } else {
                            Ok(())
                        }
                    })
                    .is_err()
            );
            let check = |lane: &AppleSpeechLane| {
                assert_eq!(lane.sample_rate, 48000);
                assert_eq!(lane.timeline_offset, 6.0);
                assert_eq!(lane.consumed_samples, 0);
                assert!(lane.session.is_none());
            };
            match &engine.lanes {
                SpeechLanes::Mono(lane) => check(lane),
                SpeechLanes::Dual { me, them } => {
                    check(me);
                    check(them);
                }
            }
            let reject_frame = |lane: &mut AppleSpeechLane, audio: &[f32]| {
                lane.transcribe_with(audio, |_| Err(EngineError::NotInitialized))
            };
            if dual {
                assert!(
                    engine
                        .transcribe_dual_with(&vec![0.0; 4800], &[], reject_frame, |_| Err(
                            EngineError::NotInitialized
                        ),)
                        .is_err()
                );
            } else {
                let rejected = match &mut engine.lanes {
                    SpeechLanes::Mono(lane) => reject_frame(lane, &vec![0.0; 4800]),
                    SpeechLanes::Dual { .. } => panic!("expected mono"),
                };
                assert!(
                    engine
                        .finish_push(rejected, |_| Err(EngineError::NotInitialized))
                        .is_err()
                );
            }
            assert!((engine.lanes.primary().timeline_offset - 6.1).abs() < 1e-10);
            engine.restart_with(true, |_| Ok(())).unwrap();
            assert!((engine.lanes.primary().timeline_offset - 6.1).abs() < 1e-10);
            // A new recording may negotiate a different format normally.
            engine
                .restart_with(false, |lane| {
                    lane.sample_rate = 16000;
                    Ok(())
                })
                .unwrap();
            assert_eq!(engine.lanes.primary().sample_rate, 16000);
            assert_eq!(engine.lanes.primary().timeline_offset, 0.0);
        }
    }
    #[test]
    fn failed_delivery_recovery_keeps_peer_final_available_for_stop_salvage_once() {
        let mut engine = AppleSpeechEngine {
            dual: true,
            lanes: SpeechLanes::Dual {
                me: lane_with_capture(7.0, 48000),
                them: lane_with_capture(7.0, 48000),
            },
            ..Default::default()
        };
        let mut pushes = 0;
        assert!(
            engine
                .transcribe_dual_with(
                    &vec![0.0; 4000],
                    &[],
                    |lane, audio| {
                        pushes += 1;
                        lane.transcribe_with(audio, |_| {
                            if pushes == 1 {
                                Ok(result("Saved peer final.", true, 2.0, 2.25))
                            } else {
                                Err(EngineError::NotInitialized)
                            }
                        })
                    },
                    |_| Err(EngineError::NotInitialized),
                )
                .is_err()
        );
        let final_segments = engine.salvage_pending_after_flush_error();
        assert_eq!(final_segments.len(), 1);
        assert_eq!(final_segments[0].text, "Saved peer final.");
        assert_eq!(final_segments[0].speaker, Some(Speaker::Me));
        assert_eq!(
            (final_segments[0].start_time, final_segments[0].end_time),
            (9.0, 9.25)
        );
        assert!(engine.salvage_pending_after_flush_error().is_empty());
    }
    #[test]
    fn mono_rejected_frame_is_carried_once_before_delivery_can_resume() {
        let mut engine = AppleSpeechEngine {
            lanes: SpeechLanes::Mono(lane_with_capture(7.0, 48000)),
            ..Default::default()
        };
        let failed = match &mut engine.lanes {
            SpeechLanes::Mono(lane) => lane.transcribe_with(&vec![0.0; 4000], |_| {
                Err(EngineError::InferenceError("delivery failed".into()))
            }),
            SpeechLanes::Dual { .. } => panic!("expected mono"),
        };
        assert!(engine.finish_push(failed, |_| Ok(())).is_err());
        assert_eq!(engine.lanes.primary().timeline_offset, 10.25);
        engine.restart_with(true, |_| Ok(())).unwrap();
        assert_eq!(engine.lanes.primary().timeline_offset, 10.25);
        match &mut engine.lanes {
            SpeechLanes::Mono(lane) => {
                let out = lane
                    .transcribe_with(&vec![0.0; 4000], |_| {
                        Ok(result("After retry.", true, 0.0, 0.25))
                    })
                    .unwrap();
                assert_eq!((out[0].start_time, out[0].end_time), (10.25, 10.5));
            }
            SpeechLanes::Dual { .. } => panic!("recovery changed mode"),
        }
    }
    #[test]
    fn failed_first_or_second_source_restart_preserves_the_capture_interval_once() {
        for failed_begin in [1, 2] {
            let mut engine = AppleSpeechEngine {
                dual: true,
                lanes: SpeechLanes::Dual {
                    me: lane_with_capture(7.0, 48000),
                    them: lane_with_capture(7.0, 64000),
                },
                ..Default::default()
            };
            let mut calls = 0;
            assert!(
                engine
                    .restart_with(true, |lane| {
                        calls += 1;
                        if calls == failed_begin {
                            return Err(EngineError::NotInitialized);
                        }
                        lane.sample_rate = 16000;
                        Ok(())
                    })
                    .is_err()
            );
            match &engine.lanes {
                SpeechLanes::Dual { me, them } => {
                    for lane in [me, them] {
                        assert_eq!(lane.timeline_offset, 11.0);
                        assert_eq!(lane.consumed_samples, 0);
                        assert!(lane.session.is_none());
                        assert!(lane.preview.is_none());
                    }
                }
                SpeechLanes::Mono(_) => panic!("dual restart lost a source"),
            }
            engine
                .restart_with(true, |lane| {
                    lane.sample_rate = 16000;
                    Ok(())
                })
                .unwrap();
            match &mut engine.lanes {
                SpeechLanes::Dual { me, them } => {
                    for (lane, speaker) in [(me, Speaker::Me), (them, Speaker::Them)] {
                        lane.consumed_samples = 16000;
                        let out = label(
                            lane.segments(result("Recovered.", true, 0.5, 1.0)).unwrap(),
                            speaker,
                        );
                        assert_eq!((out[0].start_time, out[0].end_time), (11.5, 12.0));
                        assert_eq!(out[0].speaker, Some(speaker));
                    }
                }
                SpeechLanes::Mono(_) => panic!("dual retry lost a source"),
            }
            engine
                .restart_with(true, |lane| {
                    lane.sample_rate = 16000;
                    Ok(())
                })
                .unwrap();
            assert_eq!(engine.lanes.primary().timeline_offset, 12.0);
            engine
                .restart_with(false, |lane| {
                    lane.sample_rate = 16000;
                    Ok(())
                })
                .unwrap();
            assert_eq!(engine.lanes.primary().timeline_offset, 0.0);
        }
    }
    #[test]
    fn failed_mono_engine_restart_and_retry_do_not_advance_the_clock_twice() {
        let mut engine = AppleSpeechEngine {
            lanes: SpeechLanes::Mono(lane_with_capture(7.0, 48000)),
            ..Default::default()
        };
        assert!(
            engine
                .restart_with(true, |_| Err(EngineError::NotInitialized))
                .is_err()
        );
        assert_eq!(engine.lanes.primary().timeline_offset, 10.0);
        assert_eq!(engine.lanes.primary().consumed_samples, 0);
        engine
            .restart_with(true, |lane| {
                lane.sample_rate = 16000;
                Ok(())
            })
            .unwrap();
        assert_eq!(engine.lanes.primary().timeline_offset, 10.0);
    }
    #[test]
    fn incompatible_restarted_source_rates_leave_a_retryable_committed_clock() {
        let mut engine = AppleSpeechEngine {
            dual: true,
            lanes: SpeechLanes::Mono(lane()),
            ..Default::default()
        };
        let mut calls = 0;
        assert!(
            engine
                .restart_with(true, |lane| {
                    calls += 1;
                    lane.sample_rate = if calls == 1 { 16000 } else { 48000 };
                    Ok(())
                })
                .is_err()
        );
        assert_eq!(engine.lanes.primary().timeline_offset, 4.0);
        engine
            .restart_with(true, |lane| {
                lane.sample_rate = 16000;
                Ok(())
            })
            .unwrap();
        assert_eq!(engine.lanes.primary().timeline_offset, 4.0);
    }
    #[test]
    fn delayed_finals_after_long_silence_keep_their_ranges_and_other_lane_preview() {
        let mut me = lane();
        let mut them = lane();
        me.segments(result("My preview", false, 0.1, 0.8)).unwrap();
        them.segments(result("Their preview", false, 0.3, 1.2))
            .unwrap();
        me.consumed_samples += 30 * 16000;
        them.consumed_samples += 30 * 16000;
        let finals = label(
            me.segments(result("My final.", true, 0.1, 0.8)).unwrap(),
            Speaker::Me,
        );
        assert_eq!((finals[0].start_time, finals[0].end_time), (0.1, 0.8));
        assert_eq!(them.preview.as_ref().unwrap().text, "Their preview");
        let finals = label(
            them.segments(result("Their final.", true, 0.3, 1.2))
                .unwrap(),
            Speaker::Them,
        );
        assert_eq!((finals[0].start_time, finals[0].end_time), (0.3, 1.2));
        assert_eq!(finals[0].speaker, Some(Speaker::Them));
        assert!(me.preview.is_none() && them.preview.is_none());
    }
    #[test]
    fn volatile_revisions_are_replacements_until_the_final_and_empty_final_withdraws() {
        let mut lane = lane();
        for text in ["I", "I scream", "Ice cream"] {
            let out = lane.segments(result(text, false, 0.0, 2.0)).unwrap();
            assert_eq!(out.len(), 1);
            assert!(!out[0].is_final);
            assert_eq!(lane.preview.as_ref().unwrap().text, text);
        }
        let out = lane.segments(result("Ice cream.", true, 0.0, 2.0)).unwrap();
        assert_eq!(out.len(), 1);
        assert!(out[0].is_final);
        assert!(lane.preview.is_none());
        lane.segments(result("noise", false, 2.0, 3.0)).unwrap();
        let out = lane.segments(result("", true, 2.0, 3.0)).unwrap();
        assert_eq!(out.len(), 1);
        assert!(!out[0].is_final && out[0].text.is_empty());
        assert!(lane.preview.is_none());
    }
    #[test]
    fn lane_finalization_and_stop_withdrawal_never_affect_the_other_preview() {
        let mut me = lane();
        let mut them = lane();
        me.segments(result("My words", false, 0.0, 1.0)).unwrap();
        them.segments(result("Their words", false, 0.5, 2.0))
            .unwrap();
        let finals = label(
            me.segments(result("My words.", true, 0.0, 1.0)).unwrap(),
            Speaker::Me,
        );
        assert_eq!(finals[0].speaker, Some(Speaker::Me));
        assert_eq!(them.preview.as_ref().unwrap().text, "Their words");
        let withdrawal = label(them.withdraw_preview().into_iter().collect(), Speaker::Them);
        assert_eq!(withdrawal[0].speaker, Some(Speaker::Them));
        assert!(!withdrawal[0].is_final && withdrawal[0].text.is_empty());
        assert!(them.withdraw_preview().is_none());
    }
    #[test]
    fn revised_ranges_stay_on_the_capture_clock_and_empty_previews_are_not_dropped() {
        let mut lane = lane();
        lane.timeline_offset = 12.0;
        let preview = lane.segments(result("Preview", false, 3.0, 9.0)).unwrap();
        assert_eq!((preview[0].start_time, preview[0].end_time), (15.0, 16.0));
        let out = lane.segments(result("", false, 3.0, 4.0)).unwrap();
        assert_eq!(out.len(), 1);
        assert!(!out[0].is_final && out[0].text.is_empty());
        assert!(
            lane.segments(
                serde_json::json!({"segments":[{"text":"untyped","start_time":0.0,"end_time":1.0}]})
            )
            .is_err()
        );
    }
    #[test]
    fn asymmetric_sources_preserve_pcm_and_pad_only_the_missing_capture_time() {
        assert_eq!(&*aligned_audio(&[0.1, 0.2], 3), &[0.1, 0.2, 0.0]);
        assert_eq!(&*aligned_audio(&[], 3), &[0.0, 0.0, 0.0]);
        assert!(matches!(
            aligned_audio(&[0.1, 0.2], 2),
            std::borrow::Cow::Borrowed(_)
        ));
    }
    #[test]
    fn one_failed_lane_preserves_only_other_lane_finals_once() {
        for successful in [Speaker::Me, Speaker::Them] {
            let mut engine = AppleSpeechEngine::default();
            let mut lane = lane();
            let mut segments = lane.segments(result("Draft", false, 0.0, 1.0)).unwrap();
            segments.extend(lane.segments(result("Saved.", true, 0.0, 1.0)).unwrap());
            let failed = Err(EngineError::InferenceError("source failed".into()));
            let outcome = match successful {
                Speaker::Me => combine_lanes(Ok(segments), failed, &mut engine.pending_finals),
                Speaker::Them => combine_lanes(failed, Ok(segments), &mut engine.pending_finals),
            };
            assert!(outcome.is_err());
            let saved = engine.salvage_pending_after_flush_error();
            assert_eq!(saved.len(), 1);
            assert_eq!(saved[0].text, "Saved.");
            assert!(saved[0].is_final);
            assert_eq!(saved[0].speaker, Some(successful));
            assert!(engine.salvage_pending_after_flush_error().is_empty());
        }
    }
    #[test]
    fn catchup_suppresses_previews_but_keeps_finals_and_withdrawals() {
        let mut lane = lane();
        let mut segments = lane.segments(result("Draft", false, 0.0, 1.0)).unwrap();
        segments.extend(lane.segments(result("Saved.", true, 0.0, 1.0)).unwrap());
        segments.extend(lane.segments(result("", false, 1.0, 2.0)).unwrap());
        visible_results(&mut segments, false);
        assert_eq!(segments.len(), 2);
        assert!(segments[0].is_final);
        assert!(!segments[1].is_final && segments[1].text.is_empty());
    }
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
            serde_json::json!({"available":false,"reason":"locale_unsupported"}),
        )
        .unwrap();
        assert!(!unsupported.is_available());
        assert!(unsupported.locale().is_none());
    }
    #[test]
    fn availability_preserves_negotiated_locale_and_system_localized_names() {
        let observed = availability_from_wire(serde_json::json!({
            "available": true, "locale": "en_US", "installed": true,
            "locale_names": {"en": "English (United States)", "fr": "anglais (États-Unis)"}
        }))
        .unwrap();
        assert_eq!(observed.locale(), Some("en_US"));
        assert_eq!(
            observed.locale_names().unwrap()["fr"],
            "anglais (États-Unis)"
        );
        assert!(observed.is_installed_for("en_US"));
        assert!(!observed.is_installed_for("fr_FR"));
        for reason in [
            "os_unsupported",
            "device_unsupported",
            "build_unsupported",
            "locale_unsupported",
            "assets_unsupported",
            "check_failed",
        ] {
            let observed =
                availability_from_wire(serde_json::json!({"available": false, "reason": reason}))
                    .unwrap();
            assert!(!observed.is_available());
            assert!(observed.reason().is_some());
            assert!(observed.locale_names().is_none());
        }
        // Malformed/future bridge replies must become CheckFailed at status(),
        // never pretend the model is supported or suggest enabling Siri.
        assert!(
            availability_from_wire(serde_json::json!({"available": false, "reason": "new_reason"}))
                .is_err()
        );
        assert!(availability_from_wire(serde_json::json!({"available": false})).is_err());
    }
    #[test]
    fn speech_segments_keep_timestamps_and_never_invent_a_speaker() {
        let mut engine = AppleSpeechLane {
            locale: Some("fr_FR".into()),
            preview: None,
            timeline_offset: 12.0,
            sample_rate: 16000,
            consumed_samples: 4 * 16000,
            ..Default::default()
        };
        let segments = engine.segments(serde_json::json!({"segments":[{"text":"Bonjour","start_time":2.0,"end_time":3.0,"is_final":true}]})).unwrap();
        assert_eq!((segments[0].start_time, segments[0].end_time), (14.0, 15.0));
        assert_eq!(segments[0].language.as_deref(), Some("fr"));
        assert!(segments[0].speaker.is_none());
        assert!(
            engine
                .segments(
                    serde_json::json!({"segments":[{"text":"Bad","start_time":3.0,"end_time":2.0,"is_final":true}]})
                )
                .is_err()
        );
    }
}
