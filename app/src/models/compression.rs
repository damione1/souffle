//! Immutable upstream checkpoint, atomically prepared as local fp16 weights.
use candle_core::{DType, Device, Tensor};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::fd::AsRawFd,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

pub const REPOSITORY: &str = "microsoft/llmlingua-2-bert-base-multilingual-cased-meetingbank";
pub const REVISION: &str = "5f0c82792b7ea14c6484e015b6a072009496b7f2";
pub const WEIGHTS: &str = "model.fp16.safetensors";
pub const CONVERSION_VERSION: u32 = 1;
pub const FILES: &[(&str, u64, &str)] = &[
    (
        "model.safetensors",
        709388104,
        "22b9ecde52fec5c97e8c54a293be768727df95a81c6c8dccb03f262a50c58324",
    ),
    (
        "config.json",
        875,
        "e6c33ec2f099e659e125efaa8b7bb07a6e65b7d9fc36211c6fa3206e1c399085",
    ),
    (
        "tokenizer.json",
        2919362,
        "bf1b59b7b11c95f194f51708d918eea378e09d05f84c0e1656dc5180e8117088",
    ),
    (
        "tokenizer_config.json",
        1191,
        "214f78f6c5d034a8b405632d301ee3710eb6f075b3f5faf8241a273a13908746",
    ),
    (
        "special_tokens_map.json",
        125,
        "b6d346be366a7d1d48332dbc9fdf3bf8960b5d879522b7799ddba59e76237ee3",
    ),
    (
        "vocab.txt",
        995526,
        "fe0fda7c425b48c516fc8f160d594c8022a0808447475c1a7c6d6479763f310c",
    ),
    (
        "README.md",
        3309,
        "ab01af377c6285e0bd040661569238aba5f66f842dcecaa4c2fc704f40c7402b",
    ),
];
/// Total immutable upstream transfer size, including pinned sidecars.
pub fn download_bytes() -> u64 {
    FILES.iter().map(|f| f.1).sum()
}
/// Revision-scoped cache beneath the current application profile.
pub fn model_dir() -> PathBuf {
    crate::constants::app_data_dir()
        .join("models/compression/llmlingua-2-mbert")
        .join(REVISION)
}
#[derive(Debug, Clone, Serialize, Deserialize)]
/// Conversion lineage saved beside immutable runtime weights; the derived hash
/// is checked before inference, while startup reads only this small metadata.
pub struct Installation {
    pub repository: String,
    pub revision: String,
    pub source_sha256: String,
    pub conversion_version: u32,
    pub dtype: safetensors::Dtype,
    pub bytes: u64,
    pub sha256: String,
    pub license: String,
}
#[derive(Debug, Clone, PartialEq)]
/// Installation lifecycle with only the progress/error payload valid in each state.
pub enum ModelStatus {
    NotDownloaded,
    Downloading { downloaded: u64, total: u64 },
    Preparing,
    Ready { bytes: u64 },
    Failed { detail: String },
}
/// Check cooperative cancellation between network, hash and tensor boundaries.
pub fn cancelled(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::Relaxed) {
        Err("Compression cancelled".into())
    } else {
        Ok(())
    }
}
/// Hash a file incrementally, releasing the buffer and checking cancellation
/// at each MiB boundary; callers keep this blocking operation off the UI thread.
pub fn checksum(path: &Path, cancel: &AtomicBool) -> Result<String, String> {
    let mut file = File::open(path).map_err(|e| e.to_string())?;
    let mut hash = Sha256::new();
    let mut buf = vec![0u8; 1024 * 1024];
    loop {
        cancelled(cancel)?;
        let n = file.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        hash.update(&buf[..n]);
    }
    Ok(format!("{:x}", hash.finalize()))
}
/// Require the exact manifest size and SHA-256; partial or wrong-revision bytes
/// never qualify as a source artifact.
pub fn validate_file(
    path: &Path,
    bytes: u64,
    hash: &str,
    cancel: &AtomicBool,
) -> Result<(), String> {
    if fs::metadata(path).map_err(|e| e.to_string())?.len() != bytes {
        return Err(format!("Invalid size: {}", path.display()));
    }
    if checksum(path, cancel)? != hash {
        return Err(format!("Invalid checksum: {}", path.display()));
    }
    Ok(())
}
/// Cheap readiness inspection; content hashing happens only on a worker before use.
pub fn installed(dir: &Path) -> Option<Installation> {
    let metadata: Installation =
        serde_json::from_reader(File::open(dir.join("installation.json")).ok()?).ok()?;
    if metadata.repository != REPOSITORY
        || metadata.revision != REVISION
        || metadata.source_sha256 != FILES[0].2
        || metadata.conversion_version != CONVERSION_VERSION
        || metadata.dtype != safetensors::Dtype::F16
        || metadata.sha256.len() != 64
        || metadata.bytes == 0
    {
        return None;
    }
    if fs::metadata(dir.join(WEIGHTS)).ok()?.len() != metadata.bytes {
        return None;
    }
    for (file, bytes, _) in &FILES[1..] {
        if fs::metadata(dir.join(file)).ok()?.len() != *bytes {
            return None;
        }
    }
    Some(metadata)
}
/// Verify runtime bytes, every pinned sidecar and required tensor layout on a
/// blocking worker. This deliberately does more work than cheap readiness.
pub fn verify(dir: &Path, cancel: &AtomicBool) -> Result<Installation, String> {
    let meta = installed(dir).ok_or("Compression model is not installed")?;
    validate_file(&dir.join(WEIGHTS), meta.bytes, &meta.sha256, cancel)?;
    for (file, bytes, hash) in &FILES[1..] {
        validate_file(&dir.join(file), *bytes, hash, cancel)?;
    }
    validate_tensors(&dir.join(WEIGHTS))?;
    Ok(meta)
}
/// Reject malformed, non-fp16 or incompatible core tensor layouts before load.
fn validate_tensors(path: &Path) -> Result<(), String> {
    // SAFETY: installed files are immutable and published by atomic rename.
    let map = unsafe { memmap2::Mmap::map(&File::open(path).map_err(|e| e.to_string())?) }
        .map_err(|e| e.to_string())?;
    let tensors = safetensors::SafeTensors::deserialize(&map).map_err(|e| e.to_string())?;
    for (name, tensor) in tensors.tensors() {
        if tensor.dtype() != safetensors::Dtype::F16 {
            return Err(format!("Non-fp16 tensor: {name}"));
        }
    }
    for (name, shape) in [
        ("bert.embeddings.word_embeddings.weight", vec![119647, 768]),
        ("classifier.weight", vec![2, 768]),
        ("classifier.bias", vec![2]),
    ] {
        if tensors.tensor(name).map_err(|e| e.to_string())?.shape() != shape {
            return Err(format!("Invalid tensor shape: {name}"));
        }
    }
    Ok(())
}
/// Prepare fp16 from verified mapped fp32 weights, checking cancellation between
/// tensors and retaining the exact upstream and derived checksum lineage.
fn convert(stage: &Path, cancel: &AtomicBool) -> Result<Installation, String> {
    // A mapped source plus fp16 tensor storage: never a second fp32 model copy.
    let source = stage.join(FILES[0].0);
    let mapped = unsafe { candle_core::safetensors::MmapedSafetensors::new(&source) }
        .map_err(|e| e.to_string())?;
    let mut converted: HashMap<String, Tensor> = HashMap::new();
    for (name, _) in mapped.tensors() {
        cancelled(cancel)?;
        let tensor = mapped
            .load(&name, &Device::Cpu)
            .map_err(|e| e.to_string())?
            .to_dtype(DType::F16)
            .map_err(|e| e.to_string())?;
        converted.insert(name, tensor);
    }
    cancelled(cancel)?;
    candle_core::safetensors::save(&converted, stage.join(WEIGHTS)).map_err(|e| e.to_string())?;
    drop(converted);
    drop(mapped);
    validate_tensors(&stage.join(WEIGHTS))?;
    let meta = Installation {
        repository: REPOSITORY.into(),
        revision: REVISION.into(),
        source_sha256: FILES[0].2.into(),
        conversion_version: CONVERSION_VERSION,
        dtype: safetensors::Dtype::F16,
        bytes: fs::metadata(stage.join(WEIGHTS))
            .map_err(|e| e.to_string())?
            .len(),
        sha256: checksum(&stage.join(WEIGHTS), cancel)?,
        license: "Apache-2.0; Microsoft LLMLingua-2; locally converted fp16".into(),
    };
    fs::write(
        stage.join("installation.json"),
        serde_json::to_vec_pretty(&meta).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    fs::remove_file(source).map_err(|e| e.to_string())?;
    Ok(meta)
}
struct InstallLock(File);
impl Drop for InstallLock {
    fn drop(&mut self) {
        unsafe {
            libc::flock(self.0.as_raw_fd(), libc::LOCK_UN);
        }
    }
}
/// Remove only abandoned directories created by this installer, while its exclusive
/// filesystem lock is held. Symlinks and unrelated directory names are never followed.
fn clean_abandoned_staging(parent: &Path) -> Result<(), String> {
    for entry in fs::read_dir(parent).map_err(|e| format!("Read compression cache: {e}"))? {
        let entry = entry.map_err(|e| format!("Inspect compression cache: {e}"))?;
        if !entry
            .file_type()
            .map_err(|e| format!("Inspect staging type: {e}"))?
            .is_dir()
        {
            continue;
        }
        let name = entry.file_name();
        let Some(id) = name
            .to_str()
            .and_then(|name| name.strip_prefix(".staging-"))
        else {
            continue;
        };
        if uuid::Uuid::parse_str(id).is_ok_and(|uuid| uuid.hyphenated().to_string() == id) {
            fs::remove_dir_all(entry.path())
                .map_err(|e| format!("Clean abandoned compression staging: {e}"))?;
        }
    }
    Ok(())
}
/// Install the pinned checkpoint under an advisory lock, publish it atomically,
/// and leave a retryable cache after cancellation or failure. Healthy reads have
/// no total transfer deadline; connect/read stalls remain bounded.
pub fn install(
    dir: &Path,
    cancel: &AtomicBool,
    progress: impl Fn(ModelStatus),
) -> Result<Installation, String> {
    let parent = dir.parent().ok_or("Invalid model path")?;
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let lock = InstallLock(
        OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(parent.join("install.lock"))
            .map_err(|e| e.to_string())?,
    );
    loop {
        cancelled(cancel)?;
        if unsafe { libc::flock(lock.0.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    clean_abandoned_staging(parent)?;
    if installed(dir).is_some() {
        match verify(dir, cancel) {
            Ok(meta) => return Ok(meta),
            Err(error) => {
                cancelled(cancel)?;
                tracing::warn!("Repairing invalid compression installation: {error}");
            }
        }
    }
    let stage = parent.join(format!(".staging-{}", uuid::Uuid::new_v4()));
    fs::create_dir(&stage).map_err(|e| e.to_string())?;
    let result = (|| {
        // A private current-thread runtime lives only on this blocking installer.
        // Per-read timeout bounds stalls; cancellation is checked between chunks.
        let network = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| e.to_string())?;
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(20))
            .read_timeout(Duration::from_secs(15))
            .build()
            .map_err(|e| e.to_string())?;
        let mut done = 0;
        for (name, bytes, hash) in FILES {
            cancelled(cancel)?;
            let url = format!("https://huggingface.co/{REPOSITORY}/resolve/{REVISION}/{name}");
            let mut response = network
                .block_on(client.get(url).send())
                .and_then(|r| r.error_for_status())
                .map_err(|e| e.to_string())?;
            let part = stage.join(format!("{name}.part"));
            let mut out = File::create(&part).map_err(|e| e.to_string())?;
            let mut read = 0u64;
            loop {
                cancelled(cancel)?;
                let Some(chunk) = network
                    .block_on(response.chunk())
                    .map_err(|e| e.to_string())?
                else {
                    break;
                };
                read += chunk.len() as u64;
                if read > *bytes {
                    return Err(format!("Oversized file: {name}"));
                }
                out.write_all(&chunk).map_err(|e| e.to_string())?;
                progress(ModelStatus::Downloading {
                    downloaded: done + read,
                    total: download_bytes(),
                });
            }
            out.sync_all().map_err(|e| e.to_string())?;
            drop(out);
            validate_file(&part, *bytes, hash, cancel)?;
            fs::rename(part, stage.join(name)).map_err(|e| e.to_string())?;
            done += bytes;
        }
        progress(ModelStatus::Preparing);
        let meta = convert(&stage, cancel)?;
        cancelled(cancel)?;
        if dir.exists() {
            fs::remove_dir_all(dir).map_err(|e| e.to_string())?;
        }
        fs::rename(&stage, dir).map_err(|e| e.to_string())?;
        Ok(meta)
    })();
    if stage.exists() {
        let _ = fs::remove_dir_all(stage);
    }
    result
}
/// Explicit cache preparation for Rust fixture/CLI QA; validates the same immutable manifest.
#[cfg(any(test, feature = "test-support"))]
pub fn prepare_cached(source: &Path, dir: &Path) -> Result<Installation, String> {
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let cancel = AtomicBool::new(false);
    for (name, bytes, hash) in FILES {
        validate_file(&source.join(name), *bytes, hash, &cancel)?;
        fs::copy(source.join(name), dir.join(name)).map_err(|e| e.to_string())?;
    }
    convert(dir, &cancel)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn abandoned_staging_cleanup_preserves_unrelated_and_symlink_targets() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let abandoned = dir
            .path()
            .join(format!(".staging-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&abandoned).unwrap();
        fs::write(abandoned.join("partial"), b"partial").unwrap();
        let unrelated = dir.path().join(".staging-personal");
        fs::create_dir(&unrelated).unwrap();
        let noncanonical = dir.path().join(".staging-11111111111111111111111111111111");
        fs::create_dir(&noncanonical).unwrap();
        fs::write(outside.path().join("keep"), b"preserved").unwrap();
        let symlink = dir
            .path()
            .join(format!(".staging-{}", uuid::Uuid::new_v4()));
        std::os::unix::fs::symlink(outside.path(), &symlink).unwrap();
        clean_abandoned_staging(dir.path()).unwrap();
        assert!(!abandoned.exists());
        assert!(unrelated.is_dir() && noncanonical.is_dir());
        assert!(symlink.symlink_metadata().unwrap().file_type().is_symlink());
        assert_eq!(fs::read(outside.path().join("keep")).unwrap(), b"preserved");
    }
    #[test]
    fn manifest_refuses_partial_wrong_hash_and_cancel() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("x");
        fs::write(&p, b"abc").unwrap();
        let c = AtomicBool::new(false);
        assert!(validate_file(&p, 4, "bad", &c).is_err());
        assert!(validate_file(&p, 3, "bad", &c).is_err());
        c.store(true, Ordering::Relaxed);
        assert!(checksum(&p, &c).is_err());
        assert!(installed(tmp.path()).is_none());
    }
}
