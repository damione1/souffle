//! Temporary 16kHz mono WAV capture of mic-only meeting audio, so a
//! background task can run offline speaker diarization on it once the
//! recording stops. Same shape as `recorder.rs`'s `MeetingRecorder`: a
//! dedicated writer thread fed by a bounded channel, so the realtime
//! audio-capture callback never blocks on resampling or disk I/O:
//! `DiarizeTapWriter::push`/`DiarizeTapHandle::push` are `try_send`, and a
//! full channel just drops the chunk (counted, logged at session end).
//!
//! Unlike the opus recorder, this tap is meeting-and-mic-only, opt-in via
//! `AppSettings::diarize_enabled`, and its output is always transient: every
//! WAV is deleted once `pipeline::diarize_task` finishes with it (or, for a
//! leftover from a crash, at the next app startup).

use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{SyncSender, sync_channel};
use std::thread::JoinHandle;

use hound::{SampleFormat, WavSpec, WavWriter};

use crate::constants::app_data_dir;
use crate::diarize::segmentation::SAMPLE_RATE as DIARIZE_SAMPLE_RATE;

/// Bounded channel capacity between the audio callback and the writer
/// thread. Matches `recorder.rs`'s `CHANNEL_CAPACITY`: generous enough to
/// absorb a brief disk stall without dropping chunks, bounded so a wedged
/// writer thread can't grow memory without limit.
const CHANNEL_CAPACITY: usize = 256;

/// Root directory for transient per-session diarization WAVs.
pub fn diarize_tmp_root() -> PathBuf {
    app_data_dir().join("diarize-tmp")
}

/// Directory holding every pending diarization WAV for one meeting.
pub fn meeting_diarize_tmp_dir(meeting_id: &str) -> PathBuf {
    diarize_tmp_root().join(meeting_id)
}

/// WAV path for one recording session's tapped mic audio, at 16kHz mono.
/// `session_index` matches the corresponding `MeetingRecordingSession`'s
/// position in the meeting's `recording_sessions`.
pub fn session_wav_path(meeting_id: &str, session_index: usize) -> PathBuf {
    meeting_diarize_tmp_dir(meeting_id).join(format!("{session_index}.wav"))
}

/// Best-effort recursive delete: missing directories are a silent no-op,
/// and a failure is logged rather than propagated. Cleanup is opportunistic
/// and must never turn into a user-facing error.
fn remove_dir_best_effort(dir: &Path) {
    if !dir.exists() {
        return;
    }
    if let Err(e) = std::fs::remove_dir_all(dir) {
        tracing::warn!(dir = %dir.display(), "Failed to clean up diarization tmp dir: {e}");
    }
}

/// Best-effort delete of one meeting's pending diarization WAVs (and the now
/// presumably-empty directory). Called once `diarize_task` is done with a
/// meeting, on success or failure alike.
pub fn cleanup_meeting_tmp(meeting_id: &str) {
    remove_dir_best_effort(&meeting_diarize_tmp_dir(meeting_id));
}

/// Best-effort delete of every leftover diarization WAV, from meetings whose
/// post-stop diarization never got to run (e.g. the app crashed or was
/// force-quit mid-task). Called once at app startup, off the main thread.
/// Nothing recoverable is lost: those meetings simply stay unlabeled, same
/// as if diarization had been skipped for them.
pub fn cleanup_all_tmp() {
    remove_dir_best_effort(&diarize_tmp_root());
}

enum TapMsg {
    Chunk(Vec<f32>),
    /// Explicit end-of-stream. Sent by `DiarizeTapWriter::drop` instead of
    /// relying on channel closure: `DiarizeTapHandle` clones the sender into
    /// the capture callback, so the channel may still have live senders when
    /// the writer is torn down. Without this marker the join would deadlock
    /// waiting for a close that never comes.
    Finish,
}

/// Cheaply cloneable handle for pushing audio into a `DiarizeTapWriter` from
/// a different thread (the realtime cpal callback), matching the pattern
/// other capture-thread state uses (`Sender<AudioCommand>`, etc.). The
/// writer itself stays owned by `AudioCapture`.
#[derive(Clone)]
pub struct DiarizeTapHandle {
    sender: SyncSender<TapMsg>,
    dropped: Arc<AtomicU64>,
}

impl DiarizeTapHandle {
    /// Queue a chunk of mono samples for resampling and encoding.
    /// Non-blocking: a full channel drops the chunk (counted) instead of
    /// ever stalling the realtime caller.
    pub fn push(&self, samples: &[f32]) {
        if samples.is_empty() {
            return;
        }
        if self.sender.try_send(TapMsg::Chunk(samples.to_vec())).is_err() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// Feeds mono f32 PCM at `source_sample_rate` to a background writer thread
/// that resamples to 16kHz and encodes a mono WAV file. One instance per
/// recording session that has diarization capture enabled.
pub struct DiarizeTapWriter {
    session_id: u64,
    sender: Option<SyncSender<TapMsg>>,
    handle: Option<JoinHandle<()>>,
    dropped: Arc<AtomicU64>,
}

impl DiarizeTapWriter {
    /// Start tapping `session_id`'s audio to `path`. `source_sample_rate` is
    /// the rate of samples that will be `push`ed (the engine's target rate,
    /// already mono, already gain-applied); resampling to 16kHz happens on
    /// the writer thread, never on the caller's.
    pub fn start(path: PathBuf, source_sample_rate: u32, session_id: u64) -> Result<Self, String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("Create diarize tmp dir: {e}"))?;
        }

