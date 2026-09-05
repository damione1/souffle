use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, SampleFormat, Stream, StreamConfig, SupportedStreamConfigRange};
use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};
use ringbuf::HeapRb;
use ringbuf::traits::{Producer, Split};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tracing::{debug, error, info, warn};

use super::mixer::MeetingMixer;
use super::recorder::MeetingRecorder;
use super::resampler::Resampler;
use crate::audio::device::AudioInputDevice;
use crate::audio::priority::{InputPriority, ResolveInputParams, resolve_input};
use crate::state::AudioCommand;

/// Tell the frontend whether the system-audio leg of a meeting is live.
fn emit_system_audio_status(app: Option<&tauri::AppHandle>, active: bool, reason: Option<String>) {
    use tauri_specta::Event;
    if let Some(app) = app {
        let _ = crate::app_events::SystemAudioStatus { active, reason }.emit(app);
    }
}

/// Mixer cadence while a meeting session is active. It only paces the mixer;
/// the real-time callbacks never wait on it, and the rings they fill hold
/// ~2s. Audio arrives in 10ms frames, so waking twice per frame is already
/// generous, and every wake-up here lands on a UserInteractive thread that
/// would otherwise let the CPU idle.
const MEETING_TICK: Duration = Duration::from_millis(20);

/// How often the meeting tick re-checks the default output route. Property
/// reads are a handful of cheap HAL calls; polling keeps everything on this
/// thread instead of a listener callback.
const ROUTE_CHECK_INTERVAL: Duration = Duration::from_secs(2);
const ROUTE_CHECK_TICKS: u32 = (ROUTE_CHECK_INTERVAL.as_millis() / MEETING_TICK.as_millis()) as u32;

/// Wake-up cadence during dictation sessions: fast enough to feed the
/// AudioLevel stream for the waveform (the mic health check keeps its own
/// coarser MIC_CHECK_INTERVAL gate, so it does not run this often).
const DICTATION_TICK: Duration = LEVEL_EMIT_INTERVAL;

/// How often an active session verifies its input device is still alive and
/// still the system default (closing the laptop lid switches the default
/// input to a headset or webcam mic — the stream must follow it).
const MIC_CHECK_INTERVAL: Duration = Duration::from_secs(2);

/// A live input stream delivers callbacks many times a second. If none
/// arrive for this long the AudioUnit is a zombie (USB dock reboot that
/// never fires cpal's error callback) and the mic leg must be rebuilt.
const MIC_STALE_AFTER: Duration = Duration::from_secs(2);

/// Ceiling on how often AudioLevel is pushed to the frontend. The meeting
/// tick is faster than this; the dictation tick matches this interval, so both
/// modes stream levels at ~15Hz.
const LEVEL_EMIT_INTERVAL: Duration = Duration::from_millis(66);

/// Consecutive rebuild failures after which a dictation session (mic is the
/// only source) gives up and ends itself, rather than retrying forever
/// while the UI still claims to be recording. ~10s at the `MIC_CHECK_INTERVAL`
/// cadence — failures closer together than that interval do not increment
/// the counter (a BT/dock DeviceList burst must not burn the 5 slots in ms
/// and then kill the capture thread for the process lifetime).
const DICTATION_ABORT_AFTER_FAILURES: u32 = 5;

/// `spawn_tap` can block this thread for up to 5s. A missing tap is retried
/// on the next mic rebuild, but not more often than this, so a flapping
/// mic does not stall the mixer every `MIC_CHECK_INTERVAL`.
const TAP_RETRY_INTERVAL: Duration = Duration::from_secs(15);

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

fn unix_now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// True when the cpal input callback has been silent longer than `threshold`.
fn mic_callbacks_stale(last_callback_ms: u64, now_ms: u64, threshold: Duration) -> bool {
    last_callback_ms > 0 && now_ms.saturating_sub(last_callback_ms) >= threshold.as_millis() as u64
}

/// USB unplug/replug keeps the device UID and allocates a new CoreAudio
/// object. `None` current means the UID disappeared from the HAL list.
fn opened_device_replaced(opened_id: Option<u32>, current_id: Option<u32>) -> bool {
    match (opened_id, current_id) {
        (Some(opened), Some(current)) => opened != current,
        (Some(_), None) => true,
        _ => false,
    }
}

/// Whether the capture leg should be torn down and reopened. UID-only
/// comparison misses dock reboots: the pin still resolves, the stream is
/// just bound to a dead HAL object.
fn should_rebuild_mic(
    stream_failed: bool,
    callbacks_stale: bool,
    device_replaced: bool,
    device_not_alive: bool,
    resolved_changed: bool,
) -> bool {
    stream_failed || callbacks_stale || device_replaced || device_not_alive || resolved_changed
}

