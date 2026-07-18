//! Post-stop offline speaker diarization. Spawned by
//! `stop_meeting_recording` right after the transcript is saved and
//! `MeetingFinalized` is emitted (never delaying either): for each recording
//! session whose mic and/or system audio was tapped to temporary WAVs
//! (`audio::diarize_tap`), run the offline diarization engine, match the
//! detected voice clusters against persistent speakers (`diarize::persist`),
//! map the diarized time ranges onto that session's transcript segments
//! (`diarize::assign`), and write the labels back in one transaction.
//!
//! Mic and system-audio passes are independent for which transcript lane
//! they may rewrite: the mic WAV rewrites Me-lane segments (Them and
//! persistent labels stay locked); the system WAV rewrites Them-lane
//! segments (Me and persistent labels stay locked). Live Me/Them lanes
//! during recording are unchanged. They are NOT independent for persistent
//! speaker matching: within one session, both lanes' inference runs first,
//! then `diarize::persist::resolve_session` matches their clusters together
//! (see its doc comment), so a mic cluster that is just the system audio
//! leaking into the microphone can never spawn a duplicate persistent
//! speaker or steal a stored speaker's claim from the real system cluster.
//!
//! Everything here is best-effort: any failure just leaves the meeting
//! unlabeled (logged, surfaced via `DiarizationProgress.error`), and the
//! temporary WAVs are deleted whether the pass succeeded or not. The body
//! runs under `catch_unwind` so a bug in the ONNX stack can never take the
//! app down from a background thread.

use std::sync::Mutex;

use tauri::Manager;
use tauri_specta::Event;
use tracing::{info, warn};

use crate::app_events::{DiarizationProgress, MeetingDiarized};
use crate::audio::diarize_tap;
use crate::db::Database;
use crate::diarize::assign::{SegmentSpan, assign_by_overlap};
use crate::diarize::persist::{
    MatchDecision, MatchOutcome, MicResolution, StoredSpeaker, decode_embedding, encode_embedding, resolve_session,
};
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

/// Serializes diarization runs across meetings. `diarize_session` reads
/// `list_speakers_with_embeddings`, decides matches, then writes
/// `create_speaker`/`append_speaker_embedding` with no transaction spanning
/// that whole read-decide-write sequence, so two runs interleaving it can
/// both decide "no match" for the same voice and each create their own
/// "Speaker N" row. Held for the whole `run()` call, which also serializes
/// the ONNX inference itself: a smaller cost than the duplicate rows.
static DIARIZE_TASK_LOCK: Mutex<()> = Mutex::new(());

