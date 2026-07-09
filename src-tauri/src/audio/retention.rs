//! Retention sweep for recorded meeting audio: deletes recording directories
//! older than the configured policy. Runs once at app startup (see
//! `lib::run`); `off` never deletes existing recordings (the user may
//! re-enable recording later), only `keep_7d`/`keep_30d` do.

use std::path::Path;
use std::time::{Duration, SystemTime};

use crate::settings::MeetingAudioRetention;

use super::recorder;

const DAY: Duration = Duration::from_secs(24 * 3600);

/// Pure decision: is a meeting recording whose newest file has `age` old
/// past the retention window for `policy`?
pub fn is_expired(age: Duration, policy: MeetingAudioRetention) -> bool {
    match policy {
        MeetingAudioRetention::Off | MeetingAudioRetention::KeepForever => false,
        MeetingAudioRetention::Keep7d => age > DAY * 7,
        MeetingAudioRetention::Keep30d => age > DAY * 30,
    }
}

fn newest_mtime(dir: &Path) -> Option<SystemTime> {
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter_map(|entry| entry.metadata().ok()?.modified().ok())
        .max()
}

/// Delete every meeting recording directory directly under `root` whose
/// newest file is older than `policy`'s window, as of `now`. Best-effort:
/// missing/unreadable entries are skipped rather than failing the sweep.
fn sweep_dir(root: &Path, policy: MeetingAudioRetention, now: SystemTime) {
    if matches!(policy, MeetingAudioRetention::Off | MeetingAudioRetention::KeepForever) {
        return;
    }
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(newest) = newest_mtime(&path) else {
            continue;
        };
        let age = now.duration_since(newest).unwrap_or_default();
        if !is_expired(age, policy) {
            continue;
        }
        match std::fs::remove_dir_all(&path) {
            Ok(()) => tracing::info!(dir = %path.display(), "Deleted expired meeting recording"),
            Err(e) => tracing::warn!(dir = %path.display(), "Failed to delete expired meeting recording: {e}"),
        }
    }
}

/// Sweep the real recordings directory. Called once at app startup on a
/// background thread (directory walks can take a moment with a large
/// history).
pub fn sweep_expired_recordings(policy: MeetingAudioRetention, now: SystemTime) {
    sweep_dir(&recorder::recordings_root(), policy, now);
}

fn dir_size_bytes(dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .map(|entry| {
            let path = entry.path();
            if path.is_dir() {
                dir_size_bytes(&path)
            } else {
                entry.metadata().map(|m| m.len()).unwrap_or(0)
            }
        })
        .sum()
}

/// Total size on disk of every meeting recording, for the Settings > Data
/// stats line.
pub fn recordings_size_bytes() -> u64 {
    dir_size_bytes(&recorder::recordings_root())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch_with_age(path: &Path, age: Duration, now: SystemTime) {
        std::fs::write(path, b"opus data").expect("write");
        let file = std::fs::File::options().write(true).open(path).expect("open");
        file.set_modified(now - age).expect("set_modified");
    }

    #[test]
    fn is_expired_matrix() {
        assert!(!is_expired(DAY * 1000, MeetingAudioRetention::Off));
        assert!(!is_expired(DAY * 1000, MeetingAudioRetention::KeepForever));

        assert!(!is_expired(DAY * 6, MeetingAudioRetention::Keep7d));
        assert!(!is_expired(DAY * 7, MeetingAudioRetention::Keep7d));
        assert!(is_expired(DAY * 8, MeetingAudioRetention::Keep7d));

        assert!(!is_expired(DAY * 29, MeetingAudioRetention::Keep30d));
        assert!(is_expired(DAY * 31, MeetingAudioRetention::Keep30d));
    }

    #[test]
    fn sweep_deletes_only_expired_meeting_dirs() {
        let root = tempfile::tempdir().expect("tempdir");
        let now = SystemTime::now();

        let old_meeting = root.path().join("old-meeting");
        std::fs::create_dir_all(&old_meeting).expect("mkdir");
        touch_with_age(&old_meeting.join("0.ogg"), DAY * 10, now);

        let recent_meeting = root.path().join("recent-meeting");
        std::fs::create_dir_all(&recent_meeting).expect("mkdir");
        touch_with_age(&recent_meeting.join("0.ogg"), Duration::from_secs(3600), now);

        sweep_dir(root.path(), MeetingAudioRetention::Keep7d, now);

        assert!(!old_meeting.exists(), "expired meeting recording must be deleted");
        assert!(recent_meeting.exists(), "recent meeting recording must survive");
    }

    #[test]
    fn sweep_keeps_a_multi_session_meeting_whose_newest_session_is_recent() {
        let root = tempfile::tempdir().expect("tempdir");
        let now = SystemTime::now();

        let meeting = root.path().join("resumed-meeting");
        std::fs::create_dir_all(&meeting).expect("mkdir");
        touch_with_age(&meeting.join("0.ogg"), DAY * 20, now); // old first session
        touch_with_age(&meeting.join("1.ogg"), Duration::from_secs(60), now); // fresh resumed session

        sweep_dir(root.path(), MeetingAudioRetention::Keep7d, now);

        assert!(meeting.exists(), "a meeting with any recent session file must survive");
    }

    #[test]
    fn sweep_is_noop_when_off_or_forever() {
        let root = tempfile::tempdir().expect("tempdir");
        let now = SystemTime::now();
        let meeting = root.path().join("ancient-meeting");
        std::fs::create_dir_all(&meeting).expect("mkdir");
        touch_with_age(&meeting.join("0.ogg"), DAY * 365, now);

        sweep_dir(root.path(), MeetingAudioRetention::Off, now);
        assert!(meeting.exists(), "off must never delete existing recordings");

        sweep_dir(root.path(), MeetingAudioRetention::KeepForever, now);
        assert!(meeting.exists(), "forever must never delete");
    }

    #[test]
    fn sweep_on_missing_root_does_not_panic() {
        let root = tempfile::tempdir().expect("tempdir");
        let missing = root.path().join("does-not-exist");
        sweep_dir(&missing, MeetingAudioRetention::Keep7d, SystemTime::now());
    }

    #[test]
    fn dir_size_bytes_sums_nested_files() {
        let root = tempfile::tempdir().expect("tempdir");
        let meeting = root.path().join("m1");
        std::fs::create_dir_all(&meeting).expect("mkdir");
        std::fs::write(meeting.join("0.ogg"), vec![0u8; 100]).expect("write");
        std::fs::write(meeting.join("1.ogg"), vec![0u8; 250]).expect("write");

        assert_eq!(dir_size_bytes(root.path()), 350);
    }
}
