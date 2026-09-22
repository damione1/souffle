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
//!
//! Channel layout (SOU-173): a diarized meeting is recorded as **stereo**
//! Opus, left = you (mic, post-AEC, exactly what the engine hears), right =
//! the other participants (system-audio tap). The lanes stay separable on
//! disk for offline diarization or debugging (`ffmpeg -map_channel 0.0.0` /
//! `0.0.1`), while the built-in player plays an L+R mono mix via
//! [`decode_ogg_opus`]. Non-diarized meetings and legacy files stay mono;
//! the decoder handles both. Stereo costs about 1.5x the disk of a mono
//! recording (48 kbps vs 32 kbps).

use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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

/// Target bitrate for the voice-tuned Opus profile (mono).
const BITRATE_BPS: i32 = 32_000;

/// Target bitrate for a stereo (diarized: L = me, R = them) recording. Each
/// lane carries one voice, so 24 kbps per lane is the same voice quality as
/// the mono profile; on disk a stereo recording costs ~1.5x a mono one
/// (about 6 MB/h at 48 kbps vs 4 MB/h at 32 kbps, before Ogg framing).
const STEREO_BITRATE_BPS: i32 = 48_000;

/// Sample rate every Opus decoder in this module runs at: the rate Opus's own
/// granule positions are always expressed in (RFC 7845), so decoding at
/// 48kHz is always lossless with respect to the encoder's input rate.
const DECODE_RATE: u32 = 48_000;

/// Largest frame (per channel) a decoder will ever see: 120ms at 48kHz,
/// comfortably above the encoder's fixed 20ms frames.
const MAX_FRAME_SAMPLES: usize = 5_760;

/// Byte offset of the channel-count field inside an OpusHead packet: after
/// the 8-byte `OpusHead` magic and the 1-byte version (RFC 7845 §5.1).
const OPUS_HEAD_CHANNEL_OFFSET: usize = 9;

/// Number of interleaved lanes for an Opus channel layout.
fn channel_count(channels: Channels) -> usize {
    match channels {
        Channels::Mono => 1,
        Channels::Stereo => 2,
    }
}

fn bitrate_for(channels: Channels) -> i32 {
    match channels {
        Channels::Mono => BITRATE_BPS,
        Channels::Stereo => STEREO_BITRATE_BPS,
    }
}

/// Opus channel layout for an OpusHead channel-count byte. Only the two
/// layouts this module ever writes (mapping family 0) are accepted.
fn channels_from_count(count: u8) -> Result<Channels, String> {
    match count {
        1 => Ok(Channels::Mono),
        2 => Ok(Channels::Stereo),
        other => Err(format!(
            "Unsupported Opus channel count {other} (expected 1 or 2)"
        )),
    }
}

/// Ogg logical-stream serial number. Each file holds exactly one stream, so
/// any constant works — it only has to be unique within the file.
const STREAM_SERIAL: u32 = 1;

/// Bounded channel capacity between the audio thread and the writer thread:
/// generous enough that a brief disk/encoder stall doesn't drop chunks
/// before the writer catches up, without letting a wedged writer thread
/// build up unbounded memory.
const CHANNEL_CAPACITY: usize = 256;

/// Comfortably above the largest Opus packet this encoder ever produces: a
/// 20ms frame at the stereo 48kbps target is ~120 bytes nominal, and even a
/// VBR peak stays far below this; matches the size libopus's own examples
/// use.
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

/// Recorded session files in `dir`, sorted by session index. Ignores
/// anything that isn't `{index}.ogg`, and header-only files (OpusHead +
/// OpusTags, no audio packets) so the UI does not offer a dead player.
pub fn list_session_files_in(dir: &std::path::Path) -> std::io::Result<Vec<(usize, PathBuf)>> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut sessions = Vec::new();
    for entry in entries {
        let path = entry?.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("ogg") {
            continue;
        }
        let Some(session_index) = path
            .file_stem()
            .and_then(|s| s.to_str())
            .and_then(|s| s.parse::<usize>().ok())
        else {
            continue;
        };
        if !ogg_file_has_audio(&path) {
            continue;
        }
        sessions.push((session_index, path));
    }
    sessions.sort_by_key(|(index, _)| *index);
    Ok(sessions)
}

/// Checks if an Ogg file has at least one audio packet (i.e. packet index >= 3
/// after OpusHead and OpusTags). This accurately detects audio even for very
/// short, low-bitrate VBR recordings that might fall under arbitrary size thresholds.
fn ogg_file_has_audio(path: &std::path::Path) -> bool {
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return false,
    };
    let mut reader = ogg::reading::PacketReader::new(file);
    let mut packet_count = 0;
    while let Ok(Some(_)) = reader.read_packet() {
        packet_count += 1;
        if packet_count >= 3 {
            return true;
        }
    }
    false
}

/// Recorded session files for a meeting (empty if recording was never
/// enabled, the directory is missing, or nothing survived retention).
pub fn list_session_files(meeting_id: &str) -> std::io::Result<Vec<(usize, PathBuf)>> {
    list_session_files_in(&meeting_recordings_dir(meeting_id))
}

