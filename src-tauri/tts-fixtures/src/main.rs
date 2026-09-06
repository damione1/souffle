//! Renders the spoken-audio fixtures the local engine tests read.
//!
//! The engine tests need speech with silences of an exact, chosen length
//! between clauses. A recording cannot give that, and a person cannot pause
//! for exactly 300 ms on request, so the audio is synthesized and the gaps
//! are inserted as digital silence. That is the part under test: the speech
//! backend only has to produce intelligible clauses.
//!
//! Fixtures are committed rather than generated at test time. Speech
//! synthesis output drifts between macOS releases, and a fixture that
//! changes underneath a measurement makes the measurement meaningless.
//!
//! Usage:
//!   cargo run -p souffle-tts-fixtures -- <spec.toml> [--only <name>] [--dry-run]

use std::collections::HashMap;
use std::error::Error;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

type Res<T> = Result<T, Box<dyn Error>>;

/// One rendered WAV: lead silence, then clauses separated by the requested
/// gaps, then trailing silence.
#[derive(Debug, Clone, serde::Deserialize)]
struct Fixture {
    name: String,
    clauses: Vec<String>,
    /// Silence between clause i and clause i+1. Needs `clauses.len() - 1`
    /// entries; a single value is reused between every pair.
    #[serde(default)]
    gaps_ms: Vec<u32>,
    #[serde(default = "default_lead_ms")]
    lead_ms: u32,
    #[serde(default = "default_trail_ms")]
    trail_ms: u32,
}

/// Expands to one `Fixture` per entry in `gaps_ms`, all sharing the same two
/// clauses. This is how a threshold ladder is described.
#[derive(Debug, Clone, serde::Deserialize)]
struct Ladder {
    name: String,
    clauses: Vec<String>,
    gaps_ms: Vec<u32>,
    #[serde(default = "default_lead_ms")]
    lead_ms: u32,
    #[serde(default = "default_trail_ms")]
    trail_ms: u32,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(tag = "kind")]
enum Backend {
    /// macOS `say`. Present on every dev machine, deterministic, and needs
    /// no model download. See the skill for why this is not Kyutai TTS.
    #[serde(rename = "macos_say")]
    MacosSay {
        voice: String,
        #[serde(default = "default_rate_wpm")]
        rate_wpm: u32,
    },
}

#[derive(Debug, serde::Deserialize)]
struct Spec {
    #[serde(default = "default_sample_rate")]
    sample_rate_hz: u32,
    /// Where the WAVs land, relative to the repo's `src-tauri` directory.
    out_dir: String,
    backend: Backend,
    #[serde(default)]
    ladder: Vec<Ladder>,
    #[serde(default)]
    fixture: Vec<Fixture>,
}

fn default_sample_rate() -> u32 {
    16_000
}
fn default_lead_ms() -> u32 {
    300
}
fn default_trail_ms() -> u32 {
    800
}
fn default_rate_wpm() -> u32 {
    180
}

impl Ladder {
    fn expand(&self) -> Vec<Fixture> {
        self.gaps_ms
            .iter()
            .map(|gap| Fixture {
                name: format!("{}-gap{gap:04}ms", self.name),
                clauses: self.clauses.clone(),
                gaps_ms: vec![*gap],
                lead_ms: self.lead_ms,
                trail_ms: self.trail_ms,
            })
            .collect()
    }
}

impl Fixture {
    /// Gap before clause `idx`, for `idx` in `1..clauses.len()`. A spec with
    /// one gap value reuses it between every pair.
    fn gap_before(&self, idx: usize) -> Res<u32> {
        match self.gaps_ms.len() {
            0 => Err(format!("fixture {}: no gaps_ms", self.name).into()),
            1 => Ok(self.gaps_ms[0]),
            n if n == self.clauses.len() - 1 => Ok(self.gaps_ms[idx - 1]),
            n => Err(format!(
                "fixture {}: {n} gaps for {} clauses, expected 1 or {}",
                self.name,
                self.clauses.len(),
                self.clauses.len() - 1
            )
            .into()),
        }
    }
}

/// Turns a clause into mono PCM at the spec's sample rate.
trait SpeechBackend {
    fn synthesize(&self, text: &str, sample_rate_hz: u32) -> Res<Vec<i16>>;
    fn describe(&self) -> String;
}

struct MacosSay {
    voice: String,
    rate_wpm: u32,
}

