//! Compressed on-disk recording of meeting audio (opt-in — see
//! `settings::MeetingAudioRetention`). Encoding and file I/O run on a
//! dedicated writer thread fed by a bounded channel, so the realtime
//! audio-capture thread (`capture::AudioCapture`) never blocks on codec or
//! disk work: `MeetingRecorder::push` is a `try_send`, and a full channel
//! just drops the chunk (counted, logged at session end).
//!
//! Container: Ogg, built by hand (OpusHead/OpusTags packets plus one Opus
//! packet per 20ms frame, with granule positions expressed as 48kHz-
//! equivalent samples per RFC 7845) via the `ogg` crate's `PacketWriter`.
//! Verified to load, report correct duration, and seek correctly in WebKit
//! (Safari on macOS 15.6, the same engine Tauri's WKWebView embeds).

use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{SyncSender, sync_channel};
use std::thread::JoinHandle;

use ogg::writing::{PacketWriteEndInfo, PacketWriter};
use opus::{Application, Channels, Encoder as OpusEncoder};

use crate::constants::app_data_dir;

/// Sample rates Opus accepts directly; anything else must be resampled to
/// the nearest one before encoding.
const OPUS_VALID_RATES: [u32; 5] = [8_000, 12_000, 16_000, 24_000, 48_000];

/// Frame duration used for encoding: the standard "good default" for voice,
/// small enough for low latency and large enough to amortize per-frame
/// overhead.
const FRAME_MS: u32 = 20;

/// Target bitrate for the voice-tuned Opus profile.
const BITRATE_BPS: i32 = 32_000;

/// Ogg logical-stream serial number. Each file holds exactly one stream, so
/// any constant works — it only has to be unique within the file.
const STREAM_SERIAL: u32 = 1;

/// Bounded channel capacity between the audio thread and the writer thread:
/// generous enough that a brief disk/encoder stall doesn't drop chunks
/// before the writer catches up, without letting a wedged writer thread
/// build up unbounded memory.
const CHANNEL_CAPACITY: usize = 256;

/// Comfortably above the largest Opus packet this encoder ever produces at
/// 32kbps/20ms frames; matches the size libopus's own examples use.
const MAX_OPUS_PACKET_BYTES: usize = 4000;

/// Pick the Opus-valid sample rate closest to `rate`.
fn nearest_opus_rate(rate: u32) -> u32 {
    OPUS_VALID_RATES
        .iter()
        .copied()
        .min_by_key(|candidate| candidate.abs_diff(rate))
        .unwrap_or(48_000)
}

/// Root directory for all meeting recordings.
pub fn recordings_root() -> PathBuf {
    app_data_dir().join("recordings")
}

/// Directory holding every recorded session file for one meeting.
pub fn meeting_recordings_dir(meeting_id: &str) -> PathBuf {
    recordings_root().join(meeting_id)
}

/// File path for one recording session within a meeting. `session_index` is
/// the position of that session in the meeting's `recording_sessions`.
pub fn session_path(meeting_id: &str, session_index: usize) -> PathBuf {
    meeting_recordings_dir(meeting_id).join(format!("{session_index}.ogg"))
}

fn opus_head(pre_skip: u16, input_rate: u32) -> Vec<u8> {
    let mut v = Vec::with_capacity(19);
    v.extend_from_slice(b"OpusHead");
    v.push(1); // version
    v.push(1); // channel count (mono)
    v.extend_from_slice(&pre_skip.to_le_bytes());
    v.extend_from_slice(&input_rate.to_le_bytes());
    v.extend_from_slice(&0i16.to_le_bytes()); // output gain
    v.push(0); // channel mapping family (mono/stereo, no extra table)
    v
}

fn opus_tags() -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(b"OpusTags");
    let vendor = b"souffle";
    v.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
    v.extend_from_slice(vendor);
    v.extend_from_slice(&0u32.to_le_bytes()); // no user comments
    v
}

