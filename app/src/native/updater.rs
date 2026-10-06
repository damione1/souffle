//! Native replacement for `tauri-plugin-updater` (SOU-191, AC7).
//!
//! Detection (`update_check::check_for_updates`) was already Tauri-free —
//! plain `reqwest` against the GitHub releases API. This module replaces the
//! other half: fetching the signed `latest.json` manifest tauri's release
//! workflow publishes, downloading the `.app.tar.gz` it points at, verifying
//! its minisign signature against the same public key `tauri.conf.json`
//! already carries (`plugins.updater.pubkey`), and replacing the running app
//! bundle.
//!
//! The signature check is the one guard this module must never weaken: it
//! runs before a single byte of the downloaded archive touches disk as
//! anything other than an in-memory `Vec<u8>` under verification.

use std::path::{Path, PathBuf};

use minisign_verify::{PublicKey, Signature};
use serde::Deserialize;

use crate::update_check::current_version;

/// Base64 of the minisign public key *file* (comment line + key line), same
/// value as `tauri.conf.json`'s `plugins.updater.pubkey` — this is not the
/// bare key bytes, decoding it yields the two-line minisign key file text.
const PUBKEY_B64: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IDRCNEIzOEFGNTQwMUM1RkIKUldUN3hRRlVyemhMUzBqLzZuSWpCY2RtYnM0djNQUHZvdkFGNWxNVFA1d1hUZllDUjFJK3NPZzMK";

const MANIFEST_URL: &str =
    "https://github.com/damione1/souffle/releases/latest/download/latest.json";

/// Only Apple Silicon macOS ships (see CLAUDE.md); this is the one platform
/// key the manifest needs to carry.
const PLATFORM: &str = "darwin-aarch64";

#[derive(Debug, Deserialize)]
struct Manifest {
    version: String,
    #[serde(default)]
    notes: Option<String>,
    platforms: std::collections::HashMap<String, PlatformEntry>,
}

#[derive(Debug, Deserialize)]
struct PlatformEntry {
    signature: String,
    url: String,
}

pub struct ReadyUpdate {
    pub version: String,
    pub notes: Option<String>,
    pub bytes: Vec<u8>,
}

fn public_key() -> Result<PublicKey, String> {
    let decoded = base64_decode(PUBKEY_B64)?;
    let text = String::from_utf8(decoded).map_err(|e| format!("Public key is not UTF-8: {e}"))?;
    PublicKey::decode(&text).map_err(|e| format!("Invalid embedded public key: {e}"))
}

fn base64_decode(s: &str) -> Result<Vec<u8>, String> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(s.trim())
        .map_err(|e| format!("Base64 decode failed: {e}"))
}

/// Fetch and parse `latest.json`. Does not download or verify the artifact
/// itself — that happens in [`download_and_verify`], kept separate so a
/// manifest fetch failure and a signature failure are distinguishable.
async fn fetch_manifest(client: &reqwest::Client) -> Result<(Manifest, PlatformEntry), String> {
    let response = client
        .get(MANIFEST_URL)
        .send()
        .await
        .map_err(|e| format!("Fetch update manifest: {e}"))?;
    if !response.status().is_success() {
        return Err(format!(
            "Update manifest request returned HTTP {}",
            response.status()
        ));
    }
    let text = response
        .text()
        .await
        .map_err(|e| format!("Read update manifest: {e}"))?;
    parse_manifest(&text)
}

/// Parse `latest.json` and pick this platform's entry. Pure, so the exact
/// manifest a release publishes can be checked in a test.
fn parse_manifest(text: &str) -> Result<(Manifest, PlatformEntry), String> {
    let manifest: Manifest =
        serde_json::from_str(text).map_err(|e| format!("Parse update manifest: {e}"))?;
    let entry = manifest
        .platforms
        .get(PLATFORM)
        .ok_or_else(|| format!("Update manifest has no entry for {PLATFORM}"))?
        .clone_entry();
    Ok((manifest, entry))
}

impl PlatformEntry {
    fn clone_entry(&self) -> Self {
        Self {
            signature: self.signature.clone(),
            url: self.url.clone(),
        }
    }
}

