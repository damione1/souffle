use chrono::{DateTime, Utc};
use serde::de::{self, Deserializer};
use serde::{Deserialize, Serialize};

use crate::engine::{
    TranscriptionProfile, TranscriptionSegment, default_transcription_profile,
    resolve_transcription_profile,
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, specta::Type)]
pub struct MeetingRecordingSession {
    pub id: String,
    pub started_at: DateTime<Utc>,
    pub ended_at: DateTime<Utc>,
    pub duration_seconds: f64,
    pub start_segment_index: u64,
    pub end_segment_index: u64,
}

impl MeetingRecordingSession {
    pub fn completed(
        id: String,
        started_at: DateTime<Utc>,
        ended_at: DateTime<Utc>,
        start_segment_index: u64,
        end_segment_index: u64,
    ) -> Self {
        Self {
            id,
            started_at,
            ended_at,
            duration_seconds: (ended_at - started_at).num_seconds().max(0) as f64,
            start_segment_index,
            end_segment_index,
        }
    }
}

/// Full meeting transcript stored as JSON
#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct MeetingTranscript {
    pub id: String,
    pub title: String,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub duration_seconds: f64,
    pub transcription_profile: TranscriptionProfile,
    pub recording_sessions: Vec<MeetingRecordingSession>,
    pub segments: Vec<TranscriptionSegment>,
    pub summary: Option<String>,
    pub summary_is_stale: bool,
    pub summary_model: Option<String>,
    pub summary_generated_at: Option<DateTime<Utc>>,
    pub edited_transcript: Option<String>,
    /// Free-form notes the user typed during the meeting; fed into the
    /// summary prompt.
    pub notes: Option<String>,
}

#[derive(Deserialize)]
struct MeetingTranscriptWire {
    id: String,
    title: String,
    started_at: DateTime<Utc>,
    ended_at: Option<DateTime<Utc>>,
    duration_seconds: f64,
    #[serde(default)]
    transcription_profile: Option<TranscriptionProfile>,
    #[serde(default)]
    recording_sessions: Option<Vec<MeetingRecordingSession>>,
    #[serde(default)]
    engine: Option<String>,
    segments: Vec<TranscriptionSegment>,
    summary: Option<String>,
    #[serde(default)]
    summary_is_stale: Option<bool>,
    summary_model: Option<String>,
    summary_generated_at: Option<DateTime<Utc>>,
    #[serde(default)]
    edited_transcript: Option<String>,
    #[serde(default)]
    notes: Option<String>,
}

impl<'de> Deserialize<'de> for MeetingTranscript {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = MeetingTranscriptWire::deserialize(deserializer)?;
        let transcription_profile = resolve_legacy_transcription_profile(
            wire.transcription_profile,
            wire.engine.as_deref(),
        )
        .map_err(de::Error::custom)?;
        let recording_sessions = resolve_legacy_recording_sessions(
            wire.recording_sessions,
            &wire.id,
            wire.started_at,
            wire.ended_at,
            wire.duration_seconds,
            wire.segments.len(),
        );

        Ok(Self {
            id: wire.id,
            title: wire.title,
            started_at: wire.started_at,
            ended_at: wire.ended_at,
            duration_seconds: wire.duration_seconds,
            transcription_profile,
            recording_sessions,
            segments: wire.segments,
            summary: wire.summary,
            summary_is_stale: wire.summary_is_stale.unwrap_or(false),
            summary_model: wire.summary_model,
            summary_generated_at: wire.summary_generated_at,
            edited_transcript: wire.edited_transcript,
            notes: wire.notes,
        })
    }
}

pub fn resolve_legacy_transcription_profile(
    transcription_profile: Option<TranscriptionProfile>,
    legacy_engine: Option<&str>,
) -> Result<TranscriptionProfile, String> {
    if let Some(profile) = transcription_profile {
        return resolve_transcription_profile(
            Some(&profile.engine_id),
            Some(&profile.model_id),
            Some(&profile.backend_id),
        )
        .or(Ok(profile));
    }

    if let Some(engine_label) = legacy_engine {
        return Ok(TranscriptionProfile::from_legacy_engine(engine_label));
    }

    Ok(default_transcription_profile())
}

pub fn legacy_recording_session(
    meeting_id: &str,
    started_at: DateTime<Utc>,
    ended_at: Option<DateTime<Utc>>,
    duration_seconds: f64,
    segment_count: usize,
) -> MeetingRecordingSession {
    MeetingRecordingSession {
        id: format!("{meeting_id}-session-1"),
        started_at,
        ended_at: ended_at.unwrap_or(started_at),
        duration_seconds,
        start_segment_index: 0,
        end_segment_index: segment_count as u64,
    }
}

pub fn resolve_legacy_recording_sessions(
    recording_sessions: Option<Vec<MeetingRecordingSession>>,
    meeting_id: &str,
    started_at: DateTime<Utc>,
    ended_at: Option<DateTime<Utc>>,
    duration_seconds: f64,
    segment_count: usize,
) -> Vec<MeetingRecordingSession> {
    match recording_sessions {
        Some(sessions) if !sessions.is_empty() => sessions,
        _ => vec![legacy_recording_session(
            meeting_id,
            started_at,
            ended_at,
            duration_seconds,
            segment_count,
        )],
    }
}

