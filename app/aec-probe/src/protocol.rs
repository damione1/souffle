use crate::Result;
use clap::ValueEnum;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{fs, path::Path};

#[derive(Clone, Copy, Debug, Serialize, Deserialize, ValueEnum, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Mode {
    Baseline,
    Apple,
}
impl Mode {
    pub fn name(self) -> &'static str {
        match self {
            Self::Baseline => "baseline",
            Self::Apple => "apple",
        }
    }
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, ValueEnum, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Scene {
    Remote,
    Local,
    Double,
}
impl Scene {
    pub fn name(self) -> &'static str {
        match self {
            Self::Remote => "remote",
            Self::Local => "local",
            Self::Double => "double",
        }
    }
    pub fn local(self) -> bool {
        match self {
            Self::Remote => false,
            Self::Local | Self::Double => true,
        }
    }
    pub fn remote(self) -> bool {
        match self {
            Self::Local => false,
            Self::Remote | Self::Double => true,
        }
    }
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum TrialStatus {
    Starting,
    Complete,
    Failed,
}
impl TrialStatus {
    pub fn complete(self) -> bool {
        match self {
            Self::Complete => true,
            Self::Starting | Self::Failed => false,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum Track {
    MicNative,
    TapNative,
    Raw,
    Fdaf,
    Apple,
    Them,
}
impl Track {
    pub fn stem(self) -> &'static str {
        match self {
            Self::MicNative => "mic-native-rate",
            Self::TapNative => "tap-native-rate",
            Self::Raw => "raw",
            Self::Fdaf => "fdaf",
            Self::Apple => "apple",
            Self::Them => "them",
        }
    }
    pub fn filename(self) -> String {
        format!("{}.wav", self.stem())
    }
}
impl Mode {
    pub fn me_tracks(self) -> &'static [Track] {
        match self {
            Self::Baseline => &[Track::Raw, Track::Fdaf],
            Self::Apple => &[Track::Apple],
        }
    }
    pub fn asr_tracks(self) -> impl Iterator<Item = Track> {
        self.me_tracks()
            .iter()
            .copied()
            .chain(std::iter::once(Track::Them))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Protocol {
    pub fixture: String,
    pub fixture_sha256: String,
    /// Manually verified audible reference for the full 30s remote script.
    pub remote_reference: String,
    pub local_reference: String,
    pub standard_microphone_mode_confirmed: bool,
    pub machine_position: String,
    pub mouth_distance_cm: f64,
    pub output_volume_description: String,
    pub browser_version: String,
    pub schedule: String,
    /// Exact catalogue model/backend ids and immutable weights hashes/versions.
    pub whisper_profile: AsrProfile,
    pub parakeet_profile: AsrProfile,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AsrProfile {
    pub model: String,
    pub backend: String,
    pub weights_sha256: String,
}
pub fn init(dir: &Path) -> Result<()> {
    fs::create_dir(dir)?;
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    write_json(&dir.join("protocol.json"), &Protocol {
        fixture: String::new(), fixture_sha256: String::new(), remote_reference: String::new(),
        local_reference: String::new(), standard_microphone_mode_confirmed: false,
        machine_position: String::new(), mouth_distance_cm: 0.0,
        output_volume_description: String::new(), browser_version: String::new(),
        schedule: "0-10s speech; 10-15s pause; 15-30s resume. Annotate exact audible script and timings before runs.".into(),
        whisper_profile: AsrProfile::default(), parakeet_profile: AsrProfile::default(),
    })
}
pub fn load(dir: &Path) -> Result<Protocol> {
    let p: Protocol = serde_json::from_slice(&fs::read(dir.join("protocol.json"))?)?;
    p.validate()?;
    if hash(Path::new(&p.fixture))? != p.fixture_sha256 {
        return Err("fixture hash differs from fixed protocol".into());
    }
    Ok(p)
}
impl Protocol {
    pub fn validate(&self) -> Result<()> {
        for s in [
            &self.fixture,
            &self.fixture_sha256,
            &self.remote_reference,
            &self.local_reference,
            &self.machine_position,
            &self.output_volume_description,
            &self.browser_version,
            &self.schedule,
        ] {
            if s.trim().is_empty() {
                return Err("complete every protocol field before capture (references must be manually checked)".into());
            }
        }
        if !self.remote_reference.chars().any(char::is_alphanumeric)
            || !self.local_reference.chars().any(char::is_alphanumeric)
        {
            return Err("references require audible words".into());
        }
        for p in [&self.whisper_profile, &self.parakeet_profile] {
            if p.model.is_empty()
                || p.backend.is_empty()
                || p.weights_sha256.len() != 64
                || !p.weights_sha256.bytes().all(|b| b.is_ascii_hexdigit())
            {
                return Err(
                    "pin catalogue model, backend and SHA256 weights manifest for both engines"
                        .into(),
                );
            }
        }
        if !self.standard_microphone_mode_confirmed
            || !self.mouth_distance_cm.is_finite()
            || self.mouth_distance_cm <= 0.0
        {
            return Err("confirm Standard microphone mode and a positive mouth distance".into());
        }
        Ok(())
    }
}
pub fn hash(path: &Path) -> Result<String> {
    use std::io::Read;
    let mut f = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut b = [0u8; 65536];
    loop {
        let n = f.read(&mut b)?;
        if n == 0 {
            break;
        }
        hasher.update(&b[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}
pub fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    fs::write(path, serde_json::to_vec_pretty(value)?)?;
    Ok(())
}
pub fn read_wav(path: &Path) -> Result<(Vec<f32>, u32)> {
    let mut r = hound::WavReader::open(path)?;
    let spec = r.spec();
    if spec.channels != 1 || !(8000..=192000).contains(&spec.sample_rate) {
        return Err("fixture must be mono WAV with a nonzero sample rate".into());
    }
    if r.duration() > spec.sample_rate * 60 {
        return Err("fixture exceeds bounded 60s limit".into());
    }
    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => r.samples::<f32>().collect::<std::result::Result<_, _>>()?,
        hound::SampleFormat::Int => r
            .samples::<i32>()
            .map(|s| s.map(|v| v as f32 / 2f32.powi(i32::from(spec.bits_per_sample) - 1)))
            .collect::<std::result::Result<_, _>>()?,
    };
    if samples.is_empty() || samples.iter().any(|x| !x.is_finite()) {
        return Err("empty or nonfinite WAV".into());
    }
    Ok((samples, spec.sample_rate))
}
pub fn write_wav(path: &Path, pcm: &[f32], rate: u32) -> Result<()> {
    if !(8000..=192000).contains(&rate) || pcm.is_empty() || pcm.iter().any(|x| !x.is_finite()) {
        return Err("cannot export empty/nonfinite PCM".into());
    }
    let mut w = hound::WavWriter::create(
        path,
        hound::WavSpec {
            channels: 1,
            sample_rate: rate,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        },
    )?;
    for s in pcm {
        w.write_sample(*s)?;
    }
    w.finalize()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn empty_and_nonfinite_recordings_cannot_be_exported() {
        let dir = tempfile::tempdir().unwrap();
        assert!(write_wav(&dir.path().join("empty.wav"), &[], 48000).is_err());
        assert!(write_wav(&dir.path().join("nan.wav"), &[f32::NAN], 48000).is_err());
    }
    #[test]
    fn template_requires_explicit_operator_protocol() {
        let parent = tempfile::tempdir().unwrap();
        let dir = parent.path().join("qa");
        init(&dir).unwrap();
        assert!(load(&dir).is_err());
        assert!(init(&dir).is_err());
    }
    #[test]
    fn wav_roundtrip_preserves_native_sample_rate_and_frames() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audio.wav");
        write_wav(&path, &[0.25, -0.5, 0.0], 44100).unwrap();
        let (samples, rate) = read_wav(&path).unwrap();
        assert_eq!(rate, 44100);
        assert_eq!(samples, vec![0.25, -0.5, 0.0]);
    }
}
