//! Optional pre-AEC capture. Only enabled with meeting-audio retention.
//!
//! Fixed-size blocks enter a preallocated bounded ring with `try_push`;
//! callbacks neither allocate nor wait. The independent writer publishes a
//! lossless stereo WAV only after a complete, successful capture. Any dropped
//! block invalidates the entire artifact rather than silently shortening its
//! timeline. Shutdown never joins a possibly stalled disk writer.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[cfg(test)]
use crossbeam_channel::{Receiver, bounded};
use ringbuf::HeapRb;
use ringbuf::traits::{Consumer, Producer, Split};

use super::mixer::{FRAME_SAMPLES, MIX_RATE};
use super::resampler::Resampler;

const QUEUE_BLOCKS: usize = 200; // Two seconds at the mixer clock; ~0.8 MB.
pub(crate) const MAX_MINUTES: u32 = 60;
pub(crate) const MEGABYTES_PER_HOUR: u32 = MIX_RATE * 2 * 4 * 3600 / 1_000_000;
const MAX_FRAMES: u64 = MIX_RATE as u64 * MAX_MINUTES as u64 * 60;
const MIN_FREE_BYTES: u64 = 256 * 1024 * 1024;

fn disk_has_headroom(directory: &Path) -> Result<bool, String> {
    use std::os::unix::ffi::OsStrExt;
    let directory = std::ffi::CString::new(directory.as_os_str().as_bytes())
        .map_err(|e| format!("Diagnostic directory: {e}"))?;
    let mut stats = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: the C string is terminated and valid for the call; statvfs
    // initializes the output on success. This runs only on the disk writer.
    if unsafe { libc::statvfs(directory.as_ptr(), stats.as_mut_ptr()) } != 0 {
        return Err(format!(
            "Diagnostic disk headroom: {}",
            std::io::Error::last_os_error()
        ));
    }
    // SAFETY: successful statvfs initialized the structure above.
    let stats = unsafe { stats.assume_init() };
    let bytes = u128::from(stats.f_bavail) * u128::from(stats.f_frsize);
    Ok(bytes >= u128::from(MIN_FREE_BYTES))
}

pub(crate) fn session_path(primary: &Path) -> PathBuf {
    primary.with_extension("pre-aec.wav")
}

fn partial_path(primary: &Path) -> PathBuf {
    // A failed start may retry the same primary index while its diagnostic
    // writer is still on disk. Give each writer an exclusive temporary path.
    primary.with_extension(format!("pre-aec-{}.wav.partial", uuid::Uuid::new_v4()))
}

#[derive(Clone, Copy, PartialEq)]
enum Source {
    Mixer,
    // No tap was requested: resample the optional raw copy on the writer,
    // leaving the existing callback/primary resampler exactly as it was.
    Microphone {
        generation: u64,
        rate: u32,
        gain: f32,
    },
}

struct Block {
    source: Source,
    len: usize,
    frames: [[f32; 2]; FRAME_SAMPLES],
}

#[derive(Clone)]
pub(crate) struct DiagnosticPush {
    // Cloned handles survive capture rebuilds. `try_lock` protects producer
    // ownership only; contention invalidates this optional capture instead
    // of waiting. The writer owns the independent consumer and never locks.
    producer: Arc<Mutex<ringbuf::HeapProd<Block>>>,
    invalid: Arc<AtomicBool>,
    stopped: Arc<AtomicBool>,
    finished: Arc<AtomicBool>,
    start_confirmed: Arc<AtomicBool>,
}

impl DiagnosticPush {
    pub(crate) fn invalidate(&self) {
        self.invalid.store(true, Ordering::Release);
    }

    fn available(&self) -> bool {
        !self.invalid.load(Ordering::Acquire)
            && !self.finished.load(Ordering::Acquire)
            && !self.stopped.load(Ordering::Acquire)
    }

    fn send(&self, block: Block) -> bool {
        if !self.available() {
            return false;
        }
        let queued = match self.producer.try_lock() {
            Ok(mut producer) => producer.try_push(block).is_ok(),
            Err(_) => false,
        };
        if !queued {
            if !self.finished.load(Ordering::Acquire) && !self.stopped.load(Ordering::Acquire) {
                self.invalidate();
            }
            return false;
        }
        true
    }

    /// Mixer-clock lanes before AEC; absent lanes are zero padded together.
    pub(crate) fn push(&self, mic: &[f32], tap: &[f32]) {
        if !self.available() {
            return;
        }
        let n = mic.len().max(tap.len());
        for start in (0..n).step_by(FRAME_SAMPLES) {
            let len = (n - start).min(FRAME_SAMPLES);
            let mut block = Block {
                source: Source::Mixer,
                len,
                frames: [[0.0; 2]; FRAME_SAMPLES],
            };
            for (i, pair) in block.frames[..len].iter_mut().enumerate() {
                *pair = [
                    mic.get(start + i).copied().unwrap_or(0.0),
                    tap.get(start + i).copied().unwrap_or(0.0),
                ];
            }
            if !self.send(block) {
                return;
            }
        }
    }