        let spec = WavSpec {
            channels: 1,
            sample_rate: DIARIZE_SAMPLE_RATE,
            bits_per_sample: 32,
            sample_format: SampleFormat::Float,
        };
        let file = std::fs::File::create(&path).map_err(|e| format!("Create diarize tmp WAV: {e}"))?;
        let mut writer =
            WavWriter::new(BufWriter::new(file), spec).map_err(|e| format!("Create WAV writer: {e}"))?;

        let (tx, rx) = sync_channel::<TapMsg>(CHANNEL_CAPACITY);
        let dropped = Arc::new(AtomicU64::new(0));

        let handle = std::thread::Builder::new()
            .name("diarize-tap".into())
            .spawn(move || {
                let mut resampler = (source_sample_rate != DIARIZE_SAMPLE_RATE)
                    .then(|| super::resampler::Resampler::new(source_sample_rate, 1, DIARIZE_SAMPLE_RATE, 1.0));

                let write_all = |writer: &mut WavWriter<BufWriter<std::fs::File>>, samples: &[f32]| {
                    for &s in samples {
                        if let Err(e) = writer.write_sample(s) {
                            tracing::warn!("Diarize tap write error: {e}");
                            break;
                        }
                    }
                };

                while let Ok(msg) = rx.recv() {
                    let samples = match msg {
                        TapMsg::Chunk(samples) => samples,
                        TapMsg::Finish => break,
                    };
                    let owned;
                    let out: &[f32] = match resampler.as_mut() {
                        Some(r) => {
                            owned = r.process(&samples);
                            &owned
                        }
                        None => &samples,
                    };
                    write_all(&mut writer, out);
                }

                if let Some(r) = resampler.as_mut() {
                    let tail = r.flush();
                    if !tail.is_empty() {
                        write_all(&mut writer, &tail);
                    }
                }

                if let Err(e) = writer.finalize() {
                    tracing::warn!("Diarize tap finalize error: {e}");
                }
            })
            .map_err(|e| format!("Spawn diarize tap thread: {e}"))?;

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