/// Lightweight item for listing meetings
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct MeetingListItem {
    pub id: String,
    pub title: String,
    pub started_at: DateTime<Utc>,
    pub duration_seconds: f64,
    pub has_summary: bool,
    pub summary_is_stale: bool,
}

impl From<&MeetingTranscript> for MeetingListItem {
    fn from(t: &MeetingTranscript) -> Self {
        Self {
            id: t.id.clone(),
            title: t.title.clone(),
            started_at: t.started_at,
            duration_seconds: t.duration_seconds,
            has_summary: t.summary.is_some(),
            summary_is_stale: t.summary_is_stale,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::TranscriptionSegment;

    #[test]
    fn meeting_transcript_serialization_round_trip() {
        let meeting = MeetingTranscript {
            id: "test-id".to_string(),
            title: "Test".to_string(),
            started_at: Utc::now(),
            ended_at: None,
            duration_seconds: 30.0,
            transcription_profile: default_transcription_profile(),
            recording_sessions: vec![MeetingRecordingSession::completed(
                "session-1".to_string(),
                Utc::now(),
                Utc::now(),
                0,
                1,
            )],
            segments: vec![TranscriptionSegment {
                text: "hello".to_string(),
                start_time: 0.0,
                end_time: 1.0,
                is_final: true,
                language: None,
                confidence: None,
                speaker: None,
            }],
            summary: Some("A summary".to_string()),
            summary_is_stale: false,
            summary_model: Some("test-model".to_string()),
            summary_generated_at: None,
            edited_transcript: None,
            notes: None,
        };

        let json = serde_json::to_string(&meeting).unwrap();
        let deserialized: MeetingTranscript = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.id, "test-id");
        assert_eq!(deserialized.segments.len(), 1);
        assert_eq!(deserialized.summary.as_deref(), Some("A summary"));
        assert_eq!(
            deserialized.transcription_profile,
            default_transcription_profile()
        );
        assert_eq!(deserialized.recording_sessions.len(), 1);
        assert!(!deserialized.summary_is_stale);
    }

    #[test]
    fn meeting_list_item_from_transcript() {
        let transcript = MeetingTranscript {
            id: "m1".to_string(),
            title: "My Meeting".to_string(),
            started_at: Utc::now(),
            ended_at: None,
            duration_seconds: 120.0,
            transcription_profile: default_transcription_profile(),
            recording_sessions: vec![MeetingRecordingSession::completed(
                "session-1".to_string(),
                Utc::now(),
                Utc::now(),
                0,
                0,
            )],
            segments: vec![],
            summary: Some("summary".to_string()),
            summary_is_stale: true,
            summary_model: None,
            summary_generated_at: None,
            edited_transcript: None,
            notes: None,
        };

        let item = MeetingListItem::from(&transcript);
        assert_eq!(item.id, "m1");
        assert_eq!(item.title, "My Meeting");
        assert!(item.has_summary);
        assert!(item.summary_is_stale);
    }

    #[test]
    fn resolve_legacy_profile_both_none() {
        let p = resolve_legacy_transcription_profile(None, None).unwrap();
        assert_eq!(p, default_transcription_profile());
    }

    #[test]
    fn resolve_legacy_profile_wins_over_engine() {
        let profile = default_transcription_profile();
        let p =
            resolve_legacy_transcription_profile(Some(profile.clone()), Some("ignored")).unwrap();
        assert_eq!(p, profile);
    }

    #[test]
    fn resolve_legacy_sessions_empty_creates_fallback() {
        let now = Utc::now();
        let sessions = resolve_legacy_recording_sessions(None, "test-id", now, Some(now), 10.0, 0);
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, "test-id-session-1");
    }

    #[test]
    fn recording_session_completed_duration() {
        let start = Utc::now();
        let end = start + chrono::Duration::seconds(120);
        let session = MeetingRecordingSession::completed("s1".to_string(), start, end, 0, 10);
        assert!((session.duration_seconds - 120.0).abs() < 0.1);
    }

    #[test]
    fn legacy_meeting_json_uses_engine_as_profile_fallback() {
        let json = serde_json::json!({
            "id": "legacy-id",
            "title": "Legacy",
            "started_at": Utc::now(),
            "ended_at": null,
            "duration_seconds": 42.0,
            "engine": "Custom Engine",
            "segments": [],
            "summary": null,
            "summary_model": null,
            "summary_generated_at": null
        });

        let deserialized: MeetingTranscript = serde_json::from_value(json).unwrap();
        assert_eq!(
            deserialized.transcription_profile.engine_label,
            "Custom Engine"
        );
        assert_eq!(deserialized.transcription_profile.model_id, "legacy");
        assert_eq!(deserialized.recording_sessions.len(), 1);
        assert!(!deserialized.summary_is_stale);
    }
}