/// One past the highest session index already listed (0 if none). Pure
/// helper behind [`next_session_index`], split out so the derivation can be
/// tested without touching the filesystem.
fn next_session_index_from(sessions: &[(usize, PathBuf)]) -> usize {
    sessions.last().map(|(index, _)| index + 1).unwrap_or(0)
}

/// The on-disk session index a new recording for `meeting_id` should use:
/// one past the highest `{index}.ogg` already on disk. Deliberately not
/// `recording_sessions.len()` from the DB: if that metadata ever undercounts
/// (a crash recovery that failed to close a session, say), trusting it would
/// have `File::create` truncate an existing recording instead of appending a
/// new one.
pub fn next_session_index(meeting_id: &str) -> std::io::Result<usize> {
    Ok(next_session_index_from(&list_session_files(meeting_id)?))
}

/// Decodes an Ogg Opus recording (written by [`OggOpusWriter`]) to
/// interleaved f32 PCM at 48kHz, returning the samples and the channel count
/// (1 or 2) declared by the file's OpusHead. The decoder is created with the
/// file's own layout, so a stereo file comes back as `[L0, R0, L1, R1, …]`
/// and a legacy mono file as a flat mono buffer; [`downmix_to_mono`] folds
/// either into the single-channel contract [`decode_ogg_opus`] keeps.
pub fn decode_ogg_opus_interleaved(path: &std::path::Path) -> Result<(Vec<f32>, usize), String> {
    let file = std::fs::File::open(path).map_err(|e| format!("Open recording: {e}"))?;
    let mut reader = ogg::reading::PacketReader::new(file);

    let head = match reader.read_packet() {
        Ok(Some(packet)) => packet,
        Ok(None) => return Err("Recording has no OpusHead packet".into()),
        Err(e) => return Err(format!("Read Ogg packet: {e}")),
    };
    if head.data.len() <= OPUS_HEAD_CHANNEL_OFFSET || !head.data.starts_with(b"OpusHead") {
        return Err("Recording does not start with an OpusHead packet".into());
    }
    let channels = channels_from_count(head.data[OPUS_HEAD_CHANNEL_OFFSET])?;
    let lanes = channel_count(channels);
    let mut decoder = opus::Decoder::new(DECODE_RATE, channels)
        .map_err(|e| format!("Create Opus decoder: {e}"))?;

    // Packet 2 is OpusTags - it carries no audio.
    match reader.read_packet() {
        Ok(Some(_)) => {}
        Ok(None) => return Ok((Vec::new(), lanes)),
        Err(e) => return Err(format!("Read Ogg packet: {e}")),
    }

    let mut samples = Vec::new();
    let mut out_buf = vec![0f32; MAX_FRAME_SAMPLES * lanes];
    loop {
        let packet = match reader.read_packet() {
            Ok(Some(packet)) => packet,
            Ok(None) => break,
            Err(e) => return Err(format!("Read Ogg packet: {e}")),
        };
        // `decode_float` returns the frame length per channel; the buffer
        // holds `lanes` interleaved samples per frame position.
        let frames = decoder
            .decode_float(&packet.data, &mut out_buf, false)
            .map_err(|e| format!("Decode Opus packet: {e}"))?;
        samples.extend_from_slice(&out_buf[..frames * lanes]);
    }
    Ok((samples, lanes))
}

/// Averages the `channels` interleaved lanes of every frame into one mono
/// sample: the "merged meeting" mix the built-in player plays. A single
/// channel is returned as-is (copied), so legacy mono files are untouched.
/// A trailing partial frame (input length not a multiple of `channels`) is
/// dropped; the writer never produces one.
pub fn downmix_to_mono(interleaved: &[f32], channels: usize) -> Vec<f32> {
    if channels <= 1 {
        return interleaved.to_vec();
    }
    interleaved
        .chunks_exact(channels)
        .map(|frame| frame.iter().sum::<f32>() / channels as f32)
        .collect()
}

/// Decodes an Ogg Opus recording to **mono** f32 PCM at 48kHz, whatever
/// channel layout the file has: a stereo (diarized) recording is folded into
/// an L+R average so the player and waveform keep their single-channel
/// contract, and a legacy mono file decodes exactly as before.
pub fn decode_ogg_opus(path: &std::path::Path) -> Result<Vec<f32>, String> {
    let (interleaved, channels) = decode_ogg_opus_interleaved(path)?;
    Ok(downmix_to_mono(&interleaved, channels))
}

/// Interleaves two mono lanes into one stereo buffer `[L0, R0, L1, R1, …]`,
/// zero-padding the shorter lane to the longer one's length so neither lane
/// ever slips in time relative to the other.
pub fn interleave_stereo(left: &[f32], right: &[f32]) -> Vec<f32> {
    let frames = left.len().max(right.len());
    let mut out = Vec::with_capacity(frames * 2);
    for i in 0..frames {
        out.push(left.get(i).copied().unwrap_or(0.0));
        out.push(right.get(i).copied().unwrap_or(0.0));
    }
    out
}

