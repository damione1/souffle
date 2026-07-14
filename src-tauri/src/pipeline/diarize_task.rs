//! Post-stop offline speaker diarization for mic-only meetings. Spawned by
//! `stop_meeting_recording` right after the transcript is saved and
//! `MeetingFinalized` is emitted (never delaying either): for each recording
//! session whose mic audio was tapped to a temporary WAV
//! (`audio::diarize_tap`), run the offline diarization engine, match the
//! detected voice clusters against persistent speakers (`diarize::persist`),
//! map the diarized time ranges onto that session's transcript segments
//! (`diarize::assign`), and write the labels back in one transaction.
//!
//! Everything here is best-effort: any failure just leaves the meeting
//! unlabeled (logged, surfaced via `DiarizationProgress.error`), and the
//! temporary WAVs are deleted whether the pass succeeded or not. The body
//! runs under `catch_unwind` so a bug in the ONNX stack can never take the
//! app down from a background thread.

use tauri::Manager;
use tauri_specta::Event;
use tracing::{info, warn};

use crate::app_events::{DiarizationProgress, MeetingDiarized};
use crate::audio::diarize_tap;
use crate::db::Database;
use crate::diarize::assign::{SegmentSpan, assign_by_overlap};
use crate::diarize::persist::{StoredSpeaker, decode_centroid, encode_centroid, match_speakers};
use crate::diarize::{DiarizeConfig, models};
use crate::engine::Speaker;
use crate::state::AppState;

/// Kick off the background diarization pass for `meeting_id`. Returns
/// immediately; all work happens on a dedicated thread. A meeting with no
/// pending diarization WAVs (system-audio meeting, feature off, models
/// missing at record time) is a silent no-op: no events, no logs beyond
/// trace level.
pub fn spawn_post_meeting_diarization(app: tauri::AppHandle, meeting_id: String) {
    let spawned = std::thread::Builder::new()
        .name("diarize-task".into())
        .spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run(&app, &meeting_id);
            }));
            if result.is_err() {
                warn!(meeting_id = %meeting_id, "Diarization task panicked; meeting left unlabeled");
                diarize_tap::cleanup_meeting_tmp(&meeting_id);
                let _ = DiarizationProgress {
                    meeting_id,
                    done_sessions: 0,
                    total_sessions: 0,
                    finished: true,
                    error: Some("Speaker recognition failed unexpectedly.".to_string()),
                }
                .emit(&app);
            }
        });
    if let Err(e) = spawned {
        warn!("Failed to spawn diarization task thread: {e}");
    }
}

fn run(app: &tauri::AppHandle, meeting_id: &str) {
    // Pending WAVs are the trigger: they only exist if this was a mic-only
    // meeting recorded with the feature enabled and models present.
    let session_wavs = pending_session_wavs(meeting_id);
    if session_wavs.is_empty() {
        return;
    }

    // Models could have been deleted between record and now: skip silently
    // (the Settings UI is where downloading happens, never this task).
    if !models::models_downloaded() {
        info!(meeting_id = %meeting_id, "Diarization models missing; skipping speaker recognition");
        diarize_tap::cleanup_meeting_tmp(meeting_id);
        return;
    }

    let state = app.state::<AppState>();
    let db = &state.db;
    let total_sessions = session_wavs.len() as u32;

    let emit_progress = |done: u32, finished: bool, error: Option<String>| {
        let _ = DiarizationProgress {
            meeting_id: meeting_id.to_string(),
            done_sessions: done,
            total_sessions,
            finished,
            error,
        }
        .emit(app);
    };
    emit_progress(0, false, None);

    let outcome = diarize_meeting(db, meeting_id, &session_wavs, |done| {
        emit_progress(done, false, None);
    });

    // Always delete the temp audio, success or failure.
    diarize_tap::cleanup_meeting_tmp(meeting_id);

    match outcome {
        Ok(changed) => {
            emit_progress(total_sessions, true, None);
            if changed > 0 {
                info!(meeting_id = %meeting_id, segments = changed, "Speaker recognition labeled meeting segments");
                let _ = MeetingDiarized {
                    meeting_id: meeting_id.to_string(),
                }
                .emit(app);
            }
        }
        Err(e) => {
            warn!(meeting_id = %meeting_id, "Speaker recognition failed: {e}");
            emit_progress(total_sessions, true, Some(e));
        }
    }
}