/// Keep the meeting tap/mixer across a mic rebuild of the same session.
/// Tearing them down first meant a failed reopen also killed system audio,
/// after which the `capture_system_audio` *setting* still counted as a
/// fallback and the session recorded silence.
///
/// `tap_owned` is a live `TapHandle`, not the setting. A `MeetingState`
/// whose `spawn_tap` failed (`tap: None`) must fall through and retry —
/// unless `tap_retry_ready` is false, which rate-limits the 5s spawn.
fn should_reuse_meeting(
    existing_session: Option<u64>,
    new_session: u64,
    capture_system_audio: bool,
    tap_owned: bool,
    tap_retry_ready: bool,
) -> bool {
    if !(capture_system_audio && existing_session == Some(new_session)) {
        return false;
    }
    tap_owned || !tap_retry_ready
}

/// Count a rebuild failure only when `min_interval` has elapsed since the
/// last counted one. Returns the new (count, timestamp-of-this-count).
fn count_rebuild_failure(
    failures: u32,
    last_counted: Option<Instant>,
    now: Instant,
    min_interval: Duration,
) -> (u32, Instant) {
    match last_counted {
        Some(t) if now.saturating_duration_since(t) < min_interval => (failures, t),
        _ => (failures.saturating_add(1), now),
    }
}

/// The abort ladder is per-session. A meeting that KeepRetrying-ed on a
/// live tap can leave the counter at 50; the next dictation must not
/// Abort on its first transient SetInputPolicy.
fn reset_mic_loss_ladder(
    failures: &mut u32,
    last_counted: &mut Option<Instant>,
    warned: &mut bool,
) {
    *failures = 0;
    *last_counted = None;
    *warned = false;
}

/// The mic-loss ladder's "other source" is a tap that is actually running,
/// not the user's setting. A meeting that requested system audio but never
/// got a tap (or lost it) is dictation: abort rather than fake-record.
fn has_fallback_audio(tap_live: bool) -> bool {
    tap_live
}

/// Fallback rate when the device's current one can't be read. 48 kHz is what
/// every conferencing stack negotiates; the device maximum is not, and picking
/// it is what broke other apps.
const FALLBACK_INPUT_SAMPLE_RATE: u32 = 48_000;

/// Rate to open a shared input device at.
///
/// cpal writes `kAudioDevicePropertyNominalSampleRate` from
/// `build_input_stream` whenever the requested rate differs from the device's
/// current one, and that property is machine-wide: opening the built-in mic at
/// its 96 kHz maximum re-rates it for every other client. Meeting apps that
/// bring up their own pipeline afterwards (Teams) fail to start on a mic above
/// 48 kHz, so keep whatever rate the device is already on and let CoreAudio
/// stay untouched. Souffle resamples to the engine rate regardless, so a
/// higher device rate buys nothing.
fn choose_input_sample_rate(current: Option<u32>, min: u32, max: u32) -> u32 {
    match current {
        Some(rate) if (min..=max).contains(&rate) => rate,
        _ => FALLBACK_INPUT_SAMPLE_RATE.clamp(min, max),
    }
}

/// Prefer an F32 range that already covers `current`, so we don't fall back
/// and re-rate the device just because the first listed range is narrower.
fn pick_input_config_range(
    supported: &[SupportedStreamConfigRange],
    current: Option<u32>,
) -> Option<SupportedStreamConfigRange> {
    let f32_ranges: Vec<_> = supported
        .iter()
        .filter(|c| c.sample_format() == SampleFormat::F32)
        .cloned()
        .collect();
    let candidates: &[SupportedStreamConfigRange] = if f32_ranges.is_empty() {
        supported
    } else {
        &f32_ranges
    };

    if let Some(rate) = current
        && let Some(range) = candidates
            .iter()
            .find(|c| (c.min_sample_rate()..=c.max_sample_rate()).contains(&rate))
    {
        return Some(range.clone());
    }
    candidates.first().cloned()
}

#[cfg(test)]
mod input_rate_tests {
    use super::*;

    /// The whole point: never move a rate another app may depend on.
    #[test]
    fn keeps_the_rate_the_device_already_runs_at() {
        assert_eq!(
            choose_input_sample_rate(Some(96_000), 8_000, 96_000),
            96_000
        );
        assert_eq!(
            choose_input_sample_rate(Some(48_000), 8_000, 96_000),
            48_000
        );
        assert_eq!(
            choose_input_sample_rate(Some(16_000), 8_000, 96_000),
            16_000
        );
    }

    #[test]
    fn falls_back_to_48k_when_the_current_rate_is_unknown() {
        assert_eq!(choose_input_sample_rate(None, 8_000, 96_000), 48_000);
    }

    /// A stale reading from another device, or a rate the range no longer
    /// covers, must not be requested: cpal would reject the whole stream.
    #[test]
    fn ignores_a_current_rate_outside_the_supported_range() {
        assert_eq!(
            choose_input_sample_rate(Some(192_000), 8_000, 96_000),
            48_000
        );
    }

    #[test]
    fn clamps_the_fallback_into_a_narrow_range() {
        assert_eq!(choose_input_sample_rate(None, 16_000, 16_000), 16_000);
        assert_eq!(choose_input_sample_rate(None, 88_200, 192_000), 88_200);
    }

