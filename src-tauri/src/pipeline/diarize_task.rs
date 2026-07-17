//! Post-stop offline speaker diarization. Spawned by
//! `stop_meeting_recording` right after the transcript is saved and
//! `MeetingFinalized` is emitted (never delaying either): for each recording
//! session whose mic and/or system audio was tapped to temporary WAVs
//! (`audio::diarize_tap`), run the offline diarization engine, match the
//! detected voice clusters against persistent speakers (`diarize::persist`),
//! map the diarized time ranges onto that session's transcript segments
//! (`diarize::assign`), and write the labels back in one transaction.
//!
//! Mic and system-audio passes are independent: the mic WAV rewrites Me-lane
//! segments (Them and persistent labels stay locked); the system WAV rewrites
//! Them-lane segments (Me and persistent labels stay locked). Live Me/Them
//! lanes during recording are unchanged.
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
use crate::diarize::persist::{MatchOutcome, StoredSpeaker, decode_embedding, encode_embedding, match_speakers};
use crate::diarize::{DiarizeConfig, SpeakerCentroid, models};
use crate::engine::Speaker;
use crate::state::AppState;

/// Which transcript lane a diarization pass may rewrite.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DiarizationPass {
    /// Mic WAV: assign onto Me-lane (or unlabeled mic-only) segments.
    Mic,
    /// System WAV: assign onto Them-lane segments.
    System,
}

/// Pending diarization WAVs for one recording session.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SessionDiarizeFiles {
    session_index: usize,
    mic_wav: Option<std::path::PathBuf>,
    system_wav: Option<std::path::PathBuf>,
}

/// Kick off the background diarization pass for `meeting_id`. Returns
/// immediately; all work happens on a dedicated thread. A meeting with no
/// pending diarization WAVs (feature off, models missing at record time) is
/// a silent no-op: no events, no logs beyond trace level.
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
    // Pending WAVs are the trigger: they only exist if this meeting was
    // recorded with speaker recognition enabled and models present.
    let session_files = pending_session_diarize_files(meeting_id);
    let total_passes = session_files.iter().map(SessionDiarizeFiles::pass_count).sum::<usize>();
    if total_passes == 0 {
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
    let total_sessions = total_passes as u32;

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

    let outcome = diarize_meeting(db, meeting_id, &session_files, |done| {
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
            } else {
                info!(
                    meeting_id = %meeting_id,
                    "Speaker recognition finished without labeling any segments"
                );
            }
        }
        Err(e) => {
            warn!(meeting_id = %meeting_id, "Speaker recognition failed: {e}");
            emit_progress(total_sessions, true, Some(e));
        }
    }
}

impl SessionDiarizeFiles {
    fn pass_count(&self) -> usize {
        usize::from(self.mic_wav.is_some()) + usize::from(self.system_wav.is_some())
    }
}

/// Discover mic and/or system WAVs waiting for this meeting, grouped by
/// session index and sorted. Recognizes `{index}-mic.wav`, `{index}-system.wav`,
/// and legacy `{index}.wav` (mic-only from older builds).
fn pending_session_diarize_files(meeting_id: &str) -> Vec<SessionDiarizeFiles> {
    let dir = diarize_tap::meeting_diarize_tmp_dir(meeting_id);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };

    let mut by_session: std::collections::BTreeMap<usize, SessionDiarizeFiles> =
        std::collections::BTreeMap::new();

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("wav") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let (session_index, lane) = match parse_diarize_wav_stem(stem) {
            Some(parsed) => parsed,
            None => continue,
        };
        let slot = by_session.entry(session_index).or_insert_with(|| SessionDiarizeFiles {
            session_index,
            mic_wav: None,
            system_wav: None,
        });
        match lane {
            DiarizationPass::Mic => slot.mic_wav = Some(path),
            DiarizationPass::System => slot.system_wav = Some(path),
        }
    }

    by_session.into_values().collect()
}

/// Parse a diarization WAV stem into `(session_index, lane)`.
fn parse_diarize_wav_stem(stem: &str) -> Option<(usize, DiarizationPass)> {
    if let Some(index_str) = stem.strip_suffix("-mic") {
        return Some((index_str.parse().ok()?, DiarizationPass::Mic));
    }
    if let Some(index_str) = stem.strip_suffix("-system") {
        return Some((index_str.parse().ok()?, DiarizationPass::System));
    }
    stem.parse::<usize>().ok().map(|index| (index, DiarizationPass::Mic))
}