/// Inverse of [`interleave_stereo`]: splits `[L0, R0, L1, R1, …]` into its
/// two lanes. A trailing odd sample is dropped.
pub fn deinterleave_stereo(interleaved: &[f32]) -> (Vec<f32>, Vec<f32>) {
    let frames = interleaved.len() / 2;
    let mut left = Vec::with_capacity(frames);
    let mut right = Vec::with_capacity(frames);
    for frame in interleaved.chunks_exact(2) {
        left.push(frame[0]);
        right.push(frame[1]);
    }
    (left, right)
}

/// Downsamples `samples` into `bucket_count` peak (max absolute value)
/// buckets, normalized to `0.0..=1.0` - a bar-style overview, not a literal
/// point-by-point waveform (the same kind of summary any DAW peak view
/// shows).
pub fn waveform_peaks(samples: &[f32], bucket_count: usize) -> Vec<f32> {
    if bucket_count == 0 || samples.is_empty() {
        return Vec::new();
    }
    let bucket_size = samples.len().div_ceil(bucket_count);
    let mut peaks: Vec<f32> = samples
        .chunks(bucket_size)
        .map(|chunk| chunk.iter().fold(0.0f32, |acc, s| acc.max(s.abs())))
        .collect();
    peaks.resize(bucket_count, 0.0);
    let max_peak = peaks.iter().cloned().fold(0.0f32, f32::max);
    if max_peak > 0.0 {
        for p in &mut peaks {
            *p /= max_peak;
        }
    }
    peaks
}