    fn f32_range(min: u32, max: u32) -> SupportedStreamConfigRange {
        SupportedStreamConfigRange::new(
            1,
            min,
            max,
            cpal::SupportedBufferSize::Unknown,
            SampleFormat::F32,
        )
    }

    #[test]
    fn picks_later_f32_range_that_covers_current() {
        let ranges = vec![f32_range(8_000, 44_100), f32_range(48_000, 96_000)];
        let picked = pick_input_config_range(&ranges, Some(48_000)).unwrap();
        assert_eq!(picked.min_sample_rate(), 48_000);
        assert_eq!(picked.max_sample_rate(), 96_000);
        let rate = choose_input_sample_rate(
            Some(48_000),
            picked.min_sample_rate(),
            picked.max_sample_rate(),
        );
        assert_eq!(rate, 48_000);
    }

    #[test]
    fn first_f32_range_still_wins_when_it_covers_current() {
        let ranges = vec![f32_range(8_000, 96_000), f32_range(48_000, 48_000)];
        let picked = pick_input_config_range(&ranges, Some(48_000)).unwrap();
        assert_eq!(picked.min_sample_rate(), 8_000);
        assert_eq!(picked.max_sample_rate(), 96_000);
    }

    #[test]
    fn falls_back_to_first_f32_when_current_fits_none() {
        let ranges = vec![f32_range(8_000, 16_000), f32_range(44_100, 44_100)];
        let picked = pick_input_config_range(&ranges, Some(48_000)).unwrap();
        assert_eq!(picked.min_sample_rate(), 8_000);
        assert_eq!(picked.max_sample_rate(), 16_000);
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

    #[test]
    fn callbacks_stale_after_threshold_not_before() {
        let t0 = 1_000;
        assert!(!mic_callbacks_stale(t0, t0 + 1_999, MIC_STALE_AFTER));
        assert!(mic_callbacks_stale(t0, t0 + 2_000, MIC_STALE_AFTER));
        assert!(
            !mic_callbacks_stale(0, t0 + 10_000, MIC_STALE_AFTER),
            "unset timestamp must not look stale (pre-start)"
        );
    }

    #[test]
    fn replaced_when_object_id_changes_or_uid_vanishes() {
        assert!(!opened_device_replaced(Some(42), Some(42)));
        assert!(opened_device_replaced(Some(42), Some(99)));
        assert!(opened_device_replaced(Some(42), None));
        assert!(!opened_device_replaced(None, Some(99)));
        assert!(!opened_device_replaced(None, None));
    }

    #[test]
    fn rebuilds_on_dock_reboot_signals_not_just_uid_change() {
        assert!(
            !should_rebuild_mic(false, false, false, false, false),
            "healthy stream must not rebuild"
        );
        assert!(
            should_rebuild_mic(false, false, true, false, false),
            "same UID, new HAL object (USB replug)"
        );
        assert!(
            should_rebuild_mic(false, true, false, false, false),
            "callbacks stopped"
        );
        assert!(
            should_rebuild_mic(false, false, false, true, false),
            "DeviceIsAlive flipped off"
        );
        assert!(should_rebuild_mic(true, false, false, false, false));
        assert!(should_rebuild_mic(false, false, false, false, true));
    }

    #[test]
    fn reuses_meeting_only_for_same_session_with_system_audio() {
        assert!(should_reuse_meeting(Some(7), 7, true, true, false));
        assert!(
            !should_reuse_meeting(Some(7), 8, true, true, false),
            "a new session must tear the old tap down"
        );
        assert!(
            !should_reuse_meeting(Some(7), 7, false, true, false),
            "dictation must not keep a leftover meeting tap"
        );
        assert!(!should_reuse_meeting(None, 7, true, true, false));
    }

    #[test]
    fn missing_tap_is_retried_once_the_window_opens() {
        assert!(
            should_reuse_meeting(Some(7), 7, true, false, false),
            "a just-failed spawn_tap must not be retried on the next 2s tick"
        );
        assert!(
            !should_reuse_meeting(Some(7), 7, true, false, true),
            "a MeetingState with tap: None must fall through to spawn_tap"
        );
        assert!(
            should_reuse_meeting(Some(7), 7, true, true, true),
            "an owned tap is never torn down just because the retry window opened"
        );
    }

    #[test]
    fn rebuild_failures_are_not_counted_inside_the_check_interval() {
        let t0 = Instant::now();
        let (n1, t1) = count_rebuild_failure(0, None, t0, MIC_CHECK_INTERVAL);
        assert_eq!(n1, 1);
        let (n2, t2) = count_rebuild_failure(
            n1,
            Some(t1),
            t0 + Duration::from_millis(10),
            MIC_CHECK_INTERVAL,
        );
        assert_eq!(n2, 1, "a DeviceList burst must not burn the abort ladder");
        assert_eq!(t2, t1);
        let (n3, _) =
            count_rebuild_failure(n2, Some(t1), t0 + MIC_CHECK_INTERVAL, MIC_CHECK_INTERVAL);
        assert_eq!(n3, 2);
    }

    #[test]
    fn mic_loss_ladder_resets_between_sessions() {
        let mut failures = 50;
        let mut last = Some(Instant::now());
        let mut warned = true;
        reset_mic_loss_ladder(&mut failures, &mut last, &mut warned);
        assert_eq!(failures, 0);
        assert_eq!(last, None);
        assert!(!warned);
        let (n, _) = count_rebuild_failure(failures, last, Instant::now(), MIC_CHECK_INTERVAL);
        assert_eq!(n, 1, "first failure of the new session counts from zero");
        assert_eq!(
            decide_mic_loss(n, false, false),
            MicLossAction::KeepRetrying,
            "a leftover meeting counter must not abort the next dictation"
        );
    }

    #[test]
    fn fallback_is_a_live_tap_not_the_system_audio_setting() {
        // The setting alone used to keep a meeting alive after a rebuild
        // had already dropped the tap — UI still "recording", no audio.
        assert!(
            !has_fallback_audio(false),
            "tap missing → abort like dictation"
        );
        assert!(has_fallback_audio(true));
        assert_eq!(
            decide_mic_loss(
                DICTATION_ABORT_AFTER_FAILURES,
                has_fallback_audio(false),
                false
            ),
            MicLossAction::Abort
        );
        assert_eq!(
            decide_mic_loss(
                DICTATION_ABORT_AFTER_FAILURES,
                has_fallback_audio(true),
                false
            ),
            MicLossAction::WarnOnce
        );
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

    /// The route check is expressed in seconds; if the tick changes, the
    /// derived counter must still land on that interval.
    #[test]
    fn route_check_counter_matches_its_interval() {
        assert_eq!(
            MEETING_TICK * ROUTE_CHECK_TICKS,
            ROUTE_CHECK_INTERVAL,
            "ROUTE_CHECK_TICKS no longer divides evenly into the tick"
        );
    }

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
    /// Last `spawn_tap` attempt (success or fail). Caps retries so a
    /// flapping mic rebuild cannot block this thread for 5s every 2s.
    last_tap_attempt: Instant,
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
                self.mixer
                    .set_aec(Some(aec::Aec::new_with_default_delay_hint(mixer::MIX_RATE)));
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

/// List all available input devices. Logged at info level (device count) so
/// enumeration events show up in the Diagnostics live log.
pub fn list_input_devices() -> Vec<AudioInputDevice> {
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
fn list_input_devices_impl() -> Vec<AudioInputDevice> {
    super::device_watch::list_devices()
}

#[cfg(not(target_os = "macos"))]
fn list_input_devices_impl() -> Vec<AudioInputDevice> {
    use crate::audio::device::TransportType;

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
            Some(AudioInputDevice {
                uid: name.clone(),
                name,
                transport: TransportType::Unknown,
                is_default: name == default_name,
            })
        })
        .collect()
}

/// CoreAudio UID of a cpal device (`kAudioDevicePropertyDeviceUID`). Cheap
/// property read — unlike `name()` / `description()`, this does not open
/// an AudioUnit.
fn cpal_device_uid(device: &Device) -> Option<String> {
    device.id().ok().map(|id| id.1)
}

/// Locate a cpal device by UID without probing supported configs.
fn find_cpal_device_by_uid(host: &cpal::Host, uid: &str) -> Option<Device> {
    host.devices()
        .ok()?
        .find(|device| cpal_device_uid(device).as_deref() == Some(uid))
}

fn current_mic_object_id(uid: &str) -> Option<u32> {
    #[cfg(target_os = "macos")]
    {
        super::device_watch::object_id_for_uid(uid)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = uid;
        None
    }
}

fn mic_device_is_alive(id: u32) -> bool {
    #[cfg(target_os = "macos")]
    {
        super::device_watch::device_is_alive(id)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = id;
        true
    }
}

/// Manages audio capture from a selected input device.
/// Sends resampled 24kHz mono f32 chunks over a crossbeam channel.
///
/// Lives on a dedicated thread so CoreAudio IO and Stream lifecycle stay
/// off the UI / async runtime. (cpal 0.17's macOS Stream is Send; isolation
/// is still the right ownership model.)
pub struct AudioCapture {
    stream: Option<Stream>,
    audio_sender: Sender<AudioMessage>,
    /// Pinned input device UID (or legacy name during migration).
    selected_device: Option<String>,
    /// Preferred microphone UID while the lid is closed with an external display
    /// attached (clamshell mode); `None` disables the override entirely, so
    /// `find_device` never pays for the `is_clamshell()` probe.
    clamshell_device: Option<String>,
    /// Ordered input preferences and remembered devices.
    input_priority: InputPriority,
    /// When false, skip Bluetooth inputs during automatic device selection.
    allow_bluetooth_mic: bool,
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
    /// UID resolved when the current stream was built (for route hot-swap).
    mic_device_uid: Option<String>,
    /// CoreAudio object ID of that UID at open. USB replug keeps the UID
    /// and issues a new ID; comparing them is how we detect a dock reboot.
    mic_device_object_id: Option<u32>,
    /// Last cpal input-callback time (unix ms). 0 = never.
    last_mic_callback_ms: Arc<AtomicU64>,
    /// Resolved target UID of the last route-change rebuild. When the opened
    /// device cannot converge on that target (duplicate device names, or a
    /// resolved device cpal cannot open), this stops the periodic health
    /// check from tearing the stream down every interval; explicit route
    /// events (`refresh_input_route`) clear it so a real change retries.
    route_attempt_uid: Option<String>,
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
    /// When the last rebuild failure was counted toward `mic_rebuild_failures`.
    /// Bursts of CoreAudio DeviceList / DefaultInput notifications must not
    /// increment the counter faster than `MIC_CHECK_INTERVAL`.
    last_counted_failure: Option<Instant>,
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
                    input_priority: InputPriority::default(),
                    allow_bluetooth_mic: false,
                    active_session_id: Arc::new(AtomicU64::new(0)),
                    audio_rms,
                    resampler: None,
                    dropped_counter,
                    meeting: None,
                    recorder: None,
                    app: None,
                    active_params: None,
                    mic_device_name: None,
                    mic_device_uid: None,
                    mic_device_object_id: None,
                    last_mic_callback_ms: Arc::new(AtomicU64::new(0)),
                    route_attempt_uid: None,
                    stream_failed: Arc::new(std::sync::atomic::AtomicBool::new(false)),
                    last_mic_check: Instant::now(),
                    level_throttle: AudioLevelThrottle::new(),
                    mic_rebuild_failures: 0,
                    mic_loss_warned: false,
                    last_counted_failure: None,
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
                        AudioCommand::SelectDevice(uid) => {
                            if uid.is_empty() {
                                info!("Cleared explicit input device pin");
                                capture.selected_device = None;
                            } else {
                                info!("Selected input device UID: {uid}");
                                capture.selected_device = Some(uid);
                            }
                            if capture.refresh_input_route() {
                                break;
                            }
                        }
                        AudioCommand::SetClamshellDevice(uid) => {
                            if let Some(ref uid) = uid {
                                info!("Clamshell-mode microphone preference UID: {uid}");
                            } else {
                                info!("Clamshell-mode microphone preference cleared");
                            }
                            capture.clamshell_device = uid;
                            if capture.refresh_input_route() {
                                break;
                            }
                        }
                        AudioCommand::SetInputPolicy {
                            priority,
                            allow_bluetooth_mic,
                        } => {
                            capture.input_priority = priority;
                            capture.allow_bluetooth_mic = allow_bluetooth_mic;
                            if capture.refresh_input_route() {
                                break;
                            }
                        }
                        AudioCommand::AttachApp(app) => {
                            capture.app = Some(app);
                        }
                        AudioCommand::RefreshInputRoute => {
                            if capture.refresh_input_route() {
                                break;
                            }
                        }
                    }
                }