    /// Raw device input in meetings that never requested a tap. Downmix only;
    /// gain/resampling and all allocations happen on the diagnostic writer.
    pub(crate) fn push_microphone(
        &self,
        samples: &[f32],
        channels: u16,
        rate: u32,
        gain: f32,
        generation: u64,
    ) {
        if !self.available() {
            return;
        }
        let channels = usize::from(channels);
        if channels == 0 || !samples.len().is_multiple_of(channels) {
            self.invalidate();
            return;
        }
        for chunk in samples.chunks(FRAME_SAMPLES * channels) {
            let len = chunk.len() / channels;
            let mut block = Block {
                source: Source::Microphone {
                    generation,
                    rate,
                    gain,
                },
                len,
                frames: [[0.0; 2]; FRAME_SAMPLES],
            };
            for (pair, input) in block.frames[..len]
                .iter_mut()
                .zip(chunk.chunks_exact(channels))
            {
                pair[0] = input.iter().sum::<f32>() / channels as f32;
            }
            if !self.send(block) {
                return;
            }
        }
    }
}

struct Recorder {
    push: DiagnosticPush,
    #[cfg(test)]
    completed: Receiver<()>,
    #[cfg(test)]
    finalized: Receiver<()>,
    #[cfg(test)]
    partial: PathBuf,
}

impl Drop for Recorder {
    fn drop(&mut self) {
        self.push.finished.store(true, Ordering::Release);
    }
}

/// The snapshot includes inactive sessions: a rebuild cannot turn a setting
/// change into newly persisted audio midway through the same meeting.
#[derive(Default)]
pub(crate) struct DiagnosticSession {
    session_id: Option<u64>,
    recorder: Option<Recorder>,
}

impl DiagnosticSession {
    pub(crate) fn sync(&mut self, session_id: u64, primary: Option<&Path>, enabled: bool) {
        if self.session_id == Some(session_id) {
            return;
        }
        self.finish();
        self.cancel_unconfirmed_start();
        self.recorder.take();
        self.session_id = Some(session_id);
        if enabled && let Some(primary) = primary {
            match start_writer(primary, QUEUE_BLOCKS, MAX_FRAMES, || {}) {
                Ok(recorder) => self.recorder = Some(recorder),
                Err(e) => tracing::warn!("Pre-AEC diagnostic could not start: {e}"),
            }
        }
    }

    pub(crate) fn push_handle(&self) -> Option<DiagnosticPush> {
        self.recorder.as_ref().map(|r| r.push.clone())
    }

    pub(crate) fn finish(&mut self) {
        if let Some(recorder) = &self.recorder {
            recorder.push.finished.store(true, Ordering::Release);
        }
        // Stop can precede the engine's start reply. Keep the keyed owner
        // until that reply so a failed start can still cancel its writer.
    }

    pub(crate) fn confirm_start(&mut self, session_id: u64) {
        if self.session_id == Some(session_id)
            && let Some(recorder) = &self.recorder
        {
            recorder.push.start_confirmed.store(true, Ordering::Release);
        }
    }

    pub(crate) fn discard(&mut self, session_id: u64) {
        if self.session_id == Some(session_id) {
            if let Some(recorder) = &self.recorder {
                recorder.push.invalidate();
            }
            self.finish();
            self.recorder.take();
            self.session_id = None;
        }
    }

    fn cancel_unconfirmed_start(&self) {
        if let Some(recorder) = &self.recorder
            && !recorder.push.start_confirmed.load(Ordering::Acquire)
        {
            recorder.push.invalidate();
        }
    }

    #[cfg(test)]
    pub(crate) fn finish_and_wait(&mut self) {
        let completed = self.recorder.as_ref().map(|r| r.completed.clone());
        self.finish();
        if let Some(completed) = completed {
            completed
                .recv_timeout(Duration::from_secs(3))
                .expect("diagnostic writer finished");
        }
    }
}

impl Drop for DiagnosticSession {
    fn drop(&mut self) {
        self.cancel_unconfirmed_start();
        self.finish();
    }
}

fn start_writer(
    primary: &Path,
    capacity: usize,
    max_frames: u64,
    before_open: impl FnOnce() + Send + 'static,
) -> Result<Recorder, String> {
    let directory = primary.parent().unwrap_or(Path::new(".")).to_path_buf();
    start_writer_with_headroom(primary, capacity, max_frames, before_open, move || {
        disk_has_headroom(&directory)
    })
}