    /// Queue a chunk directly from the owning thread (e.g. flushing the
    /// engine resampler's tail at session stop). Equivalent to
    /// `self.handle().push(...)` but without the clone.
    pub fn push(&self, samples: &[f32]) {
        if samples.is_empty() {
            return;
        }
        if let Some(sender) = &self.sender
            && sender.try_send(TapMsg::Chunk(samples.to_vec())).is_err()
        {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// A cheaply cloneable handle that can be moved into another thread's
    /// closure (the realtime cpal callback) to push audio into this tap.
    /// `None` only if this writer has already been torn down (never true for
    /// a `DiarizeTapWriter` still owned by its caller).
    pub fn handle(&self) -> Option<DiarizeTapHandle> {
        self.sender.as_ref().map(|sender| DiarizeTapHandle {
            sender: sender.clone(),
            dropped: Arc::clone(&self.dropped),
        })
    }

    pub fn dropped_chunks(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

impl Drop for DiarizeTapWriter {
    fn drop(&mut self) {
        // An explicit Finish marker, not just channel closure: cloned
        // `DiarizeTapHandle` senders (in the capture callback) may outlive
        // this writer, so the channel wouldn't necessarily close on its own
        // and the join below would deadlock. The blocking send is safe: the
        // writer thread is actively draining this very channel.
        if let Some(sender) = self.sender.take() {
            let _ = sender.send(TapMsg::Finish);
        }
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// Read a WAV file already at 16kHz mono f32 (as written by this module)
/// back into samples, for `pipeline::diarize_task` to feed to `diarize()`.
/// Tolerates 16-bit PCM too, in case a future writer variant or a
/// hand-crafted test fixture uses it.
pub fn read_diarize_wav(path: &Path) -> Result<Vec<f32>, String> {
    let mut reader = hound::WavReader::open(path).map_err(|e| format!("Open diarize WAV: {e}"))?;
    let spec = reader.spec();
    if spec.sample_rate != DIARIZE_SAMPLE_RATE || spec.channels != 1 {
        return Err(format!(
            "Unexpected diarize WAV format: {}Hz, {}ch (expected {}Hz mono)",
            spec.sample_rate, spec.channels, DIARIZE_SAMPLE_RATE
        ));
    }
    let samples = match spec.sample_format {
        SampleFormat::Float => reader
            .samples::<f32>()
            .filter_map(|s| s.ok())
            .collect(),
        SampleFormat::Int => reader
            .samples::<i16>()
            .filter_map(|s| s.ok())
            .map(|s| s as f32 / i16::MAX as f32)
            .collect(),
    };
    Ok(samples)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(seconds: f64, rate: u32) -> Vec<f32> {
        let n = (seconds * f64::from(rate)) as usize;
        (0..n)
            .map(|i| ((2.0 * std::f64::consts::PI * 220.0 * i as f64 / f64::from(rate)).sin() * 0.3) as f32)
            .collect()
    }

    #[test]
    fn session_wav_path_is_scoped_by_meeting_and_index() {
        let a = session_wav_path("meeting-1", 0);
        let b = session_wav_path("meeting-1", 1);
        let c = session_wav_path("meeting-2", 0);
        assert_ne!(a, b);
        assert_ne!(a, c);
        assert!(a.ends_with("meeting-1/0.wav") || a.to_string_lossy().replace('\\', "/").ends_with("meeting-1/0.wav"));
    }

    #[test]
    fn writer_produces_a_readable_16khz_mono_wav_without_resampling() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("m").join("0.wav");

        let writer = DiarizeTapWriter::start(path.clone(), 16_000, 1).expect("start");
        let handle = writer.handle().expect("handle");
        handle.push(&sine(0.5, 16_000));
        drop(writer); // joins the writer thread, finalizing the file

        let samples = read_diarize_wav(&path).expect("read back");
        assert!(!samples.is_empty());
        // ~0.5s at 16kHz, give or take frame boundaries.
        assert!(samples.len() > 7_000 && samples.len() < 9_000);
    }

    #[test]
    fn writer_resamples_from_a_different_source_rate() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("m").join("0.wav");

        let writer = DiarizeTapWriter::start(path.clone(), 24_000, 1).expect("start");
        let handle = writer.handle().expect("handle");
        handle.push(&sine(1.0, 24_000));
        drop(writer);

        let samples = read_diarize_wav(&path).expect("read back");
        // ~1s of 24kHz audio resampled to 16kHz should be roughly 16000 samples.
        assert!(samples.len() > 14_000 && samples.len() < 18_000);
    }

    /// Regression test for the teardown deadlock: a `DiarizeTapHandle`
    /// clone that outlives the writer keeps the channel open, so Drop must
    /// finalize via the explicit Finish marker rather than waiting for a
    /// channel closure that never comes.
    #[test]
    fn drop_with_a_live_handle_clone_still_finalizes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("m").join("0.wav");

        let writer = DiarizeTapWriter::start(path.clone(), 16_000, 1).expect("start");
        let handle = writer.handle().expect("handle");
        handle.push(&sine(0.25, 16_000));
        drop(writer); // must join and finalize despite `handle` still alive

        let samples = read_diarize_wav(&path).expect("read back");
        assert!(!samples.is_empty());
        drop(handle);
    }

    #[test]
    fn session_id_is_exposed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("m").join("0.wav");
        let writer = DiarizeTapWriter::start(path, 16_000, 42).expect("start");
        assert_eq!(writer.session_id(), 42);
    }

    #[test]
    fn full_channel_drops_chunks_without_blocking() {
        let (tx, _rx) = sync_channel::<TapMsg>(0);
        let handle = DiarizeTapHandle {
            sender: tx,
            dropped: Arc::new(AtomicU64::new(0)),
        };
        for _ in 0..5 {
            handle.push(&[0.1, 0.2, 0.3]);
        }
        assert_eq!(handle.dropped.load(Ordering::Relaxed), 5);
    }

    #[test]
    fn remove_dir_best_effort_deletes_an_existing_dir() {
        let dir = tempfile::tempdir().expect("tempdir");
        let meeting_dir = dir.path().join("meeting-x");
        std::fs::create_dir_all(&meeting_dir).expect("mkdir");
        std::fs::write(meeting_dir.join("0.wav"), b"fake").expect("write");

        remove_dir_best_effort(&meeting_dir);
        assert!(!meeting_dir.exists());
    }

    #[test]
    fn remove_dir_best_effort_on_missing_dir_does_not_panic() {
        let dir = tempfile::tempdir().expect("tempdir");
        remove_dir_best_effort(&dir.path().join("does-not-exist"));
    }

    #[test]
    fn read_diarize_wav_rejects_wrong_sample_rate() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("bad.wav");
        let spec = WavSpec {
            channels: 1,
            sample_rate: 44_100,
            bits_per_sample: 32,
            sample_format: SampleFormat::Float,
        };
        let mut writer = WavWriter::create(&path, spec).expect("create");
        writer.write_sample(0.0f32).expect("write");
        writer.finalize().expect("finalize");

        assert!(read_diarize_wav(&path).is_err());
    }
}
