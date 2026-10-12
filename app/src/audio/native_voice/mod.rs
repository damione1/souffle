//! AVAudioEngine owns the real input/output device. All configuration happens
//! while stopped. Input tap only copies to a bounded ring; source renders either
//! silence or immutable fixture PCM, never the global system tap.
mod audio_unit;
pub mod devices;
use audio_unit::audio_unit;
use block2::RcBlock;
use devices::{Device, Selection, Side};
use objc2::{
    AnyThread,
    rc::Retained,
    runtime::{Bool, NSObjectProtocol},
    sel,
};
use objc2_audio_toolbox::{
    AudioUnit, AudioUnitGetProperty, AudioUnitSetProperty, kAudioOutputUnitProperty_CurrentDevice,
    kAudioUnitScope_Global,
};
use objc2_avf_audio::*;
use objc2_core_audio_types::{AudioBufferList, AudioTimeStamp};
use ringbuf::{HeapProd, traits::Producer};
use serde_json::{Value, json};
use std::{
    ptr::NonNull,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
};

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Clone, Copy, Default)]
pub struct Sample {
    pub value: f32,
    pub host: u64,
}

#[derive(Default)]
pub struct Telemetry {
    pub callbacks: AtomicU64,
    pub dropped: AtomicU64,
    pub errors: AtomicU64,
    pub accepting: AtomicBool,
}

/// The probe preserves host timestamps; the meeting mixer consumes mono PCM.
/// Both destinations are bounded rings and have no blocking push operation.
pub enum Sink {
    Timestamped(HeapProd<Sample>),
    Mono(HeapProd<f32>),
}
impl Sink {
    fn push(&mut self, value: f32, host: u64) -> bool {
        match self {
            Self::Timestamped(producer) => producer.try_push(Sample { value, host }).is_ok(),
            Self::Mono(producer) => producer.try_push(value).is_ok(),
        }
    }
}

pub struct Native {
    engine: Retained<AVAudioEngine>,
    input: Retained<AVAudioInputNode>,
    pub rate: u32,
    pub evidence: Value,
    pub rendered: Arc<AtomicUsize>,
    telemetry: Arc<Telemetry>,
}

#[derive(Clone, Copy)]
enum ReadbackPhase {
    BeforeStart,
    AfterStart,
    Running,
}
impl ReadbackPhase {
    fn name(self) -> &'static str {
        match self {
            Self::BeforeStart => "before_start",
            Self::AfterStart => "after_start",
            Self::Running => "running",
        }
    }
}
unsafe fn read_device(
    unit: AudioUnit,
    side: Side,
    expected: &Device,
    phase: ReadbackPhase,
    log: &mut Value,
) -> Result<()> {
    if unit.is_null() {
        return Err(format!("{} AudioUnit missing", side.name()).into());
    }
    let mut id = 0u32;
    let mut size = size_of::<u32>() as u32;
    let status = unsafe {
        AudioUnitGetProperty(
            unit,
            kAudioOutputUnitProperty_CurrentDevice,
            kAudioUnitScope_Global,
            side.bus(),
            NonNull::from(&mut id).cast(),
            NonNull::from(&mut size),
        )
    };
    let entry = &mut log["devices"][side.name()][phase.name()];
    *entry = json!({"get_status": status, "id": id, "size": size, "bus": side.bus()});
    if status != 0 || size != size_of::<u32>() as u32 {
        return Err(format!(
            "{} CurrentDevice readback: OSStatus {status}, size {size}",
            side.name()
        )
        .into());
    }
    let uid = crate::audio::output_route::device_uid(id);
    entry["uid"] = json!(uid);
    devices::confirm(expected, id, &uid?)
}
unsafe fn select_device(
    unit: AudioUnit,
    side: Side,
    expected: &Device,
    log: &mut Value,
) -> Result<()> {
    log["devices"][side.name()]["requested"] = json!(expected);
    if unit.is_null() {
        return Err(format!(
            "{} AudioUnit missing after enabling voice processing",
            side.name()
        )
        .into());
    }
    // VPIO CurrentDevice uses global scope: microphone bus 1, speaker bus 0.
    // Only this stopped AudioUnit is changed, never CoreAudio's global defaults.
    let status = unsafe {
        AudioUnitSetProperty(
            unit,
            kAudioOutputUnitProperty_CurrentDevice,
            kAudioUnitScope_Global,
            side.bus(),
            std::ptr::from_ref(&expected.id).cast(),
            size_of::<u32>() as u32,
        )
    };
    log["devices"][side.name()]["set_status"] = json!(status);
    if status != 0 {
        return Err(format!("{} CurrentDevice selection: OSStatus {status}", side.name()).into());
    }
    unsafe { read_device(unit, side, expected, ReadbackPhase::BeforeStart, log) }
}
impl Native {
    /// Called by the owning capture thread, never by an audio callback.
    /// Any route/format change requires a stopped-engine reconstruction.
    pub fn check_route(&self, expected: &Selection, log: &mut Value) -> Result<()> {
        use objc2_core_audio::{
            kAudioDevicePropertyNominalSampleRate, kAudioHardwarePropertyDefaultOutputDevice,
            kAudioObjectSystemObject,
        };
        let output_id = devices::global_u32(
            kAudioObjectSystemObject as u32,
            kAudioHardwarePropertyDefaultOutputDevice,
        )?;
        if output_id != expected.output.id {
            return Err("system output changed; native capture needs reconstruction".into());
        }
        for device in [&expected.input, &expected.output] {
            let rate = devices::global_f64(device.id, kAudioDevicePropertyNominalSampleRate)?;
            log["nominal_rate_readback"][&device.uid] = json!(rate);
            if rate != device.nominal_rate {
                return Err(format!(
                    "device rate changed for {:?}: {} -> {rate}",
                    device.uid, device.nominal_rate
                )
                .into());
            }
        }
        unsafe {
            let output = self.engine.outputNode();
            if !self.engine.isRunning()
                || !self.input.isVoiceProcessingEnabled()
                || !output.isVoiceProcessingEnabled()
                || self.input.isVoiceProcessingBypassed()
                || self.input.isVoiceProcessingAGCEnabled()
                || self.input.outputFormatForBus(0).sampleRate() != self.rate as f64
            {
                return Err("native running configuration changed".into());
            }
            read_device(
                audio_unit(&self.input)?,
                Side::Input,
                &expected.input,
                ReadbackPhase::Running,
                log,
            )?;
            read_device(
                audio_unit(&output)?,
                Side::Output,
                &expected.output,
                ReadbackPhase::Running,
                log,
            )
        }
    }