fn start_writer_with_headroom(
    primary: &Path,
    capacity: usize,
    max_frames: u64,
    before_open: impl FnOnce() + Send + 'static,
    mut headroom: impl FnMut() -> Result<bool, String> + Send + 'static,
) -> Result<Recorder, String> {
    let path = session_path(primary);
    let partial = partial_path(primary);
    #[cfg(test)]
    let test_partial = partial.clone();
    let (producer, receiver) = HeapRb::<Block>::new(capacity).split();
    let push = DiagnosticPush {
        producer: Arc::new(Mutex::new(producer)),
        invalid: Arc::new(AtomicBool::new(false)),
        stopped: Arc::new(AtomicBool::new(false)),
        finished: Arc::new(AtomicBool::new(false)),
        start_confirmed: Arc::new(AtomicBool::new(false)),
    };
    let worker = push.clone();
    #[cfg(test)]
    let (completed_tx, completed) = bounded(1);
    #[cfg(test)]
    let (finalized_tx, finalized) = bounded(1);
    std::thread::Builder::new()
        .name("pre-aec-diagnostic".into())
        .spawn(move || {
            // Everything that can wait on disk stays on this thread, including
            // open/finalize/rename/remove. A panic invalidates the partial too.
            let result =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<(), String> {
                    before_open();
                    if !headroom()? {
                        return Err("less than 256 MiB of free disk space".into());
                    }
                    write_capture(&partial, receiver, &worker, max_frames, &mut headroom)?;
                    #[cfg(test)]
                    let _ = finalized_tx.try_send(());
                    // A capped/low-space prefix is finalized privately but only
                    // published after both audio end and successful engine
                    // start. Stop may precede the start reply; a later Discard
                    // must still invalidate that unpublished prefix.
                    while !(worker.finished.load(Ordering::Acquire)
                        && worker.start_confirmed.load(Ordering::Acquire))
                        && !worker.invalid.load(Ordering::Acquire)
                    {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    if worker.invalid.load(Ordering::Acquire) {
                        return Err("capture lost a block".into());
                    }
                    std::fs::rename(&partial, &path).map_err(|e| format!("Publish WAV: {e}"))?;
                    Ok(())
                }));
            match result {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    worker.invalidate();
                    tracing::warn!("Pre-AEC diagnostic discarded: {e}");
                    let _ = std::fs::remove_file(&partial);
                }
                Err(_) => {
                    worker.invalidate();
                    tracing::warn!("Pre-AEC diagnostic writer panicked; artifact discarded");
                    let _ = std::fs::remove_file(&partial);
                }
            }
            #[cfg(test)]
            let _ = completed_tx.try_send(());
        })
        .map_err(|e| format!("Spawn diagnostic writer: {e}"))?;
    Ok(Recorder {
        push,
        #[cfg(test)]
        completed,
        #[cfg(test)]
        finalized,
        #[cfg(test)]
        partial: test_partial,
    })
}

fn write_capture(
    path: &Path,
    mut receiver: ringbuf::HeapCons<Block>,
    push: &DiagnosticPush,
    max_frames: u64,
    headroom: &mut impl FnMut() -> Result<bool, String>,
) -> Result<(), String> {
    // The primary recorder creates the parent, before this writer starts.
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: MIX_RATE,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut writer =
        hound::WavWriter::create(path, spec).map_err(|e| format!("Create WAV: {e}"))?;
    let mut frames_written = 0;
    let mut microphone: Option<(Source, Resampler)> = None;
    let mut next_disk_check = u64::from(MIX_RATE);
    loop {
        if push.invalid.load(Ordering::Acquire) {
            return Err("capture lost a block".into());
        }
        let block = match receiver.try_pop() {
            Some(block) => block,
            None => {
                if push.finished.load(Ordering::Acquire) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
                continue;
            }
        };
        match block.source {
            Source::Mixer => {
                if let Some((_, mut r)) = microphone.take() {
                    write_mono(&mut writer, &r.flush(), &mut frames_written, max_frames)?;
                }
                write_pairs(
                    &mut writer,
                    &block.frames[..block.len],
                    &mut frames_written,
                    max_frames,
                )?;
            }
            source @ Source::Microphone { rate, gain, .. } => {
                if microphone.as_ref().is_none_or(|(old, _)| *old != source) {
                    if let Some((_, mut r)) = microphone.take() {
                        write_mono(&mut writer, &r.flush(), &mut frames_written, max_frames)?;
                    }
                    microphone = Some((source, Resampler::new(rate, 1, MIX_RATE, gain)));
                }
                let raw: Vec<f32> = block.frames[..block.len]
                    .iter()
                    .map(|pair| pair[0])
                    .collect();
                let samples = microphone
                    .as_mut()
                    .expect("microphone initialized")
                    .1
                    .process(&raw);
                write_mono(&mut writer, &samples, &mut frames_written, max_frames)?;
            }
        }
        if frames_written == max_frames {
            // The exact, aligned prefix is useful and has a truthful WAV
            // duration. Hitting the declared size cap is not a missing block.
            push.stopped.store(true, Ordering::Release);
            break;
        }
        // A WAV is much larger than the primary Ogg. Stop its exact prefix
        // before it spends the primary's headroom. External concurrent disk
        // use can still consume this reserve; this is not an ENOSPC guarantee.
        if frames_written >= next_disk_check {
            if !headroom()? {
                push.stopped.store(true, Ordering::Release);
                break;
            }
            next_disk_check = frames_written + u64::from(MIX_RATE);
        }
    }
    if let Some((_, mut r)) = microphone {
        write_mono(&mut writer, &r.flush(), &mut frames_written, max_frames)?;
    }
    if frames_written == 0 {
        return Err("no samples captured".into());
    }
    writer.finalize().map_err(|e| format!("Finalize WAV: {e}"))
}