/// Download the platform artifact and verify its minisign signature before
/// returning it. `on_progress(downloaded_bytes, total_bytes)` is called as
/// chunks arrive, mirroring `tauri_plugin_updater::Update::download`'s
/// callback shape closely enough that `commands/updater.rs` barely changes.
pub async fn download_and_verify(
    on_progress: impl Fn(u64, Option<u64>) + Send + 'static,
) -> Result<ReadyUpdate, String> {
    let client = reqwest::Client::builder()
        .user_agent(format!("souffle/{}", current_version()))
        .build()
        .map_err(|e| format!("HTTP client: {e}"))?;

    let (manifest, entry) = fetch_manifest(&client).await?;

    let sig_text_bytes = base64_decode(&entry.signature)?;
    let sig_text = String::from_utf8(sig_text_bytes)
        .map_err(|e| format!("Update signature is not UTF-8: {e}"))?;
    let signature =
        Signature::decode(&sig_text).map_err(|e| format!("Invalid update signature: {e}"))?;

    let response = client
        .get(&entry.url)
        .send()
        .await
        .map_err(|e| format!("Download update: {e}"))?;
    if !response.status().is_success() {
        return Err(format!(
            "Update download returned HTTP {}",
            response.status()
        ));
    }
    let total_bytes = response.content_length();

    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    use futures_util::StreamExt;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("Download update: {e}"))?;
        bytes.extend_from_slice(&chunk);
        on_progress(bytes.len() as u64, total_bytes);
    }

    let key = public_key()?;
    key.verify(&bytes, &signature, false)
        .map_err(|e| format!("Update signature verification failed: {e}"))?;

    Ok(ReadyUpdate {
        version: manifest.version,
        notes: manifest.notes,
        bytes,
    })
}

/// The `.app` bundle currently running, derived from the executable path
/// (`.../Foo.app/Contents/MacOS/exe` -> `.../Foo.app`).
pub(crate) fn running_bundle_path() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| format!("Current executable path: {e}"))?;
    exe.ancestors()
        .find(|p| p.extension().is_some_and(|ext| ext == "app"))
        .map(Path::to_path_buf)
        .ok_or_else(|| "Not running from an .app bundle".to_string())
}

/// Extract the downloaded `.tar.gz` and replace the running bundle with the
/// extracted one. Does not relaunch: the caller shuts the engine down and
/// then calls [`relaunch_and_exit`], so a failure here leaves the running
/// app fully usable. Only reached after [`download_and_verify`] already
/// checked the signature, so `bytes` here is trusted.
pub fn install_bundle(bytes: &[u8]) -> Result<(), String> {
    let bundle_path = running_bundle_path()?;
    let tmp_dir = std::env::temp_dir().join(format!("souffle-update-{}", std::process::id()));
    let result = extract_and_swap(bytes, &bundle_path, &tmp_dir);
    let _ = std::fs::remove_dir_all(&tmp_dir);
    result
}

fn extract_and_swap(bytes: &[u8], bundle_path: &Path, tmp_dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(tmp_dir).map_err(|e| format!("Create temp extract dir: {e}"))?;
    let archive_path = tmp_dir.join("update.tar.gz");
    std::fs::write(&archive_path, bytes).map_err(|e| format!("Write downloaded archive: {e}"))?;

    let status = std::process::Command::new("/usr/bin/tar")
        .arg("-xzf")
        .arg(&archive_path)
        .arg("-C")
        .arg(tmp_dir)
        .status()
        .map_err(|e| format!("Run tar: {e}"))?;
    if !status.success() {
        return Err(format!("tar extraction failed with status {status}"));
    }

    let extracted_app = std::fs::read_dir(tmp_dir)
        .map_err(|e| format!("Read extracted archive dir: {e}"))?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .find(|p| p.extension().is_some_and(|ext| ext == "app"))
        .ok_or_else(|| "Downloaded archive contained no .app bundle".to_string())?;

    let backup_path = bundle_path.with_extension("app.update-backup");
    let _ = std::fs::remove_dir_all(&backup_path);
    std::fs::rename(bundle_path, &backup_path)
        .map_err(|e| format!("Move current app aside: {e}"))?;

    if let Err(e) = std::fs::rename(&extracted_app, bundle_path) {
        // Roll back: the app must not be left missing.
        let _ = std::fs::rename(&backup_path, bundle_path);
        return Err(format!("Install new app bundle: {e}"));
    }
    let _ = std::fs::remove_dir_all(&backup_path);
    Ok(())
}

/// Reopen the (already replaced) bundle once this process is gone, then
/// exit. Opening it before exiting would race this instance: Launch Services
/// can just reactivate the still-running process, or the new one finds the
/// single-instance lock held and quits, leaving no Soufflé running at all
/// (see `relaunch.rs`). If the relaunch cannot even be scheduled the process
/// still exits: the new bundle is on disk and the old code must stop, so the
/// user reopening Soufflé lands on the update.
pub fn relaunch_and_exit() -> ! {
    if let Err(e) = super::relaunch::relaunch_after_exit() {
        tracing::warn!("Relaunch after update could not be scheduled: {e}");
    }
    std::process::exit(0);
}