/// Encodes mono f32 PCM into an Ogg Opus stream. Buffers samples into exact
/// `FRAME_MS` frames (Opus requires one of a handful of exact frame sizes)
/// and tracks the RFC 7845 granule position (48kHz-equivalent samples,
/// regardless of the actual encode rate) so players report correct duration
/// and can seek.
struct OggOpusWriter<W: Write> {
    encoder: OpusEncoder,
    packet_writer: PacketWriter<'static, W>,
    input_rate: u32,
    frame_samples: usize,
    pending: Vec<f32>,
    granule_position: u64,
    packets_written: u64,
    finished: bool,
}

impl<W: Write> OggOpusWriter<W> {
    fn new(writer: W, input_rate: u32) -> Result<Self, String> {
        let mut encoder = OpusEncoder::new(input_rate, Channels::Mono, Application::Voip)
            .map_err(|e| format!("Create Opus encoder: {e}"))?;
        encoder
            .set_bitrate(opus::Bitrate::Bits(BITRATE_BPS))
            .map_err(|e| format!("Set Opus bitrate: {e}"))?;
        let lookahead = encoder
            .get_lookahead()
            .map_err(|e| format!("Read Opus lookahead: {e}"))?;
        // Pre-skip is always expressed in 48kHz-equivalent samples per RFC
        // 7845, regardless of the actual input/encode rate.
        let pre_skip = ((lookahead as u64) * 48_000 / u64::from(input_rate)) as u16;

        let mut packet_writer = PacketWriter::new(writer);
        packet_writer
            .write_packet(
                opus_head(pre_skip, input_rate),
                STREAM_SERIAL,
                PacketWriteEndInfo::EndPage,
                0,
            )
            .map_err(|e| format!("Write OpusHead: {e}"))?;
        packet_writer
            .write_packet(opus_tags(), STREAM_SERIAL, PacketWriteEndInfo::EndPage, 0)
            .map_err(|e| format!("Write OpusTags: {e}"))?;

        let frame_samples = (input_rate * FRAME_MS / 1000) as usize;

        Ok(Self {
            encoder,
            packet_writer,
            input_rate,
            frame_samples,
            pending: Vec::with_capacity(frame_samples * 2),
            granule_position: 0,
            packets_written: 0,
            finished: false,
        })
    }

    fn encode_and_write(&mut self, frame: &[f32], end_info: PacketWriteEndInfo) -> Result<(), String> {
        let mut out_buf = [0u8; MAX_OPUS_PACKET_BYTES];
        let len = self
            .encoder
            .encode_float(frame, &mut out_buf)
            .map_err(|e| format!("Opus encode: {e}"))?;
        self.granule_position += (self.frame_samples as u64) * 48_000 / u64::from(self.input_rate);
        self.packet_writer
            .write_packet(out_buf[..len].to_vec(), STREAM_SERIAL, end_info, self.granule_position)
            .map_err(|e| format!("Write Opus packet: {e}"))?;
        self.packets_written += 1;
        Ok(())
    }

    fn write_chunk(&mut self, samples: &[f32]) -> Result<(), String> {
        self.pending.extend_from_slice(samples);
        while self.pending.len() >= self.frame_samples {
            let frame: Vec<f32> = self.pending.drain(..self.frame_samples).collect();
            self.encode_and_write(&frame, PacketWriteEndInfo::NormalPacket)?;
        }
        Ok(())
    }

    /// Flush any partial frame (zero-padded) and mark the Ogg stream ended.
    /// Idempotent — a second call is a no-op, so callers don't have to track
    /// whether finalize already ran.
    fn finish(&mut self) -> Result<(), String> {
        if self.finished {
            return Ok(());
        }
        self.finished = true;

        if !self.pending.is_empty() {
            let mut frame = std::mem::take(&mut self.pending);
            frame.resize(self.frame_samples, 0.0);
            self.encode_and_write(&frame, PacketWriteEndInfo::EndStream)?;
        } else if self.packets_written > 0 {
            // Every audio packet so far was a full frame with no pending
            // tail; the Ogg stream still needs one page carrying the
            // end-of-stream flag, so close it out with a frame of silence.
            let silence = vec![0.0f32; self.frame_samples];
            self.encode_and_write(&silence, PacketWriteEndInfo::EndStream)?;
        }
        // No audio was ever written (near-instant start/stop): the file has
        // only OpusHead/OpusTags and no EndStream page. Acceptable — this is
        // the same "truncated but structurally valid" tolerance as a crash.

        self.packet_writer
            .inner_mut()
            .flush()
            .map_err(|e| format!("Flush recording file: {e}"))
    }
}

