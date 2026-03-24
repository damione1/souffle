use chrono::{DateTime, Utc};
use serde::de::{self, Deserializer};
use serde::{Deserialize, Serialize};

use crate::engine::{TranscriptionProfile, TranscriptionSegment, default_transcription_profile};

/// Full meeting transcript stored as JSON
#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct MeetingTranscript {
    pub id: String,
    pub title: String,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub duration_seconds: f64,
    pub transcription_profile: TranscriptionProfile,
    pub segments: Vec<TranscriptionSegment>,
    pub summary: Option<String>,
    pub summary_model: Option<String>,
    pub summary_generated_at: Option<DateTime<Utc>>,
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
    engine: Option<String>,
    segments: Vec<TranscriptionSegment>,
    summary: Option<String>,
    summary_model: Option<String>,
    summary_generated_at: Option<DateTime<Utc>>,
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

        Ok(Self {
            id: wire.id,
            title: wire.title,
            started_at: wire.started_at,
            ended_at: wire.ended_at,
            duration_seconds: wire.duration_seconds,
            transcription_profile,
            segments: wire.segments,
            summary: wire.summary,
            summary_model: wire.summary_model,
            summary_generated_at: wire.summary_generated_at,
        })
    }
}

pub fn resolve_legacy_transcription_profile(
    transcription_profile: Option<TranscriptionProfile>,
    legacy_engine: Option<&str>,
) -> Result<TranscriptionProfile, String> {
    if let Some(profile) = transcription_profile {
        return Ok(profile);
    }

    if let Some(engine_label) = legacy_engine {
        return Ok(TranscriptionProfile::from_legacy_engine(engine_label));
    }

    Ok(default_transcription_profile())
}

/// Lightweight item for listing meetings
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct MeetingListItem {
    pub id: String,
    pub title: String,
    pub started_at: DateTime<Utc>,
    pub duration_seconds: f64,
    pub has_summary: bool,
}

impl From<&MeetingTranscript> for MeetingListItem {
    fn from(t: &MeetingTranscript) -> Self {
        Self {
            id: t.id.clone(),
            title: t.title.clone(),
            started_at: t.started_at,
            duration_seconds: t.duration_seconds,
            has_summary: t.summary.is_some(),
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
            segments: vec![TranscriptionSegment {
                text: "hello".to_string(),
                start_time: 0.0,
                end_time: 1.0,
                is_final: true,
                language: None,
                confidence: None,
            }],
            summary: Some("A summary".to_string()),
            summary_model: Some("test-model".to_string()),
            summary_generated_at: None,
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
            segments: vec![],
            summary: Some("summary".to_string()),
            summary_model: None,
            summary_generated_at: None,
        };

        let item = MeetingListItem::from(&transcript);
        assert_eq!(item.id, "m1");
        assert_eq!(item.title, "My Meeting");
        assert!(item.has_summary);
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
        assert_eq!(deserialized.transcription_profile.engine_label, "Custom Engine");
        assert_eq!(deserialized.transcription_profile.model_id, "legacy");
    }
}
