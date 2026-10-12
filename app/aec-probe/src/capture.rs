use crate::{
    Result,
    devices::{Selection, global_f64, global_u32, property_f32, property_u32},
    protocol::{self, Mode, Scene, Track, TrialPurpose, TrialStatus},
};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use objc2_core_audio::*;
use ringbuf::{
    HeapRb,
    traits::{Consumer, Producer, Split},
};
use serde_json::{Value, json};
use souffle_lib::audio::{
    aec::Aec,
    mixer::{FRAME_SAMPLES, MIX_RATE, MeetingMixer},
    native_voice::{Native, Sample, Sink, Telemetry},
    system_tap::SystemTap,
};
use std::{
    path::Path,
    sync::{Arc, atomic::Ordering, mpsc},
    time::{Duration, Instant},
};

// mach_absolute_time is the CoreAudio host clock; libc deprecates its wrapper, not the API.
#[allow(deprecated)]
pub fn host_time() -> u64 {
    unsafe { libc::mach_absolute_time() }
}

fn snapshot(selection: Option<&Selection>) -> Value {
    let input = global_u32(
        kAudioObjectSystemObject as u32,
        kAudioHardwarePropertyDefaultInputDevice,
    );
    let output = global_u32(
        kAudioObjectSystemObject as u32,
        kAudioHardwarePropertyDefaultOutputDevice,
    );
    let describe = |id: std::result::Result<u32, String>| match id {
        Ok(id) => json!({"id": id, "uid": souffle_lib::audio::output_route::device_uid(id),
            "rate": global_f64(id, kAudioDevicePropertyNominalSampleRate),
            "transport": global_u32(id, kAudioDevicePropertyTransportType),
            "volume_main": property_f32(id, kAudioDevicePropertyVolumeScalar, kAudioObjectPropertyScopeOutput, 0),
            "volume_channel1": property_f32(id, kAudioDevicePropertyVolumeScalar, kAudioObjectPropertyScopeOutput, 1),
            "mute": property_u32(id, kAudioDevicePropertyMute, kAudioObjectPropertyScopeOutput, 0)}),
        Err(e) => json!({"error": e}),
    };
    let selected_input = selection.map_or_else(|| input.clone(), |s| Ok(s.input.id));
    let selected_output = selection.map_or_else(|| output.clone(), |s| Ok(s.output.id));
    json!({"input": describe(selected_input), "output": describe(selected_output),
        "default_input_id": input, "default_output_id": output,
        "production_aec_route_eligible": souffle_lib::audio::output_route::output_can_leak_into_mic()})
}
fn usage() -> libc::rusage {
    let mut u = std::mem::MaybeUninit::zeroed();
    // RUSAGE_SELF and correctly sized writable storage.
    unsafe {
        libc::getrusage(libc::RUSAGE_SELF, u.as_mut_ptr());
        u.assume_init()
    }
}
fn cpu(u: &libc::rusage) -> f64 {
    u.ru_utime.tv_sec as f64
        + u.ru_utime.tv_usec as f64 / 1e6
        + u.ru_stime.tv_sec as f64
        + u.ru_stime.tv_usec as f64 / 1e6
}
fn command(program: &str, args: &[&str]) -> String {
    std::process::Command::new(program)
        .args(args)
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .unwrap_or_else(|e| e.to_string())
}

// Close and wait on every error path as well as a normal recording. A HAL
// teardown can wedge; the worker remains bounded and the failed trial is kept.
struct TapWorker {
    stop: Option<mpsc::Sender<()>>,
    thread: Option<std::thread::JoinHandle<std::result::Result<(u64, u64), String>>>,
}
impl TapWorker {
    fn finish(&mut self) -> Result<(u64, u64)> {
        self.stop.take();
        let Some(thread) = self.thread.take() else {
            return Err("tap already stopped".into());
        };
        let deadline = Instant::now() + Duration::from_secs(5);
        while !thread.is_finished() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        if !thread.is_finished() {
            return Err("tap teardown timed out; trial invalid".into());
        }
        Ok(thread.join().map_err(|_| "tap thread panicked")??)
    }
}
impl Drop for TapWorker {
    fn drop(&mut self) {
        if self.thread.is_some() {
            let _ = self.finish();
        }
    }
}

