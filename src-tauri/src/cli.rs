//! Headless CLI mode: `souffle --transcribe-file input.wav [...]`.
//!
//! Drives the same engine trait (`crate::engine::TranscriptionEngine`) used by
//! the app, but on the calling thread directly — no engine actor, no audio
//! capture, no Tauri window, no model download. Doubles as a performance
//! regression harness via `--repeat`.
//!
//! `try_run_headless` is called from `main` before Tauri's `run()`. It must
//! stay lenient about unrecognized arguments: macOS Finder/launchd can pass
//! flags like `-psn_0_123456` to the binary, and those must fall through to
//! the normal GUI launch rather than aborting the app.

use std::path::{Path, PathBuf};

use clap::Parser;
use rusqlite::{Connection, OpenFlags};

use crate::constants::{SAMPLE_RATE_F64, SILENCE_SUFFIX_SAMPLES};
use crate::engine::{
    TranscriptionEngine, TranscriptionProfile, TranscriptionSegment, collapse_whitespace,
    default_transcription_profile, resolve_transcription_profile, transcription_engine_catalog,
};

#[derive(Parser, Debug, Clone)]
#[command(
    name = "souffle",
    about = "Souffle headless transcription CLI",
    disable_help_subcommand = true
)]
struct CliArgs {
    /// Transcribe a WAV file through the configured engine and exit.
    #[arg(long, value_name = "WAV")]
    transcribe_file: Option<PathBuf>,

    /// Run offline speaker diarization on a WAV file and exit. Downloads the
    /// segmentation/embedding models on first use.
    #[arg(long, value_name = "WAV")]
    diarize_file: Option<PathBuf>,

    /// Override the selected transcription engine id.
    #[arg(long)]
    engine: Option<String>,

    /// Override the selected transcription model id.
    #[arg(long)]
    model: Option<String>,

    /// Override the selected transcription runtime backend id.
    #[arg(long)]
    backend: Option<String>,

    /// Emit a single machine-readable JSON object instead of text.
    #[arg(long)]
    json: bool,

    /// Run the transcription N times to measure performance. The model is
    /// loaded once; the engine's internal state is reset between runs.
    #[arg(long, default_value_t = 1)]
    repeat: u32,

    /// List every engine/model/backend combination and its install status, then exit.
    #[arg(long)]
    list_models: bool,

    /// List available engines and exit.
    #[arg(long)]
    list_engines: bool,
}

impl CliArgs {
    fn is_headless(&self) -> bool {
        self.transcribe_file.is_some() || self.diarize_file.is_some() || self.list_models || self.list_engines
    }
}

/// Entry point called from `main` before Tauri boots. Returns `Some(exit_code)`
/// when headless work ran (the caller should `std::process::exit` with it), or
/// `None` when the app should launch normally.
pub fn try_run_headless() -> Option<i32> {
    let args: Vec<String> = std::env::args().collect();
    dispatch(args)
}

/// Split out from `try_run_headless` so tests can drive it with a fixed
/// argument vector instead of the real `std::env::args()`.
fn dispatch(args: Vec<String>) -> Option<i32> {
    if !has_headless_flag(&args) {
        return None;
    }

    let cli = match CliArgs::try_parse_from(&args) {
        Ok(cli) if cli.is_headless() => cli,
        Ok(_) => return None,
        Err(e) => {
            let _ = e.print();
            return Some(if e.exit_code() == 0 { 0 } else { 1 });
        }
    };

    Some(run(cli))
}

/// Cheap pre-check over raw args, run before handing anything to clap. Only
/// our own headless flags can trigger CLI parsing; every other argument shape
/// (including whatever Finder/launchd hands the bundled binary) falls through
/// to `None` untouched.
fn has_headless_flag(args: &[String]) -> bool {
    args.iter().any(|a| {
        a == "--transcribe-file"
            || a.starts_with("--transcribe-file=")
            || a == "--diarize-file"
            || a.starts_with("--diarize-file=")
            || a == "--list-models"
            || a == "--list-engines"
    })
}

/// Where the resolved transcription profile came from — surfaced in output so
/// `--repeat` benchmark runs are reproducible and auditable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProfileSource {
    Flags,
    PersistedSettings,
    Default,
}

impl ProfileSource {
    fn as_str(self) -> &'static str {
        match self {
            ProfileSource::Flags => "flags",
            ProfileSource::PersistedSettings => "persisted_settings",
            ProfileSource::Default => "default",
        }
    }
}

