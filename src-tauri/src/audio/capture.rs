use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, SampleFormat, Stream, StreamConfig};
use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};
use ringbuf::HeapRb;
use ringbuf::traits::{Producer, Split};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tracing::{debug, error, info, warn};

use super::mixer::MeetingMixer;
use super::recorder::MeetingRecorder;
use super::resampler::Resampler;
use crate::state::AudioCommand;

/// Tell the frontend whether the system-audio leg of a meeting is live.
fn emit_system_audio_status(app: Option<&tauri::AppHandle>, active: bool, reason: Option<String>) {
    use tauri_specta::Event;
    if let Some(app) = app {
        let _ = crate::app_events::SystemAudioStatus { active, reason }.emit(app);
    }
}

/// Mixer cadence while a meeting session is active. Rings hold ~2s, so the
/// 5ms tick has plenty of slack; it only exists to pace the mixer, the
/// real-time callbacks never wait on it.
const MEETING_TICK: Duration = Duration::from_millis(5);

/// How often the meeting tick re-checks the default output route
/// (~2s at the 5ms tick). Property reads are a handful of cheap HAL calls;
/// polling keeps everything on this thread instead of a listener callback.
const ROUTE_CHECK_TICKS: u32 = 400;

/// Wake-up cadence during dictation sessions: fast enough to feed the
/// AudioLevel stream for the waveform (the mic health check keeps its own
/// coarser MIC_CHECK_INTERVAL gate, so it does not run this often).
const DICTATION_TICK: Duration = LEVEL_EMIT_INTERVAL;

/// How often an active session verifies its input device is still alive and
/// still the system default (closing the laptop lid switches the default
/// input to a headset or webcam mic — the stream must follow it).
const MIC_CHECK_INTERVAL: Duration = Duration::from_secs(2);

/// Ceiling on how often AudioLevel is pushed to the frontend. Meeting mode's
/// 5ms tick would otherwise emit at ~200Hz; the dictation tick matches this
/// interval so both modes stream levels at ~15Hz.
const LEVEL_EMIT_INTERVAL: Duration = Duration::from_millis(66);

/// Consecutive rebuild failures after which a dictation session (mic is the
/// only source) gives up and ends itself, rather than retrying forever
/// while the UI still claims to be recording. ~10s at the `MIC_CHECK_INTERVAL`
/// cadence.
const DICTATION_ABORT_AFTER_FAILURES: u32 = 5;

/// Decision for one mic-loss episode in `check_mic_health`, given how many
/// rebuilds have failed in a row, whether the session has another audio
/// source to fall back on, and whether this episode already warned once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MicLossAction {
    /// Nothing to surface yet; keep retrying at the normal cadence.
    KeepRetrying,
    /// Surface a one-time, non-fatal warning: the mic is gone but another
    /// source (system audio) is still capturing.
    WarnOnce,
    /// No other source exists; give up and end the session.
    Abort,
}

/// Pure decision function backing `check_mic_health`'s mic-loss ladder, kept
/// free of any capture state so it can be unit-tested directly.
fn decide_mic_loss(
    consecutive_failures: u32,
    has_other_source: bool,
    already_warned_this_episode: bool,
) -> MicLossAction {
    if consecutive_failures == 0 {
        return MicLossAction::KeepRetrying;
    }
    if has_other_source {
        if already_warned_this_episode {
            MicLossAction::KeepRetrying
        } else {
            MicLossAction::WarnOnce
        }
    } else if consecutive_failures >= DICTATION_ABORT_AFTER_FAILURES {
        MicLossAction::Abort
    } else {
        MicLossAction::KeepRetrying
    }
}

#[cfg(test)]
mod mic_loss_tests {
    use super::*;

    #[test]
    fn keeps_retrying_with_no_failures_yet() {
        assert_eq!(
            decide_mic_loss(0, false, false),
            MicLossAction::KeepRetrying
        );
        assert_eq!(decide_mic_loss(0, true, false), MicLossAction::KeepRetrying);
    }

    #[test]
    fn dictation_keeps_retrying_below_threshold() {
        for n in 1..DICTATION_ABORT_AFTER_FAILURES {
            assert_eq!(
                decide_mic_loss(n, false, false),
                MicLossAction::KeepRetrying,
                "failure {n} should not abort yet"
            );
        }
    }

    #[test]
    fn dictation_aborts_at_threshold() {
        assert_eq!(
            decide_mic_loss(DICTATION_ABORT_AFTER_FAILURES, false, false),
            MicLossAction::Abort
        );
        assert_eq!(
            decide_mic_loss(DICTATION_ABORT_AFTER_FAILURES + 3, false, false),
            MicLossAction::Abort
        );
    }

    #[test]
    fn meeting_never_aborts_even_well_past_threshold() {
        assert_eq!(
            decide_mic_loss(DICTATION_ABORT_AFTER_FAILURES + 50, true, true),
            MicLossAction::KeepRetrying
        );
    }

    #[test]
    fn meeting_warns_once_then_keeps_retrying_quietly() {
        assert_eq!(decide_mic_loss(1, true, false), MicLossAction::WarnOnce);
        assert_eq!(decide_mic_loss(2, true, true), MicLossAction::KeepRetrying);
        assert_eq!(decide_mic_loss(9, true, true), MicLossAction::KeepRetrying);
    }

    #[test]
    fn meeting_rearms_after_episode_flag_clears() {
        // A caller resets `already_warned_this_episode` to false once a
        // rebuild succeeds; the next loss episode should warn again.
        assert_eq!(decide_mic_loss(1, true, false), MicLossAction::WarnOnce);
    }
}

/// Gates AudioLevel emission to at most once per `LEVEL_EMIT_INTERVAL`.
struct AudioLevelThrottle {
    last_emit: Option<Instant>,
}

impl AudioLevelThrottle {
    fn new() -> Self {
        Self { last_emit: None }
    }

