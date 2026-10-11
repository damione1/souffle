//! AVAudioEngine owns the real input/output device. All configuration happens
//! while stopped. Input tap only copies to a bounded ring; source renders either
//! silence or immutable fixture PCM, never the global system tap.
use crate::{
    Result,
    capture::{Sample, Telemetry},
};
use block2::RcBlock;
use objc2::{
    AnyThread,
    rc::Retained,
    runtime::{Bool, NSObjectProtocol},
    sel,
};
use objc2_avf_audio::*;
use objc2_core_audio_types::{AudioBufferList, AudioTimeStamp};
use ringbuf::{HeapProd, traits::Producer};
use serde_json::{Value, json};
use std::{
    ptr::NonNull,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

pub struct Native {
    engine: Retained<AVAudioEngine>,
    input: Retained<AVAudioInputNode>,
    pub rate: u32,
    pub evidence: Value,
    pub rendered: Arc<AtomicUsize>,
}
impl Native {
    pub fn start(
        producer: HeapProd<Sample>,
        telemetry: Arc<Telemetry>,
        playback: Option<(Vec<f32>, u32)>,
        log: &mut Value,
    ) -> Result<Self> {
        // Engine retains nodes/blocks; Drop stops IO before releasing them.
        unsafe {
            let engine = AVAudioEngine::new();
            let input = engine.inputNode();
            let output = engine.outputNode();
            let enabled = input.setVoiceProcessingEnabled_error(true);
            log["setVoiceProcessingEnabled"] = match &enabled {
                Ok(()) => json!({"success": true}),
                Err(e) => json!({"success": false, "error": format!("{e:?}")}),
            };
            enabled.map_err(|e| format!("setVoiceProcessingEnabled: {e:?}"))?;
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
                        if p.try_push(Sample {
                            value: value / channels as f32,
                            host: timestamp,
                        })
                        .is_err()
                        {
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
                    let mut r =
                        souffle_lib::audio::Resampler::new(source_rate, 1, rate as u32, 1.0);
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
            Ok(Self {
                engine,
                input,
                rate: rate as u32,
                evidence,
                rendered,
            })
        }
    }
}
impl Drop for Native {
    fn drop(&mut self) {
        unsafe {
            self.engine.stop();
            self.input.removeTapOnBus(0);
        }
    }
}