fn write_pairs<W: std::io::Write + std::io::Seek>(
    writer: &mut hound::WavWriter<W>,
    pairs: &[[f32; 2]],
    count: &mut u64,
    max: u64,
) -> Result<(), String> {
    for pair in pairs.iter().take(max.saturating_sub(*count) as usize) {
        write_pair(writer, *pair, count)?;
    }
    Ok(())
}

fn write_mono<W: std::io::Write + std::io::Seek>(
    writer: &mut hound::WavWriter<W>,
    samples: &[f32],
    count: &mut u64,
    max: u64,
) -> Result<(), String> {
    for sample in samples.iter().take(max.saturating_sub(*count) as usize) {
        write_pair(writer, [*sample, 0.0], count)?;
    }
    Ok(())
}

fn write_pair<W: std::io::Write + std::io::Seek>(
    writer: &mut hound::WavWriter<W>,
    pair: [f32; 2],
    count: &mut u64,
) -> Result<(), String> {
    if !pair.iter().all(|s| s.is_finite()) {
        return Err("non-finite input".into());
    }
    for sample in pair {
        writer
            .write_sample(sample)
            .map_err(|e| format!("Write WAV: {e}"))?;
    }
    *count += 1;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ringbuf::HeapRb;
    use ringbuf::traits::{Producer, Split};
    use std::time::Instant;

    fn wait_stopped(push: &DiagnosticPush) {
        let deadline = Instant::now() + Duration::from_secs(3);
        while !push.stopped.load(Ordering::Acquire) {
            assert!(
                Instant::now() < deadline,
                "diagnostic stopped at cap/headroom"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn record_real_mixer(
        primary: &Path,
        channels: opus::Channels,
        push: Option<DiagnosticPush>,
    ) -> Vec<f32> {
        record_mixer_frames(primary, channels, push, 30)
    }

    fn record_mixer_frames(
        primary: &Path,
        channels: opus::Channels,
        push: Option<DiagnosticPush>,
        frames: usize,
    ) -> Vec<f32> {
        let (mut mic, mic_cons) = HeapRb::<f32>::new(FRAME_SAMPLES).split();
        let (mut tap, tap_cons) = HeapRb::<f32>::new(FRAME_SAMPLES).split();
        let mut mixer = super::super::mixer::MeetingMixer::new(
            mic_cons, MIX_RATE, 1, 1.0, tap_cons, MIX_RATE, MIX_RATE,
        );
        mixer.set_aec(Some(super::super::aec::Aec::new_with_default_delay_hint(
            MIX_RATE,
        )));
        mixer.set_diagnostic(push);
        let recorder = super::super::recorder::MeetingRecorder::start(
            primary.to_path_buf(),
            MIX_RATE,
            1,
            channels,
        )
        .unwrap();
        let mut engine_input = Vec::new();
        for frame in 0..frames {
            let left: Vec<f32> = (0..FRAME_SAMPLES)
                .map(|i| ((frame * FRAME_SAMPLES + i) as f32 * 0.033).sin() * 0.2)
                .collect();
            let right: Vec<f32> = (0..FRAME_SAMPLES)
                .map(|i| ((frame * FRAME_SAMPLES + i) as f32 * 0.059).sin() * 0.3)
                .collect();
            mic.push_slice(&left);
            tap.push_slice(&right);
            let samples = match channels {
                opus::Channels::Mono => mixer.tick(),
                opus::Channels::Stereo => {
                    let (me, them) = mixer.tick_split();
                    super::super::recorder::interleave_stereo(&me, &them)
                }
            };
            // These are the actual mixer samples captured by the existing
            // meeting_tick before it queues the same data to the engine.
            engine_input.extend_from_slice(&samples);
            recorder.push(&samples);
        }
        let tail = match channels {
            opus::Channels::Mono => mixer.flush(),
            opus::Channels::Stereo => {
                let (me, them) = mixer.flush_split();
                super::super::recorder::interleave_stereo(&me, &them)
            }
        };
        engine_input.extend_from_slice(&tail);
        recorder.push(&tail);
        drop(recorder);
        engine_input
    }

    fn samples(path: &Path) -> Vec<f32> {
        hound::WavReader::open(path)
            .unwrap()
            .samples::<f32>()
            .map(Result::unwrap)
            .collect()
    }

    #[test]
    fn disabled_and_retention_off_create_no_writer_or_file_and_snapshot_rebuild() {
        let dir = tempfile::tempdir().unwrap();
        let primary = dir.path().join("0.ogg");
        for (enabled, path) in [(false, Some(primary.as_path())), (true, None)] {
            let mut session = DiagnosticSession::default();
            session.sync(1, path, enabled);
            assert!(session.recorder.is_none());
            assert!(session.push_handle().is_none());
            session.sync(1, Some(&primary), true);
            assert!(
                session.recorder.is_none(),
                "same-session rebuild preserves inactive snapshot"
            );
            session.finish_and_wait();
        }
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn contended_producer_invalidates_without_waiting() {
        let dir = tempfile::tempdir().unwrap();
        for channels in [opus::Channels::Mono, opus::Channels::Stereo] {
            let primary = dir.path().join(format!("{channels:?}.ogg"));
            let baseline = dir.path().join(format!("baseline-{channels:?}.ogg"));
            let (release, blocked) = bounded::<()>(1);
            let recorder = start_writer(&primary, 2, MAX_FRAMES, move || {
                blocked.recv().unwrap();
            })
            .unwrap();
            let completed = recorder.completed.clone();
            let producer = recorder.push.producer.lock().unwrap();
            let before = Instant::now();
            let actual = record_real_mixer(&primary, channels, Some(recorder.push.clone()));
            assert!(before.elapsed() < Duration::from_secs(1));
            assert!(recorder.push.invalid.load(Ordering::Acquire));
            drop(producer);
            drop(recorder);
            release.send(()).unwrap();
            completed.recv_timeout(Duration::from_secs(3)).unwrap();
            assert_eq!(actual, record_real_mixer(&baseline, channels, None));
            assert_eq!(
                std::fs::read(&primary).unwrap(),
                std::fs::read(&baseline).unwrap()
            );
            assert!(!session_path(&primary).exists());
        }
    }

    #[test]
    fn microphone_only_rebuild_resamples_raw_lanes_once_and_keeps_session() {
        let dir = tempfile::tempdir().unwrap();
        let primary = dir.path().join("0.ogg");
        let mut session = DiagnosticSession::default();
        session.sync(42, Some(&primary), true);
        session.confirm_start(42);
        let push = session.push_handle().unwrap();
        let input = vec![0.2; 1234 * 2];
        push.push_microphone(&input, 2, 44_100, 1.5, 1);
        session.sync(42, Some(&dir.path().join("ignored.ogg")), false);
        session
            .push_handle()
            .unwrap()
            .push_microphone(&[0.7; 29], 1, 48_000, 1.0, 2);
        session.finish_and_wait();
        let mut r = Resampler::new(44_100, 2, MIX_RATE, 1.5);
        let mut expected_mic = r.process(&input);
        expected_mic.extend(r.flush());
        expected_mic.extend([0.7; 29]);
        let expected: Vec<f32> = expected_mic.into_iter().flat_map(|s| [s, 0.0]).collect();
        assert_eq!(samples(&session_path(&primary)), expected);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn file_open_failure_and_nonfinite_input_never_publish_partial_diagnostics() {
        let dir = tempfile::tempdir().unwrap();
        let primary = dir.path().join("missing-parent").join("0.ogg");
        let mut session = DiagnosticSession::default();
        session.sync(1, Some(&primary), true);
        session
            .push_handle()
            .unwrap()
            .push(&[0.1; 480], &[0.2; 480]);
        session.finish_and_wait();
        assert!(!session_path(&primary).exists());
        let primary = dir.path().join("0.ogg");
        session.sync(2, Some(&primary), true);
        let partial = session.recorder.as_ref().unwrap().partial.clone();
        session.push_handle().unwrap().push(&[f32::NAN], &[0.2]);
        session.finish_and_wait();
        assert!(!session_path(&primary).exists());
        assert!(!partial.exists());
    }

    #[test]
    fn one_hour_cap_finalizes_exact_aligned_prefix_and_stops_only_diagnostics() {
        let dir = tempfile::tempdir().unwrap();
        let primary = dir.path().join("0.ogg");
        let recorder = start_writer(&primary, 8, 7, || {}).unwrap();
        recorder.push.start_confirmed.store(true, Ordering::Release);
        let push = recorder.push.clone();
        let completed = recorder.completed.clone();
        push.push(&[0.1; 11], &[0.2; 11]);
        wait_stopped(&push);
        assert!(
            !session_path(&primary).exists(),
            "capped prefix stays private until actual session end"
        );
        assert!(!push.invalid.load(Ordering::Acquire));
        push.push(&[0.9; 480], &[0.8; 480]);
        drop(recorder);
        completed.recv_timeout(Duration::from_secs(3)).unwrap();
        assert_eq!(samples(&session_path(&primary)), [0.1, 0.2].repeat(7));
    }

    #[test]
    fn blocked_writer_overflow_and_shutdown_never_block_primary_or_transcription() {
        let dir = tempfile::tempdir().unwrap();
        for channels in [opus::Channels::Mono, opus::Channels::Stereo] {
            let primary = dir.path().join(format!("{channels:?}.ogg"));
            let baseline = dir.path().join(format!("baseline-{channels:?}.ogg"));
            let (release, blocked) = bounded::<()>(1);
            let diagnostic = start_writer(&primary, 1, MAX_FRAMES, move || {
                blocked.recv().unwrap();
            })
            .unwrap();
            let push = diagnostic.push.clone();
            let completed = diagnostic.completed.clone();
            let partial = diagnostic.partial.clone();
            let before = Instant::now();
            let actual_engine_input = record_real_mixer(&primary, channels, Some(push.clone()));
            assert!(push.invalid.load(Ordering::Acquire));
            drop(diagnostic); // Must not join the blocked thread.
            assert!(before.elapsed() < Duration::from_secs(1));
            assert_eq!(
                actual_engine_input,
                record_real_mixer(&baseline, channels, None)
            );
            assert_eq!(
                std::fs::read(&primary).unwrap(),
                std::fs::read(baseline).unwrap()
            );
            release.send(()).unwrap();
            completed.recv_timeout(Duration::from_secs(3)).unwrap();
            assert!(!session_path(&primary).exists());
            assert!(!partial.exists());
        }
    }

    #[test]
    fn failed_diagnostic_open_preserves_real_mixer_and_primary_in_both_layouts() {
        let dir = tempfile::tempdir().unwrap();
        for channels in [opus::Channels::Mono, opus::Channels::Stereo] {
            let primary = dir.path().join(format!("{channels:?}.ogg"));
            let baseline = dir.path().join(format!("baseline-{channels:?}.ogg"));
            // A directory at the WAV partial path makes open fail, while the
            // real primary file in the same directory remains writable.
            let (release, blocked) = bounded::<()>(1);
            let diagnostic = start_writer(&primary, QUEUE_BLOCKS, MAX_FRAMES, move || {
                blocked.recv().unwrap();
            })
            .unwrap();
            std::fs::create_dir(&diagnostic.partial).unwrap();
            release.send(()).unwrap();
            let completed = diagnostic.completed.clone();
            let actual = record_real_mixer(&primary, channels, Some(diagnostic.push.clone()));
            completed.recv_timeout(Duration::from_secs(3)).unwrap();
            assert!(diagnostic.push.invalid.load(Ordering::Acquire));
            assert_eq!(actual, record_real_mixer(&baseline, channels, None));
            assert_eq!(
                std::fs::read(&primary).unwrap(),
                std::fs::read(&baseline).unwrap()
            );
            assert!(!session_path(&primary).exists());
        }
    }

    #[test]
    fn low_disk_headroom_stops_diagnostic_prefix_and_preserves_real_primary() {
        let dir = tempfile::tempdir().unwrap();
        for channels in [opus::Channels::Mono, opus::Channels::Stereo] {
            let primary = dir.path().join(format!("{channels:?}.ogg"));
            let baseline = dir.path().join(format!("baseline-{channels:?}.ogg"));
            let mut checks = 0;
            let recorder = start_writer_with_headroom(
                &primary,
                QUEUE_BLOCKS,
                MAX_FRAMES,
                || {},
                move || {
                    checks += 1;
                    Ok(checks == 1) // Open allowed, low space at first periodic check.
                },
            )
            .unwrap();
            recorder.push.start_confirmed.store(true, Ordering::Release);
            let completed = recorder.completed.clone();
            let actual = record_mixer_frames(&primary, channels, Some(recorder.push.clone()), 160);
            wait_stopped(&recorder.push);
            assert!(!recorder.push.invalid.load(Ordering::Acquire));
            assert!(!session_path(&primary).exists());
            drop(recorder);
            completed.recv_timeout(Duration::from_secs(3)).unwrap();
            let mut reader = hound::WavReader::open(session_path(&primary)).unwrap();
            assert_eq!(
                reader.duration(),
                MIX_RATE,
                "exact first-second prefix at injected low-space cutoff"
            );
            let expected: Vec<f32> = (0..MIX_RATE as usize)
                .flat_map(|i| {
                    [
                        (i as f32 * 0.033).sin() * 0.2,
                        (i as f32 * 0.059).sin() * 0.3,
                    ]
                })
                .collect();
            assert_eq!(
                reader
                    .samples::<f32>()
                    .map(Result::unwrap)
                    .collect::<Vec<_>>(),
                expected
            );
            assert_eq!(actual, record_mixer_frames(&baseline, channels, None, 160));
            assert_eq!(
                std::fs::read(&primary).unwrap(),
                std::fs::read(&baseline).unwrap()
            );
        }
    }

    #[test]
    fn low_disk_headroom_at_start_creates_no_diagnostic() {
        let dir = tempfile::tempdir().unwrap();
        let primary = dir.path().join("0.ogg");
        let recorder =
            start_writer_with_headroom(&primary, QUEUE_BLOCKS, MAX_FRAMES, || {}, || Ok(false))
                .unwrap();
        recorder
            .completed
            .recv_timeout(Duration::from_secs(3))
            .unwrap();
        assert!(recorder.push.invalid.load(Ordering::Acquire));
        assert!(!session_path(&primary).exists());
        assert!(!recorder.partial.exists());
    }

    #[test]
    fn stopped_pending_start_then_failed_start_discards_delayed_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let primary = dir.path().join("0.ogg");
        let (entered, at_check) = bounded::<()>(1);
        let (release, blocked) = bounded::<()>(1);
        let mut checks = 0;
        let recorder = start_writer_with_headroom(
            &primary,
            QUEUE_BLOCKS,
            MAX_FRAMES,
            || {},
            move || {
                checks += 1;
                if checks == 2 {
                    entered.send(()).unwrap();
                    blocked.recv().unwrap();
                    return Ok(false);
                }
                Ok(true)
            },
        )
        .unwrap();
        let partial = recorder.partial.clone();
        let completed = recorder.completed.clone();
        let mut session = DiagnosticSession {
            session_id: Some(7),
            recorder: Some(recorder),
        };
        session
            .push_handle()
            .unwrap()
            .push(&[0.1; 48000], &[0.2; 48000]);
        at_check.recv_timeout(Duration::from_secs(3)).unwrap();
        // halt_starting_capture sends Stop before the engine start reply.
        // AudioCapture::stop ends the diagnostic owner here.
        session.finish();
        // Then engine_actor_ready fails and AudioCapture::discard runs.
        session.discard(7);
        release.send(()).unwrap();
        completed.recv_timeout(Duration::from_secs(3)).unwrap();
        assert!(!partial.exists());
        assert!(
            !session_path(&primary).exists(),
            "Stop before failed engine readiness must not publish raw audio"
        );
    }

    #[test]
    fn stopped_pending_start_waits_for_confirmation_then_publishes_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let primary = dir.path().join("0.ogg");
        let mut session = DiagnosticSession::default();
        session.sync(7, Some(&primary), true);
        let recorder = session.recorder.as_ref().unwrap();
        let partial = recorder.partial.clone();
        let completed = recorder.completed.clone();
        let finalized = recorder.finalized.clone();
        recorder.push.push(&[0.1; 31], &[0.2; 31]);
        session.finish();
        finalized.recv_timeout(Duration::from_secs(3)).unwrap();
        assert!(partial.exists());
        assert!(!session_path(&primary).exists());
        assert_eq!(
            completed.try_recv(),
            Err(crossbeam_channel::TryRecvError::Empty)
        );
        session.confirm_start(7);
        completed.recv_timeout(Duration::from_secs(3)).unwrap();
        assert!(!partial.exists());
        assert_eq!(samples(&session_path(&primary)), [0.1, 0.2].repeat(31));
    }

    #[test]
    fn pending_start_drop_discards_finalized_prefix_without_waiting() {
        let dir = tempfile::tempdir().unwrap();
        let primary = dir.path().join("0.ogg");
        let mut session = DiagnosticSession::default();
        session.sync(7, Some(&primary), true);
        let recorder = session.recorder.as_ref().unwrap();
        let partial = recorder.partial.clone();
        let completed = recorder.completed.clone();
        let finalized = recorder.finalized.clone();
        recorder.push.push(&[0.1; 31], &[0.2; 31]);
        session.finish();
        finalized.recv_timeout(Duration::from_secs(3)).unwrap();
        let before = Instant::now();
        drop(session);
        assert!(before.elapsed() < Duration::from_secs(1));
        completed.recv_timeout(Duration::from_secs(3)).unwrap();
        assert!(!partial.exists());
        assert!(!session_path(&primary).exists());
    }

    #[test]
    fn stopped_pending_start_late_replies_cannot_touch_same_target_retry() {
        let dir = tempfile::tempdir().unwrap();
        let primary = dir.path().join("0.ogg");
        let (entered, at_check) = bounded::<()>(1);
        let (release, blocked) = bounded::<()>(1);
        let mut checks = 0;
        let old = start_writer_with_headroom(
            &primary,
            QUEUE_BLOCKS,
            MAX_FRAMES,
            || {},
            move || {
                checks += 1;
                if checks == 2 {
                    entered.send(()).unwrap();
                    blocked.recv().unwrap();
                    return Ok(false);
                }
                Ok(true)
            },
        )
        .unwrap();
        let old_partial = old.partial.clone();
        let old_done = old.completed.clone();
        let mut session = DiagnosticSession {
            session_id: Some(7),
            recorder: Some(old),
        };
        session
            .push_handle()
            .unwrap()
            .push(&[0.1; 48000], &[0.2; 48000]);
        at_check.recv_timeout(Duration::from_secs(3)).unwrap();
        session.finish();
        session.sync(8, Some(&primary), true);
        let new = session.recorder.as_ref().unwrap();
        let new_partial = new.partial.clone();
        let new_done = new.completed.clone();
        let new_finalized = new.finalized.clone();
        assert_ne!(old_partial, new_partial);
        new.push.push(&[0.7; 31], &[0.8; 31]);
        session.finish();
        new_finalized.recv_timeout(Duration::from_secs(3)).unwrap();
        session.confirm_start(7);
        session.discard(7);
        assert!(
            !session_path(&primary).exists(),
            "late confirmation did not approve retry"
        );
        assert!(
            !session
                .push_handle()
                .unwrap()
                .invalid
                .load(Ordering::Acquire)
        );
        session.confirm_start(8);
        new_done.recv_timeout(Duration::from_secs(3)).unwrap();
        let expected = [0.7, 0.8].repeat(31);
        assert_eq!(samples(&session_path(&primary)), expected);
        release.send(()).unwrap();
        old_done.recv_timeout(Duration::from_secs(3)).unwrap();
        assert!(!old_partial.exists());
        assert!(!new_partial.exists());
        session.confirm_start(7);
        session.discard(7);
        assert_eq!(samples(&session_path(&primary)), expected);
    }

    #[test]
    fn cancelled_delayed_writer_cannot_overwrite_or_delete_same_target_retry() {
        let dir = tempfile::tempdir().unwrap();
        let primary = dir.path().join("0.ogg");
        let (entered, at_check) = bounded::<()>(1);
        let (release, blocked) = bounded::<()>(1);
        let mut checks = 0;
        let old = start_writer_with_headroom(
            &primary,
            QUEUE_BLOCKS,
            MAX_FRAMES,
            || {},
            move || {
                checks += 1;
                if checks == 2 {
                    entered.send(()).unwrap();
                    blocked.recv().unwrap();
                    return Ok(false);
                }
                Ok(true)
            },
        )
        .unwrap();
        let old_partial = old.partial.clone();
        let old_done = old.completed.clone();
        old.push.push(&[0.1; 48000], &[0.2; 48000]);
        at_check.recv_timeout(Duration::from_secs(3)).unwrap();
        assert!(old_partial.exists());
        assert!(!session_path(&primary).exists());
        // This is capture::discard ordering: invalidate, then end. The old
        // writer is still blocked off-thread at its low-space check.
        old.push.invalidate();
        drop(old);

        let new = start_writer(&primary, 8, MAX_FRAMES, || {}).unwrap();
        new.push.start_confirmed.store(true, Ordering::Release);
        let new_partial = new.partial.clone();
        let new_done = new.completed.clone();
        assert_ne!(old_partial, new_partial);
        new.push.push(&[0.7; 31], &[0.8; 31]);
        drop(new);
        new_done.recv_timeout(Duration::from_secs(3)).unwrap();
        let expected = [0.7, 0.8].repeat(31);
        assert_eq!(samples(&session_path(&primary)), expected);

        release.send(()).unwrap();
        old_done.recv_timeout(Duration::from_secs(3)).unwrap();
        assert!(!old_partial.exists());
        assert!(!new_partial.exists());
        assert_eq!(
            samples(&session_path(&primary)),
            expected,
            "late cancelled worker leaves the successful retry untouched"
        );
    }
}