    /// Whether enough time has passed since the last emit to send another one.
    /// Always true the first time (or after `reset`).
    fn should_emit(&mut self, now: Instant) -> bool {
        if let Some(last) = self.last_emit
            && now.duration_since(last) < LEVEL_EMIT_INTERVAL
        {
            return false;
        }
        self.last_emit = Some(now);
        true
    }

    /// Forget the last emit so the next session's first tick emits immediately
    /// instead of waiting out the interval left over from a previous session.
    fn reset(&mut self) {
        self.last_emit = None;
    }
}

#[cfg(test)]
mod level_throttle_tests {
    use super::*;

    #[test]
    fn emits_immediately_then_suppresses_within_interval() {
        let mut throttle = AudioLevelThrottle::new();
        let t0 = Instant::now();
        assert!(throttle.should_emit(t0));
        assert!(!throttle.should_emit(t0 + Duration::from_millis(30)));
        assert!(!throttle.should_emit(t0 + Duration::from_millis(65)));
    }

    #[test]
    fn emits_again_once_interval_elapses() {
        let mut throttle = AudioLevelThrottle::new();
        let t0 = Instant::now();
        assert!(throttle.should_emit(t0));
        assert!(throttle.should_emit(t0 + Duration::from_millis(66)));
    }

    #[test]
    fn reset_allows_immediate_emit() {
        let mut throttle = AudioLevelThrottle::new();
        let t0 = Instant::now();
        assert!(throttle.should_emit(t0));
        throttle.reset();
        assert!(throttle.should_emit(t0 + Duration::from_millis(1)));
    }
}

/// Everything needed to rebuild the capture leg mid-session when the input
/// device fails or the default input changes.
#[derive(Clone)]
struct StartParams {
    session_id: u64,
    target_sample_rate: u32,
    mic_gain: f32,
    capture_system_audio: bool,
    diarize: bool,
    /// File to record mixed meeting audio to, if the retention setting is
    /// not `off`. `None` for dictation sessions and for meetings recorded
    /// with retention off.
    record_path: Option<PathBuf>,
}

/// Per-session state for meeting mode (mic + system audio).
struct MeetingState {
    session_id: u64,
    mixer: MeetingMixer,
    /// None when tap creation failed — the mixer then runs mic-only.
    /// The tap itself lives on its own thread; dropping the handle tears
    /// it down without blocking this thread. Its aggregate references no
    /// physical device, so output-device changes don't require a rebuild.
    #[cfg(target_os = "macos")]
    tap: Option<super::system_tap::TapHandle>,
    /// Whether echo cancellation is currently engaged (speakers audible).
    aec_active: bool,
    /// Emit mic (Me) and system audio (Them) as two source-tagged streams
    /// instead of one mixed stream.
    diarize: bool,
    ticks: u32,
}

impl MeetingState {
    /// Engage/disengage echo cancellation when whether speaker output can
    /// leak into the mic changes: built-in speakers versus anything else
    /// (headphones, Bluetooth), and muted or silent versus audible.
    #[cfg(target_os = "macos")]
    fn check_output_route(&mut self, _app: Option<&tauri::AppHandle>) {
        use super::{aec, mixer, output_route};

        let can_leak = self.tap.is_some() && output_route::output_can_leak_into_mic();
        if can_leak != self.aec_active {
            if can_leak {
                info!("Speakers audible, echo cancellation engaged");
                self.mixer.set_aec(Some(aec::Aec::new(mixer::MIX_RATE)));
            } else {
                info!("Output muted or off speakers, echo cancellation disengaged");
                self.mixer.set_aec(None);
            }
            self.aec_active = can_leak;
        }
    }

    #[cfg(not(target_os = "macos"))]
    fn check_output_route(&mut self, _app: Option<&tauri::AppHandle>) {}
}

#[derive(Debug, Clone)]
pub struct AudioChunk {
    pub session_id: u64,
    pub samples: Vec<f32>,
    /// When the chunk left the capture callback — used for lag tracking.
    pub captured_at: Instant,
    /// Source of this audio in a diarized meeting (Me = mic, Them = system
    /// audio). `None` for single-stream sessions (dictation, mixed meetings),
    /// in which case the actor routes it to the sole engine.
    pub speaker: Option<crate::engine::Speaker>,
}

/// Messages flowing from the capture thread to the engine actor.
#[derive(Debug, Clone)]
pub enum AudioMessage {
    Chunk(AudioChunk),
    /// Sent after the cpal stream is dropped and the resampler flushed —
    /// guaranteed to be the last message of a session, so the actor can
    /// drain deterministically instead of sleeping.
    EndOfStream {
        session_id: u64,
    },
}

/// Info about an available audio input device, sent to frontend
#[derive(Debug, Clone, serde::Serialize, specta::Type)]
pub struct AudioDeviceInfo {
    pub name: String,
    pub is_default: bool,
}

/// List all available input devices. Logged at info level (device count) so
/// enumeration events show up in the Diagnostics live log.
pub fn list_input_devices() -> Vec<AudioDeviceInfo> {
    let devices = list_input_devices_impl();
    info!("Enumerated {} input device(s)", devices.len());
    devices
}

/// CoreAudio property queries only, no cpal enumeration: cpal's
/// `Host::input_devices()` filters every device through
/// `supported_input_configs()`, which on coreaudio opens an AudioUnit on the
/// device's input side just to check it has one. That's enough to wake a
/// Bluetooth headset's input side and flip it from A2DP to HFP mono — with
/// this list called on every Settings page mount, just opening Settings was
/// enough to do that with no recording active. `device_watch::list_devices`
/// does the same enumeration with only cheap property reads.
#[cfg(target_os = "macos")]
fn list_input_devices_impl() -> Vec<AudioDeviceInfo> {
    super::device_watch::list_devices()
        .into_iter()
        .map(|(name, is_default)| AudioDeviceInfo { name, is_default })
        .collect()
}