fn run(cli: CliArgs) -> i32 {
    init_cli_logging(cli.json);

    if cli.list_engines {
        return list_engines(cli.json);
    }
    if cli.list_models {
        return list_models(cli.json);
    }

    if let Some(wav_path) = cli.diarize_file.clone() {
        return run_diarize(&wav_path, cli.json);
    }

    let Some(wav_path) = cli.transcribe_file.clone() else {
        eprintln!("No action requested. Use --transcribe-file, --diarize-file, --list-models, or --list-engines.");
        return 1;
    };

    let (profile, source) = match resolve_cli_profile(&cli) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("Error: {e}");
            return 1;
        }
    };

    if !crate::models::model_exists(&profile) {
        eprintln!(
            "Error: model '{} • {} • {}' is not downloaded. Download it from the app first, or run `souffle --list-models` to see what's installed.",
            profile.engine_label, profile.model_label, profile.backend_label
        );
        return 1;
    }

    let wav = match load_wav(&wav_path) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("Error: {e}");
            return 1;
        }
    };

    let mut engine = match crate::engine::create_engine(&profile) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("Error: failed to create engine: {e}");
            return 1;
        }
    };

    let model_dir = crate::models::model_dir(&profile);
    if let Err(e) = engine.load_model(&model_dir) {
        eprintln!("Error: failed to load model: {e}");
        return 1;
    }

    let target_rate = engine.audio_requirements().sample_rate_hz;
    let pcm = resample_to_engine_rate(wav.samples, wav.sample_rate, target_rate);

    let outcome = match run_transcription(engine.as_mut(), &pcm, cli.repeat) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("Error: transcription failed: {e}");
            return 2;
        }
    };

    print_outcome(&cli, &profile, source, &outcome);
    0
}

/// `--diarize-file`: load/download the two diarization models, resample the
/// WAV to 16kHz mono, run offline speaker diarization, and print the result.
fn run_diarize(wav_path: &Path, json: bool) -> i32 {
    if !crate::diarize::models::models_downloaded()
        && let Err(e) = crate::diarize::models::download_models(&|progress| {
            eprintln!(
                "Downloading {} ({}/{})...",
                progress.file, progress.completed_files, progress.total_files
            );
        })
    {
        eprintln!("Error: failed to download diarization models: {e}");
        return 1;
    }

    let wav = match load_wav(wav_path) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("Error: {e}");
            return 1;
        }
    };

    let samples = resample_to_engine_rate(wav.samples, wav.sample_rate, crate::diarize::segmentation::SAMPLE_RATE);

    let cfg = crate::diarize::DiarizeConfig::new(
        crate::diarize::models::segmentation_model_path(),
        crate::diarize::models::embedding_model_path(),
    );

    let result = match crate::diarize::diarize(&samples, crate::diarize::segmentation::SAMPLE_RATE, &cfg) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Error: diarization failed: {e}");
            return 2;
        }
    };

    let stored = load_stored_speakers_for_calibration();
    print_diarization_result(&result, &stored, json);
    0
}

/// One stored speaker's id, name, and decoded embeddings, loaded for the
/// `--diarize-file` calibration report.
struct CalibrationSpeaker {
    id: i64,
    name: String,
    embeddings: Vec<Vec<f32>>,
}

/// A `load_calibration_speakers` failure, distinguishing "this database
/// predates the feature" (silent) from every other failure (reported).
enum CalibrationLoadError {
    /// `speakers` or `speaker_embeddings` doesn't exist: a pre-v13 (or
    /// pre-v12) database that has simply never had persistent speakers.
    MissingTable,
    Other(rusqlite::Error),
}

impl From<rusqlite::Error> for CalibrationLoadError {
    fn from(e: rusqlite::Error) -> Self {
        let missing_table = matches!(
            &e,
            rusqlite::Error::SqliteFailure(_, Some(msg)) if msg.contains("no such table")
        );
        if missing_table {
            CalibrationLoadError::MissingTable
        } else {
            CalibrationLoadError::Other(e)
        }
    }
}