enum Plan {
    Formal {
        protocol: Box<protocol::Protocol>,
        repetition: u8,
        positive: bool,
    },
    Listening {
        selection: Selection,
    },
}
impl Plan {
    fn selection(&self) -> Option<&Selection> {
        match self {
            Self::Formal { .. } => None,
            Self::Listening { selection } => Some(selection),
        }
    }
    fn purpose(&self) -> TrialPurpose {
        match self {
            Self::Formal { .. } => TrialPurpose::Comparison,
            Self::Listening { .. } => TrialPurpose::ListeningCheck,
        }
    }
    fn positive(&self) -> bool {
        match self {
            Self::Formal { positive, .. } => *positive,
            Self::Listening { .. } => false,
        }
    }
    fn playback(&self) -> Result<Option<(Vec<f32>, u32)>> {
        match self {
            Self::Formal {
                protocol, positive, ..
            } => {
                if *positive {
                    Ok(Some(protocol::read_wav(Path::new(&protocol.fixture))?))
                } else {
                    Ok(None)
                }
            }
            Self::Listening { .. } => Ok(None),
        }
    }
}
fn private_trial(trial: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir(trial)?;
    std::fs::set_permissions(trial, std::fs::Permissions::from_mode(0o700))?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn check(
    trial: &Path,
    mode: Mode,
    scene: Scene,
    input_uid: &str,
    output_uid: Option<&str>,
    notes: &str,
    seconds: u32,
) -> Result<()> {
    if trial.exists() {
        return Err("trial directory already exists; choose a new private folder".into());
    }
    let selection = Selection::resolve(input_uid, output_uid)?;
    eprintln!(
        "{} / {}: device identity and formats will be recorded in the private manifest",
        mode.name(),
        scene.name()
    );
    run_trial(
        trial,
        mode,
        scene,
        notes,
        seconds,
        Plan::Listening { selection },
    )
}

#[allow(clippy::too_many_arguments)]
pub fn run(
    dir: &Path,
    mode: Mode,
    scene: Scene,
    repetition: u8,
    notes: &str,
    seconds: u32,
    positive: bool,
) -> Result<()> {
    let protocol = protocol::load(dir)?;
    if positive && mode != Mode::Apple {
        return Err("positive control requires apple mode".into());
    }
    let name = format!(
        "{}-{}-{repetition}{}{}",
        mode.name(),
        scene.name(),
        if positive { "-control" } else { "" },
        if seconds != 30 { "-aux" } else { "" }
    );
    let trial = dir.join(name);
    run_trial(
        &trial,
        mode,
        scene,
        notes,
        seconds,
        Plan::Formal {
            protocol: Box::new(protocol),
            repetition,
            positive,
        },
    )
}

fn run_trial(
    trial: &Path,
    mode: Mode,
    scene: Scene,
    notes: &str,
    seconds: u32,
    plan: Plan,
) -> Result<()> {
    if !(1..=60).contains(&seconds) || notes.trim().is_empty() {
        return Err("duration must be 1..60 seconds and notes nonempty (use 'none')".into());
    }
    private_trial(trial)?;
    let mut manifest = json!({"schema": 2, "valid": false, "status": TrialStatus::Starting, "purpose": plan.purpose(),
        "comparison_eligible": false, "mode": mode, "scene": scene,
        "seconds_requested": seconds, "positive_control": plan.positive(), "operator_notes": notes,
        "machine": command("sysctl", &["-n", "hw.model"]), "os": command("sw_vers", &[]),
        "git_commit": option_env!("SOU277_COMMIT").unwrap_or("not embedded"),
        "before": snapshot(plan.selection()), "api": {},
        "tap_alignment": "common drain epoch, nominal 10ms polling plus unbounded scheduling/device phase uncertainty; no hardware timestamp alignment claimed",
        "latency_definition": "maximum mic capture host timestamp to worker delivery (includes callback buffer duration); tap timestamp latency unavailable",
        "microphone_mode": "unverified; operator should choose Standard and record deviations in notes"});
    match &plan {
        Plan::Formal {
            protocol,
            repetition,
            ..
        } => {
            protocol::write_json(&trial.join("protocol.json"), protocol)?;
            manifest["protocol_sha256"] = json!(protocol::hash(&trial.join("protocol.json"))?);
            manifest["repetition"] = json!(repetition);
            manifest["microphone_mode"] =
                json!("Standard, operator assertion; not programmatically verified");
        }
        Plan::Listening { selection } => manifest["requested_devices"] = json!(selection),
    }
    protocol::write_json(&trial.join("manifest.json"), &manifest)?;
    let outcome = record(trial, mode, seconds, &plan, &mut manifest);
    manifest["after"] = snapshot(plan.selection());
    match &outcome {
        Ok(()) => manifest["status"] = json!(TrialStatus::Complete),
        Err(e) => {
            manifest["status"] = json!(TrialStatus::Failed);
            manifest["error"] = json!(e.to_string());
            manifest["valid"] = json!(false);
        }
    }
    protocol::write_json(&trial.join("manifest.json"), &manifest)?;
    outcome
}

fn record(trial: &Path, mode: Mode, seconds: u32, plan: &Plan, manifest: &mut Value) -> Result<()> {
    // Off the realtime callbacks: retain the existing tap/DSP tracing, including
    // exact CoreAudio teardown OSStatus values, in this private trial folder.
    tracing_subscriber::fmt()
        .with_ansi(false)
        .with_writer(std::sync::Mutex::new(std::fs::File::create(
            trial.join("runtime.log"),
        )?))
        .try_init()
        .map_err(|e| format!("cannot install trial logger: {e}"))?;
    let before = snapshot(plan.selection());
    // A new process hosts exactly one tap. No production orphan sweep is called.
    let (tap_prod, mut tap_cons) = HeapRb::<f32>::new(192000 * 2).split();
    let (ready_tx, ready_rx) = mpsc::channel();
    let (stop_tx, stop_rx) = mpsc::channel::<()>();
    let tap_thread = std::thread::spawn(move || -> std::result::Result<(u64, u64), String> {
        match SystemTap::start(tap_prod) {
            Ok(tap) => {
                let _ = ready_tx.send(Ok(tap.sample_rate() as u32));
                let _ = stop_rx.recv();
                let result = (tap.callback_count(), tap.dropped_samples());
                drop(tap);
                if souffle_lib::audio::system_tap::teardown_left_orphans() {
                    return Err(
                        "CoreAudio tap teardown failed; exact step/OSStatus in runtime.log".into(),
                    );
                }
                Ok(result)
            }
            Err(e) => {
                let _ = ready_tx.send(Err(e.clone()));
                Err(e)
            }
        }
    });
    let mut tap_worker = TapWorker {
        stop: Some(stop_tx),
        thread: Some(tap_thread),
    };
    let tap_rate = ready_rx
        .recv_timeout(Duration::from_secs(8))
        .map_err(|e| format!("system tap startup: {e}"))??;
    if !(8000..=192000).contains(&tap_rate) {
        return Err("unsupported tap rate".into());
    }
    manifest["tap_rate"] = json!(tap_rate);
    let (mic_prod, mut mic_cons) = HeapRb::<Sample>::new(192000 * 2).split();
    let telemetry = Arc::new(Telemetry::default());
    let mut native = None;
    let mut cpal_stream = None;
    let rate = match mode {
        Mode::Apple => {
            let playback = plan.playback()?;
            let n = Native::start(
                Sink::Timestamped(mic_prod),
                Arc::clone(&telemetry),
                playback,
                plan.selection(),
                &mut manifest["api"],
            )?;
            let rate = n.rate;
            manifest["native"] = n.evidence.clone();
            native = Some(n);
            rate
        }
        Mode::Baseline => {
            let host = cpal::default_host();
            let (device, input_id) = match plan.selection() {
                Some(s) => (s.cpal_input()?, s.input.id),
                None => (
                    host.default_input_device().ok_or("no default input")?,
                    global_u32(
                        kAudioObjectSystemObject as u32,
                        kAudioHardwarePropertyDefaultInputDevice,
                    )?,
                ),
            };
            let current = global_f64(input_id, kAudioDevicePropertyNominalSampleRate)? as u32;
            let config = device
                .supported_input_configs()?
                .find(|r| {
                    r.sample_format() == cpal::SampleFormat::F32
                        && r.min_sample_rate() <= current
                        && current <= r.max_sample_rate()
                })
                .ok_or(
                    "no f32 input config at existing hardware rate; refusing to change shared rate",
                )?
                .with_sample_rate(current)
                .config();
            let channels = config.channels as usize;
            manifest["cpal"] = json!({"sample_rate": current, "channels": channels, "sample_format": "f32",
                    "input_id": input_id, "input_uid": device.id()?.1, "input_name": device.description()?.name()});
            let mut producer = mic_prod;
            let t = Arc::clone(&telemetry);
            let errors = Arc::clone(&telemetry);
            let stream = device.build_input_stream(
                &config,
                move |data: &[f32], info: &cpal::InputCallbackInfo| {
                    t.callbacks.fetch_add(1, Ordering::Relaxed);
                    if !t.accepting.load(Ordering::Relaxed) {
                        return;
                    }
                    let now = host_time();
                    let ts = info.timestamp();
                    let delay = ts
                        .callback
                        .duration_since(&ts.capture)
                        .map(|d| d.as_secs_f64());
                    let host = match delay {
                        Some(d) => now.saturating_sub(unsafe {
                            objc2_avf_audio::AVAudioTime::hostTimeForSeconds(d)
                        }),
                        None => {
                            t.errors.fetch_add(1, Ordering::Relaxed);
                            now
                        }
                    };
                    for frame in data.chunks_exact(channels) {
                        let sample = Sample {
                            value: frame.iter().sum::<f32>() / channels as f32,
                            host,
                        };
                        if producer.try_push(sample).is_err() {
                            t.dropped.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                },
                move |_| {
                    errors.errors.fetch_add(1, Ordering::Relaxed);
                },
                None,
            )?;
            stream.play()?;
            cpal_stream = Some(stream);
            current
        }
    };
    if !(8000..=192000).contains(&rate) {
        return Err("unsupported microphone rate".into());
    }
    // Allow IO startup, then discard startup only; trial duration begins below.
    eprintln!("Ready in 3 seconds; prepare the fixed browser/local script. Capture {seconds}s.");
    for _ in 0..300 {
        while tap_cons.try_pop().is_some() {}
        std::thread::sleep(Duration::from_millis(10));
    }
    while tap_cons.try_pop().is_some() {}
    let tap_drain_epoch = host_time();
    let start_usage = usage();
    let start = Instant::now();
    telemetry.accepting.store(true, Ordering::Relaxed);
    eprintln!("RECORDING");
    let mut mic = Vec::with_capacity(rate as usize * (seconds as usize + 1));
    let mut tap = Vec::with_capacity(tap_rate as usize * (seconds as usize + 1));
    let mut max_latency = 0.0f64;
    let mut first_host = None;
    let mut last_host = None;
    let mut routes = Vec::new();
    let mut next_route = 0;
    loop {
        while let Some(s) = mic_cons.try_pop() {
            first_host.get_or_insert(s.host);
            last_host = Some(s.host);
            let age = unsafe {
                objc2_avf_audio::AVAudioTime::secondsForHostTime(host_time().saturating_sub(s.host))
            };
            max_latency = max_latency.max(age);
            mic.push(s.value);
        }
        while let Some(s) = tap_cons.try_pop() {
            tap.push(s);
        }
        if start.elapsed().as_secs() >= next_route {
            routes.push(json!({"seconds": start.elapsed().as_secs_f64(), "route": snapshot(plan.selection())}));
            next_route += 1;
        }
        if start.elapsed() >= Duration::from_secs(u64::from(seconds)) {
            break;
        }
        if mic.len() > rate as usize * 61 || tap.len() > tap_rate as usize * 61 {
            return Err("capture exceeded bounded duration".into());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    telemetry.accepting.store(false, Ordering::Relaxed);
    if let Some(n) = &native {
        manifest["native_rendered_frames"] = json!(n.rendered.load(Ordering::Relaxed));
    }
    drop(native);
    drop(cpal_stream);
    let (tap_callbacks, tap_dropped) = tap_worker.finish()?;
    let end_usage = usage();
    let duration = start.elapsed().as_secs_f64();
    manifest["telemetry"] = json!({"mic_rate": rate, "mic_frames": mic.len(), "mic_pcm_seconds": mic.len() as f64 / rate as f64,
        "tap_frames": tap.len(), "tap_pcm_seconds": tap.len() as f64 / tap_rate as f64,
        "mic_callbacks": telemetry.callbacks.load(Ordering::Relaxed), "mic_dropped": telemetry.dropped.load(Ordering::Relaxed),
        "mic_errors": telemetry.errors.load(Ordering::Relaxed), "tap_callbacks": tap_callbacks, "tap_dropped": tap_dropped,
        "mic_first_host": first_host, "mic_last_host": last_host, "tap_drain_epoch_host": tap_drain_epoch,
        "capture_to_worker_max_ms": max_latency * 1000.0, "wall_seconds_including_teardown": duration,
        "cpu_seconds": cpu(&end_usage) - cpu(&start_usage), "cpu_percent_one_core": (cpu(&end_usage)-cpu(&start_usage))/duration*100.0,
        "max_rss_bytes_process_lifetime": end_usage.ru_maxrss});
    manifest["route_samples"] = json!(routes);
    protocol::write_wav(&trial.join(Track::MicNative.filename()), &mic, rate)?;
    protocol::write_wav(&trial.join(Track::TapNative.filename()), &tap, tap_rate)?;
    let after = snapshot(plan.selection());
    let known_rates = ["input", "output"]
        .iter()
        .all(|side| before[side]["rate"]["Ok"].as_f64().is_some());
    let rates_unchanged = known_rates
        && before["input"]["rate"] == after["input"]["rate"]
        && before["output"]["rate"] == after["output"]["rate"];
    let stable = routes.iter().all(|r| r["route"] == before);
    let duration_ok = (mic.len() as f64 / rate as f64 - seconds as f64).abs() < 0.25
        && (tap.len() as f64 / tap_rate as f64 - seconds as f64).abs() < 0.25;
    let scene: Scene = serde_json::from_value(manifest["scene"].clone())?;
    let signal_present = (!scene.local() || mic.iter().any(|x| *x != 0.0))
        && (!scene.remote() || tap.iter().any(|x| *x != 0.0));
    let render_active = match mode {
        Mode::Baseline => true,
        Mode::Apple => manifest["native_rendered_frames"]
            .as_u64()
            .is_some_and(|n| n > 0),
    };
    let valid = signal_present
        && render_active
        && rates_unchanged
        && stable
        && duration_ok
        && tap_callbacks > 0
        && tap_dropped == 0
        && telemetry.dropped.load(Ordering::Relaxed) == 0
        && telemetry.errors.load(Ordering::Relaxed) == 0;
    manifest["comparison_eligible"] = json!(
        valid
            && plan.purpose().allows_comparison()
            && seconds == 30
            && !plan.positive()
            && before["production_aec_route_eligible"] == true
            && before["input"]["transport"]["Ok"] == kAudioDeviceTransportTypeBuiltIn
    );
    manifest["rates_unchanged"] = json!(rates_unchanged);
    manifest["route_stable"] = json!(stable);
    let mut mic_resampler = souffle_lib::audio::Resampler::new(rate, 1, MIX_RATE, 1.0);
    let mut me = mic_resampler.process(&mic);
    me.extend(mic_resampler.flush());
    let mut tap_resampler = souffle_lib::audio::Resampler::new(tap_rate, 1, MIX_RATE, 1.0);
    let mut them = tap_resampler.process(&tap);
    them.extend(tap_resampler.flush());
    protocol::write_wav(&trial.join(Track::Them.filename()), &them, MIX_RATE)?;
    let mut files = vec![Track::MicNative, Track::TapNative, Track::Them];
    match mode {
        Mode::Baseline => {
            protocol::write_wav(&trial.join(Track::Raw.filename()), &me, MIX_RATE)?;
            let eligible = before["production_aec_route_eligible"]
                .as_bool()
                .unwrap_or(false);
            manifest["fdaf_engaged"] = json!(eligible);
            let replay_start = Instant::now();
            let replay_usage = usage();
            let fdaf = replay(&me, &them, eligible);
            manifest["fdaf_offline_cost"] = json!({"wall_seconds": replay_start.elapsed().as_secs_f64(), "cpu_seconds": cpu(&usage()) - cpu(&replay_usage), "note": "offline DSP cost, not live capture CPU"});
            protocol::write_wav(&trial.join(Track::Fdaf.filename()), &fdaf, MIX_RATE)?;
            files.extend([Track::Raw, Track::Fdaf]);
        }
        Mode::Apple => {
            protocol::write_wav(&trial.join(Track::Apple.filename()), &me, MIX_RATE)?;
            files.push(Track::Apple);
        }
    }
    for track in files {
        let file = track.filename();
        manifest["files"][&file] = json!(protocol::hash(&trial.join(&file))?);
    }
    manifest["valid"] = json!(valid);
    if !valid {
        return Err("invalid trial: drops/errors, incomplete PCM, changed route or shared rate; inspect manifest and repeat under a new directory".into());
    }
    eprintln!(
        "Saved {} (physical listening and Standard-mode checks still required)",
        trial.display()
    );
    Ok(())
}

/// Reuse the actual production mixer, including tap-energy bypass and crossfade.
/// Raw and FDAF get identical input samples, never an Apple-processed microphone.
fn replay(mic: &[f32], tap: &[f32], engage: bool) -> Vec<f32> {
    let (mut mp, mc) = HeapRb::new(FRAME_SAMPLES).split();
    let (mut tp, tc) = HeapRb::new(FRAME_SAMPLES).split();
    let mut mixer = MeetingMixer::new(mc, MIX_RATE, 1, 1.0, tc, MIX_RATE, MIX_RATE);
    if engage {
        mixer.set_aec(Some(Aec::new_with_default_delay_hint(MIX_RATE)));
    }
    let mut out = Vec::new();
    for (i, frame) in mic.chunks(FRAME_SAMPLES).enumerate() {
        mp.push_slice(frame);
        let start = i * FRAME_SAMPLES;
        let end = (start + frame.len()).min(tap.len());
        if start < end {
            tp.push_slice(&tap[start..end]);
        }
        let (me, _) = mixer.tick_split();
        out.extend(me);
    }
    out.extend(mixer.flush_split().0);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bypass_replay_preserves_partial_tail() {
        let mic: Vec<f32> = (0..1003).map(|i| i as f32 / 2000.0).collect();
        assert_eq!(replay(&mic, &vec![0.0; mic.len()], false), mic);
    }
    #[test]
    fn private_trial_refuses_overwrite_and_restricts_access() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let trial = root.path().join("trial");
        private_trial(&trial).unwrap();
        std::fs::write(trial.join("keep"), "original").unwrap();
        assert_eq!(
            std::fs::metadata(&trial).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert!(private_trial(&trial).is_err());
        assert_eq!(
            std::fs::read_to_string(trial.join("keep")).unwrap(),
            "original"
        );
    }
}