#[cfg(not(target_os = "macos"))]
fn list_input_devices_impl() -> Vec<AudioDeviceInfo> {
    let host = cpal::default_host();
    let default_name = host
        .default_input_device()
        .and_then(|d| d.name().ok())
        .unwrap_or_default();

    let devices = match host.input_devices() {
        Ok(d) => d,
        Err(e) => {
            warn!("Failed to list input devices: {e}");
            return vec![];
        }
    };

    devices
        .filter_map(|d| {
            let name = d.name().ok()?;
            Some(AudioDeviceInfo {
                is_default: name == default_name,
                name,
            })
        })
        .collect()
}

/// Manages audio capture from a selected input device.
/// Sends resampled 24kHz mono f32 chunks over a crossbeam channel.
///
/// This struct lives on a dedicated thread because cpal's Stream is !Send on macOS.
pub struct AudioCapture {
    stream: Option<Stream>,
    audio_sender: Sender<AudioMessage>,
    selected_device: Option<String>,
    /// Preferred microphone while the lid is closed with an external display
    /// attached (clamshell mode); `None` disables the override entirely, so
    /// `find_device` never pays for the `is_clamshell()` probe.
    clamshell_device: Option<String>,
    active_session_id: Arc<AtomicU64>,
    audio_rms: Arc<AtomicU32>,
    /// Shared with the cpal callback so stop() can flush the resampler tail
    /// after the stream is dropped (no callback runs by then, so the lock
    /// is uncontended).
    resampler: Option<Arc<Mutex<Resampler>>>,
    /// Counts chunks dropped because the audio channel was full.
    dropped_counter: Arc<AtomicU64>,
    /// Active meeting-mode session (mic + system audio mixed on this thread).
    meeting: Option<MeetingState>,
    /// Encodes meeting audio to disk when the retention setting is not off.
    /// Lives outside `MeetingState` (not torn down/rebuilt with it) because
    /// a mic rebuild mid-session must keep recording to the same file —
    /// only a genuinely new `session_id` (or no `record_path`) replaces it.
    recorder: Option<MeetingRecorder>,
    /// For emitting SystemAudioStatus events (set during app setup).
    app: Option<tauri::AppHandle>,
    /// Parameters of the running session, kept for mid-session rebuilds.
    active_params: Option<StartParams>,
    /// Name of the input device the current stream was built on.
    mic_device_name: Option<String>,
    /// Set by the cpal error callback when the stream dies (e.g. its device
    /// disappeared); the next mic health check rebuilds the capture leg.
    stream_failed: Arc<std::sync::atomic::AtomicBool>,
    last_mic_check: Instant,
    /// Throttles pushed AudioLevel events while a session is active.
    level_throttle: AudioLevelThrottle,
    /// Consecutive failed capture rebuilds since the last success (reset to
    /// 0 on success). Drives the mic-loss ladder in `check_mic_health`.
    mic_rebuild_failures: u32,
    /// Whether the current mic-loss episode already surfaced its one-time
    /// warning (meeting mode only); re-armed on the next successful rebuild.
    mic_loss_warned: bool,
    /// Shared with the engine actor. Set right before this thread exits
    /// after giving up on an unrecoverable microphone, so the actor's
    /// AudioGone handler can surface a mic-specific message instead of its
    /// generic fallback.
    audio_gone_reason: Arc<Mutex<Option<String>>>,
}

impl AudioCapture {
    /// Spawn the audio thread. Returns channels for commanding it and receiving audio.
    /// `dropped_counter` is incremented for every chunk lost to a full channel;
    /// the engine actor resets and reads it for health reporting.
    /// `audio_gone_reason` is shared with the engine actor: this thread sets
    /// it right before an unrecoverable failure ends the whole capture
    /// thread, so the actor's `AudioGone` handler can surface a specific
    /// message instead of its generic fallback.
    pub fn spawn(
        audio_rms: Arc<AtomicU32>,
        dropped_counter: Arc<AtomicU64>,
        audio_gone_reason: Arc<Mutex<Option<String>>>,
    ) -> Result<(Sender<AudioCommand>, Receiver<AudioMessage>), String> {
        let (cmd_tx, cmd_rx) = crossbeam_channel::unbounded::<AudioCommand>();
        // Bounded channel: many small chunks per second from cpal; inference (Kyutai/Metal)
        // can lag real-time. If this fills, try_send drops audio while RMS/waveform still
        // updates — use a generous bound so the inference thread can catch up.
        let (audio_tx, audio_rx) = crossbeam_channel::bounded::<AudioMessage>(512);

        std::thread::Builder::new()
            .name("audio-capture".into())
            .spawn(move || {
                crate::thread_qos::set_current_thread_qos(crate::thread_qos::ThreadQos::UserInteractive);
                let mut capture = AudioCapture {
                    stream: None,
                    audio_sender: audio_tx,
                    selected_device: None,
                    clamshell_device: None,
                    active_session_id: Arc::new(AtomicU64::new(0)),
                    audio_rms,
                    resampler: None,
                    dropped_counter,
                    meeting: None,
                    recorder: None,
                    app: None,
                    active_params: None,
                    mic_device_name: None,
                    stream_failed: Arc::new(std::sync::atomic::AtomicBool::new(false)),
                    last_mic_check: Instant::now(),
                    level_throttle: AudioLevelThrottle::new(),
                    mic_rebuild_failures: 0,
                    mic_loss_warned: false,
                    audio_gone_reason,
                };

                // Block on commands while idle; during sessions, wake
                // periodically to run the meeting mixer and mic health checks.
                loop {
                    let timeout = if capture.meeting.is_some() {
                        Some(MEETING_TICK)
                    } else if capture.active_params.is_some() {
                        Some(DICTATION_TICK)
                    } else {
                        None
                    };
                    let cmd = if let Some(timeout) = timeout {
                        match cmd_rx.recv_timeout(timeout) {
                            Ok(cmd) => cmd,
                            Err(RecvTimeoutError::Timeout) => {
                                // The mixer/resampler/AEC run here on raw audio.
                                // A panic in any of them must not abort the whole
                                // app: catch it, end the session, and let the
                                // engine actor recover via its AudioGone path
                                // (the meeting is already incrementally saved).
                                let ticked =
                                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                        capture.meeting_tick()
                                    }));
                                if ticked.is_err() {
                                    tracing::error!(
                                        "Audio mixer panicked; ending session to keep the app alive"
                                    );
                                    capture.abort_after_panic();
                                    break;
                                }
                                if capture.last_mic_check.elapsed() >= MIC_CHECK_INTERVAL {
                                    capture.last_mic_check = Instant::now();
                                    if capture.check_mic_health() {
                                        tracing::error!(
                                            "Microphone unrecoverable; ending session to keep the app alive"
                                        );
                                        break;
                                    }
                                }
                                capture.emit_throttled_audio_level();
                                continue;
                            }
                            Err(RecvTimeoutError::Disconnected) => break,
                        }
                    } else {
                        match cmd_rx.recv() {
                            Ok(cmd) => cmd,
                            Err(_) => break,
                        }
                    };

                    match cmd {
                        AudioCommand::Start {
                            session_id,
                            target_sample_rate,
                            mic_gain,
                            capture_system_audio,
                            diarize,
                            record_path,
                        } => {
                            if let Err(e) = capture.start(
                                session_id,
                                target_sample_rate,
                                mic_gain,
                                capture_system_audio,
                                diarize,
                                record_path,
                            ) {
                                warn!("Failed to start audio capture: {e}");
                            }
                        }
                        AudioCommand::Stop => {
                            capture.stop();
                        }
                        AudioCommand::SelectDevice(name) => {
                            info!("Selected input device: {name}");
                            capture.selected_device = Some(name);
                        }
                        AudioCommand::SetClamshellDevice(name) => {
                            if let Some(ref name) = name {
                                info!("Clamshell-mode microphone preference: {name}");
                            } else {
                                info!("Clamshell-mode microphone preference cleared");
                            }
                            capture.clamshell_device = name;
                        }
                        AudioCommand::AttachApp(app) => {
                            capture.app = Some(app);
                        }
                    }
                }