    pub fn start(
        producer: Sink,
        telemetry: Arc<Telemetry>,
        playback: Option<(Vec<f32>, u32)>,
        selection: Option<&Selection>,
        log: &mut Value,
    ) -> Result<Self> {
        // Engine retains nodes/blocks; Drop stops IO before releasing them.
        unsafe {
            let engine = AVAudioEngine::new();
            let input = engine.inputNode();
            let enabled = input.setVoiceProcessingEnabled_error(true);
            log["setVoiceProcessingEnabled"] = match &enabled {
                Ok(()) => json!({"success": true}),
                Err(e) => json!({"success": false, "error": format!("{e:?}")}),
            };
            enabled.map_err(|e| format!("setVoiceProcessingEnabled: {e:?}"))?;
            // Voice processing may replace the underlying units. Reacquire the
            // nodes/units after the switch and bind devices before prepare/start.
            let input = engine.inputNode();
            let output = engine.outputNode();
            if let Some(s) = selection {
                select_device(audio_unit(&input)?, Side::Input, &s.input, log)?;
                select_device(audio_unit(&output)?, Side::Output, &s.output, log)?;
                // Selecting the output must not silently undo the input choice.
                read_device(
                    audio_unit(&input)?,
                    Side::Input,
                    &s.input,
                    ReadbackPhase::BeforeStart,
                    log,
                )?;
            }
            input.setVoiceProcessingBypassed(false);
            input.setVoiceProcessingAGCEnabled(false);
            let mut ducking = json!({"available": false, "requested": "minimum; advanced=false"});
            if input.respondsToSelector(sel!(setVoiceProcessingOtherAudioDuckingConfiguration:)) {
                input.setVoiceProcessingOtherAudioDuckingConfiguration(
                    AVAudioVoiceProcessingOtherAudioDuckingConfiguration {
                        enableAdvancedDucking: Bool::NO,
                        duckingLevel: AVAudioVoiceProcessingOtherAudioDuckingLevel::Min,
                    },
                );
                let actual = input.voiceProcessingOtherAudioDuckingConfiguration();
                ducking = json!({"available": true, "advanced": actual.enableAdvancedDucking.as_bool(), "level": actual.duckingLevel.0});
                log["ducking_readback"] = ducking.clone();
                if actual.enableAdvancedDucking.as_bool()
                    || actual.duckingLevel != AVAudioVoiceProcessingOtherAudioDuckingLevel::Min
                {
                    return Err("minimum ducking readback did not match request".into());
                }
            }
            let format = input.outputFormatForBus(0);
            let rate = format.sampleRate();
            let channels = format.channelCount();
            let output_format = output.inputFormatForBus(0);
            let output_rate = output_format.sampleRate();
            let mut evidence = json!({
                "input_voice_processing": input.isVoiceProcessingEnabled(),
                "output_voice_processing": output.isVoiceProcessingEnabled(),
                "bypassed": input.isVoiceProcessingBypassed(), "agc": input.isVoiceProcessingAGCEnabled(),
                "ducking": ducking, "input_rate": rate, "input_channels": channels,
                "input_interleaved": format.isInterleaved(), "input_common_format": format.commonFormat().0,
                "output_rate": output_rate, "output_channels": output_format.channelCount(),
                "input_presentation_latency_seconds": input.presentationLatency(),
                "output_presentation_latency_seconds": output.presentationLatency(),
                "output_content": if playback.is_some() { "positive-control fixture, once then silence" } else { "silence (external process supplies audible remote)" },
            });
            log["configuration"] = evidence.clone();
            if !input.isVoiceProcessingEnabled()
                || !output.isVoiceProcessingEnabled()
                || input.isVoiceProcessingBypassed()
                || input.isVoiceProcessingAGCEnabled()
            {
                return Err(
                    "native readback did not confirm both voice IO nodes, unbypassed, AGC off"
                        .into(),
                );
            }
            if !format.isStandard()
                || format.isInterleaved()
                || channels != 1
                || !(8000.0..=192000.0).contains(&rate)
                || !(8000.0..=192000.0).contains(&output_rate)
            {
                return Err(
                    "unsupported negotiated native format (requires mono planar f32)".into(),
                );
            }
            let retained_telemetry = Arc::clone(&telemetry);
            let render_telemetry = Arc::clone(&telemetry);
            let producer = Mutex::new(producer);
            let block = RcBlock::new(
                move |buffer: NonNull<AVAudioPCMBuffer>, time: NonNull<AVAudioTime>| {
                    let b = buffer.as_ref();
                    let n = b.frameLength() as usize;
                    telemetry.callbacks.fetch_add(1, Ordering::Relaxed);
                    if !telemetry.accepting.load(Ordering::Relaxed) {
                        return;
                    }
                    let t = time.as_ref();
                    if !t.isHostTimeValid() {
                        telemetry.errors.fetch_add(1, Ordering::Relaxed);
                        return;
                    }
                    let timestamp = t.hostTime();
                    let data = b.floatChannelData();
                    if data.is_null() {
                        telemetry.errors.fetch_add(1, Ordering::Relaxed);
                        return;
                    }
                    let Ok(mut p) = producer.try_lock() else {
                        telemetry.dropped.fetch_add(n as u64, Ordering::Relaxed);
                        return;
                    };
                    // Format was fixed at install; channel pointers valid for frameLength.
                    for i in 0..n {
                        let mut value = 0.0;
                        for ch in 0..channels as usize {
                            value += *(*data.add(ch)).as_ptr().add(i);
                        }
                        if !p.push(value / channels as f32, timestamp) {
                            telemetry.dropped.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                },
            );
            input.installTapOnBus_bufferSize_format_block(
                0,
                1024,
                Some(&format),
                RcBlock::as_ptr(&block),
            );
            let rendered = Arc::new(AtomicUsize::new(0));
            let position = Arc::clone(&rendered);
            // Apple requires input-node output == output-node input format.
            // Use the negotiated microphone format while the engine is stopped;
            // this changes a client stream format, never the device nominal rate.
            let source_format = format.clone();
            let pcm = match playback {
                Some((samples, source_rate)) => {
                    let mut r = crate::audio::Resampler::new(source_rate, 1, rate as u32, 1.0);
                    let mut out = r.process(&samples);
                    out.extend(r.flush());
                    out
                }
                None => Vec::new(),
            };
            let render = RcBlock::new(
                move |mut silence: NonNull<Bool>,
                      _time: NonNull<AudioTimeStamp>,
                      count: u32,
                      mut data: NonNull<AudioBufferList>|
                      -> i32 {
                    let active = render_telemetry.accepting.load(Ordering::Relaxed);
                    let start = if active {
                        position.fetch_add(count as usize, Ordering::Relaxed)
                    } else {
                        usize::MAX
                    };
                    *silence.as_mut() = Bool::new(start >= pcm.len());
                    let list = data.as_mut();
                    // Explicit mono format; reject unexpected buffer layouts safely.
                    if list.mNumberBuffers != 1 {
                        render_telemetry.errors.fetch_add(1, Ordering::Relaxed);
                        return -50;
                    }
                    let b = &mut list.mBuffers[0];
                    if b.mData.is_null() || b.mDataByteSize < count * 4 {
                        render_telemetry.errors.fetch_add(1, Ordering::Relaxed);
                        return -50;
                    }
                    let out = std::slice::from_raw_parts_mut(b.mData.cast::<f32>(), count as usize);
                    for (i, sample) in out.iter_mut().enumerate() {
                        *sample = pcm.get(start.saturating_add(i)).copied().unwrap_or(0.0);
                    }
                    0
                },
            );
            let source = AVAudioSourceNode::initWithFormat_renderBlock(
                AVAudioSourceNode::alloc(),
                &source_format,
                RcBlock::as_ptr(&render),
            );
            engine.attachNode(&source);
            engine.connect_to_format(&source, &output, Some(&source_format));
            let connected_output = output.inputFormatForBus(0);
            let formats_equal = format.isEqual(&connected_output);
            evidence["connected_output"] = json!({"rate": connected_output.sampleRate(), "channels": connected_output.channelCount(), "matches_input_format": formats_equal});
            log["configuration"] = evidence.clone();
            if !formats_equal {
                input.removeTapOnBus(0);
                return Err("voice-processing input/output formats differ after connection".into());
            }
            engine.prepare();

            let started = engine.startAndReturnError();
            log["startAndReturnError"] = match &started {
                Ok(()) => json!({"success": true}),
                Err(e) => json!({"success": false, "error": format!("{e:?}")}),
            };
            if let Err(e) = started {
                engine.stop();
                input.removeTapOnBus(0);
                return Err(format!("startAndReturnError: {e:?}").into());
            }
            log["running_readback"] = serde_json::json!({"running": engine.isRunning(), "input_voice_processing": input.isVoiceProcessingEnabled(), "output_voice_processing": output.isVoiceProcessingEnabled(), "agc": input.isVoiceProcessingAGCEnabled(), "bypassed": input.isVoiceProcessingBypassed()});
            if let Some(s) = selection {
                let selected = (|| {
                    read_device(
                        audio_unit(&input)?,
                        Side::Input,
                        &s.input,
                        ReadbackPhase::AfterStart,
                        log,
                    )?;
                    read_device(
                        audio_unit(&output)?,
                        Side::Output,
                        &s.output,
                        ReadbackPhase::AfterStart,
                        log,
                    )
                })();
                if let Err(error) = selected {
                    engine.stop();
                    input.removeTapOnBus(0);
                    return Err(error);
                }
            }
            if !engine.isRunning()
                || !input.isVoiceProcessingEnabled()
                || !output.isVoiceProcessingEnabled()
                || input.isVoiceProcessingAGCEnabled()
                || input.isVoiceProcessingBypassed()
            {
                engine.stop();
                input.removeTapOnBus(0);
                return Err("native post-start readback failed".into());
            }
            evidence["input_presentation_latency_seconds"] = json!(input.presentationLatency());
            evidence["output_presentation_latency_seconds"] = json!(output.presentationLatency());
            let capture = Self {
                engine,
                input,
                rate: rate as u32,
                evidence,
                rendered,
                telemetry: retained_telemetry,
            };
            if let Some(selection) = selection {
                capture.check_route(selection, log)?;
            }
            Ok(capture)
        }
    }
}
impl Drop for Native {
    fn drop(&mut self) {
        unsafe {
            self.telemetry.accepting.store(false, Ordering::Release);
            self.engine.stop();
            self.input.removeTapOnBus(0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ringbuf::{
        HeapRb,
        traits::{Consumer, Split},
    };

    #[test]
    fn native_destinations_preserve_me_samples_and_bound_overflow() {
        let (prod, mut cons) = HeapRb::new(2).split();
        let mut mic = Sink::Mono(prod);
        assert!(mic.push(0.25, 100));
        assert!(mic.push(-0.5, 101));
        assert!(!mic.push(0.75, 102));
        assert_eq!(cons.try_pop(), Some(0.25));
        assert_eq!(cons.try_pop(), Some(-0.5));
        assert_eq!(cons.try_pop(), None);

        let (prod, mut cons) = HeapRb::new(1).split();
        let mut probe = Sink::Timestamped(prod);
        assert!(probe.push(0.25, 123));
        assert!(!probe.push(-0.5, 124));
        let sample = cons.try_pop().unwrap();
        assert_eq!((sample.value, sample.host), (0.25, 123));
    }
}