fn opus_head(pre_skip: u16, input_rate: u32, channel_count: u8) -> Vec<u8> {
    let mut v = Vec::with_capacity(19);
    v.extend_from_slice(b"OpusHead");
    v.push(1); // version
    v.push(channel_count); // at OPUS_HEAD_CHANNEL_OFFSET
    v.extend_from_slice(&pre_skip.to_le_bytes());
    v.extend_from_slice(&input_rate.to_le_bytes());
    v.extend_from_slice(&0i16.to_le_bytes()); // output gain
    v.push(0); // channel mapping family 0: mono or L/R stereo, no extra table
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

/// Encodes interleaved f32 PCM (mono, or L/R stereo) into an Ogg Opus
/// stream. Buffers samples into exact `FRAME_MS` frames (Opus requires one
/// of a handful of exact frame sizes) and tracks the RFC 7845 granule
/// position (48kHz-equivalent samples per channel, regardless of the actual
/// encode rate or channel count) so players report correct duration and can
/// seek.
struct OggOpusWriter<W: Write> {
    encoder: OpusEncoder,
    packet_writer: PacketWriter<'static, W>,
    input_rate: u32,
    /// Interleaved lanes per frame position: 1 or 2.
    channels: usize,
    /// Frame length **per channel**; one encoded frame consumes
    /// `frame_samples * channels` interleaved values from `pending`.
    frame_samples: usize,
    pending: Vec<f32>,
    granule_position: u64,
    packets_written: u64,
    finished: bool,
}

impl<W: Write> OggOpusWriter<W> {
    fn new(writer: W, input_rate: u32, channels: Channels) -> Result<Self, String> {
        let mut encoder = OpusEncoder::new(input_rate, channels, Application::Voip)
            .map_err(|e| format!("Create Opus encoder: {e}"))?;
        encoder
            .set_bitrate(opus::Bitrate::Bits(bitrate_for(channels)))
            .map_err(|e| format!("Set Opus bitrate: {e}"))?;
        let lookahead = encoder
            .get_lookahead()
            .map_err(|e| format!("Read Opus lookahead: {e}"))?;
        // Pre-skip is always expressed in 48kHz-equivalent samples per RFC
        // 7845, regardless of the actual input/encode rate.
        let pre_skip = ((lookahead as u64) * 48_000 / u64::from(input_rate)) as u16;

        let lanes = channel_count(channels);
        let mut packet_writer = PacketWriter::new(writer);
        packet_writer
            .write_packet(
                opus_head(pre_skip, input_rate, lanes as u8),
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
            channels: lanes,
            frame_samples,
            pending: Vec::with_capacity(frame_samples * lanes * 2),
            granule_position: 0,
            packets_written: 0,
            finished: false,
        })
    }

    /// Interleaved values consumed per encoded frame.
    fn frame_len(&self) -> usize {
        self.frame_samples * self.channels
    }

    fn encode_and_write(
        &mut self,
        frame: &[f32],
        end_info: PacketWriteEndInfo,
    ) -> Result<(), String> {
        let mut out_buf = [0u8; MAX_OPUS_PACKET_BYTES];
        let len = self
            .encoder
            .encode_float(frame, &mut out_buf)
            .map_err(|e| format!("Opus encode: {e}"))?;
        self.granule_position += (self.frame_samples as u64) * 48_000 / u64::from(self.input_rate);
        self.packet_writer
            .write_packet(
                out_buf[..len].to_vec(),
                STREAM_SERIAL,
                end_info,
                self.granule_position,
            )
            .map_err(|e| format!("Write Opus packet: {e}"))?;
        self.packets_written += 1;
        Ok(())
    }

    /// Queue interleaved samples (`channels` values per frame position) and
    /// encode every complete frame they make up.
    fn write_chunk(&mut self, samples: &[f32]) -> Result<(), String> {
        self.pending.extend_from_slice(samples);
        let frame_len = self.frame_len();
        while self.pending.len() >= frame_len {
            let frame: Vec<f32> = self.pending.drain(..frame_len).collect();
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

        let frame_len = self.frame_len();
        if !self.pending.is_empty() {
            let mut frame = std::mem::take(&mut self.pending);
            frame.resize(frame_len, 0.0);
            self.encode_and_write(&frame, PacketWriteEndInfo::EndStream)?;
        } else if self.packets_written > 0 {
            // Every audio packet so far was a full frame with no pending
            // tail; the Ogg stream still needs one page carrying the
            // end-of-stream flag, so close it out with a frame of silence.
            let silence = vec![0.0f32; frame_len];
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

/// Messages sent from the audio capture thread to the recorder's writer thread.
enum RecorderMsg {
    /// A chunk of raw f32 PCM audio samples to be resampled (if necessary) and encoded.
    Chunk(Vec<f32>),
    /// Explicit signal to stop the writer loop, flush remaining data, and cleanly close the file.
    Finish,
}

/// Flush then finish, each under `catch_unwind`, so a poisoned resampler
/// cannot skip the Ogg EndStream page.
#[cfg(test)]
fn run_guarded_finalize(flush: impl FnOnce(), finish: impl FnOnce()) {
    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(flush)).is_err() {
        tracing::warn!("Meeting recorder panicked on resampler flush; still finalizing the file");
    }
    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(finish)).is_err() {
        tracing::warn!("Meeting recorder panicked while finishing the file");
    }
}

/// Writer-thread resampling from the capture rate to the Opus encode rate,
/// one **mono** `Resampler` per lane. A stereo stream must never go through
/// a single multi-channel `Resampler`: its `process` averages the channels
/// to mono, which would fold L and R back together.
enum LaneResamplers {
    Mono(Box<super::resampler::Resampler>),
    Stereo {
        left: Box<super::resampler::Resampler>,
        right: Box<super::resampler::Resampler>,
    },
}

impl LaneResamplers {
    fn new(source_rate: u32, target_rate: u32, channels: Channels) -> Self {
        let lane = || {
            Box::new(super::resampler::Resampler::new(
                source_rate,
                1,
                target_rate,
                1.0,
            ))
        };
        match channels {
            Channels::Mono => Self::Mono(lane()),
            Channels::Stereo => Self::Stereo {
                left: lane(),
                right: lane(),
            },
        }
    }

    /// Resample one interleaved chunk, lane by lane, re-interleaving the
    /// output (the lanes are fed equal lengths, so they stay aligned).
    fn process(&mut self, interleaved: &[f32]) -> Vec<f32> {
        match self {
            Self::Mono(r) => r.process(interleaved),
            Self::Stereo { left, right } => {
                let (l, r) = deinterleave_stereo(interleaved);
                interleave_stereo(&left.process(&l), &right.process(&r))
            }
        }
    }

    /// Flush every lane's buffered tail, re-interleaved.
    fn flush(&mut self) -> Vec<f32> {
        match self {
            Self::Mono(r) => r.flush(),
            Self::Stereo { left, right } => interleave_stereo(&left.flush(), &right.flush()),
        }
    }
}

fn finalize_recorder_writer<W: Write>(
    resampler: &mut Option<LaneResamplers>,
    writer: &mut OggOpusWriter<W>,
) {
    if let Some(r) = resampler.as_mut() {
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| r.flush())) {
            Ok(tail) if !tail.is_empty() => {
                if let Err(e) = writer.write_chunk(&tail) {
                    tracing::warn!("Meeting recorder tail flush error: {e}");
                }
            }
            Ok(_) => {}
            Err(_) => tracing::warn!(
                "Meeting recorder panicked on resampler flush; still finalizing the file"
            ),
        }
    }
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| writer.finish())) {
        Ok(Ok(())) => {}
        Ok(Err(e)) => tracing::warn!("Meeting recorder finalize error: {e}"),
        Err(_) => tracing::warn!("Meeting recorder panicked while finishing the file"),
    }
}

/// Feeds f32 PCM (mono, or L/R interleaved stereo — fixed at `start`) to a
/// background writer thread that encodes it to an Ogg Opus file. One
/// instance per recording session (see `capture::AudioCapture`'s recorder
/// field — it survives mid-session capture rebuilds by session id, matching
/// the session lifecycle, so the channel layout cannot change mid-file).
pub struct MeetingRecorder {
    session_id: u64,
    sender: Option<SyncSender<RecorderMsg>>,
    handle: Option<JoinHandle<()>>,
    dropped: Arc<AtomicU64>,
    samples_received: Arc<AtomicU64>,
    shutdown: Arc<AtomicBool>,
}

