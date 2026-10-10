//! Streaming AEC replay of the lossless pre-AEC diagnostic artifact.
//! Real meetings have no isolated near-end ground truth: the reported ERLE
//! fields are explicitly mic/output energy attenuation proxies. Synthetic
//! fixtures can supply known voice to the same metric evaluator instead.

use std::collections::VecDeque;
use std::path::Path;

use ringbuf::HeapRb;
use ringbuf::traits::{Producer, Split};

use super::aec::Aec;
use super::mixer::{FRAME_SAMPLES, MIX_RATE, MeetingMixer};

const WINDOW_SAMPLES: usize = MIX_RATE as usize * 8;
const LAG_STEP: usize = 16;
const MAX_LAG: usize = MIX_RATE as usize / 5;

#[derive(Debug, serde::Serialize, PartialEq)]
pub struct ReplayMetrics {
    pub sample_rate: u32,
    pub captured_frames: u64,
    pub measured_frames: usize,
    /// Input-to-output lag, not acoustic render-to-mic delay. Null means
    /// silence/uncorrelated signals prevented an estimate.
    pub estimated_delay_samples: Option<usize>,
    pub raw_erle_db: f64,
    pub compensated_erle_db: f64,
    pub metric_definition: &'static str,
}

/// Shared by synthetic fixtures (with known voice) and real replay (without).
pub(crate) fn measure(
    capture: &[f32],
    output: &[f32],
    voice: Option<&[f32]>,
    frames: u64,
) -> ReplayMetrics {
    let len = capture.len().min(output.len());
    let start = len * 3 / 4;
    let reference = voice.unwrap_or(capture);
    let mut best = (0.0f64, None);
    for lag in (0..=MAX_LAG.min(len / 4)).step_by(LAG_STEP) {
        let (mut dot, mut a, mut b) = (0.0f64, 0.0f64, 0.0f64);
        for i in (start..len.saturating_sub(lag)).step_by(LAG_STEP) {
            let x = f64::from(reference[i]);
            let y = f64::from(output[i + lag]);
            dot += x * y;
            a += x * x;
            b += y * y;
        }
        let score = dot / (a * b).sqrt().max(1e-20);
        if score > best.0 && score >= 0.1 {
            best = (score, Some(lag));
        }
    }
    let ratio = |lag: usize| {
        let (mut pre, mut post) = (0.0f64, 0.0f64);
        for i in start..len.saturating_sub(lag) {
            let near = voice.map_or(0.0, |v| f64::from(v[i]));
            pre += (f64::from(capture[i]) - near).powi(2);
            post += (f64::from(output[i + lag]) - near).powi(2);
        }
        10.0 * (pre.max(1e-20) / post.max(1e-20)).log10()
    };
    ReplayMetrics {
        sample_rate: MIX_RATE,
        captured_frames: frames,
        measured_frames: len - start,
        estimated_delay_samples: best.1,
        raw_erle_db: ratio(0),
        compensated_erle_db: ratio(best.1.unwrap_or(0)),
        metric_definition: match voice {
            Some(_) => {
                "Synthetic ground truth: 10 log10(E(capture - known voice) / E(output - known voice)); compensated aligns output to voice by estimated input/output lag."
            }
            None => {
                "Real meeting attenuation proxy, not echo-isolated ERLE or voice distortion: 10 log10(E(raw mic) / E(AEC mic output)). Compensated uses overlapping input/output samples aligned by estimated pipeline lag (16-sample resolution, 0..200 ms); final quarter of the last <=8 seconds; silence gives 0 dB and no delay estimate."
            }
        },
    }
}

struct Replay {
    mic: ringbuf::HeapProd<f32>,
    tap: ringbuf::HeapProd<f32>,
    mixer: MeetingMixer,
    capture: VecDeque<f32>,
    output: VecDeque<f32>,
    frames: u64,
}