/// Stored speakers for the `--diarize-file` calibration report, read
/// straight from `souffle.db` in read-only mode: this is a diagnostic tool
/// run against whatever database the app happens to have, not a codepath
/// that should ever create, migrate, or lock one for writing. Three
/// outcomes: no database file yet is a silent empty list (the app has never
/// been launched); a database that predates the `speaker_embeddings` table
/// (schema v13) is also a silent empty list (there's nothing to have
/// migrated); any other failure (permissions, corruption, a locked file) is
/// reported on stderr before falling back to an empty list, since silently
/// swallowing those would make "No stored speakers" a misleading message.
fn load_stored_speakers_for_calibration() -> Vec<CalibrationSpeaker> {
    let db_path = crate::constants::app_data_dir().join("souffle.db");
    if !db_path.exists() {
        return Vec::new();
    }
    match load_calibration_speakers(&db_path) {
        Ok(speakers) => speakers,
        Err(CalibrationLoadError::MissingTable) => Vec::new(),
        Err(CalibrationLoadError::Other(e)) => {
            eprintln!(
                "Warning: could not read stored speakers from '{}': {e}",
                db_path.display()
            );
            Vec::new()
        }
    }
}

fn load_calibration_speakers(db_path: &Path) -> Result<Vec<CalibrationSpeaker>, CalibrationLoadError> {
    let conn = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;

    let mut speakers_stmt = conn.prepare("SELECT id, name FROM speakers ORDER BY id")?;
    let speakers: Vec<(i64, String)> = speakers_stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<Result<Vec<_>, _>>()?;

    let mut embeddings_stmt =
        conn.prepare("SELECT speaker_id, embedding FROM speaker_embeddings ORDER BY speaker_id, id")?;
    let embedding_rows: Vec<(i64, Vec<u8>)> = embeddings_stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    drop(embeddings_stmt);

    let mut by_speaker: std::collections::HashMap<i64, Vec<Vec<f32>>> = std::collections::HashMap::new();
    for (speaker_id, blob) in embedding_rows {
        if let Some(decoded) = crate::diarize::persist::decode_embedding(&blob) {
            by_speaker.entry(speaker_id).or_default().push(decoded);
        }
    }

    Ok(speakers
        .into_iter()
        .map(|(id, name)| CalibrationSpeaker {
            id,
            name,
            embeddings: by_speaker.remove(&id).unwrap_or_default(),
        })
        .collect())
}

/// Cosine similarity of `embedding` against the MAX-similarity embedding of
/// each stored speaker, for calibrating `persist::MATCH_THRESHOLD`/
/// `MATCH_MARGIN` against real recordings. `None` for a stored speaker with
/// no embeddings recorded yet.
fn max_similarity_per_speaker<'a>(
    embedding: &[f32],
    stored: &'a [CalibrationSpeaker],
) -> Vec<(i64, &'a str, Option<f32>)> {
    stored
        .iter()
        .map(|s| {
            (
                s.id,
                s.name.as_str(),
                crate::diarize::persist::max_similarity(embedding, &s.embeddings),
            )
        })
        .collect()
}