/// Cloneable, realtime-safe handle that queues PCM to a [`MeetingRecorder`].
/// Used by the no-tap capture callback, which cannot hold `&self.recorder`.
#[derive(Clone)]
pub struct RecorderPush {
    sender: SyncSender<RecorderMsg>,
    dropped: Arc<AtomicU64>,
    samples_received: Arc<AtomicU64>,
    shutdown: Arc<AtomicBool>,
}

fn try_push_chunk(
    sender: &SyncSender<RecorderMsg>,
    dropped: &AtomicU64,
    samples_received: &AtomicU64,
    shutdown: &AtomicBool,
    samples: &[f32],
) {
    if samples.is_empty() || shutdown.load(Ordering::Relaxed) {
        return;
    }
    samples_received.fetch_add(samples.len() as u64, Ordering::Relaxed);
    if sender
        .try_send(RecorderMsg::Chunk(samples.to_vec()))
        .is_err()
    {
        dropped.fetch_add(1, Ordering::Relaxed);
    }
}

impl RecorderPush {
    /// Queue a chunk for encoding if the recorder hasn't been shut down.
    pub fn push(&self, samples: &[f32]) {
        try_push_chunk(
            &self.sender,
            &self.dropped,
            &self.samples_received,
            &self.shutdown,
            samples,
        );
    }
}

impl MeetingRecorder {
    /// Start recording `session_id`'s audio to `path`, at the nearest
    /// Opus-valid rate to `sample_rate` (resampling on the writer thread if
    /// they differ — never on the caller's, presumably realtime, thread).
    /// `channels` fixes the layout every later `push` must follow: mono
    /// samples, or L/R interleaved stereo (see [`interleave_stereo`]).
    pub fn start(
        path: PathBuf,
        sample_rate: u32,
        session_id: u64,
        channels: Channels,
    ) -> Result<Self, String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("Create recordings dir: {e}"))?;
        }
        let encode_rate = nearest_opus_rate(sample_rate);

        let file =
            std::fs::File::create(&path).map_err(|e| format!("Create recording file: {e}"))?;
        let mut writer = OggOpusWriter::new(std::io::BufWriter::new(file), encode_rate, channels)?;

        let (tx, rx) = sync_channel::<RecorderMsg>(CHANNEL_CAPACITY);
        let dropped = Arc::new(AtomicU64::new(0));
        let samples_received = Arc::new(AtomicU64::new(0));
        let shutdown = Arc::new(AtomicBool::new(false));

        let handle = std::thread::Builder::new()
            .name("meeting-recorder".into())
            .spawn(move || {
                let mut resampler = (encode_rate != sample_rate)
                    .then(|| LaneResamplers::new(sample_rate, encode_rate, channels));

                while let Ok(msg) = rx.recv() {
                    match msg {
                        RecorderMsg::Chunk(samples) => {
                            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                let owned;
                                let encode_samples: &[f32] = match resampler.as_mut() {
                                    Some(r) => {
                                        owned = r.process(&samples);
                                        &owned
                                    }
                                    None => &samples,
                                };
                                writer.write_chunk(encode_samples)
                            }));
                            match result {
                                Ok(Ok(())) => {}
                                Ok(Err(e)) => tracing::warn!("Meeting recorder encode error: {e}"),
                                Err(_) => tracing::warn!(
                                    "Meeting recorder panicked on a chunk; skipping it and keeping the file"
                                ),
                            }
                        }
                        RecorderMsg::Finish => break,
                    }
                }

                finalize_recorder_writer(&mut resampler, &mut writer);
            })
            .map_err(|e| format!("Spawn meeting recorder thread: {e}"))?;

        Ok(Self {
            session_id,
            sender: Some(tx),
            handle: Some(handle),
            dropped,
            samples_received,
            shutdown,
        })
    }

    /// Retrieve the session ID this recorder is associated with.
    pub fn session_id(&self) -> u64 {
        self.session_id
    }

    /// Queue a chunk for encoding, in the layout given to `start` (mono, or
    /// L/R interleaved). Non-blocking: a full channel drops the chunk (and
    /// counts it) instead of ever stalling the realtime audio-capture thread
    /// that calls this.
    pub fn push(&self, samples: &[f32]) {
        if let Some(sender) = &self.sender {
            try_push_chunk(
                sender,
                &self.dropped,
                &self.samples_received,
                &self.shutdown,
                samples,
            );
        }
    }

    /// Clone of the push path for the no-tap cpal callback.
    pub fn push_handle(&self) -> Option<RecorderPush> {
        Some(RecorderPush {
            sender: self.sender.as_ref()?.clone(),
            dropped: Arc::clone(&self.dropped),
            samples_received: Arc::clone(&self.samples_received),
            shutdown: Arc::clone(&self.shutdown),
        })
    }

    /// Get the number of dropped chunks due to a full channel.
    pub fn dropped_chunks(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// Get the total number of audio samples received for recording.
    pub fn samples_received(&self) -> u64 {
        self.samples_received.load(Ordering::Relaxed)
    }
}