                info!("Audio thread exiting");
            })
            .map_err(|e| format!("Failed to spawn audio thread: {e}"))?;

        Ok((cmd_tx, audio_rx))
    }

    /// Which device name (if any) `find_device` should target, given the
    /// user's explicit pin, the configured clamshell-mode preference, and
    /// whether clamshell mode is currently active *and* a preference is
    /// configured. Returns `None` to mean "follow whatever the OS reports as
    /// the default input" — presence of the named device is checked by the
    /// caller, not here, so this stays a pure decision with no I/O.
    fn effective_device<'a>(
        selected: Option<&'a str>,
        clamshell_pref: Option<&'a str>,
        clamshell_active: bool,
    ) -> Option<&'a str> {
        // An explicit pin always wins: the clamshell preference only stands
        // in for "the system default", it never overrides a deliberate
        // user choice of device.
        selected.or_else(|| clamshell_pref.filter(|_| clamshell_active))
    }

    fn find_device(&self) -> Result<Device, String> {
        let host = cpal::default_host();

        // The ioreg probe behind `is_clamshell()` shells out; only pay for it
        // when a clamshell device is actually configured, and skip it
        // entirely once an explicit pin already decides the device.
        let clamshell_active = self.selected_device.is_none()
            && self.clamshell_device.is_some()
            && crate::power::is_clamshell();

        if let Some(name) = Self::effective_device(
            self.selected_device.as_deref(),
            self.clamshell_device.as_deref(),
            clamshell_active,
        ) {
            // Unfiltered `devices()`, not `input_devices()`: the latter's
            // default filter probes every device's supported input configs
            // (opens an AudioUnit per device on coreaudio) just to enumerate
            // them, which can flip a Bluetooth headset into HFP mono. This
            // runs on every session start and every mic health check
            // (MIC_CHECK_INTERVAL), so the unfiltered form matters even when
            // no device is pinned by name below the fallback. `Device::name`
            // itself is a cheap property read, safe to call on every device.
            let devices = host
                .devices()
                .map_err(|e| format!("Failed to list devices: {e}"))?;

            for device in devices {
                if let Ok(n) = device.name()
                    && n == name
                {
                    return Ok(device);
                }
            }
            warn!("Input device '{name}' not found, falling back to default");
        }

        host.default_input_device()
            .ok_or_else(|| "No input device available".to_string())
    }

    #[allow(clippy::too_many_arguments)]
    fn start(
        &mut self,
        session_id: u64,
        target_sample_rate: u32,
        mic_gain: f32,
        capture_system_audio: bool,
        diarize: bool,
        record_path: Option<PathBuf>,
    ) -> Result<(), String> {
        // Ensure any previous callback stops emitting immediately, and tear
        // down any leftover meeting state (tap included). The recorder is
        // NOT torn down here — see `sync_recorder` — so a mid-session mic
        // rebuild (same session_id) keeps recording to the same file.
        self.active_session_id.store(0, Ordering::Release);
        self.stream.take();
        self.meeting.take();
        self.sync_recorder(session_id, record_path.as_deref(), target_sample_rate);

        // Stored before any fallible step so a failed (re)build is retried
        // by the next mic health check instead of killing the session.
        self.active_params = Some(StartParams {
            session_id,
            target_sample_rate,
            mic_gain,
            capture_system_audio,
            diarize,
            record_path,
        });
        self.stream_failed
            .store(false, std::sync::atomic::Ordering::Relaxed);
        self.mic_device_name = None;

        let device = self.find_device()?;

        let device_name = device.name().unwrap_or_else(|_| "Unknown".into());
        info!("Using input device: {device_name}");
        self.mic_device_name = Some(device_name.clone());

        let config = Self::preferred_config(&device)?;
        let sample_rate = config.sample_rate.0;
        let channels = config.channels;

        info!("Audio config: {sample_rate}Hz, {channels}ch");

        if capture_system_audio {
            return self.start_meeting(
                &device,
                &config,
                session_id,
                target_sample_rate,
                mic_gain,
                diarize,
            );
        }

        let resampler = Arc::new(Mutex::new(Resampler::new(
            sample_rate,
            channels,
            target_sample_rate,
            mic_gain,
        )));
        self.resampler = Some(Arc::clone(&resampler));
        let sender = self.audio_sender.clone();
        let active_session_id = Arc::clone(&self.active_session_id);
        let rms_ref = Arc::clone(&self.audio_rms);
        let dropped_counter = Arc::clone(&self.dropped_counter);

        let stream_failed = Arc::clone(&self.stream_failed);
        let err_fn = move |err: cpal::StreamError| {
            error!("Audio stream error: {err}");
            stream_failed.store(true, std::sync::atomic::Ordering::Relaxed);
        };
        // Second handle so a panic inside the realtime callback (e.g. the
        // resampler) can flag the stream for rebuild instead of unwinding
        // across the CoreAudio C boundary (which would be UB).
        let stream_failed_cb = Arc::clone(&self.stream_failed);

        // Reset the first-chunk logging flag for each new recording session
        static LOGGED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        LOGGED.store(false, std::sync::atomic::Ordering::Relaxed);

        let stream = device
            .build_input_stream(
                &config,
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    if active_session_id.load(Ordering::Acquire) != session_id {
                        return;
                    }

                    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let resampled = match resampler.lock() {
                        Ok(mut r) => r.process(data),
                        Err(_) => return,
                    };
                    if !resampled.is_empty() {
                        // Compute RMS for waveform visualization
                        let sum_sq: f32 = resampled.iter().map(|s| s * s).sum();
                        let rms = (sum_sq / resampled.len() as f32).sqrt();
                        // Clamp to 0.0-1.0 (typical speech RMS is 0.01-0.15)
                        let normalized = (rms * 8.0).min(1.0);
                        rms_ref.store(normalized.to_bits(), Ordering::Relaxed);

                        // Log first chunk to confirm audio is flowing
                        if crate::debug::transcription_debug_enabled()
                            && !LOGGED.swap(true, std::sync::atomic::Ordering::Relaxed)
                        {
                            let max_amp = resampled.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
                            debug!(
                                "First audio chunk: {} samples, max_amp={max_amp:.4}",
                                resampled.len(),
                            );
                        }
                        if sender
                            .try_send(AudioMessage::Chunk(AudioChunk {
                                session_id,
                                samples: resampled,
                                captured_at: Instant::now(),
                                speaker: None,                            }))
                            .is_err()
                        {
                            let dropped = dropped_counter.fetch_add(1, Ordering::Relaxed) + 1;
                            if dropped == 1 || dropped.is_multiple_of(100) {
                                warn!("Audio buffer full, dropping samples ({dropped} chunks dropped this session)");
                            }
                        }
                    }
                    }));
                    if caught.is_err() {
                        stream_failed_cb.store(true, Ordering::Relaxed);
                    }
                },
                err_fn,
                None,
            )
            .map_err(|e| format!("Failed to build input stream: {e}"))?;

        self.active_session_id.store(session_id, Ordering::Release);
        stream
            .play()
            .map_err(|e| format!("Failed to start stream: {e}"))?;
        self.stream = Some(stream);

        info!("Audio capture started on '{device_name}'");
        Ok(())
    }

    /// Reconcile `self.recorder` with what this `start()` call wants:
    /// - Same `session_id` as the existing recorder: keep it — this is a
    ///   mid-session capture rebuild (mic loss, device change), not a new
    ///   recording session, so it must keep writing to the same file.
    /// - Different session (or none yet) and a path was given: finalize any
    ///   stale recorder and start a new one.
    /// - No path (dictation, or retention off): finalize any stale recorder.
    ///
    /// A failure to start the recorder is logged and otherwise ignored —
    /// recording is a best-effort opt-in feature, never a reason to fail the
    /// audio session itself.
    fn sync_recorder(&mut self, session_id: u64, record_path: Option<&std::path::Path>, sample_rate: u32) {
        let same_session = self.recorder.as_ref().is_some_and(|r| r.session_id() == session_id);
        match record_path {
            Some(_) if same_session => {}
            Some(path) => {
                self.finish_recording();
                match MeetingRecorder::start(path.to_path_buf(), sample_rate, session_id) {
                    Ok(recorder) => self.recorder = Some(recorder),
                    Err(e) => warn!("Failed to start meeting audio recorder: {e}"),
                }
            }
            None => self.finish_recording(),
        }
    }

    /// Finalize and drop the active recorder, if any, logging how many
    /// chunks the realtime audio thread had to drop because the writer
    /// thread couldn't keep up.
    fn finish_recording(&mut self) {
        if let Some(recorder) = self.recorder.take() {
            let dropped = recorder.dropped_chunks();
            if dropped > 0 {
                warn!("Meeting recorder dropped {dropped} audio chunks this session");
            }
            // Drop joins the writer thread, flushing the encoder and closing
            // the file — not on the realtime path, only at session end.
        }
    }

    /// Feed one mono meeting-audio chunk to the active recorder, if any.
    fn push_recording_mono(&self, samples: &[f32]) {
        if let Some(recorder) = &self.recorder {
            recorder.push(samples);
        }
    }

    /// Feed one diarized meeting-audio tick to the active recorder, if any:
    /// the two legs are summed with soft clipping into the single mixed
    /// stream the recording represents (diarization only affects
    /// transcription, not the recorded audio).
    fn push_recording_diarized(&self, me: &[f32], them: &[f32]) {
        let Some(recorder) = &self.recorder else {
            return;
        };
        let n = me.len().max(them.len());
        if n == 0 {
            return;
        }
        let mixed: Vec<f32> = (0..n)
            .map(|i| {
                let a = me.get(i).copied().unwrap_or(0.0);
                let b = them.get(i).copied().unwrap_or(0.0);
                (a + b).clamp(-1.0, 1.0)
            })
            .collect();
        recorder.push(&mixed);
    }

    /// Meeting mode: the cpal callback only pushes raw samples into a ring
    /// buffer; a system-audio tap fills a second ring; `meeting_tick()` on
    /// this thread resamples, mixes, and forwards to the engine.
    fn start_meeting(
        &mut self,
        device: &Device,
        config: &StreamConfig,
        session_id: u64,
        target_sample_rate: u32,
        mic_gain: f32,
        diarize: bool,
    ) -> Result<(), String> {
        let sample_rate = config.sample_rate.0;
        let channels = config.channels;

        // ~2s of headroom per ring; the 5ms tick drains far faster.
        let mic_capacity = (sample_rate as usize * channels as usize) * 2;
        let (mut mic_prod, mic_cons) = HeapRb::<f32>::new(mic_capacity).split();
        let (tap_prod, tap_cons) = HeapRb::<f32>::new(super::mixer::MIX_RATE as usize * 2).split();

        // Mic stream first: it's the session's pacing clock and must never
        // be held hostage by tap startup (see system_tap.rs module docs).
        let active_session_id = Arc::clone(&self.active_session_id);
        let stream_failed = Arc::clone(&self.stream_failed);
        let err_fn = move |err: cpal::StreamError| {
            error!("Audio stream error: {err}");
            stream_failed.store(true, std::sync::atomic::Ordering::Relaxed);
        };
        let stream = device
            .build_input_stream(
                config,
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    if active_session_id.load(Ordering::Acquire) != session_id {
                        return;
                    }
                    // Ring full means the mixer is wedged; losing mic samples
                    // here is the only safe option in a realtime callback.
                    let _ = mic_prod.push_slice(data);
                },
                err_fn,
                None,
            )
            .map_err(|e| format!("Failed to build input stream: {e}"))?;
        stream
            .play()
            .map_err(|e| format!("Failed to start stream: {e}"))?;

        // Accept mic samples into the ring buffer from here on, even though
        // `spawn_tap` below can block this thread for up to its 5s timeout
        // when coreaudiod is slow or wedged. The mic callback gates on this
        // id, so storing it only after the tap resolves silently dropped
        // every mic sample captured during that wait, up to several
        // seconds of the meeting's start, despite the mixer/meeting_tick not
        // running yet to drain them. The ring buffer (~2s capacity) still
        // bounds how much of a slow tap's wait it can retain, but that beats
        // discarding all of it outright.
        self.active_session_id.store(session_id, Ordering::Release);

        #[cfg(target_os = "macos")]
        let (tap, tap_rate) = match super::system_tap::spawn_tap(tap_prod, Duration::from_secs(5)) {
            Ok(tap) => {
                let rate = tap.sample_rate;
                emit_system_audio_status(self.app.as_ref(), true, None);
                (Some(tap), rate)
            }
            Err(e) => {
                warn!("System audio capture unavailable, recording mic only: {e}");
                emit_system_audio_status(self.app.as_ref(), false, Some(e));
                (None, super::mixer::MIX_RATE)
            }
        };
        #[cfg(not(target_os = "macos"))]
        let tap_rate = {
            drop(tap_prod);
            super::mixer::MIX_RATE
        };

        let mut mixer = MeetingMixer::new(
            mic_cons,
            sample_rate,
            channels,
            mic_gain,
            tap_cons,
            tap_rate,
            target_sample_rate,
        );

        // Echo cancellation only matters when system audio can leak from
        // the speakers back into the mic, and only if we actually have the
        // system-audio reference signal to cancel against.
        #[cfg(target_os = "macos")]
        let aec_active = {
            let can_leak = tap.is_some() && super::output_route::output_can_leak_into_mic();
            if can_leak {
                info!("Speakers audible, echo cancellation engaged");
                mixer.set_aec(Some(super::aec::Aec::new(super::mixer::MIX_RATE)));
            }
            can_leak
        };
        #[cfg(not(target_os = "macos"))]
        let aec_active = false;

        self.stream = Some(stream);
        self.meeting = Some(MeetingState {
            session_id,
            mixer,
            #[cfg(target_os = "macos")]
            tap,
            aec_active,
            diarize,
            ticks: 0,
        });

        info!("Meeting audio capture started (mic + system audio)");
        Ok(())
    }

    /// Rebuild the capture leg when the stream died or the default input
    /// device changed (lid closed, headset plugged in…). The session keeps
    /// its id, so the engine actor sees one continuous stream; at most a
    /// couple of seconds of audio are lost.
    ///
    /// Returns `true` if the microphone is unrecoverable and has no other
    /// audio source to fall back on (dictation): the session has already
    /// been ended and the caller must break its run loop so the whole
    /// thread exits. Returns `false` in every other case, including a
    /// meeting session that lost its mic but keeps retrying while the
    /// system-audio leg still captures the other participants.
    fn check_mic_health(&mut self) -> bool {
        let Some(params) = self.active_params.clone() else {
            return false;
        };

        let failed = self
            .stream_failed
            .swap(false, std::sync::atomic::Ordering::Relaxed);

        // Only follow the system default (or a configured clamshell
        // override) when the user hasn't pinned a device; a pinned device
        // that vanishes surfaces as a stream error and falls back to the
        // default on rebuild. Comparing against `find_device`'s own
        // resolution (rather than the raw OS default) means this settles
        // after one rebuild even when the clamshell device is configured but
        // not currently present — `find_device` falls back to the same OS
        // default every time in that case, so the names converge.
        let default_changed = self.selected_device.is_none()
            && self.mic_device_name.is_some()
            && self.find_device().ok().and_then(|d| d.name().ok()) != self.mic_device_name;

        if !failed && !default_changed {
            return false;
        }

        info!(
            "Input device {} — rebuilding audio capture",
            if failed { "failed" } else { "changed" }
        );
        match self.start(
            params.session_id,
            params.target_sample_rate,
            params.mic_gain,
            params.capture_system_audio,
            params.diarize,
            params.record_path.clone(),
        ) {
            Ok(()) => {
                self.mic_rebuild_failures = 0;
                self.mic_loss_warned = false;
                false
            }
            Err(e) => {
                self.mic_rebuild_failures += 1;
                warn!(
                    "Capture rebuild failed ({} in a row): {e}",
                    self.mic_rebuild_failures
                );
                match decide_mic_loss(
                    self.mic_rebuild_failures,
                    params.capture_system_audio,
                    self.mic_loss_warned,
                ) {
                    MicLossAction::KeepRetrying => false,
                    MicLossAction::WarnOnce => {
                        self.mic_loss_warned = true;
                        self.emit_pipeline_warning(
                            "Microphone lost; still recording system audio.".to_string(),
                        );
                        false
                    }
                    MicLossAction::Abort => {
                        warn!(
                            "Microphone unrecoverable after {} attempts; ending session",
                            self.mic_rebuild_failures
                        );
                        self.abort_after_mic_loss();
                        true
                    }
                }
            }
        }
    }

    /// Periodic mixer pump while a meeting session is active.
    fn meeting_tick(&mut self) {
        // Do all mixer work inside this borrow, then release it before calling
        // &self/&mut self helpers (sending, RMS) to satisfy the borrow checker.
        let (session_id, diarize, mixed, me, them) = {
            let Some(meeting) = self.meeting.as_mut() else {
                return;
            };
            meeting.ticks += 1;
            if meeting.ticks.is_multiple_of(ROUTE_CHECK_TICKS) {
                meeting.check_output_route(self.app.as_ref());
            }
            if meeting.diarize {
                let (me, them) = meeting.mixer.tick_split();
                (meeting.session_id, true, Vec::new(), me, them)
            } else {
                let mixed = meeting.mixer.tick();
                (meeting.session_id, false, mixed, Vec::new(), Vec::new())
            }
        };

        use crate::engine::Speaker;
        if diarize {
            self.push_recording_diarized(&me, &them);
            self.store_meeting_rms(&me, &them);
            self.send_meeting_chunk(session_id, me, Some(Speaker::Me));
            self.send_meeting_chunk(session_id, them, Some(Speaker::Them));
        } else {
            self.push_recording_mono(&mixed);
            self.store_meeting_rms(&mixed, &[]);
            self.send_meeting_chunk(session_id, mixed, None);
        }
    }

    /// Push the current RMS level to the frontend, respecting `level_throttle`.
    /// Called from the active-session timeout branch, so it only ever runs
    /// while a session is running.
    fn emit_throttled_audio_level(&mut self) {
        if !self.level_throttle.should_emit(Instant::now()) {
            return;
        }
        let level = f32::from_bits(self.audio_rms.load(Ordering::Relaxed));
        self.emit_audio_level(level);
    }

    /// Emit an AudioLevel event unconditionally (bypassing the throttle) —
    /// used for the final zero-level emit when a session ends, so the
    /// waveform decays instead of freezing on its last value.
    fn emit_audio_level(&self, level: f32) {
        use tauri_specta::Event;
        if let Some(app) = &self.app {
            let _ = crate::app_events::AudioLevel { level }.emit(app);
        }
    }

    /// Surface a non-fatal pipeline problem: the session keeps running.
    /// Reuses the `Frame` scope (a transient, non-fatal problem the user
    /// should see) rather than adding a dedicated warning scope — the
    /// frontend only ever displays `message` and doesn't branch on `scope`.
    fn emit_pipeline_warning(&self, message: String) {
        use tauri_specta::Event;
        if let Some(app) = &self.app {
            let _ = crate::app_events::PipelineError {
                scope: crate::app_events::PipelineErrorScope::Frame,
                message,
            }
            .emit(app);
        }
    }

    /// Update the shared RMS level (waveform) from one or two legs combined.
    fn store_meeting_rms(&self, a: &[f32], b: &[f32]) {
        let n = a.len() + b.len();
        if n == 0 {
            return;
        }
        let sum_sq: f32 = a.iter().chain(b).map(|s| s * s).sum();
        let rms = (sum_sq / n as f32).sqrt();
        self.audio_rms
            .store((rms * 8.0).min(1.0).to_bits(), Ordering::Relaxed);
    }

    /// Forward one meeting chunk to the engine actor, tagged with its source.
    fn send_meeting_chunk(
        &self,
        session_id: u64,
        samples: Vec<f32>,
        speaker: Option<crate::engine::Speaker>,
    ) {
        if samples.is_empty() {
            return;
        }
        if self
            .audio_sender
            .try_send(AudioMessage::Chunk(AudioChunk {
                session_id,
                samples,
                captured_at: Instant::now(),
                speaker,
            }))
            .is_err()
        {
            let dropped = self.dropped_counter.fetch_add(1, Ordering::Relaxed) + 1;
            if dropped == 1 || dropped.is_multiple_of(100) {
                warn!(
                    "Audio buffer full, dropping samples ({dropped} chunks dropped this session)"
                );
            }
        }
    }

    /// Tear down all session state after a fatal, unrecoverable capture
    /// failure, without running any of the (possibly corrupt) flush paths.
    /// Shared by the panic recovery path and the terminal mic-loss path:
    /// both end with the caller breaking the audio thread's run loop, so
    /// the thread exits and the engine actor observes the closed audio
    /// channel (AudioGone) and recovers — salvaging the meeting accumulated
    /// so far and surfacing a recoverable error — instead of the whole app
    /// aborting.
    fn teardown_session_state(&mut self) {
        self.active_session_id.store(0, Ordering::Release);
        self.audio_rms.store(0f32.to_bits(), Ordering::Relaxed);
        self.active_params = None;
        self.mic_device_name = None;
        self.stream.take();
        self.resampler.take();
        #[cfg(target_os = "macos")]
        if let Some(mut meeting) = self.meeting.take() {
            meeting.tap.take();
        }
        #[cfg(not(target_os = "macos"))]
        {
            self.meeting.take();
        }
        // Best-effort close: no final flush from the (possibly corrupt)
        // mixer, just whatever the recorder already buffered. A truncated
        // but structurally valid Ogg file is acceptable here.
        self.finish_recording();
        self.emit_audio_level(0.0);
        self.level_throttle.reset();
    }

    fn abort_after_panic(&mut self) {
        self.teardown_session_state();
    }

    /// Ends a dictation session whose microphone could not be rebuilt after
    /// repeated attempts and has no other audio source to fall back on.
    /// Records the reason for the engine actor's `AudioGone` handler before
    /// tearing down — see `teardown_session_state` for why exiting this
    /// thread is what lets the actor salvage and fail the session, exactly
    /// like a panic abort.
    fn abort_after_mic_loss(&mut self) {
        if let Ok(mut reason) = self.audio_gone_reason.lock() {
            *reason = Some(
                "The microphone was lost and could not be reconnected; \
                 the recording so far was saved."
                    .to_string(),
            );
        }
        self.teardown_session_state();
    }

    fn stop(&mut self) {
        let mut session_id = self.active_session_id.swap(0, Ordering::AcqRel);
        // A session whose stream died mid-rebuild has id 0 in the atomic but
        // still owes the actor its EndOfStream marker.
        if session_id == 0
            && let Some(params) = &self.active_params
        {
            session_id = params.session_id;
        }
        self.audio_rms.store(0f32.to_bits(), Ordering::Relaxed);
        self.active_params = None;
        self.mic_device_name = None;
        self.emit_audio_level(0.0);
        self.level_throttle.reset();

        // Dropping the stream is synchronous — after this, no callback runs.
        let had_stream = self.stream.take().is_some();

        if let Some(mut meeting) = self.meeting.take() {
            // Tear down the tap first so its ring stops filling; then one
            // final flush drains both rings and all resampler tails.
            #[cfg(target_os = "macos")]
            meeting.tap.take();

            if session_id != 0 {
                if meeting.diarize {
                    let (me, them) = meeting.mixer.flush_split();
                    self.push_recording_diarized(&me, &them);
                    self.send_meeting_chunk(session_id, me, Some(crate::engine::Speaker::Me));
                    self.send_meeting_chunk(session_id, them, Some(crate::engine::Speaker::Them));
                } else {
                    let tail = meeting.mixer.flush();
                    self.push_recording_mono(&tail);
                    self.send_meeting_chunk(session_id, tail, None);
                }
                let discarded = meeting.mixer.tap_discarded();
                if discarded > 0 {
                    warn!("Discarded {discarded} system-audio samples to bound drift");
                }
                self.send_end_of_stream(session_id);
            }
            self.finish_recording();

            if had_stream {
                info!("Meeting audio capture stopped");
            }
            return;
        }
        self.finish_recording();

        if session_id != 0 {
            // Flush the resampler's remaining partial chunk so the last
            // spoken samples reach the engine instead of being discarded.
            if let Some(resampler) = self.resampler.take()
                && let Ok(mut r) = resampler.lock()
            {
                let tail = r.flush();
                if !tail.is_empty() {
                    let _ = self.audio_sender.send(AudioMessage::Chunk(AudioChunk {
                        session_id,
                        samples: tail,
                        captured_at: Instant::now(),
                        speaker: None,
                    }));
                }
            }

            self.send_end_of_stream(session_id);
        }

        if had_stream {
            info!("Audio capture stopped");
        }
    }

    /// EndOfStream is the signal the actor's stop waits on. The actor is
    /// normally draining, so this sends immediately; the timeout only
    /// matters if no one is consuming (e.g. session aborted) — then we give
    /// up rather than wedge the audio thread, and the actor's own EOS-wait
    /// deadline covers the stop path.
    fn send_end_of_stream(&self, session_id: u64) {
        if self
            .audio_sender
            .send_timeout(
                AudioMessage::EndOfStream { session_id },
                Duration::from_secs(1),
            )
            .is_err()
        {
            warn!("Could not deliver end-of-stream marker (channel full or closed)");
        }
    }

    fn preferred_config(device: &Device) -> Result<StreamConfig, String> {
        let mut supported = device
            .supported_input_configs()
            .map_err(|e| format!("Failed to get supported configs: {e}"))?;

        let config = supported
            .find(|c| c.sample_format() == SampleFormat::F32)
            .or_else(|| {
                device
                    .supported_input_configs()
                    .ok()
                    .and_then(|mut c| c.next())
            })
            .ok_or_else(|| "No supported input config found".to_string())?;

        Ok(config.with_max_sample_rate().into())
    }
}

#[cfg(test)]
mod effective_device_tests {
    use super::AudioCapture;

    #[test]
    fn explicit_pin_wins_over_clamshell_preference() {
        assert_eq!(
            AudioCapture::effective_device(Some("Pinned Mic"), Some("USB Mic"), true),
            Some("Pinned Mic"),
        );
    }

    #[test]
    fn explicit_pin_wins_even_without_clamshell_active() {
        assert_eq!(
            AudioCapture::effective_device(Some("Pinned Mic"), Some("USB Mic"), false),
            Some("Pinned Mic"),
        );
    }

    #[test]
    fn clamshell_preference_applies_when_no_pin_and_active() {
        assert_eq!(
            AudioCapture::effective_device(None, Some("USB Mic"), true),
            Some("USB Mic"),
        );
    }

    #[test]
    fn clamshell_preference_ignored_when_not_active() {
        assert_eq!(AudioCapture::effective_device(None, Some("USB Mic"), false), None);
    }

    #[test]
    fn no_pin_no_clamshell_preference_follows_default() {
        assert_eq!(AudioCapture::effective_device(None, None, true), None);
        assert_eq!(AudioCapture::effective_device(None, None, false), None);
    }
}