fn print_diarization_result(
    result: &crate::diarize::DiarizationResult,
    stored: &[CalibrationSpeaker],
    json: bool,
) {
    if json {
        let json_value = serde_json::json!({
            "speaker_count": result.speakers.len(),
            "segments": result.segments.iter().map(|s| serde_json::json!({
                "start_ms": s.start_ms,
                "end_ms": s.end_ms,
                "speaker": s.speaker,
            })).collect::<Vec<_>>(),
            "clusters": result.speakers.iter().map(|c| serde_json::json!({
                "speaker": c.speaker,
                "speech_seconds": c.speech_seconds,
                "similarities": max_similarity_per_speaker(&c.embedding, stored).into_iter()
                    .map(|(id, name, sim)| serde_json::json!({
                        "speaker_id": id,
                        "name": name,
                        "max_similarity": sim,
                    }))
                    .collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
        });
        println!("{}", serde_json::to_string(&json_value).unwrap_or_default());
        return;
    }

    println!("Detected {} speaker(s), {} segment(s)", result.speakers.len(), result.segments.len());
    for seg in &result.segments {
        println!(
            "[{:>8.2}s .. {:>8.2}s] speaker {}",
            seg.start_ms as f64 / 1000.0,
            seg.end_ms as f64 / 1000.0,
            seg.speaker
        );
    }

    println!();
    if stored.is_empty() {
        println!("No stored speakers in the database; nothing to calibrate against.");
        return;
    }
    println!("Calibration: cluster similarity against stored speakers");
    for cluster in &result.speakers {
        println!("  Cluster {} ({:.1}s of speech):", cluster.speaker, cluster.speech_seconds);
        for (id, name, sim) in max_similarity_per_speaker(&cluster.embedding, stored) {
            match sim {
                Some(sim) => println!("    vs speaker {id} '{name}': {sim:.3}"),
                None => println!("    vs speaker {id} '{name}': no embeddings recorded"),
            }
        }
    }
}

fn init_cli_logging(quiet: bool) {
    use tracing_subscriber::EnvFilter;

    let default_level = if quiet { "warn" } else { "info" };
    let filter = EnvFilter::try_from_env("SOUFFLE_LOG")
        .unwrap_or_else(|_| EnvFilter::new(format!("souffle={default_level},warn")));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init();
}

/// Resolve the profile to run: explicit flags win outright; otherwise fall
/// back to the user's persisted selection in `souffle.db`; if that can't be
/// read, fall back to the hardcoded default profile and say so on stderr.
fn resolve_cli_profile(cli: &CliArgs) -> Result<(TranscriptionProfile, ProfileSource), String> {
    if cli.engine.is_some() || cli.model.is_some() || cli.backend.is_some() {
        let profile = resolve_transcription_profile(
            cli.engine.as_deref(),
            cli.model.as_deref(),
            cli.backend.as_deref(),
        )?;
        return Ok((profile, ProfileSource::Flags));
    }

    match load_persisted_profile() {
        Ok(profile) => Ok((profile, ProfileSource::PersistedSettings)),
        Err(e) => {
            eprintln!(
                "Warning: could not read persisted transcription settings ({e}); using the default profile instead."
            );
            Ok((default_transcription_profile(), ProfileSource::Default))
        }
    }
}

fn load_persisted_profile() -> Result<TranscriptionProfile, String> {
    let db_path = crate::constants::app_data_dir().join("souffle.db");
    let db = crate::db::Database::open(&db_path)?;
    let settings = crate::settings::AppSettings::load(&db)?;
    resolve_transcription_profile(
        Some(&settings.transcription_engine_id),
        Some(&settings.transcription_model_id),
        Some(&settings.transcription_backend_id),
    )
}

#[derive(Debug)]
struct WavAudio {
    samples: Vec<f32>,
    sample_rate: u32,
}

/// Load a mono WAV file as f32 samples in `[-1.0, 1.0]`. Supports 16-bit PCM
/// and 32-bit float only — the two formats every capture/export path in this
/// app produces.
fn load_wav(path: &Path) -> Result<WavAudio, String> {
    let mut reader = hound::WavReader::open(path)
        .map_err(|e| format!("Failed to open WAV '{}': {e}", path.display()))?;
    let spec = reader.spec();

    if spec.channels != 1 {
        return Err(format!(
            "WAV file '{}' has {} channels; only mono WAV files are supported. Convert with e.g. `ffmpeg -i input.wav -ac 1 output.wav`.",
            path.display(),
            spec.channels
        ));
    }

    let samples: Vec<f32> = match (spec.sample_format, spec.bits_per_sample) {
        (hound::SampleFormat::Float, 32) => reader
            .samples::<f32>()
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to read WAV samples from '{}': {e}", path.display()))?,
        (hound::SampleFormat::Int, 16) => reader
            .samples::<i16>()
            .map(|s| s.map(|v| v as f32 / i16::MAX as f32))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to read WAV samples from '{}': {e}", path.display()))?,
        (format, bits) => {
            return Err(format!(
                "Unsupported WAV format ({format:?}, {bits}-bit) in '{}'; only 16-bit PCM and 32-bit float WAV are supported.",
                path.display()
            ));
        }
    };

    Ok(WavAudio {
        samples,
        sample_rate: spec.sample_rate,
    })
}

/// Resample to the engine's required rate using the same rubato-backed
/// resampler the live capture pipeline uses. A no-op when the rates already
/// match. No gain is applied: unlike live mic input, file audio isn't
/// systematically quiet.
fn resample_to_engine_rate(samples: Vec<f32>, source_rate: u32, target_rate: u32) -> Vec<f32> {
    if source_rate == target_rate || samples.is_empty() {
        return samples;
    }
    let mut resampler = crate::audio::Resampler::new(source_rate, 1, target_rate, 1.0);
    let mut out = resampler.process(&samples);
    out.extend(resampler.flush());
    out
}

/// 1.5s of silence (the app's flush suffix, `SILENCE_SUFFIX_SAMPLES` at
/// `SAMPLE_RATE`) expressed in the given engine's own sample rate.
fn silence_suffix_samples(engine_sample_rate_hz: u32) -> usize {
    let seconds = SILENCE_SUFFIX_SAMPLES as f64 / SAMPLE_RATE_F64;
    (seconds * engine_sample_rate_hz as f64).round() as usize
}

pub struct RunStat {
    pub wall_ms: f64,
    pub rtf: f64,
}

pub struct RunOutcome {
    pub text: String,
    pub audio_seconds: f64,
    pub runs: Vec<RunStat>,
}

/// Drive `engine` directly (no actor thread): feed `pcm` in the engine's
/// native chunk size, flush, and repeat `repeat` times reusing the loaded
/// model (only `reset_state` runs between repeats, mirroring how `--repeat`
/// is meant to measure steady-state inference cost, not load cost).
fn run_transcription(
    engine: &mut dyn TranscriptionEngine,
    pcm: &[f32],
    repeat: u32,
) -> Result<RunOutcome, String> {
    let requirements = engine.audio_requirements();
    let sample_rate = requirements.sample_rate_hz as f64;
    let audio_seconds = pcm.len() as f64 / sample_rate;
    let chunk_size = requirements.chunk_size_samples.max(1) as usize;

    let mut padded = pcm.to_vec();
    padded.resize(
        padded.len() + silence_suffix_samples(requirements.sample_rate_hz),
        0.0,
    );

    let repeat = repeat.max(1);
    let mut runs = Vec::with_capacity(repeat as usize);
    let mut last_text = String::new();

    for i in 0..repeat {
        if i > 0 {
            engine
                .reset_state()
                .map_err(|e| format!("reset_state failed: {e}"))?;
        }

        let start = std::time::Instant::now();
        let mut segments = Vec::new();
        for chunk in padded.chunks(chunk_size) {
            let frame = if chunk.len() < chunk_size {
                let mut buf = chunk.to_vec();
                buf.resize(chunk_size, 0.0);
                buf
            } else {
                chunk.to_vec()
            };
            let mut produced = engine
                .transcribe(&frame, None)
                .map_err(|e| format!("transcribe failed: {e}"))?;
            segments.append(&mut produced);
        }
        let mut flushed = engine.flush().map_err(|e| format!("flush failed: {e}"))?;
        segments.append(&mut flushed);

        let elapsed = start.elapsed().as_secs_f64();
        let rtf = if elapsed > 0.0 {
            audio_seconds / elapsed
        } else {
            0.0
        };
        runs.push(RunStat {
            wall_ms: elapsed * 1000.0,
            rtf,
        });

        last_text = join_segments(engine, &segments);
    }

    Ok(RunOutcome {
        text: last_text,
        audio_seconds,
        runs,
    })
}

/// Engine-specific token normalization only (no filler/stutter/dictionary
/// filter chain — that's pipeline post-processing, not part of the engine's
/// own output contract).
fn join_segments(engine: &dyn TranscriptionEngine, segments: &[TranscriptionSegment]) -> String {
    let parts: Vec<String> = segments
        .iter()
        .map(|s| engine.normalize_text(&s.text))
        .filter(|s| !s.trim().is_empty())
        .collect();
    collapse_whitespace(&parts.join(" "))
}

struct RunSummary {
    best_wall_ms: f64,
    median_wall_ms: f64,
    best_rtf: f64,
    median_rtf: f64,
}

fn summarize_runs(runs: &[RunStat]) -> RunSummary {
    let mut wall: Vec<f64> = runs.iter().map(|r| r.wall_ms).collect();
    wall.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mut rtf: Vec<f64> = runs.iter().map(|r| r.rtf).collect();
    rtf.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    RunSummary {
        best_wall_ms: wall.first().copied().unwrap_or(0.0),
        median_wall_ms: median(&wall),
        best_rtf: rtf.last().copied().unwrap_or(0.0),
        median_rtf: median(&rtf),
    }
}

fn median(sorted: &[f64]) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let mid = sorted.len() / 2;
    if sorted.len().is_multiple_of(2) {
        (sorted[mid - 1] + sorted[mid]) / 2.0
    } else {
        sorted[mid]
    }
}

fn print_outcome(
    cli: &CliArgs,
    profile: &TranscriptionProfile,
    source: ProfileSource,
    outcome: &RunOutcome,
) {
    if cli.json {
        let json = serde_json::json!({
            "text": outcome.text,
            "audio_seconds": outcome.audio_seconds,
            "engine": profile.engine_id,
            "model": profile.model_id,
            "backend": profile.backend_id,
            "profile_source": source.as_str(),
            "runs": outcome.runs.iter().map(|r| serde_json::json!({
                "wall_ms": r.wall_ms,
                "rtf": r.rtf,
            })).collect::<Vec<_>>(),
        });
        println!("{}", serde_json::to_string(&json).unwrap_or_default());
        return;
    }

    println!(
        "Engine: {} / {} / {} (profile: {})",
        profile.engine_label,
        profile.model_label,
        profile.backend_label,
        source.as_str()
    );
    println!("Audio duration: {:.2}s", outcome.audio_seconds);
    for (i, r) in outcome.runs.iter().enumerate() {
        println!(
            "Run {}: {:.1} ms wall, {:.2}x realtime",
            i + 1,
            r.wall_ms,
            r.rtf
        );
    }
    if outcome.runs.len() > 1 {
        let summary = summarize_runs(&outcome.runs);
        println!(
            "Best: {:.1} ms ({:.2}x realtime)  Median: {:.1} ms ({:.2}x realtime)",
            summary.best_wall_ms, summary.best_rtf, summary.median_wall_ms, summary.median_rtf
        );
    }
    println!();
    println!("{}", outcome.text);
}

fn list_engines(json: bool) -> i32 {
    let catalog = transcription_engine_catalog();
    if json {
        let items: Vec<_> = catalog
            .iter()
            .map(|e| serde_json::json!({ "id": e.id, "label": e.label }))
            .collect();
        println!("{}", serde_json::to_string_pretty(&items).unwrap_or_default());
    } else {
        for e in &catalog {
            println!("{:<10} {}", e.id, e.label);
        }
    }
    0
}

fn list_models(json: bool) -> i32 {
    let catalog = transcription_engine_catalog();
    let mut rows: Vec<(TranscriptionProfile, bool)> = Vec::new();
    for engine in &catalog {
        for model in &engine.models {
            for backend in &model.backends {
                let profile = TranscriptionProfile {
                    engine_id: engine.id.clone(),
                    engine_label: engine.label.clone(),
                    model_id: model.id.clone(),
                    model_label: model.label.clone(),
                    backend_id: backend.id.clone(),
                    backend_label: backend.label.clone(),
                };
                let installed = crate::models::model_exists(&profile);
                rows.push((profile, installed));
            }
        }
    }

    if json {
        let items: Vec<_> = rows
            .iter()
            .map(|(p, installed)| {
                serde_json::json!({
                    "engine": p.engine_id,
                    "model": p.model_id,
                    "backend": p.backend_id,
                    "installed": installed,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&items).unwrap_or_default());
    } else {
        for (p, installed) in &rows {
            println!(
                "{:<10} {:<22} {:<14} {}",
                p.engine_id,
                p.model_id,
                p.backend_id,
                if *installed { "installed" } else { "not installed" }
            );
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::mock::MockEngine;

    fn segment(text: &str) -> TranscriptionSegment {
        TranscriptionSegment {
            text: text.to_string(),
            start_time: 0.0,
            end_time: 0.0,
            is_final: true,
            language: None,
            confidence: None,
            speaker: None,
        }
    }

    fn write_wav(
        dir: &std::path::Path,
        name: &str,
        sample_rate: u32,
        sample_format: hound::SampleFormat,
        bits_per_sample: u16,
        channels: u16,
        samples_f32: &[f32],
    ) -> PathBuf {
        let path = dir.join(name);
        let spec = hound::WavSpec {
            channels,
            sample_rate,
            bits_per_sample,
            sample_format,
        };
        let mut writer = hound::WavWriter::create(&path, spec).unwrap();
        match sample_format {
            hound::SampleFormat::Float => {
                for &s in samples_f32 {
                    writer.write_sample(s).unwrap();
                }
            }
            hound::SampleFormat::Int => {
                for &s in samples_f32 {
                    writer.write_sample((s * i16::MAX as f32) as i16).unwrap();
                }
            }
        }
        writer.finalize().unwrap();
        path
    }

    // ── has_headless_flag / dispatch ────────────────────────────────

    #[test]
    fn has_headless_flag_detects_transcribe_file() {
        let args = vec!["souffle".to_string(), "--transcribe-file".to_string(), "x.wav".to_string()];
        assert!(has_headless_flag(&args));
    }

    #[test]
    fn has_headless_flag_detects_equals_form() {
        let args = vec!["souffle".to_string(), "--transcribe-file=x.wav".to_string()];
        assert!(has_headless_flag(&args));
    }

    #[test]
    fn has_headless_flag_detects_diarize_file() {
        let args = vec!["souffle".to_string(), "--diarize-file".to_string(), "x.wav".to_string()];
        assert!(has_headless_flag(&args));
    }

    #[test]
    fn has_headless_flag_detects_diarize_file_equals_form() {
        let args = vec!["souffle".to_string(), "--diarize-file=x.wav".to_string()];
        assert!(has_headless_flag(&args));
    }

    #[test]
    fn has_headless_flag_detects_list_models() {
        let args = vec!["souffle".to_string(), "--list-models".to_string()];
        assert!(has_headless_flag(&args));
    }

    #[test]
    fn has_headless_flag_detects_list_engines() {
        let args = vec!["souffle".to_string(), "--list-engines".to_string()];
        assert!(has_headless_flag(&args));
    }

    #[test]
    fn has_headless_flag_false_for_normal_launch() {
        let args = vec!["souffle".to_string()];
        assert!(!has_headless_flag(&args));
    }

    #[test]
    fn has_headless_flag_false_for_finder_psn_arg() {
        // macOS Finder/launchd can pass a process-serial-number arg; this must
        // never be mistaken for a headless request.
        let args = vec!["souffle".to_string(), "-psn_0_123456".to_string()];
        assert!(!has_headless_flag(&args));
    }

    #[test]
    fn dispatch_returns_none_without_headless_flag() {
        let args = vec!["souffle".to_string(), "-psn_0_123456".to_string()];
        assert_eq!(dispatch(args), None);
    }

    #[test]
    fn dispatch_returns_none_for_bare_launch() {
        let args = vec!["souffle".to_string()];
        assert_eq!(dispatch(args), None);
    }

    // ── CliArgs parsing ─────────────────────────────────────────────

    #[test]
    fn parses_transcribe_file_with_overrides() {
        let args = [
            "souffle",
            "--transcribe-file",
            "in.wav",
            "--engine",
            "whisper",
            "--model",
            "turbo",
            "--backend",
            "whisper-rs",
            "--json",
            "--repeat",
            "3",
        ];
        let cli = CliArgs::try_parse_from(args).unwrap();
        assert_eq!(cli.transcribe_file, Some(PathBuf::from("in.wav")));
        assert_eq!(cli.engine.as_deref(), Some("whisper"));
        assert_eq!(cli.model.as_deref(), Some("turbo"));
        assert_eq!(cli.backend.as_deref(), Some("whisper-rs"));
        assert!(cli.json);
        assert_eq!(cli.repeat, 3);
        assert!(cli.is_headless());
    }

    #[test]
    fn parses_diarize_file_with_json() {
        let args = ["souffle", "--diarize-file", "in.wav", "--json"];
        let cli = CliArgs::try_parse_from(args).unwrap();
        assert_eq!(cli.diarize_file, Some(PathBuf::from("in.wav")));
        assert!(cli.json);
        assert!(cli.is_headless());
    }

    #[test]
    fn repeat_defaults_to_one() {
        let cli = CliArgs::try_parse_from(["souffle", "--transcribe-file", "in.wav"]).unwrap();
        assert_eq!(cli.repeat, 1);
    }

    #[test]
    fn list_models_flag_is_headless() {
        let cli = CliArgs::try_parse_from(["souffle", "--list-models"]).unwrap();
        assert!(cli.is_headless());
    }

    #[test]
    fn plain_parse_without_flags_is_not_headless() {
        let cli = CliArgs::try_parse_from(["souffle"]).unwrap();
        assert!(!cli.is_headless());
    }

    // ── WAV loading ─────────────────────────────────────────────────

    #[test]
    fn load_wav_reads_i16_mono() {
        let dir = tempfile::tempdir().unwrap();
        let samples = vec![0.0f32, 0.5, -0.5, 0.25];
        let path = write_wav(
            dir.path(),
            "i16.wav",
            16_000,
            hound::SampleFormat::Int,
            16,
            1,
            &samples,
        );
        let wav = load_wav(&path).unwrap();
        assert_eq!(wav.sample_rate, 16_000);
        assert_eq!(wav.samples.len(), 4);
        assert!((wav.samples[1] - 0.5).abs() < 0.01);
    }

    #[test]
    fn load_wav_reads_f32_mono() {
        let dir = tempfile::tempdir().unwrap();
        let samples = vec![0.1f32, -0.2, 0.3];
        let path = write_wav(
            dir.path(),
            "f32.wav",
            24_000,
            hound::SampleFormat::Float,
            32,
            1,
            &samples,
        );
        let wav = load_wav(&path).unwrap();
        assert_eq!(wav.sample_rate, 24_000);
        assert_eq!(wav.samples.len(), 3);
        assert!((wav.samples[0] - 0.1).abs() < 1e-6);
    }

    #[test]
    fn load_wav_rejects_multichannel() {
        let dir = tempfile::tempdir().unwrap();
        let samples = vec![0.0f32; 8];
        let path = write_wav(
            dir.path(),
            "stereo.wav",
            16_000,
            hound::SampleFormat::Int,
            16,
            2,
            &samples,
        );
        let err = load_wav(&path).unwrap_err();
        assert!(err.contains("channels"));
    }

    #[test]
    fn load_wav_missing_file_errors() {
        let err = load_wav(Path::new("/nonexistent/path/does-not-exist.wav")).unwrap_err();
        assert!(err.contains("Failed to open WAV"));
    }

    // ── Resampling ──────────────────────────────────────────────────

    #[test]
    fn resample_noop_when_rates_match() {
        let samples = vec![0.1f32, 0.2, 0.3];
        let out = resample_to_engine_rate(samples.clone(), 16_000, 16_000);
        assert_eq!(out, samples);
    }

    #[test]
    fn resample_produces_output_for_different_rate() {
        let samples = vec![0.1f32; 4000];
        let out = resample_to_engine_rate(samples, 16_000, 24_000);
        assert!(!out.is_empty());
    }

    // ── silence suffix / stats ──────────────────────────────────────

    #[test]
    fn silence_suffix_scales_with_engine_rate() {
        assert_eq!(silence_suffix_samples(24_000), SILENCE_SUFFIX_SAMPLES);
        assert_eq!(silence_suffix_samples(16_000), 24_000);
    }

    #[test]
    fn median_odd_count() {
        assert_eq!(median(&[1.0, 2.0, 3.0]), 2.0);
    }

    #[test]
    fn median_even_count() {
        assert_eq!(median(&[1.0, 2.0, 3.0, 4.0]), 2.5);
    }

    #[test]
    fn median_empty() {
        assert_eq!(median(&[]), 0.0);
    }

    #[test]
    fn summarize_runs_best_and_median() {
        let runs = vec![
            RunStat { wall_ms: 100.0, rtf: 2.0 },
            RunStat { wall_ms: 50.0, rtf: 4.0 },
            RunStat { wall_ms: 75.0, rtf: 3.0 },
        ];
        let summary = summarize_runs(&runs);
        assert_eq!(summary.best_wall_ms, 50.0);
        assert_eq!(summary.median_wall_ms, 75.0);
        assert_eq!(summary.best_rtf, 4.0);
        assert_eq!(summary.median_rtf, 3.0);
    }

    // ── Orchestration against MockEngine ─────────────────────────────

    #[test]
    fn run_transcription_joins_segments_and_times_the_run() {
        let mut engine = MockEngine::new()
            .with_transcribe_response(Ok(vec![segment("hello")]), 1)
            .with_flush_response(Ok(vec![segment("world")]));
        let pcm = vec![0.1f32; crate::constants::MIMI_FRAME_SIZE * 2];

        let outcome = run_transcription(&mut engine, &pcm, 1).unwrap();

        assert_eq!(outcome.text, "hello world");
        assert_eq!(outcome.runs.len(), 1);
        assert!(outcome.runs[0].wall_ms >= 0.0);
        assert!(outcome.audio_seconds > 0.0);
    }

    #[test]
    fn run_transcription_repeats_and_resets_state_between_runs() {
        let mut engine = MockEngine::new()
            .with_transcribe_response(Ok(vec![]), 1)
            .with_flush_response(Ok(vec![]));
        let pcm = vec![0.0f32; 100];

        let outcome = run_transcription(&mut engine, &pcm, 3).unwrap();

        assert_eq!(outcome.runs.len(), 3);
    }

    #[test]
    fn run_transcription_zero_repeat_runs_once() {
        let mut engine = MockEngine::new().with_flush_response(Ok(vec![]));
        let pcm = vec![0.0f32; 100];

        let outcome = run_transcription(&mut engine, &pcm, 0).unwrap();

        assert_eq!(outcome.runs.len(), 1);
    }

    #[test]
    fn run_transcription_propagates_transcribe_error() {
        let mut engine = MockEngine::new().with_transcribe_response(
            Err(crate::engine::EngineError::InferenceError("boom".into())),
            1,
        );
        let pcm = vec![0.0f32; crate::constants::MIMI_FRAME_SIZE];

        let result = run_transcription(&mut engine, &pcm, 1);

        assert!(result.is_err());
    }

    #[test]
    fn run_transcription_empty_audio_still_flushes() {
        let mut engine = MockEngine::new().with_flush_response(Ok(vec![segment("flushed")]));
        let outcome = run_transcription(&mut engine, &[], 1).unwrap();
        assert_eq!(outcome.text, "flushed");
        assert_eq!(outcome.audio_seconds, 0.0);
    }
}