impl Drop for MeetingRecorder {
    fn drop(&mut self) {
        // Signal the writer thread to finish and exit immediately, bypassing
        // any remaining `RecorderPush` handles that might be kept alive by
        // callbacks. This ensures the file is finalized (flush + close)
        // synchronously before `drop` returns.
        // This runs whenever a `MeetingRecorder` goes out of scope — normal
        // stop, a session end after mic loss, or stack unwinding from a
        // caught panic (`panic = "unwind"`) — so every teardown path closes
        // the file. Only a hard crash (SIGKILL/abort) skips it, leaving a
        // truncated but structurally valid Ogg file.
        self.shutdown.store(true, Ordering::Relaxed);
        if let Some(sender) = self.sender.take() {
            let _ = sender.send(RecorderMsg::Finish);
        }
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
            .map(|i| {
                ((2.0 * std::f64::consts::PI * 440.0 * i as f64 / f64::from(rate)).sin() * 0.3)
                    as f32
            })
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
    fn list_session_files_in_skips_non_ogg_and_sorts_by_index() {
        let dir = tempfile::tempdir().expect("tempdir");
        let empty = dir.path().join("1.ogg");
        let file = std::fs::File::create(&empty).expect("create");
        let mut writer = OggOpusWriter::new(std::io::BufWriter::new(file), 16_000, Channels::Mono)
            .expect("writer");
        writer.write_chunk(&sine(0.1, 16_000)).unwrap();
        writer.finish().expect("finish");
        drop(writer);

        let empty0 = dir.path().join("0.ogg");
        let file0 = std::fs::File::create(&empty0).expect("create");
        let mut writer0 =
            OggOpusWriter::new(std::io::BufWriter::new(file0), 16_000, Channels::Mono)
                .expect("writer");
        writer0.write_chunk(&sine(0.1, 16_000)).unwrap();
        writer0.finish().expect("finish");
        drop(writer0);

        std::fs::write(dir.path().join("notes.txt"), b"nope").unwrap();
        std::fs::write(dir.path().join("not-an-index.ogg"), b"skip").unwrap();

        let listed = list_session_files_in(dir.path()).unwrap();
        assert_eq!(
            listed.iter().map(|(i, _)| *i).collect::<Vec<_>>(),
            vec![0, 1]
        );
    }

    #[test]
    fn decode_ogg_opus_round_trips_a_sine_wave() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("0.ogg");
        let file = std::fs::File::create(&path).expect("create");
        let mut writer = OggOpusWriter::new(std::io::BufWriter::new(file), 16_000, Channels::Mono)
            .expect("writer");
        writer.write_chunk(&sine(1.0, 16_000)).expect("write");
        writer.finish().expect("finish");
        drop(writer);

        let decoded = decode_ogg_opus(&path).expect("decode");
        // Decoded at 48kHz regardless of the 16kHz encode rate; allow slack
        // for the encoder's pre-skip and frame rounding either side of the
        // nominal 1s * 48_000 sample count.
        assert!(
            decoded.len() > 40_000 && decoded.len() < 56_000,
            "unexpected decoded length: {}",
            decoded.len()
        );
        let rms = (decoded.iter().map(|s| s * s).sum::<f32>() / decoded.len() as f32).sqrt();
        assert!(rms > 0.05, "decoded audio looks silent: rms={rms}");
    }

    #[test]
    fn waveform_peaks_normalizes_and_buckets() {
        let samples = sine(1.0, 48_000);
        let peaks = waveform_peaks(&samples, 100);
        assert_eq!(peaks.len(), 100);
        assert!(peaks.iter().any(|p| *p > 0.9));
        assert!(peaks.iter().all(|p| (0.0..=1.0).contains(p)));
    }

    #[test]
    fn waveform_peaks_handles_empty_input() {
        assert!(waveform_peaks(&[], 100).is_empty());
        assert!(waveform_peaks(&[0.1, 0.2], 0).is_empty());
    }

    #[test]
    fn list_session_files_in_missing_dir_is_empty() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(
            list_session_files_in(&dir.path().join("missing"))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn next_session_index_from_is_one_past_the_highest() {
        let sessions = vec![
            (0, PathBuf::from("0.ogg")),
            (1, PathBuf::from("1.ogg")),
            (3, PathBuf::from("3.ogg")),
        ];
        assert_eq!(next_session_index_from(&sessions), 4);
    }

    #[test]
    fn next_session_index_from_empty_is_zero() {
        assert_eq!(next_session_index_from(&[]), 0);
    }

    #[test]
    fn encodes_sine_to_valid_nonempty_ogg_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("out.ogg");
        let file = std::fs::File::create(&path).expect("create");
        let mut writer = OggOpusWriter::new(std::io::BufWriter::new(file), 24_000, Channels::Mono)
            .expect("writer");

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
    fn finalize_still_runs_after_flush_panic() {
        use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
        let finished = AtomicBool::new(false);
        run_guarded_finalize(
            || panic!("poisoned resampler"),
            || finished.store(true, AtomicOrdering::Relaxed),
        );
        assert!(
            finished.load(AtomicOrdering::Relaxed),
            "EndStream must still run after a flush panic"
        );
    }

    #[test]
    fn finish_is_idempotent() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("out.ogg");
        let file = std::fs::File::create(&path).expect("create");
        let mut writer = OggOpusWriter::new(std::io::BufWriter::new(file), 24_000, Channels::Mono)
            .expect("writer");

        writer.write_chunk(&sine(0.5, 24_000)).expect("write");
        writer.finish().expect("finish once");
        let granule_after_first = writer.granule_position;
        writer.finish().expect("finish twice must not error");
        assert_eq!(
            writer.granule_position, granule_after_first,
            "second finish must be a no-op"
        );
    }