                info!("Audio thread exiting");
            })
            .map_err(|e| format!("Failed to spawn audio thread: {e}"))?;

        Ok((cmd_tx, audio_rx))
    }

    /// Which device UID `find_device` should target, given the user's pin,
    /// clamshell preference, priority list, and anti-Bluetooth policy.
    fn resolved_input_uid(&self, connected: &[AudioInputDevice]) -> Option<String> {
        let clamshell_active = self.selected_device.is_none()
            && self.clamshell_device.is_some()
            && crate::power::is_clamshell();

        resolve_input(
            connected,
            ResolveInputParams {
                pin: self.selected_device.as_deref(),
                clamshell_pref: self.clamshell_device.as_deref(),
                clamshell_active,
                priority: &self.input_priority,
                allow_bluetooth_mic: self.allow_bluetooth_mic,
            },
        )
    }

    /// Open the cpal device for the resolved input route. `known_devices` is
    /// the CoreAudio snapshot the caller already holds, so resolution and
    /// UID mapping work from one consistent device list.
    fn find_device(&self, known_devices: &[AudioInputDevice]) -> Result<Device, String> {
        let host = cpal::default_host();

        if let Some(uid) = self.resolved_input_uid(known_devices) {
            // Unfiltered `devices()` + `Device::id()` (`kAudioDevicePropertyDeviceUID`).
            // Do not use `input_devices()`, `name()`, or `description()`: in
            // cpal 0.17 those open an AudioUnit per device (input *and*
            // output for `description`) just to inspect it, which can flip a
            // Bluetooth headset into HFP mono. This runs on every session
            // start and every mic rebuild.
            if let Some(device) = find_cpal_device_by_uid(&host, &uid) {
                return Ok(device);
            }

            // Legacy pin stored as a display name: map through our snapshot
            // (cheap CoreAudio list) then look up by UID. Never cpal name().
            if let Some(mapped) = known_devices.iter().find(|d| d.name == uid)
                && mapped.uid != uid
                && let Some(device) = find_cpal_device_by_uid(&host, &mapped.uid)
            {
                return Ok(device);
            }

            warn!("Input device '{uid}' not found, falling back to default");
        }

        // Also reached when `resolve_input` found no auto-eligible device
        // (every connected mic hidden, or Bluetooth-only with the Bluetooth
        // preference off): recording on the OS default, even an excluded
        // one, beats not recording at all. The policy only steers the
        // automatic choice among alternatives.
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
        let is_new_session = self
            .active_params
            .as_ref()
            .is_none_or(|p| p.session_id != session_id);
        if is_new_session {
            self.clear_mic_loss_ladder();
        }
        // Ensure any previous callback stops emitting immediately. The
        // recorder is NOT torn down here; see `sync_recorder`, so a
        // mid-session mic rebuild (same session_id) keeps recording to the
        // same file. The meeting tap/mixer stay too: dropping them here
        // made a failed mic reopen also lose system audio.
        self.active_session_id.store(0, Ordering::Release);
        self.release_capture_stream();
        if !should_reuse_meeting(
            self.meeting.as_ref().map(|m| m.session_id),
            session_id,
            capture_system_audio,
            self.tap_owned_for_reuse(),
            self.tap_retry_ready(),
        ) {
            self.meeting.take();
        }
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
        self.mic_device_uid = None;
        self.mic_device_object_id = None;
        self.last_mic_callback_ms
            .store(unix_now_ms(), Ordering::Relaxed);

        let known_devices = list_input_devices_impl();
        let device = self.find_device(&known_devices)?;

        // Track the UID cpal actually opened, not just what resolve_input
        // picked: find_device may fall back to the OS default. `id()` is a
        // UID property read; `name()`/`description()` would open AudioUnits.
        let opened_uid = cpal_device_uid(&device);
        let device_name = opened_uid
            .as_deref()
            .and_then(|uid| {
                known_devices
                    .iter()
                    .find(|d| d.uid == uid)
                    .map(|d| d.name.clone())
            })
            .or_else(|| opened_uid.clone())
            .unwrap_or_else(|| "Unknown".into());
        self.mic_device_uid = opened_uid.clone();
        self.mic_device_object_id = opened_uid.as_deref().and_then(current_mic_object_id);
        info!("Using input device: {device_name}");
        self.mic_device_name = Some(device_name.clone());

        let config = Self::preferred_config(&device)?;
        let sample_rate = config.sample_rate;
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

        // Dictation: never keep a leftover meeting tap from a prior session.
        self.meeting.take();

        // The stream that fed the outgoing resampler is already released, so
        // its buffered chunk can be emitted without racing samples that came
        // after it. Only for a rebuild of the running session: a new one must
        // not inherit the previous session's audio.
        if !is_new_session {
            self.emit_resampler_tail(session_id);
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
        let last_mic_callback_ms = Arc::clone(&self.last_mic_callback_ms);
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
                    last_mic_callback_ms.store(unix_now_ms(), Ordering::Relaxed);

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
    fn sync_recorder(
        &mut self,
        session_id: u64,
        record_path: Option<&std::path::Path>,
        sample_rate: u32,
    ) {
        let same_session = self
            .recorder
            .as_ref()
            .is_some_and(|r| r.session_id() == session_id);
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
    /// stream the recording represents (lane split only affects
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
        let sample_rate = config.sample_rate;
        let channels = config.channels;

        // ~2s of headroom per ring; the 20ms tick drains far faster.
        let mic_capacity = (sample_rate as usize * channels as usize) * 2;
        let (mut mic_prod, mic_cons) = HeapRb::<f32>::new(mic_capacity).split();

        // Mic stream first: it's the session's pacing clock and must never
        // be held hostage by tap startup (see system_tap.rs module docs).
        let active_session_id = Arc::clone(&self.active_session_id);
        let stream_failed = Arc::clone(&self.stream_failed);
        let last_mic_callback_ms = Arc::clone(&self.last_mic_callback_ms);
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
                    last_mic_callback_ms.store(unix_now_ms(), Ordering::Relaxed);
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

        if let Some(meeting) = self.meeting.as_mut() {
            meeting
                .mixer
                .replace_mic(mic_cons, sample_rate, channels, mic_gain);
            meeting.session_id = session_id;
            meeting.diarize = diarize;
            self.stream = Some(stream);
            info!("Meeting microphone rebuilt; system-audio tap kept");
            return Ok(());
        }

        let (tap_prod, tap_cons) = HeapRb::<f32>::new(super::mixer::MIX_RATE as usize * 2).split();

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
                mixer.set_aec(Some(super::aec::Aec::new_with_default_delay_hint(
                    super::mixer::MIX_RATE,
                )));
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
            last_tap_attempt: Instant::now(),
        });

        info!("Meeting audio capture started (mic + system audio)");
        Ok(())
    }

    /// Rebuild the capture leg when the stream died, the HAL object behind
    /// the same UID was replaced (USB dock reboot), callbacks stopped, or
    /// the resolved input UID changed (lid closed, headset plugged in).
    /// The session keeps its id, so the engine actor sees one continuous
    /// stream; at most a couple of seconds of audio are lost.
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
        let stale = mic_callbacks_stale(
            self.last_mic_callback_ms.load(Ordering::Relaxed),
            unix_now_ms(),
            MIC_STALE_AFTER,
        );
        let current_id = self
            .mic_device_uid
            .as_deref()
            .and_then(current_mic_object_id);
        let replaced = opened_device_replaced(self.mic_device_object_id, current_id);
        let not_alive = self
            .mic_device_object_id
            .is_some_and(|id| !mic_device_is_alive(id));

        // Two guards keep UID-change rebuilds from looping every interval:
        // - `None` (no auto-eligible device under the current policy) never
        //   tears down a healthy stream; a dead one is caught above.
        // - A target already attempted without converging is parked in
        //   `route_attempt_uid` until an explicit route event retries it.
        let resolved = self.resolved_input_uid(&list_input_devices_impl());
        let resolved_changed = resolved.is_some()
            && resolved != self.mic_device_uid
            && resolved != self.route_attempt_uid;

        if !should_rebuild_mic(failed, stale, replaced, not_alive, resolved_changed) {
            return false;
        }

        if resolved_changed {
            self.route_attempt_uid = resolved.clone();
        }

        let reason = if failed {
            "failed"
        } else if stale {
            "callbacks stalled"
        } else if replaced {
            "HAL object replaced (same UID)"
        } else if not_alive {
            "device not alive"
        } else {
            "changed"
        };
        info!("Input device {reason}, rebuilding audio capture");
        match self.start(
            params.session_id,
            params.target_sample_rate,
            params.mic_gain,
            params.capture_system_audio,
            params.diarize,
            params.record_path.clone(),
        ) {
            Ok(()) => {
                self.clear_mic_loss_ladder();
                if resolved_changed && resolved != self.mic_device_uid {
                    warn!(
                        "Input route did not converge on the resolved device \
                         (target {:?}, opened {:?}); keeping the opened device \
                         until the route changes",
                        resolved, self.mic_device_uid
                    );
                }
                false
            }
            Err(e) => {
                let (n, counted_at) = count_rebuild_failure(
                    self.mic_rebuild_failures,
                    self.last_counted_failure,
                    Instant::now(),
                    MIC_CHECK_INTERVAL,
                );
                self.mic_rebuild_failures = n;
                self.last_counted_failure = Some(counted_at);
                warn!(
                    "Capture rebuild failed ({} in a row): {e}",
                    self.mic_rebuild_failures
                );
                match decide_mic_loss(
                    self.mic_rebuild_failures,
                    has_fallback_audio(self.tap_is_live()),
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

    /// Re-run input resolution and hot-swap the mic leg when recording.
    /// Called on explicit route events (device pick, policy change, CoreAudio
    /// device-list or default-input change), so a previously non-converged
    /// target is fair game again.
    ///
    /// Returns `true` when the microphone is unrecoverable and the session
    /// has been torn down: the caller must break the audio thread so the
    /// actor observes AudioGone. Returning without exiting left the actor
    /// waiting on a channel that never closed, and `Stop` could no longer
    /// send EndOfStream (`active_params` was already cleared).
    fn refresh_input_route(&mut self) -> bool {
        self.route_attempt_uid = None;
        if self.active_params.is_some() {
            self.last_mic_check = Instant::now();
            if self.check_mic_health() {
                tracing::error!(
                    "Microphone unrecoverable after input route change; ending session"
                );
                return true;
            }
        }
        false
    }

    fn clear_mic_loss_ladder(&mut self) {
        reset_mic_loss_ladder(
            &mut self.mic_rebuild_failures,
            &mut self.last_counted_failure,
            &mut self.mic_loss_warned,
        );
    }

    /// A process tap that is still owned by this session. The system-audio
    /// *setting* is not enough: a rebuild may have already dropped the tap.
    /// Ownership, not IOProc liveness — a zombie handle still counts.
    fn tap_is_live(&self) -> bool {
        #[cfg(target_os = "macos")]
        {
            self.meeting.as_ref().is_some_and(|m| m.tap.is_some())
        }
        #[cfg(not(target_os = "macos"))]
        {
            false
        }
    }

    /// Whether `should_reuse_meeting` may keep the current mixer. On macOS
    /// this is a tap handle; elsewhere there is nothing to recover so a
    /// same-session meeting mixer is always reusable.
    fn tap_owned_for_reuse(&self) -> bool {
        #[cfg(target_os = "macos")]
        {
            self.tap_is_live()
        }
        #[cfg(not(target_os = "macos"))]
        {
            self.meeting.is_some()
        }
    }

    /// Missing tap + retry window elapsed. `spawn_tap` blocks up to 5s, so
    /// this must stay false on the 2s health cadence after a failed spawn.
    fn tap_retry_ready(&self) -> bool {
        #[cfg(target_os = "macos")]
        {
            self.meeting.as_ref().is_some_and(|m| {
                m.tap.is_none() && m.last_tap_attempt.elapsed() >= TAP_RETRY_INTERVAL
            })
        }
        #[cfg(not(target_os = "macos"))]
        {
            false
        }
    }

    /// Periodic mixer pump while a meeting session is active.
    fn meeting_tick(&mut self) {
        // Do all mixer work inside this borrow, then release it before calling
        // &self/&mut self helpers (sending, RMS) to satisfy the borrow checker.
        let tap_clock = self.stream.is_none();
        let (session_id, diarize, mixed, me, them) = {
            let Some(meeting) = self.meeting.as_mut() else {
                return;
            };
            meeting.ticks += 1;
            if meeting.ticks.is_multiple_of(ROUTE_CHECK_TICKS) {
                meeting.check_output_route(self.app.as_ref());
            }
            if meeting.diarize {
                let (me, them) = if tap_clock {
                    meeting.mixer.tick_split_on_tap_clock()
                } else {
                    meeting.mixer.tick_split()
                };
                (meeting.session_id, true, Vec::new(), me, them)
            } else {
                let mixed = if tap_clock {
                    meeting.mixer.tick_on_tap_clock()
                } else {
                    meeting.mixer.tick()
                };
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
        self.mic_device_uid = None;
        self.mic_device_object_id = None;
        self.route_attempt_uid = None;
        self.last_mic_callback_ms.store(0, Ordering::Relaxed);
        self.release_capture_stream();
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
        self.clear_mic_loss_ladder();
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

    /// Emit what the mic resampler still holds before a rebuild replaces it,
    /// mirroring the flush `stop` does at session end. Without this the
    /// partial FFT chunk (~20ms of speech) dies with the old stream, on top
    /// of the gap the teardown already costs.
    ///
    /// Callers must have released the capture stream first, so no callback
    /// can push samples that would end up ordered before this tail.
    fn emit_resampler_tail(&mut self, session_id: u64) {
        let Some(resampler) = self.resampler.take() else {
            return;
        };
        let Ok(mut r) = resampler.lock() else {
            return;
        };
        let tail = r.flush();
        if tail.is_empty() {
            return;
        }
        if self
            .audio_sender
            .try_send(AudioMessage::Chunk(AudioChunk {
                session_id,
                samples: tail,
                captured_at: Instant::now(),
                speaker: None,
            }))
            .is_err()
        {
            debug!("Dropped the resampler tail on mic rebuild (channel full)");
        }
    }

    /// Stop IO and dispose the capture AudioUnit.
    ///
    /// On macOS, a Bluetooth headset cannot run A2DP stereo output and HFP
    /// input at once: opening the mic forces HFP/SCO (mono). Stopping the
    /// unit (`pause`) is not enough — Core Audio keeps the input route until
    /// `AudioUnitUninitialize` + `AudioComponentInstanceDispose`, which run
    /// when cpal's `StreamInner` drops. cpal 0.15 leaked that inner via a
    /// disconnect-listener `Arc` cycle (RustAudio/cpal#771), so the headset
    /// stayed in HFP until process exit. Pause then drop so both happen here.
    fn release_capture_stream(&mut self) -> bool {
        let Some(stream) = self.stream.take() else {
            return false;
        };
        if let Err(e) = stream.pause() {
            debug!("Audio stream pause on release: {e}");
        }
        drop(stream);
        true
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
        self.mic_device_uid = None;
        self.mic_device_object_id = None;
        self.route_attempt_uid = None;
        self.last_mic_callback_ms.store(0, Ordering::Relaxed);
        self.emit_audio_level(0.0);
        self.level_throttle.reset();
        self.clear_mic_loss_ladder();

        // Stop IO and dispose the AudioUnit — after this, no callback runs
        // and a Bluetooth headset can leave HFP/mono for A2DP stereo.
        let had_stream = self.release_capture_stream();

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
        let supported: Vec<_> = device
            .supported_input_configs()
            .map_err(|e| format!("Failed to get supported configs: {e}"))?
            .collect();

        // `default_input_config` reads the device's *current* stream format,
        // which is the one rate that opens without re-rating the hardware for
        // every other process on the machine.
        let current = device.default_input_config().ok().map(|c| c.sample_rate());
        let config = pick_input_config_range(&supported, current)
            .ok_or_else(|| "No supported input config found".to_string())?;

        let rate =
            choose_input_sample_rate(current, config.min_sample_rate(), config.max_sample_rate());
        if current != Some(rate) {
            warn!(
                "Input device current rate unreadable or unsupported ({current:?}); \
                 opening at {rate}Hz, which re-rates the device system-wide"
            );
        }

        Ok(config.with_sample_rate(rate).into())
    }
}