/// The tapped WAVs waiting for this meeting, as (session_index, path) pairs
/// sorted by session index. Non-WAV or non-numeric entries are ignored.
fn pending_session_wavs(meeting_id: &str) -> Vec<(usize, std::path::PathBuf)> {
    let dir = diarize_tap::meeting_diarize_tmp_dir(meeting_id);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut wavs: Vec<(usize, std::path::PathBuf)> = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let index = path
                .file_stem()?
                .to_str()?
                .parse::<usize>()
                .ok()?;
            (path.extension()?.to_str()? == "wav").then_some((index, path))
        })
        .collect();
    wavs.sort_by_key(|(index, _)| *index);
    wavs
}

/// Diarize every tapped session of one meeting and write the resulting
/// speaker labels back. Sessions are processed in order and independently:
/// a failed session is logged and skipped (its segments stay unlabeled), it
/// does not abort the rest. All segment updates land in one transaction at
/// the end. Returns how many segments were relabeled.
fn diarize_meeting(
    db: &Database,
    meeting_id: &str,
    session_wavs: &[(usize, std::path::PathBuf)],
    mut on_session_done: impl FnMut(u32),
) -> Result<usize, String> {
    let settings = crate::settings::AppSettings::load(db)?;
    let meeting = db.load_meeting(meeting_id)?;

    let mut cfg = DiarizeConfig::new(models::segmentation_model_path(), models::embedding_model_path());
    cfg.max_speakers = settings.diarize_max_speakers.map(|n| n as usize);

    let mut assignments: Vec<(u64, Speaker)> = Vec::new();
    for (done, (session_index, wav_path)) in session_wavs.iter().enumerate() {
        match diarize_session(db, &meeting, *session_index, wav_path, &cfg) {
            Ok(mut session_assignments) => assignments.append(&mut session_assignments),
            Err(e) => warn!(
                meeting_id = %meeting_id,
                session = session_index,
                "Diarization failed for one session (its segments stay unlabeled): {e}"
            ),
        }
        on_session_done(done as u32 + 1);
    }

    db.set_segment_speakers(meeting_id, &assignments)
}