fn retain_window(window: &mut VecDeque<f32>, samples: &[f32]) {
    window.extend(samples);
    while window.len() > WINDOW_SAMPLES {
        window.pop_front();
    }
}

impl Replay {
    fn new() -> Self {
        let (mic, mic_cons) = HeapRb::<f32>::new(FRAME_SAMPLES).split();
        let (tap, tap_cons) = HeapRb::<f32>::new(FRAME_SAMPLES).split();
        let mut mixer = MeetingMixer::new(mic_cons, MIX_RATE, 1, 1.0, tap_cons, MIX_RATE, MIX_RATE);
        mixer.set_aec(Some(Aec::new_with_default_delay_hint(MIX_RATE)));
        Self {
            mic,
            tap,
            mixer,
            capture: VecDeque::with_capacity(WINDOW_SAMPLES + FRAME_SAMPLES),
            output: VecDeque::with_capacity(WINDOW_SAMPLES + FRAME_SAMPLES),
            frames: 0,
        }
    }

    fn push(&mut self, left: &[f32], right: &[f32]) {
        assert_eq!(left.len(), right.len());
        assert_eq!(self.mic.push_slice(left), left.len());
        assert_eq!(self.tap.push_slice(right), right.len());
        retain_window(&mut self.capture, left);
        let (me, _) = self.mixer.tick_split();
        retain_window(&mut self.output, &me);
        self.frames += left.len() as u64;
    }

    fn finish(mut self) -> ReplayMetrics {
        let (tail, _) = self.mixer.flush_split();
        retain_window(&mut self.output, &tail);
        let capture: Vec<f32> = self.capture.into_iter().collect();
        let output: Vec<f32> = self.output.into_iter().collect();
        measure(&capture, &output, None, self.frames)
    }
}