impl SpeechBackend for MacosSay {
    fn synthesize(&self, text: &str, sample_rate_hz: u32) -> Res<Vec<i16>> {
        if !cfg!(target_os = "macos") {
            return Err("macos_say requires macOS (`say` is not available)".into());
        }
        let tmp = TempWav::new();
        let status = Command::new("say")
            .arg("-v")
            .arg(&self.voice)
            .arg("-r")
            .arg(self.rate_wpm.to_string())
            .arg(format!("--data-format=LEI16@{sample_rate_hz}"))
            .arg("--file-format=WAVE")
            .arg("-o")
            .arg(&tmp.0)
            .arg(text)
            .status()?;
        if !status.success() {
            return Err(format!("say failed for {text:?} (voice {})", self.voice).into());
        }
        read_mono_i16(&tmp.0, sample_rate_hz)
    }

    fn describe(&self) -> String {
        format!("macOS say (voice {}, {} wpm)", self.voice, self.rate_wpm)
    }
}

/// Unique temp WAV that is always deleted, including on `say` / decode errors.
struct TempWav(PathBuf);

impl TempWav {
    fn new() -> Self {
        let n = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
        Self(
            std::env::temp_dir().join(format!("souffle-tts-clause-{}-{n}.wav", std::process::id())),
        )
    }
}

impl Drop for TempWav {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn read_mono_i16(path: &Path, expect_rate: u32) -> Res<Vec<i16>> {
    let mut reader = hound::WavReader::open(path)?;
    let spec = reader.spec();
    if spec.channels != 1 {
        return Err(format!(
            "{}: expected mono, got {} channels",
            path.display(),
            spec.channels
        )
        .into());
    }
    if spec.sample_rate != expect_rate {
        return Err(format!(
            "{}: expected {expect_rate} Hz, got {} Hz",
            path.display(),
            spec.sample_rate
        )
        .into());
    }
    if spec.bits_per_sample != 16 || spec.sample_format != hound::SampleFormat::Int {
        return Err(format!(
            "{}: expected 16-bit PCM, got {:?}-bit {:?}",
            path.display(),
            spec.bits_per_sample,
            spec.sample_format
        )
        .into());
    }
    Ok(reader.samples::<i16>().collect::<Result<Vec<_>, _>>()?)
}

fn silence(ms: u32, sample_rate_hz: u32) -> Vec<i16> {
    vec![0i16; (sample_rate_hz as u64 * ms as u64 / 1000) as usize]
}

fn render(
    fixture: &Fixture,
    backend: &dyn SpeechBackend,
    sample_rate_hz: u32,
    cache: &mut HashMap<String, Vec<i16>>,
) -> Res<Vec<i16>> {
    if fixture.clauses.is_empty() {
        return Err(format!("fixture {}: no clauses", fixture.name).into());
    }
    let mut pcm = silence(fixture.lead_ms, sample_rate_hz);
    for (idx, clause) in fixture.clauses.iter().enumerate() {
        if idx > 0 {
            pcm.extend(silence(fixture.gap_before(idx)?, sample_rate_hz));
        }
        // One synthesis per distinct clause: a ladder reuses the same speech
        // at every rung, so only the silence between them varies.
        if !cache.contains_key(clause) {
            let rendered = backend.synthesize(clause, sample_rate_hz)?;
            cache.insert(clause.clone(), rendered);
        }
        pcm.extend_from_slice(&cache[clause]);
    }
    pcm.extend(silence(fixture.trail_ms, sample_rate_hz));
    Ok(pcm)
}

fn write_wav(path: &Path, pcm: &[i16], sample_rate_hz: u32) -> Res<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: sample_rate_hz,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec)?;
    for sample in pcm {
        writer.write_sample(*sample)?;
    }
    writer.finalize()?;
    Ok(())
}

struct Args {
    spec: PathBuf,
    only: Option<String>,
    dry_run: bool,
}

fn parse_args() -> Res<Args> {
    let mut spec = None;
    let mut only = None;
    let mut dry_run = false;
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--only" => only = Some(it.next().ok_or("--only needs a fixture name")?),
            "--dry-run" => dry_run = true,
            "-h" | "--help" => {
                println!(
                    "usage: cargo run -p souffle-tts-fixtures -- <spec.toml> [--only NAME] [--dry-run]"
                );
                std::process::exit(0);
            }
            other if spec.is_none() => spec = Some(PathBuf::from(other)),
            other => return Err(format!("unexpected argument {other}").into()),
        }
    }
    Ok(Args {
        spec: spec.ok_or("missing spec file, see --help")?,
        only,
        dry_run,
    })
}