/// Diarize one recording session's WAV and return the (sort_order, speaker)
/// assignments for that session's slice of the meeting's segments. Persists
/// speaker rows and centroid updates as a side effect, so a later session in
/// the same meeting (and any later meeting) matches against fresh centroids.
fn diarize_session(
    db: &Database,
    meeting: &crate::transcript::MeetingTranscript,
    session_index: usize,
    wav_path: &std::path::Path,
    cfg: &DiarizeConfig,
) -> Result<Vec<(u64, Speaker)>, String> {
    let session = meeting
        .recording_sessions
        .get(session_index)
        .ok_or_else(|| format!("No recording session at index {session_index}"))?;

    // The session's slice of the meeting's segments, checked before any
    // inference or speaker-row writes: a session with nothing to label must
    // not create orphan "Speaker N" rows.
    let start = session.start_segment_index as usize;
    let end = (session.end_segment_index as usize).min(meeting.segments.len());
    if start >= end {
        return Ok(Vec::new());
    }

    let samples = diarize_tap::read_diarize_wav(wav_path)?;
    let result = crate::diarize::diarize(&samples, crate::diarize::segmentation::SAMPLE_RATE, cfg)
        .map_err(|e| format!("Diarize inference: {e}"))?;
    if result.segments.is_empty() || result.speakers.is_empty() {
        return Ok(Vec::new());
    }

    // Resolve each detected cluster to a persistent speaker id, creating
    // rows / folding centroids as decided by the pure matcher.
    let stored: Vec<StoredSpeaker> = db
        .list_speakers()?
        .into_iter()
        .map(|record| StoredSpeaker {
            id: record.id,
            name: record.name,
            centroid: record.centroid.as_deref().and_then(decode_centroid),
            embedding_count: record.embedding_count,
        })
        .collect();
    let decisions = match_speakers(&result.speakers, &stored);

    let now = chrono::Utc::now();
    let mut cluster_to_speaker: std::collections::HashMap<usize, i64> = std::collections::HashMap::new();
    for decision in &decisions {
        let speaker_id = match decision.matched_speaker_id {
            Some(id) => id,
            None => {
                let name = decision
                    .new_name
                    .as_deref()
                    .ok_or("Match decision with neither id nor name")?;
                db.create_speaker(name)?
            }
        };
        db.update_speaker_centroid(
            speaker_id,
            &encode_centroid(&decision.updated_centroid),
            decision.updated_embedding_count,
            now,
        )?;
        cluster_to_speaker.insert(decision.cluster, speaker_id);
    }

    // Segment times restart at zero for each recording session, and so does
    // the tapped WAV, so session-local segment spans line up with diarized
    // ranges directly.
    let start = session.start_segment_index as usize;
    let end = (session.end_segment_index as usize).min(meeting.segments.len());
    if start >= end {
        return Ok(Vec::new());
    }
    let spans: Vec<SegmentSpan> = meeting.segments[start..end]
        .iter()
        .map(|seg| SegmentSpan {
            start_s: seg.start_time,
            end_s: seg.end_time,
            has_speaker: seg.speaker.is_some(),
        })
        .collect();
    let cluster_per_span = assign_by_overlap(&result.segments, &spans);

    Ok(cluster_per_span
        .into_iter()
        .enumerate()
        .filter_map(|(offset, cluster)| {
            let speaker_id = cluster_to_speaker.get(&cluster?)?;
            Some(((start + offset) as u64, Speaker::Persistent(*speaker_id)))
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::fixtures::{sample_meeting, test_db};

    #[test]
    fn pending_session_wavs_on_missing_dir_is_empty() {
        assert!(pending_session_wavs("no-such-meeting-id-for-tests").is_empty());
    }

    /// End-to-end match-then-assign against the real downloaded models and a
    /// real WAV, with a fake stored speaker seeded so the persistent-match
    /// path is exercised too. Requires both diarization models in the app
    /// models dir and a 16kHz mono WAV at `/tmp/diarize_test.wav` (same
    /// fixture as `diarize::tests::diarize_real_pipeline`). Run with:
    ///   cargo test diarize_task_end_to_end -- --ignored --nocapture
    #[test]
    #[ignore = "requires downloaded diarization models (~32MB) and a test WAV"]
    fn diarize_task_end_to_end_matches_and_assigns() {
        assert!(
            models::models_downloaded(),
            "diarization models missing, download them first"
        );

        let (db, _dir) = test_db();

        // A meeting whose single recording session covers a few unlabeled
        // segments spread over the test WAV's duration.
        let mut meeting = sample_meeting("diarize-e2e");
        meeting.segments = (0..6)
            .map(|i| crate::engine::TranscriptionSegment {
                text: format!("segment {i}"),
                start_time: i as f64 * 2.0,
                end_time: i as f64 * 2.0 + 1.8,
                is_final: true,
                language: None,
                confidence: None,
                speaker: None,
            })
            .collect();
        meeting.recording_sessions[0].start_segment_index = 0;
        meeting.recording_sessions[0].end_segment_index = meeting.segments.len() as u64;
        db.save_meeting(&meeting).unwrap();

        // Seed a fake stored speaker with an orthogonal-ish centroid that
        // should NOT match any real voice cluster (unit vector on dim 0 of a
        // 256-dim space is essentially uncorrelated with real embeddings),
        // proving new-speaker creation coexists with the stored row.
        let fake_id = db.create_speaker("Preexisting").unwrap();
        let mut fake_centroid = vec![0.0f32; 256];
        fake_centroid[0] = 1.0;
        db.update_speaker_centroid(fake_id, &encode_centroid(&fake_centroid), 1, chrono::Utc::now())
            .unwrap();

        let cfg = DiarizeConfig::new(models::segmentation_model_path(), models::embedding_model_path());
        let loaded = db.load_meeting("diarize-e2e").unwrap();
        let assignments = diarize_session(
            &db,
            &loaded,
            0,
            std::path::Path::new("/tmp/diarize_test.wav"),
            &cfg,
        )
        .expect("diarize session");
        assert!(!assignments.is_empty(), "expected at least one labeled segment");

        let changed = db.set_segment_speakers("diarize-e2e", &assignments).unwrap();
        assert_eq!(changed, assignments.len());

        let relabeled = db.load_meeting("diarize-e2e").unwrap();
        let labeled: Vec<_> = relabeled
            .segments
            .iter()
            .filter_map(|s| s.speaker)
            .collect();
        eprintln!("labeled {} of {} segments: {labeled:?}", labeled.len(), relabeled.segments.len());
        assert!(
            labeled
                .iter()
                .all(|s| matches!(s, Speaker::Persistent(id) if *id != fake_id)),
            "real voices must create new speakers, not glom onto the orthogonal fake"
        );

        // Second pass over the same audio must resolve to the SAME speaker
        // rows (the whole point of persistent matching). Reset the labels
        // first so assignment isn't blocked by the has_speaker guard.
        let speakers_before = db.list_speakers().unwrap().len();
        let assignments2 = diarize_session(
            &db,
            &loaded,
            0,
            std::path::Path::new("/tmp/diarize_test.wav"),
            &cfg,
        )
        .expect("second diarize session");
        let speakers_after = db.list_speakers().unwrap().len();
        assert_eq!(
            speakers_before, speakers_after,
            "the same voices must match existing rows, not create new ones"
        );
        assert!(!assignments2.is_empty());
    }
}