/// The CLI reads only a complete diagnostic WAV. Parsing and replay allocate
/// a bounded <=8-second measurement window, independent of meeting duration.
pub fn replay_file(path: &Path) -> Result<ReplayMetrics, String> {
    if path
        .extension()
        .is_some_and(|extension| extension == "partial")
    {
        return Err("Unfinalized diagnostic: .partial files cannot be replayed".into());
    }
    let mut reader =
        hound::WavReader::open(path).map_err(|e| format!("Open diagnostic WAV: {e}"))?;
    let spec = reader.spec();
    if spec.channels != 2
        || spec.sample_rate != MIX_RATE
        || spec.bits_per_sample != 32
        || spec.sample_format != hound::SampleFormat::Float
    {
        return Err("Expected a lossless stereo float32 pre-AEC WAV at 48000 Hz (left mic, right system tap)".into());
    }
    if reader.len() == 0 || !reader.len().is_multiple_of(2) {
        return Err("Diagnostic WAV is empty or has unpaired stereo samples".into());
    }
    let mut replay = Replay::new();
    let mut samples = reader.samples::<f32>();
    loop {
        let (mut left, mut right) = ([0.0; FRAME_SAMPLES], [0.0; FRAME_SAMPLES]);
        let mut len = 0;
        while len < FRAME_SAMPLES {
            let Some(mic) = samples.next() else {
                break;
            };
            let mic = mic.map_err(|e| format!("Read microphone lane: {e}"))?;
            let tap = samples
                .next()
                .ok_or("Unpaired diagnostic stereo sample")?
                .map_err(|e| format!("Read tap lane: {e}"))?;
            if !mic.is_finite() || !tap.is_finite() {
                return Err("Non-finite diagnostic samples".into());
            }
            left[len] = mic;
            right[len] = tap;
            len += 1;
        }
        if len == 0 {
            break;
        }
        replay.push(&left[..len], &right[..len]);
    }
    Ok(replay.finish())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::diagnostic::{DiagnosticSession, session_path};

    #[test]
    fn captured_synthetic_stereo_replay_matches_direct_mixer_metrics() {
        let dir = tempfile::tempdir().unwrap();
        let primary = dir.path().join("0.ogg");
        let mut diagnostic = DiagnosticSession::default();
        diagnostic.sync(1, Some(&primary), true);
        diagnostic.confirm_start(1);
        let mut direct = Replay::new();
        direct.mixer.set_diagnostic(diagnostic.push_handle());
        let n = MIX_RATE as usize * 9 + 123;
        let mut seed = 0x9e3779b9u32;
        let render: Vec<f32> = (0..n)
            .map(|i| {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                ((i as f32 * 0.029).sin() * 0.2) + (seed as f32 / u32::MAX as f32 - 0.5) * 0.03
            })
            .collect();
        let capture: Vec<f32> = (0..n)
            .map(|i| {
                (i as f32 * 0.017).sin() * 0.1
                    + if i >= 2400 {
                        render[i - 2400] * 0.3
                    } else {
                        0.0
                    }
            })
            .collect();
        for (mic, tap) in capture
            .chunks(FRAME_SAMPLES)
            .zip(render.chunks(FRAME_SAMPLES))
        {
            direct.push(mic, tap);
        }
        assert_eq!(direct.capture.len(), WINDOW_SAMPLES);
        assert!(direct.output.len() <= WINDOW_SAMPLES);
        let expected = direct.finish();
        diagnostic.finish_and_wait();
        let actual = replay_file(&session_path(&primary)).unwrap();
        println!(
            "Captured synthetic AEC replay: {}",
            serde_json::to_string_pretty(&actual).unwrap()
        );
        assert!(actual.raw_erle_db.is_finite());
        assert!(actual.compensated_erle_db.is_finite());
        assert!(actual.estimated_delay_samples.is_some());
        assert_eq!(
            actual, expected,
            "actual lossless captured lanes reproduce direct mixer metrics exactly"
        );
        let mut reader = hound::WavReader::open(session_path(&primary)).unwrap();
        let written: Vec<f32> = reader.samples::<f32>().map(Result::unwrap).collect();
        assert_eq!(
            written,
            capture
                .iter()
                .zip(&render)
                .flat_map(|(l, r)| [*l, *r])
                .collect::<Vec<_>>(),
            "capture is raw before AEC, even with cancellation active"
        );
    }

    #[test]
    fn shared_metrics_compensate_known_fixture_delay_and_handle_silence() {
        let mut seed = 0x1234abcd_u32;
        let voice: Vec<f32> = (0..48000)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                seed as f32 / u32::MAX as f32 - 0.5
            })
            .collect();
        let capture: Vec<f32> = voice.iter().map(|v| v + 0.01).collect();
        let output: Vec<f32> = (0..voice.len())
            .map(|i| if i < 480 { 0.0 } else { voice[i - 480] + 0.001 })
            .collect();
        let metrics = measure(&capture, &output, Some(&voice), 48000);
        assert_eq!(metrics.estimated_delay_samples, Some(480));
        assert!(metrics.compensated_erle_db > 19.9);
        assert!(metrics.raw_erle_db < 0.0);
        let silence = measure(&[0.0; 960], &[0.0; 960], None, 960);
        assert_eq!(silence.estimated_delay_samples, None);
        assert_eq!(silence.raw_erle_db, 0.0);
        assert_eq!(silence.compensated_erle_db, 0.0);
    }

    #[test]
    fn rejects_unfinalized_truncated_and_wrong_layout_artifacts() {
        let dir = tempfile::tempdir().unwrap();
        assert!(
            replay_file(&dir.path().join("0.pre-aec.wav.partial"))
                .unwrap_err()
                .contains("Unfinalized")
        );
        let path = dir.path().join("bad.wav");
        std::fs::write(&path, b"RIFFtruncated").unwrap();
        assert!(replay_file(&path).is_err());
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: MIX_RATE,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let mut writer = hound::WavWriter::create(&path, spec).unwrap();
        writer.write_sample(0.1f32).unwrap();
        writer.finalize().unwrap();
        assert!(replay_file(&path).unwrap_err().contains("stereo"));
    }
}