fn main() -> Res<()> {
    let args = parse_args()?;
    let text =
        std::fs::read_to_string(&args.spec).map_err(|e| format!("{}: {e}", args.spec.display()))?;
    let spec: Spec = toml::from_str(&text)?;

    let backend: Box<dyn SpeechBackend> = match &spec.backend {
        Backend::MacosSay { voice, rate_wpm } => Box::new(MacosSay {
            voice: voice.clone(),
            rate_wpm: *rate_wpm,
        }),
    };

    // Paths in the spec are relative to src-tauri, so the tool works the same
    // whether it is run from the repo root or from src-tauri.
    let base = spec_base_dir()?;
    let out_dir = base.join(&spec.out_dir);

    let mut fixtures: Vec<Fixture> = spec.ladder.iter().flat_map(|l| l.expand()).collect();
    fixtures.extend(spec.fixture.iter().cloned());
    if let Some(only) = &args.only {
        fixtures.retain(|f| matches_only(&f.name, only));
        if fixtures.is_empty() {
            return Err(format!("no fixture named {only} in {}", args.spec.display()).into());
        }
    }

    println!("backend: {}", backend.describe());
    println!("output:  {}", out_dir.display());
    let mut cache = HashMap::new();
    for fixture in &fixtures {
        let path = out_dir.join(format!("{}.wav", fixture.name));
        if args.dry_run {
            println!("would write {}", path.display());
            continue;
        }
        let pcm = render(fixture, backend.as_ref(), spec.sample_rate_hz, &mut cache)?;
        write_wav(&path, &pcm, spec.sample_rate_hz)?;
        let seconds = pcm.len() as f64 / spec.sample_rate_hz as f64;
        println!("wrote {} ({seconds:.2}s)", path.display());
    }
    println!("{} fixture(s)", fixtures.len());
    Ok(())
}

/// The `src-tauri` directory, so `out_dir` in a spec means the same thing
/// regardless of the working directory the tool was launched from.
fn spec_base_dir() -> Res<PathBuf> {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| "cannot locate src-tauri from the crate manifest".into())
}

/// Exact name, or a prefix so `--only hesitation-a` renders the whole ladder.
fn matches_only(name: &str, only: &str) -> bool {
    name == only || name.starts_with(&format!("{only}-"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str, clauses: &[&str], gaps_ms: &[u32]) -> Fixture {
        Fixture {
            name: name.into(),
            clauses: clauses.iter().map(|s| (*s).to_string()).collect(),
            gaps_ms: gaps_ms.to_vec(),
            lead_ms: 300,
            trail_ms: 800,
        }
    }

    #[test]
    fn ladder_names_zero_pad_the_gap() {
        let ladder = Ladder {
            name: "hesitation-a".into(),
            clauses: vec!["a".into(), "b".into()],
            gaps_ms: vec![100, 1000],
            lead_ms: 300,
            trail_ms: 800,
        };
        let expanded = ladder.expand();
        assert_eq!(expanded[0].name, "hesitation-a-gap0100ms");
        assert_eq!(expanded[1].name, "hesitation-a-gap1000ms");
        assert_eq!(expanded[0].gaps_ms, vec![100]);
        assert_eq!(expanded[1].gaps_ms, vec![1000]);
    }

    #[test]
    fn gap_before_reuses_a_single_value() {
        let f = fixture("x", &["a", "b", "c"], &[250]);
        assert_eq!(f.gap_before(1).unwrap(), 250);
        assert_eq!(f.gap_before(2).unwrap(), 250);
    }

    #[test]
    fn gap_before_uses_per_pair_values() {
        let f = fixture("x", &["a", "b", "c"], &[250, 900]);
        assert_eq!(f.gap_before(1).unwrap(), 250);
        assert_eq!(f.gap_before(2).unwrap(), 900);
    }

    #[test]
    fn gap_before_rejects_wrong_arity() {
        let f = fixture("x", &["a", "b", "c"], &[1, 2, 3]);
        assert!(f.gap_before(1).is_err());
    }

    #[test]
    fn gap_before_rejects_empty_gaps() {
        let f = fixture("x", &["a", "b"], &[]);
        assert!(f.gap_before(1).is_err());
    }

    #[test]
    fn silence_duration_is_exact_at_16k() {
        assert_eq!(silence(100, 16_000).len(), 1_600);
        assert_eq!(silence(300, 16_000).len(), 4_800);
        assert!(silence(100, 16_000).iter().all(|&s| s == 0));
    }

    #[test]
    fn only_matches_ladder_prefix() {
        assert!(matches_only("hesitation-a-gap0100ms", "hesitation-a"));
        assert!(matches_only(
            "hesitation-a-gap0100ms",
            "hesitation-a-gap0100ms"
        ));
        assert!(!matches_only("hesitation-b-gap0100ms", "hesitation-a"));
        assert!(matches_only("hesitation-a-gap0100ms", "hesitation"));
    }
}