fn run(app: &tauri::AppHandle, meeting_id: &str) {
    // See DIARIZE_TASK_LOCK's doc comment. A panic from an earlier run under
    // this lock (caught by catch_unwind in the caller) must not poison it
    // forever, since diarization for later meetings would then silently stop
    // being serialized.
    let _diarize_task_guard = DIARIZE_TASK_LOCK.lock().unwrap_or_else(|e| e.into_inner());

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
/// it does not abort the rest. Within a session, both passes' inference runs
/// first and only then are their clusters resolved together
/// (`resolve_session`), so persistent speaker matching sees the whole
/// session at once, not one lane at a time. Segment updates land at the end,
/// one transaction per lane: the mic pass may replace live `me` labels, the
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

    let mut mic_assignments: SegmentAssignments = Vec::new();
    let mut system_assignments: SegmentAssignments = Vec::new();
    let mut done_passes = 0u32;
    for session in session_files {
        let mic_pass = if let Some(wav_path) = &session.mic_wav {
            let pass = run_diarize_pass(&meeting, session.session_index, wav_path, &cfg, DiarizationPass::Mic)
                .unwrap_or_else(|e| {
                    warn!(
                        meeting_id = %meeting_id,
                        session = session.session_index,
                        pass = "mic",
                        "Diarization failed for one session (its segments stay unlabeled): {e}"
                    );
                    PassResult::empty()
                });
            done_passes += 1;
            on_pass_done(done_passes);
            pass
        } else {
            PassResult::empty()
        };

        let system_pass = if let Some(wav_path) = &session.system_wav {
            let pass = run_diarize_pass(&meeting, session.session_index, wav_path, &cfg, DiarizationPass::System)
                .unwrap_or_else(|e| {
                    warn!(
                        meeting_id = %meeting_id,
                        session = session.session_index,
                        pass = "system",
                        "Diarization failed for one session (its segments stay unlabeled): {e}"
                    );
                    PassResult::empty()
                });
            done_passes += 1;
            on_pass_done(done_passes);
            pass
        } else {
            PassResult::empty()
        };

        match resolve_and_persist_session(db, meeting_id, &system_pass, &mic_pass) {
            Ok((mut session_mic, mut session_system)) => {
                mic_assignments.append(&mut session_mic);
                system_assignments.append(&mut session_system);
            }
            Err(e) => warn!(
                meeting_id = %meeting_id,
                session = session.session_index,
                "Speaker matching failed for one session (its segments stay unlabeled): {e}"
            ),
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

/// (sort_order, speaker) pairs ready for `Database::set_segment_speakers`.
type SegmentAssignments = Vec<(u64, Speaker)>;

/// One pass's (mic or system) local cluster assignment, ready to be resolved
/// against the other lane and persistent speakers. Carries no database
/// state: `run_diarize_pass` is pure inference plus overlap assignment.
struct PassResult {
    /// Where this session's segments start in the meeting's segment list;
    /// `cluster_per_span[i]` is segment `start + i`.
    start: usize,
    cluster_per_span: Vec<Option<usize>>,
    /// Only clusters that labeled at least one span: an unassigned cluster
    /// (e.g. a stray VAD trigger nothing overlaps) must never create a
    /// stored speaker row or spend one of a speaker's limited embedding
    /// slots.
    clusters_to_match: Vec<SpeakerCentroid>,
}

impl PassResult {
    /// A pass that ran but found nothing to label: absent WAV, failed
    /// inference, an out-of-range session, or no overlapping clusters. Safe
    /// to feed into `resolve_and_persist_session` unconditionally, since an
    /// empty `cluster_per_span` never reads `start`.
    fn empty() -> Self {
        PassResult { start: 0, cluster_per_span: Vec::new(), clusters_to_match: Vec::new() }
    }
}

/// Run inference for one recording session's WAV and map the resulting
/// clusters onto that session's slice of the meeting's segments by overlap.
/// No database access: matching against persistent speakers happens
/// afterwards, once both lanes of the session are available (see
/// `resolve_and_persist_session`).
fn run_diarize_pass(
    meeting: &crate::transcript::MeetingTranscript,
    session_index: usize,
    wav_path: &std::path::Path,
    cfg: &DiarizeConfig,
    pass: DiarizationPass,
) -> Result<PassResult, String> {
    let session = meeting
        .recording_sessions
        .get(session_index)
        .ok_or_else(|| format!("No recording session at index {session_index}"))?;

    // The session's slice of the meeting's segments, checked before any
    // inference: a session with nothing to label must not spend the cost of
    // running the models at all.
    let start = session.start_segment_index as usize;
    let end = (session.end_segment_index as usize).min(meeting.segments.len());
    if start >= end {
        return Ok(PassResult::empty());
    }

    let samples = diarize_tap::read_diarize_wav(wav_path)?;
    let result = crate::diarize::diarize(&samples, crate::diarize::segmentation::SAMPLE_RATE, cfg)
        .map_err(|e| format!("Diarize inference: {e}"))?;
    if result.segments.is_empty() || result.speakers.is_empty() {
        return Ok(PassResult::empty());
    }

    // Segment times restart at zero for each recording session, and so does
    // the tapped WAV, so session-local segment spans line up with diarized
    // ranges directly.
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
        return Ok(PassResult { start, cluster_per_span, clusters_to_match: Vec::new() });
    }

    let clusters_to_match: Vec<SpeakerCentroid> = result
        .speakers
        .into_iter()
        .filter(|c| assigned_clusters.contains(&c.speaker))
        .collect();

    Ok(PassResult { start, cluster_per_span, clusters_to_match })
}

/// Resolve one session's system and mic passes together
/// (`diarize::persist::resolve_session`) and turn the result into
/// `(mic_assignments, system_assignments)`, each a list of (sort_order,
/// speaker) pairs. Persists speaker rows and embeddings as a side effect for
/// every non-echo decision, so a later session (in this meeting or any
/// later one) matches against fresh data; a mic cluster resolved as echo is
/// never persisted on its own (see `MicResolution::Echo`'s doc comment).
fn resolve_and_persist_session(
    db: &Database,
    meeting_id: &str,
    system_pass: &PassResult,
    mic_pass: &PassResult,
) -> Result<(SegmentAssignments, SegmentAssignments), String> {
    let stored: Vec<StoredSpeaker> = db
        .list_speakers_with_embeddings()?
        .into_iter()
        .map(|s| StoredSpeaker {
            id: s.speaker.id,
            name: s.speaker.name,
            embeddings: s.embeddings.iter().filter_map(|e| decode_embedding(e)).collect(),
        })
        .collect();

    let resolution = resolve_session(&system_pass.clusters_to_match, &mic_pass.clusters_to_match, &stored);
    let now = chrono::Utc::now();

    let mut system_cluster_to_speaker: std::collections::HashMap<usize, i64> = std::collections::HashMap::new();
    for decision in &resolution.system {
        if let Some(speaker_id) = persist_match_decision(db, meeting_id, decision, now)? {
            system_cluster_to_speaker.insert(decision.cluster, speaker_id);
        }
    }

    let mut mic_cluster_to_speaker: std::collections::HashMap<usize, i64> = std::collections::HashMap::new();
    for mic_resolution in &resolution.mic {
        match mic_resolution {
            MicResolution::Own(decision) => {
                if let Some(speaker_id) = persist_match_decision(db, meeting_id, decision, now)? {
                    mic_cluster_to_speaker.insert(decision.cluster, speaker_id);
                }
            }
            MicResolution::Echo { cluster, system_cluster } => {
                // A degraded copy of the system voice leaking into the mic:
                // inherit whatever the system cluster resolved to (including
                // nothing, if it stayed Unlabeled), never persist it on its
                // own.
                if let Some(&speaker_id) = system_cluster_to_speaker.get(system_cluster) {
                    mic_cluster_to_speaker.insert(*cluster, speaker_id);
                }
            }
        }
    }

    Ok((
        build_assignments(mic_pass, &mic_cluster_to_speaker),
        build_assignments(system_pass, &system_cluster_to_speaker),
    ))
}

/// Create/append the speaker row for one `MatchDecision`, returning the
/// resulting speaker id, or `None` for `Unlabeled` (nothing to persist).
fn persist_match_decision(
    db: &Database,
    meeting_id: &str,
    decision: &MatchDecision,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<Option<i64>, String> {
    let speaker_id = match &decision.outcome {
        MatchOutcome::Matched { speaker_id } => *speaker_id,
        MatchOutcome::NewSpeaker { name } => db.create_speaker(name)?,
        MatchOutcome::Unlabeled => return Ok(None),
    };
    db.append_speaker_embedding(
        speaker_id,
        Some(meeting_id),
        &encode_embedding(&decision.embedding),
        decision.speech_seconds,
        now,
    )?;
    Ok(Some(speaker_id))
}

/// Turn one pass's cluster-per-span assignment into (sort_order, speaker)
/// pairs, using the final speaker id resolved for each cluster. A span whose
/// cluster never resolved to a speaker (unlabeled, or echo of an unlabeled
/// parent) is simply absent from the result, exactly as before.
fn build_assignments(
    pass: &PassResult,
    cluster_to_speaker: &std::collections::HashMap<usize, i64>,
) -> SegmentAssignments {
    pass.cluster_per_span
        .iter()
        .enumerate()
        .filter_map(|(offset, cluster)| {
            let speaker_id = cluster_to_speaker.get(&(*cluster)?)?;
            Some(((pass.start + offset) as u64, Speaker::Persistent(*speaker_id)))
        })
        .collect()
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
        let mic_pass = run_diarize_pass(
            &loaded,
            0,
            std::path::Path::new("/tmp/diarize_test.wav"),
            &cfg,
            DiarizationPass::Mic,
        )
        .expect("diarize pass");
        let (assignments, system_assignments) =
            resolve_and_persist_session(&db, "diarize-e2e", &PassResult::empty(), &mic_pass)
                .expect("resolve and persist session");
        assert!(!assignments.is_empty(), "expected at least one labeled segment");
        assert!(system_assignments.is_empty(), "no system WAV was tapped, nothing to assign there");

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
        let mic_pass2 = run_diarize_pass(
            &loaded,
            0,
            std::path::Path::new("/tmp/diarize_test.wav"),
            &cfg,
            DiarizationPass::Mic,
        )
        .expect("second diarize pass");
        let (assignments2, _) = resolve_and_persist_session(&db, "diarize-e2e", &PassResult::empty(), &mic_pass2)
            .expect("second resolve and persist session");
        let speakers_after = db.list_speakers().unwrap().len();
        assert_eq!(
            speakers_before, speakers_after,
            "the same voices must match existing rows, not create new ones"
        );
        assert!(!assignments2.is_empty());
    }
}