/// Read-only helper for tests: whether a byte slice matches an embedded
/// (well-formed but not necessarily trusted-for-this-data) signature.
#[cfg(test)]
fn verify_for_test(bytes: &[u8], sig_file_text: &str, pubkey_file_text: &str) -> bool {
    let Ok(pk) = PublicKey::decode(pubkey_file_text) else {
        return false;
    };
    let Ok(sig) = Signature::decode(sig_file_text) else {
        return false;
    };
    pk.verify(bytes, &sig, false).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The manifest v0.16.2 actually published (signature shortened), with
    /// its extra `pub_date` field and multi-line notes.
    const PUBLISHED_MANIFEST: &str = r#"{
      "version": "0.16.2",
      "notes": "Merge pull request #366 from damione1/develop\n\nRelease v0.16.2",
      "pub_date": "2026-10-01T17:36:10Z",
      "platforms": {
        "darwin-aarch64": {
          "signature": "dW50cnVzdGVkIGNvbW1lbnQ6",
          "url": "https://github.com/damione1/souffle/releases/download/v0.16.2/Souffle.app.tar.gz"
        }
      }
    }"#;

    #[test]
    fn the_published_manifest_shape_parses() {
        let (manifest, entry) = parse_manifest(PUBLISHED_MANIFEST).unwrap();
        assert_eq!(manifest.version, "0.16.2");
        assert!(manifest.notes.unwrap().contains("Release v0.16.2"));
        assert_eq!(entry.signature, "dW50cnVzdGVkIGNvbW1lbnQ6");
        assert!(entry.url.ends_with("/v0.16.2/Souffle.app.tar.gz"));
    }

    #[test]
    fn a_manifest_without_this_platform_is_an_error() {
        let text =
            r#"{"version":"1.0.0","platforms":{"linux-x86_64":{"signature":"x","url":"y"}}}"#;
        let err = parse_manifest(text).unwrap_err();
        assert!(err.contains(PLATFORM), "{err}");
    }

    fn write_fake_app(dir: &Path, name: &str, marker: &str) -> PathBuf {
        let app = dir.join(name);
        std::fs::create_dir_all(app.join("Contents")).unwrap();
        std::fs::write(app.join("Contents/marker"), marker).unwrap();
        app
    }

    fn tar_gz_of(dir: &Path, entry: &str) -> Vec<u8> {
        let archive = dir.join("archive.tar.gz");
        let status = std::process::Command::new("/usr/bin/tar")
            .arg("-C")
            .arg(dir)
            .arg("-czf")
            .arg(&archive)
            .arg(entry)
            .status()
            .unwrap();
        assert!(status.success());
        std::fs::read(archive).unwrap()
    }

    #[test]
    fn extract_and_swap_replaces_the_bundle_with_the_archived_app() {
        let src = tempfile::tempdir().unwrap();
        write_fake_app(src.path(), "Soufflé.app", "new");
        let bytes = tar_gz_of(src.path(), "Soufflé.app");

        let install = tempfile::tempdir().unwrap();
        let bundle = write_fake_app(install.path(), "Soufflé.app", "old");
        let staging = install.path().join("staging");

        extract_and_swap(&bytes, &bundle, &staging).unwrap();

        let marker = std::fs::read_to_string(bundle.join("Contents/marker")).unwrap();
        assert_eq!(marker, "new");
        assert!(!bundle.with_extension("app.update-backup").exists());
    }

    #[test]
    fn extract_and_swap_leaves_the_bundle_alone_when_the_archive_has_no_app() {
        let src = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(src.path().join("not-an-app")).unwrap();
        let bytes = tar_gz_of(src.path(), "not-an-app");

        let install = tempfile::tempdir().unwrap();
        let bundle = write_fake_app(install.path(), "Soufflé.app", "old");
        let staging = install.path().join("staging");

        let err = extract_and_swap(&bytes, &bundle, &staging).unwrap_err();
        assert!(err.contains("no .app bundle"), "{err}");
        let marker = std::fs::read_to_string(bundle.join("Contents/marker")).unwrap();
        assert_eq!(marker, "old");
    }

    #[test]
    fn embedded_pubkey_decodes() {
        public_key().expect("the pubkey baked into this module must be well-formed");
    }

    #[test]
    fn a_tampered_payload_fails_verification_even_with_a_valid_looking_signature() {
        // Not a real signature for `bytes` - just proves garbage input is
        // rejected rather than silently accepted.
        assert!(!verify_for_test(
            b"payload",
            "untrusted comment: not a real signature\nAA==\n",
            "untrusted comment: not a real key\nAA==\n"
        ));
    }
}