    #[test]
    fn finish_with_no_audio_does_not_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("empty.ogg");
        let file = std::fs::File::create(&path).expect("create");
        let mut writer = OggOpusWriter::new(std::io::BufWriter::new(file), 16_000, Channels::Mono)
            .expect("writer");
        writer.finish().expect("finish with no audio");
    }

    #[test]
    fn list_session_files_in_skips_header_only_ogg() {
        let dir = tempfile::tempdir().expect("tempdir");
        let empty = dir.path().join("0.ogg");
        let file = std::fs::File::create(&empty).expect("create");
        let mut writer = OggOpusWriter::new(std::io::BufWriter::new(file), 16_000, Channels::Mono)
            .expect("writer");
        writer.finish().expect("finish with no audio");
        drop(writer);

        let listed = list_session_files_in(dir.path()).unwrap();
        assert!(
            listed.is_empty(),
            "header-only ogg must not be offered as playable audio, listed {listed:?}"
        );
    }

    #[test]
    fn ogg_file_has_audio_accepts_partial_frame() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("short.ogg");
        let file = std::fs::File::create(&path).expect("create");
        let mut writer = OggOpusWriter::new(std::io::BufWriter::new(file), 16_000, Channels::Mono)
            .expect("writer");
        // write 10ms of audio, less than a full 20ms frame, so it gets flushed via `finish()`
        writer.write_chunk(&sine(0.01, 16_000)).expect("write");
        writer.finish().expect("finish");
        drop(writer);

        assert!(
            ogg_file_has_audio(&path),
            "partial frame should be recognized as having audio"
        );
    }

    #[test]
    fn recorder_encodes_end_to_end_and_closes_file_on_drop() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("session.ogg");

        let recorder =
            MeetingRecorder::start(path.clone(), 24_000, 1, Channels::Mono).expect("start");
        assert_eq!(recorder.session_id(), 1);
        for _ in 0..5 {
            recorder.push(&sine(0.1, 24_000));
        }
        drop(recorder); // joins the writer thread, finalizing the file

        let bytes = std::fs::read(&path).expect("recording file must exist");
        assert!(!bytes.is_empty());
        assert_eq!(&bytes[0..4], b"OggS");
    }

    #[test]
    fn recorder_joins_cleanly_even_if_push_handle_retained() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("retained.ogg");

        let recorder =
            MeetingRecorder::start(path.clone(), 24_000, 1, Channels::Mono).expect("start");
        let retained_handle = recorder.push_handle().expect("handle");

        retained_handle.push(&sine(0.1, 24_000));

        drop(recorder); // should trigger Finish and join

        // Pushing after drop should be rejected immediately due to shutdown flag
        retained_handle.push(&sine(0.1, 24_000));

        let bytes = std::fs::read(&path).expect("recording file must exist");
        assert!(!bytes.is_empty());
        assert_eq!(&bytes[0..4], b"OggS");
    }

    fn rms(samples: &[f32]) -> f32 {
        if samples.is_empty() {
            return 0.0;
        }
        (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
    }

    /// Channel-count byte of the first (OpusHead) packet in `path`.
    fn opus_head_channel_byte(path: &std::path::Path) -> u8 {
        let file = std::fs::File::open(path).expect("open");
        let mut reader = ogg::reading::PacketReader::new(file);
        let head = reader.read_packet().expect("read").expect("OpusHead");
        assert!(head.data.starts_with(b"OpusHead"));
        head.data[OPUS_HEAD_CHANNEL_OFFSET]
    }

    #[test]
    fn interleave_stereo_zero_pads_the_shorter_lane() {
        assert_eq!(
            interleave_stereo(&[1.0, 2.0, 3.0], &[10.0]),
            vec![1.0, 10.0, 2.0, 0.0, 3.0, 0.0]
        );
        assert_eq!(
            interleave_stereo(&[], &[5.0, 6.0]),
            vec![0.0, 5.0, 0.0, 6.0]
        );
        assert!(interleave_stereo(&[], &[]).is_empty());
    }

    #[test]
    fn deinterleave_stereo_inverts_interleave_and_drops_odd_tail() {
        let left = [0.1, 0.2, 0.3];
        let right = [-0.1, -0.2, -0.3];
        let (l, r) = deinterleave_stereo(&interleave_stereo(&left, &right));
        assert_eq!(l, left.to_vec());
        assert_eq!(r, right.to_vec());
        assert_eq!(
            deinterleave_stereo(&[1.0, 2.0, 3.0]),
            (vec![1.0], vec![2.0])
        );
    }

    #[test]
    fn downmix_to_mono_averages_lanes_and_copies_mono() {
        assert_eq!(
            downmix_to_mono(&[1.0, 0.0, 0.5, 0.5, -1.0, 1.0], 2),
            vec![0.5, 0.5, 0.0]
        );
        assert_eq!(downmix_to_mono(&[0.25, -0.25], 1), vec![0.25, -0.25]);
        // A trailing partial frame is dropped rather than misread.
        assert_eq!(downmix_to_mono(&[1.0, 1.0, 9.0], 2), vec![1.0]);
    }

    #[test]
    fn opus_head_declares_the_recorder_channel_count() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mono = dir.path().join("mono.ogg");
        let stereo = dir.path().join("stereo.ogg");
        drop(MeetingRecorder::start(mono.clone(), 48_000, 1, Channels::Mono).expect("start"));
        drop(MeetingRecorder::start(stereo.clone(), 48_000, 2, Channels::Stereo).expect("start"));
        assert_eq!(opus_head_channel_byte(&mono), 1);
        assert_eq!(opus_head_channel_byte(&stereo), 2);
    }

    #[test]
    fn stereo_round_trip_keeps_the_lanes_separate() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("0.ogg");
        let file = std::fs::File::create(&path).expect("create");
        let mut writer =
            OggOpusWriter::new(std::io::BufWriter::new(file), 48_000, Channels::Stereo)
                .expect("writer");
        let left = sine(1.0, 48_000);
        let right = vec![0.0f32; left.len()];
        writer
            .write_chunk(&interleave_stereo(&left, &right))
            .expect("write");
        writer.finish().expect("finish");
        drop(writer);

        let (interleaved, channels) = decode_ogg_opus_interleaved(&path).expect("decode");
        assert_eq!(channels, 2);
        assert_eq!(interleaved.len() % 2, 0);
        let (l, r) = deinterleave_stereo(&interleaved);
        assert!(
            l.len() > 40_000 && l.len() < 56_000,
            "unexpected per-lane length: {}",
            l.len()
        );
        let (l_rms, r_rms) = (rms(&l), rms(&r));
        assert!(l_rms > 0.1, "left lane looks silent: rms={l_rms}");
        assert!(r_rms < 0.01, "right lane leaked audio: rms={r_rms}");

        // The mono contract folds both lanes: half the left amplitude.
        let mixed = decode_ogg_opus(&path).expect("decode mono");
        assert_eq!(mixed.len(), l.len());
        let mixed_rms = rms(&mixed);
        assert!(
            (mixed_rms - l_rms / 2.0).abs() < 0.02,
            "downmix rms {mixed_rms} should be ~half the left lane's {l_rms}"
        );
    }

    #[test]
    fn stereo_recorder_at_non_opus_rate_resamples_each_lane() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("stereo-44100.ogg");
        let recorder =
            MeetingRecorder::start(path.clone(), 44_100, 1, Channels::Stereo).expect("start");
        // Silence on the left, a tone on the right: the lane order must
        // survive the per-lane resample and re-interleave.
        let right = sine(1.0, 44_100);
        let left = vec![0.0f32; right.len()];
        let interleaved = interleave_stereo(&left, &right);
        for chunk in interleaved.chunks(4_410 * 2) {
            recorder.push(chunk);
        }
        drop(recorder);

        let (decoded, channels) = decode_ogg_opus_interleaved(&path).expect("decode");
        assert_eq!(channels, 2);
        let (l, r) = deinterleave_stereo(&decoded);
        assert!(
            r.len() > 40_000 && r.len() < 56_000,
            "unexpected per-lane length: {}",
            r.len()
        );
        assert!(rms(&r) > 0.1, "right lane looks silent: rms={}", rms(&r));
        assert!(rms(&l) < 0.01, "left lane leaked audio: rms={}", rms(&l));
    }

    #[test]
    fn decode_rejects_a_file_without_opus_head() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("bogus.ogg");
        let file = std::fs::File::create(&path).expect("create");
        let mut pw = PacketWriter::new(std::io::BufWriter::new(file));
        pw.write_packet(
            b"NotOpusHead".to_vec(),
            STREAM_SERIAL,
            PacketWriteEndInfo::EndStream,
            0,
        )
        .expect("write");
        drop(pw);
        let err = decode_ogg_opus_interleaved(&path).expect_err("must reject");
        assert!(err.contains("OpusHead"), "unexpected error: {err}");
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
        let samples_received = Arc::new(AtomicU64::new(0));
        let shutdown = Arc::new(AtomicBool::new(false));

        let push = |samples: &[f32]| {
            try_push_chunk(&tx, &dropped, &samples_received, &shutdown, samples);
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