enum RecorderMsg {
    Chunk(Vec<f32>),
}

/// Feeds mono f32 PCM to a background writer thread that encodes it to an
/// Ogg Opus file. One instance per recording session (see
/// `capture::AudioCapture`'s recorder field — it survives mid-session
/// capture rebuilds by session id, matching the session lifecycle).
pub struct MeetingRecorder {
    session_id: u64,
    sender: Option<SyncSender<RecorderMsg>>,
    handle: Option<JoinHandle<()>>,
    dropped: Arc<AtomicU64>,
}

impl MeetingRecorder {
    /// Start recording `session_id`'s audio to `path`, at the nearest
    /// Opus-valid rate to `sample_rate` (resampling on the writer thread if
    /// they differ — never on the caller's, presumably realtime, thread).
    pub fn start(path: PathBuf, sample_rate: u32, session_id: u64) -> Result<Self, String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("Create recordings dir: {e}"))?;
        }
        let encode_rate = nearest_opus_rate(sample_rate);

        let file = std::fs::File::create(&path).map_err(|e| format!("Create recording file: {e}"))?;
        let mut writer = OggOpusWriter::new(std::io::BufWriter::new(file), encode_rate)?;

        let (tx, rx) = sync_channel::<RecorderMsg>(CHANNEL_CAPACITY);
        let dropped = Arc::new(AtomicU64::new(0));

        let handle = std::thread::Builder::new()
            .name("meeting-recorder".into())
            .spawn(move || {
                let mut resampler = (encode_rate != sample_rate)
                    .then(|| super::resampler::Resampler::new(sample_rate, 1, encode_rate, 1.0));

                while let Ok(RecorderMsg::Chunk(samples)) = rx.recv() {
                    let owned;
                    let encode_samples: &[f32] = match resampler.as_mut() {
                        Some(r) => {
                            owned = r.process(&samples);
                            &owned
                        }
                        None => &samples,
                    };
                    if let Err(e) = writer.write_chunk(encode_samples) {
                        tracing::warn!("Meeting recorder encode error: {e}");
                    }
                }

                if let Some(r) = resampler.as_mut() {
                    let tail = r.flush();
                    if !tail.is_empty()
                        && let Err(e) = writer.write_chunk(&tail)
                    {
                        tracing::warn!("Meeting recorder tail flush error: {e}");
                    }
                }
                if let Err(e) = writer.finish() {
                    tracing::warn!("Meeting recorder finalize error: {e}");
                }
            })
            .map_err(|e| format!("Spawn meeting recorder thread: {e}"))?;

        Ok(Self {
            session_id,
            sender: Some(tx),
            handle: Some(handle),
            dropped,
        })
    }

    pub fn session_id(&self) -> u64 {
        self.session_id
    }

    /// Queue a chunk for encoding. Non-blocking: a full channel drops the
    /// chunk (and counts it) instead of ever stalling the realtime
    /// audio-capture thread that calls this.
    pub fn push(&self, samples: &[f32]) {
        if samples.is_empty() {
            return;
        }
        if let Some(sender) = &self.sender
            && sender.try_send(RecorderMsg::Chunk(samples.to_vec())).is_err()
        {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn dropped_chunks(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

impl Drop for MeetingRecorder {
    fn drop(&mut self) {
        // Dropping the sender closes the channel, so the writer thread's
        // `recv()` returns and it finalizes (flush + close) before exiting.
        // This runs whenever a `MeetingRecorder` goes out of scope — normal
        // stop, a session end after mic loss, or stack unwinding from a
        // caught panic (`panic = "unwind"`) — so every teardown path closes
        // the file. Only a hard crash (SIGKILL/abort) skips it, leaving a
        // truncated but structurally valid Ogg file.
        self.sender.take();
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(seconds: f64, rate: u32) -> Vec<f32> {
        let n = (seconds * f64::from(rate)) as usize;
        (0..n)
            .map(|i| ((2.0 * std::f64::consts::PI * 440.0 * i as f64 / f64::from(rate)).sin() * 0.3) as f32)
            .collect()
    }

    #[test]
    fn nearest_opus_rate_snaps_to_valid_values() {
        assert_eq!(nearest_opus_rate(48_000), 48_000);
        assert_eq!(nearest_opus_rate(24_000), 24_000);
        assert_eq!(nearest_opus_rate(16_000), 16_000);
        assert_eq!(nearest_opus_rate(44_100), 48_000);
        assert_eq!(nearest_opus_rate(22_050), 24_000);
    }

    #[test]
    fn encodes_sine_to_valid_nonempty_ogg_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("out.ogg");
        let file = std::fs::File::create(&path).expect("create");
        let mut writer = OggOpusWriter::new(std::io::BufWriter::new(file), 24_000).expect("writer");

        let samples = sine(2.0, 24_000);
        writer.write_chunk(&samples).expect("write");
        writer.finish().expect("finish");

        let bytes = std::fs::read(&path).expect("read back");
        assert!(!bytes.is_empty());
        assert_eq!(&bytes[0..4], b"OggS", "file must start with an Ogg page");
        // Granule position should reflect ~2s at 48kHz-equivalent samples.
        assert!(writer.granule_position >= 95_000 && writer.granule_position <= 97_000);
    }

    #[test]
    fn finish_is_idempotent() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("out.ogg");
        let file = std::fs::File::create(&path).expect("create");
        let mut writer = OggOpusWriter::new(std::io::BufWriter::new(file), 24_000).expect("writer");

        writer.write_chunk(&sine(0.5, 24_000)).expect("write");
        writer.finish().expect("finish once");
        let granule_after_first = writer.granule_position;
        writer.finish().expect("finish twice must not error");
        assert_eq!(writer.granule_position, granule_after_first, "second finish must be a no-op");
    }

    #[test]
    fn finish_with_no_audio_does_not_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("empty.ogg");
        let file = std::fs::File::create(&path).expect("create");
        let mut writer = OggOpusWriter::new(std::io::BufWriter::new(file), 16_000).expect("writer");
        writer.finish().expect("finish with no audio");
    }

    #[test]
    fn recorder_encodes_end_to_end_and_closes_file_on_drop() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("session.ogg");

        let recorder = MeetingRecorder::start(path.clone(), 24_000, 1).expect("start");
        assert_eq!(recorder.session_id(), 1);
        for _ in 0..5 {
            recorder.push(&sine(0.1, 24_000));
        }
        drop(recorder); // joins the writer thread, finalizing the file

        let bytes = std::fs::read(&path).expect("recording file must exist");
        assert!(!bytes.is_empty());
        assert_eq!(&bytes[0..4], b"OggS");
    }

    /// Exercises the exact `try_send`-or-count-a-drop mechanism `push` uses,
    /// against a rendezvous channel (capacity 0) with no receiver draining
    /// it: every send is guaranteed to fail immediately, so this is
    /// deterministic (unlike starving a real writer thread, which races the
    /// scheduler) and never blocks.
    #[test]
    fn full_channel_drops_chunks_without_blocking() {
        let (tx, _rx) = sync_channel::<RecorderMsg>(0);
        let dropped = Arc::new(AtomicU64::new(0));

        let push = |samples: &[f32]| {
            if tx.try_send(RecorderMsg::Chunk(samples.to_vec())).is_err() {
                dropped.fetch_add(1, Ordering::Relaxed);
            }
        };

        for _ in 0..10 {
            push(&[0.1, 0.2, 0.3]);
        }

        assert_eq!(
            dropped.load(Ordering::Relaxed),
            10,
            "an undrained rendezvous channel must drop every chunk, never block"
        );
    }
}