/// Diarize every tapped session of one meeting and write the resulting
/// speaker labels back. Each session may run a mic pass, a system pass, or
/// both; a failed pass is logged and skipped (its segments stay unlabeled),
/// it does not abort the rest. Segment updates land at the end, one
/// transaction per lane: the mic pass may replace live `me` labels, the
/// system pass may replace live `them` labels, and either may fill NULL
/// speakers (mic-only meetings). Returns how many segments were relabeled.
fn diarize_meeting(
    db: &Database,
    meeting_id: &str,
    session_files: &[SessionDiarizeFiles],
    mut on_pass_done: impl FnMut(u32),
) -> Result<usize, String> {
    let settings = crate::settings::AppSettings::load(db)?;
    let meeting = db.load_meeting(meeting_id)?;

    let mut cfg = DiarizeConfig::new(models::segmentation_model_path(), models::embedding_model_path());
    cfg.max_speakers = settings.diarize_max_speakers.map(|n| n as usize);

    let mut mic_assignments: Vec<(u64, Speaker)> = Vec::new();
    let mut system_assignments: Vec<(u64, Speaker)> = Vec::new();
    let mut done_passes = 0u32;
    for session in session_files {
        if let Some(wav_path) = &session.mic_wav {
            match diarize_session(
                db,
                &meeting,
                session.session_index,
                wav_path,
                &cfg,
                DiarizationPass::Mic,
            ) {
                Ok(mut session_assignments) => mic_assignments.append(&mut session_assignments),
                Err(e) => warn!(
                    meeting_id = %meeting_id,
                    session = session.session_index,
                    pass = "mic",
                    "Diarization failed for one session (its segments stay unlabeled): {e}"
                ),
            }
            done_passes += 1;
            on_pass_done(done_passes);
        }
        if let Some(wav_path) = &session.system_wav {
            match diarize_session(
                db,
                &meeting,
                session.session_index,
                wav_path,
                &cfg,
                DiarizationPass::System,
            ) {
                Ok(mut session_assignments) => system_assignments.append(&mut session_assignments),
                Err(e) => warn!(
                    meeting_id = %meeting_id,
                    session = session.session_index,
                    pass = "system",
                    "Diarization failed for one session (its segments stay unlabeled): {e}"
                ),
            }
            done_passes += 1;
            on_pass_done(done_passes);
        }
    }

    // The mic write runs first, so a NULL segment both passes claimed keeps
    // the mic label (the system write only replaces NULL or `them`).
    let mic_changed = db.set_segment_speakers(meeting_id, &mic_assignments, Some(Speaker::Me))?;
    let system_changed =
        db.set_segment_speakers(meeting_id, &system_assignments, Some(Speaker::Them))?;
    info!(
        meeting_id = %meeting_id,
        mic_assigned = mic_assignments.len(),
        mic_changed,
        system_assigned = system_assignments.len(),
        system_changed,
        "Speaker recognition assignments written"
    );
    Ok(mic_changed + system_changed)
}

/// Diarize one recording session's WAV and return the (sort_order, speaker)
/// assignments for that session's slice of the meeting's segments. Persists
/// speaker rows and embeddings as a side effect, so a later session in the
/// same meeting (and any later meeting) matches against fresh data.
fn diarize_session(
    db: &Database,
    meeting: &crate::transcript::MeetingTranscript,
    session_index: usize,
    wav_path: &std::path::Path,
    cfg: &DiarizeConfig,
    pass: DiarizationPass,
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

    // Segment times restart at zero for each recording session, and so does
    // the tapped WAV, so session-local segment spans line up with diarized
    // ranges directly. Mapped before any speaker-row write: a pass whose
    // ranges match no segment at all must not create orphan "Speaker N"
    // rows or spend embedding slots on them.
    let spans: Vec<SegmentSpan> = meeting.segments[start..end]
        .iter()
        .map(|seg| SegmentSpan {
            start_s: seg.start_time,
            end_s: seg.end_time,
            has_speaker: speaker_label_is_locked(seg.speaker.as_ref(), pass),
        })
        .collect();
    let cluster_per_span = assign_by_overlap(&result.segments, &spans);
    let assigned_clusters: std::collections::HashSet<usize> =
        cluster_per_span.iter().flatten().copied().collect();
    if assigned_clusters.is_empty() {
        return Ok(Vec::new());
    }

    // Only clusters that actually labeled at least one span are worth
    // matching: an unassigned cluster (e.g. a stray VAD trigger nothing
    // overlaps) must never create a stored speaker row or spend one of a
    // speaker's limited embedding slots.
    let clusters_to_match: Vec<SpeakerCentroid> = result
        .speakers
        .into_iter()
        .filter(|c| assigned_clusters.contains(&c.speaker))
        .collect();

    let stored: Vec<StoredSpeaker> = db
        .list_speakers_with_embeddings()?
        .into_iter()
        .map(|s| StoredSpeaker {
            id: s.speaker.id,
            name: s.speaker.name,
            embeddings: s.embeddings.iter().filter_map(|e| decode_embedding(e)).collect(),
        })
        .collect();
    let decisions = match_speakers(&clusters_to_match, &stored);

    let now = chrono::Utc::now();
    let mut cluster_to_speaker: std::collections::HashMap<usize, i64> = std::collections::HashMap::new();
    for decision in &decisions {
        let speaker_id = match &decision.outcome {
            MatchOutcome::Matched { speaker_id } => *speaker_id,
            MatchOutcome::NewSpeaker { name } => db.create_speaker(name)?,
            MatchOutcome::Unlabeled => continue,
        };
        db.append_speaker_embedding(
            speaker_id,
            Some(meeting.id.as_str()),
            &encode_embedding(&decision.embedding),
            decision.speech_seconds,
            now,
        )?;
        cluster_to_speaker.insert(decision.cluster, speaker_id);
    }

    Ok(cluster_per_span
        .into_iter()
        .enumerate()
        .filter_map(|(offset, cluster)| {
            let speaker_id = cluster_to_speaker.get(&cluster?)?;
            Some(((start + offset) as u64, Speaker::Persistent(*speaker_id)))
        })
        .collect())
}

/// Mic pass locks Them and persistent speakers; system pass locks Me and
/// persistent speakers. Unlabeled segments on the target lane are fair game.
fn speaker_label_is_locked(speaker: Option<&Speaker>, pass: DiarizationPass) -> bool {
    match pass {
        DiarizationPass::Mic => {
            matches!(speaker, Some(Speaker::Them) | Some(Speaker::Persistent(_)))
        }
        DiarizationPass::System => {
            matches!(speaker, Some(Speaker::Me) | Some(Speaker::Persistent(_)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::fixtures::{sample_meeting, test_db};

    #[test]
    fn pending_session_diarize_files_on_missing_dir_is_empty() {
        assert!(pending_session_diarize_files("no-such-meeting-id-for-tests").is_empty());
    }

    #[test]
    fn parse_diarize_wav_stem_recognizes_mic_system_and_legacy() {
        assert_eq!(
            parse_diarize_wav_stem("2-mic"),
            Some((2, DiarizationPass::Mic))
        );
        assert_eq!(
            parse_diarize_wav_stem("2-system"),
            Some((2, DiarizationPass::System))
        );
        assert_eq!(parse_diarize_wav_stem("2"), Some((2, DiarizationPass::Mic)));
        assert_eq!(parse_diarize_wav_stem("2-other"), None);
    }

    #[test]
    fn speaker_label_lock_respects_pass() {
        assert!(speaker_label_is_locked(Some(&Speaker::Them), DiarizationPass::Mic));
        assert!(!speaker_label_is_locked(Some(&Speaker::Me), DiarizationPass::Mic));
        assert!(speaker_label_is_locked(Some(&Speaker::Me), DiarizationPass::System));
        assert!(!speaker_label_is_locked(Some(&Speaker::Them), DiarizationPass::System));
        assert!(speaker_label_is_locked(
            Some(&Speaker::Persistent(1)),
            DiarizationPass::Mic
        ));
        assert!(speaker_label_is_locked(
            Some(&Speaker::Persistent(1)),
            DiarizationPass::System
        ));
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

        // Seed a fake stored speaker with an orthogonal-ish embedding that
        // should NOT match any real voice cluster (unit vector on dim 0 of a
        // 256-dim space is essentially uncorrelated with real embeddings),
        // proving new-speaker creation coexists with the stored row.
        let fake_id = db.create_speaker("Preexisting").unwrap();
        let mut fake_embedding = vec![0.0f32; 256];
        fake_embedding[0] = 1.0;
        db.append_speaker_embedding(fake_id, None, &encode_embedding(&fake_embedding), 10.0, chrono::Utc::now())
            .unwrap();

        let cfg = DiarizeConfig::new(models::segmentation_model_path(), models::embedding_model_path());
        let loaded = db.load_meeting("diarize-e2e").unwrap();
        let assignments = diarize_session(
            &db,
            &loaded,
            0,
            std::path::Path::new("/tmp/diarize_test.wav"),
            &cfg,
            DiarizationPass::Mic,
        )
        .expect("diarize session");
        assert!(!assignments.is_empty(), "expected at least one labeled segment");

        let changed = db
            .set_segment_speakers("diarize-e2e", &assignments, Some(Speaker::Me))
            .unwrap();
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
            DiarizationPass::Mic,
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
