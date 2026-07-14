use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::constants::OLLAMA_DEFAULT_URL;
use crate::db::Database;
use crate::engine::{
    CANDLE_BACKEND_ID, KYUTAI_ENGINE_ID, KYUTAI_MODEL_ID, resolve_transcription_profile,
};
use crate::logging::LogLevel;

const THEME_KEY: &str = "theme";
const AUTO_PASTE_KEY: &str = "auto_paste";
const PASTE_DELAY_MS_KEY: &str = "paste_delay_ms";
const OLLAMA_URL_KEY: &str = "ollama_url";
const OLLAMA_MODEL_KEY: &str = "ollama_model";
const DEBUG_TRANSCRIPTION_KEY: &str = "debug_transcription";
const AUDIO_DEVICE_KEY: &str = "audio_device";
const CLAMSHELL_AUDIO_DEVICE_KEY: &str = "clamshell_audio_device";
const TRANSCRIPTION_ENGINE_ID_KEY: &str = "transcription_engine_id";
const TRANSCRIPTION_MODEL_ID_KEY: &str = "transcription_model_id";
const TRANSCRIPTION_BACKEND_ID_KEY: &str = "transcription_backend_id";
const VAD_ENABLED_KEY: &str = "vad_enabled";
const FILLER_REMOVAL_KEY: &str = "filler_removal";
const STUTTER_COLLAPSE_KEY: &str = "stutter_collapse";
const DICTIONARY_CORRECTION_KEY: &str = "dictionary_correction";
const CAPTURE_SYSTEM_AUDIO_KEY: &str = "capture_system_audio";
const CALENDAR_INTEGRATION_ENABLED_KEY: &str = "calendar_integration_enabled";
const CALENDAR_SELECTED_IDS_KEY: &str = "calendar_selected_ids";
const CALENDAR_REMINDER_MINUTES_KEY: &str = "calendar_reminder_minutes";
const CALENDAR_AUTOSTART_ENABLED_KEY: &str = "calendar_autostart_enabled";
const FEEDBACK_SOUNDS_ENABLED_KEY: &str = "feedback_sounds_enabled";
const FEEDBACK_SOUNDS_VOLUME_KEY: &str = "feedback_sounds_volume";
const MODEL_UNLOAD_TIMEOUT_MINUTES_KEY: &str = "model_unload_timeout_minutes";
const MEETING_AUTOSTOP_ENABLED_KEY: &str = "meeting_autostop_enabled";
const MEETING_AUTOSTOP_MINUTES_KEY: &str = "meeting_autostop_minutes";
const MEETING_MAX_DURATION_MINUTES_KEY: &str = "meeting_max_duration_minutes";
const MEETING_SMART_START_ENABLED_KEY: &str = "meeting_smart_start_enabled";
const MEETING_SMART_STOP_ENABLED_KEY: &str = "meeting_smart_stop_enabled";
const LOCALE_KEY: &str = "locale";
const SHORTCUT_TOGGLE_KEY: &str = "shortcut_toggle";
const SHORTCUT_PUSH_TO_TALK_KEY: &str = "shortcut_push_to_talk";
const DICTATION_POLISH_ENABLED_KEY: &str = "dictation_polish_enabled";
const DICTATION_POLISH_TEMPLATE_ID_KEY: &str = "dictation_polish_template_id";
const DICTATION_POLISH_TEMPLATES_KEY: &str = "dictation_polish_templates";
const DEFAULT_SUMMARY_TEMPLATE_ID_KEY: &str = "default_summary_template_id";
const SUMMARY_TEMPLATES_KEY: &str = "summary_templates";
const LOG_LEVEL_KEY: &str = "log_level";
const PASTE_METHOD_KEY: &str = "paste_method";
const LAST_SEEN_VERSION_KEY: &str = "last_seen_version";
const MEETING_AUDIO_RETENTION_KEY: &str = "meeting_audio_retention";
const MEETING_TRANSCRIPTION_LANGUAGE_KEY: &str = "meeting_transcription_language";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum PasteMethod {
    #[default]
    Clipboard,
    Type,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, specta::Type)]
pub struct DictationPolishTemplate {
    pub id: String,
    pub label: String,
    pub prompt: String,
}

/// A meeting-summary template: `prompt` replaces the final-pass system
/// prompt only (map/merge prompts stay fixed). Built-ins ship with
/// well-known ids and are non-deletable; the user can also add their own.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, specta::Type)]
pub struct SummaryTemplate {
    pub id: String,
    pub name: String,
    pub prompt: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default, specta::Type)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    #[default]
    Dark,
    Light,
    System,
}

/// How long recorded meeting audio is kept on disk before the startup sweep
/// deletes it. Opt-in: recording itself only happens when this is not `Off`.
/// Heuristic prior for meeting language detection and mismatch resets.
/// Never passed to the STT engine as a forced decode language.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum MeetingTranscriptionLanguage {
    #[default]
    Auto,
    En,
    Fr,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum MeetingAudioRetention {
    /// No recording at all; existing recordings from a previous, more
    /// permissive setting are left alone (the user may re-enable).
    #[default]
    Off,
    // Explicit renames: serde's snake_case heuristic and specta's TS-type
    // heuristic disagree on letter/digit boundaries (serde emits "keep7d",
    // specta types it as "keep_7d") — pin the wire value explicitly so both
    // sides agree instead of relying on either deriving it the same way.
    #[serde(rename = "keep_7d")]
    Keep7d,
    #[serde(rename = "keep_30d")]
    Keep30d,
    KeepForever,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, specta::Type)]
pub struct AppSettings {
    pub theme: Theme,
    pub locale: String,
    pub auto_paste: bool,
    pub paste_delay_ms: u64,
    /// How dictation text is inserted: clipboard Cmd+V or simulated keystrokes.
    pub paste_method: PasteMethod,
    pub ollama_url: String,
    pub ollama_model: String,
    pub debug_transcription: bool,
    /// Global tracing verbosity for the `souffle` crate.
    pub log_level: LogLevel,
    pub audio_device: Option<String>,
    /// Preferred microphone while the lid is closed with an external display
    /// attached (clamshell mode). `None` means just follow whatever macOS
    /// reports as the default input, the previous behavior.
    pub clamshell_audio_device: Option<String>,
    pub transcription_engine_id: String,
    pub transcription_model_id: String,
    pub transcription_backend_id: String,
    pub vad_enabled: bool,
    pub filler_removal: bool,
    pub stutter_collapse: bool,
    pub dictionary_correction: bool,
    /// Meeting mode: capture system audio (other participants) alongside
    /// the microphone via a Core Audio tap.
    pub capture_system_audio: bool,
    /// Calendar integration is opt-in: it reads the user's calendar, so it
    /// stays off until explicitly enabled (which triggers the TCC prompt).
    pub calendar_integration_enabled: bool,
    /// Calendars shown in the today list; empty means all calendars.
    pub calendar_selected_ids: Vec<String>,
    /// How long before an event the "start transcription?" reminder fires.
    pub calendar_reminder_minutes: u32,
    /// When a calendar event starts and system audio is active, suggest
    /// starting a meeting transcription (nudge only, never auto-records).
    pub calendar_autostart_enabled: bool,
    /// Audible start/stop cues for dictation sessions.
    pub feedback_sounds_enabled: bool,
    /// Feedback sound volume (0-100).
    pub feedback_sounds_volume: u32,
    /// Unload the transcription model after this many idle minutes to
    /// reclaim RAM; 0 means never unload. The next recording reloads it
    /// through the normal load flow.
    pub model_unload_timeout_minutes: u32,
    /// Detect a meeting that has probably ended (no speech for a while, or
    /// the max-duration failsafe) and offer/auto-stop. A session snapshots
    /// these settings at start; changes apply from the next meeting.
    pub meeting_autostop_enabled: bool,
    /// Silence duration (minutes) before the meeting is considered idle.
    pub meeting_autostop_minutes: u32,
    /// Hard failsafe: stop the meeting after this many minutes regardless of
    /// speech activity.
    pub meeting_max_duration_minutes: u32,
    /// Suggest starting a meeting when a known app captures the mic, system
    /// audio is active, and/or a calendar event is in progress (coalesced).
    pub meeting_smart_start_enabled: bool,
    /// During recording, offer to stop when a meeting app closes or the mic
    /// is no longer captured (alongside silence-based auto-stop).
    pub meeting_smart_stop_enabled: bool,
    /// Opt-in recording of meeting audio to compressed files on disk, and
    /// for how long they're kept. Off by default.
    pub meeting_audio_retention: MeetingAudioRetention,
    /// Heuristic prior for meeting language stability (LID + lane resets).
    /// Does not force Kyutai/moshi decode language.
    pub meeting_transcription_language: MeetingTranscriptionLanguage,
    /// Optional LLM post-processing applied to dictation before paste/history.
    pub dictation_polish_enabled: bool,
    /// Active polish template id (email, bullets, no_fillers).
    pub dictation_polish_template_id: String,
    /// User-editable polish prompt templates.
    pub dictation_polish_templates: Vec<DictationPolishTemplate>,
    /// Active default meeting-summary template id: used by the Generate
    /// button when the user doesn't pick another template, and by any
    /// automatic summarization.
    pub default_summary_template_id: String,
    /// User-editable meeting-summary templates (final-pass system prompt).
    pub summary_templates: Vec<SummaryTemplate>,
    /// App version the user has acknowledged (What's New / post-update dialog).
    pub last_seen_version: String,
}

/// Allowed values for `model_unload_timeout_minutes`: 0 (never) plus the
/// options offered in the settings UI.
const ALLOWED_UNLOAD_TIMEOUT_MINUTES: [u32; 4] = [0, 5, 15, 60];

const MEETING_AUTOSTOP_MINUTES_RANGE: std::ops::RangeInclusive<u32> = 3..=60;
const MEETING_MAX_DURATION_MINUTES_RANGE: std::ops::RangeInclusive<u32> = 60..=720;

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            theme: Theme::Dark,
            locale: String::new(),
            auto_paste: false,
            paste_delay_ms: 100,
            paste_method: PasteMethod::default(),
            ollama_url: OLLAMA_DEFAULT_URL.to_string(),
            ollama_model: String::new(),
            debug_transcription: false,
            log_level: LogLevel::default(),
            audio_device: None,
            clamshell_audio_device: None,
            transcription_engine_id: KYUTAI_ENGINE_ID.to_string(),
            transcription_model_id: KYUTAI_MODEL_ID.to_string(),
            transcription_backend_id: CANDLE_BACKEND_ID.to_string(),
            vad_enabled: true,
            filler_removal: true,
            stutter_collapse: false,
            dictionary_correction: true,
            capture_system_audio: true,
            calendar_integration_enabled: false,
            calendar_selected_ids: Vec::new(),
            calendar_reminder_minutes: 2,
            calendar_autostart_enabled: true,
            feedback_sounds_enabled: true,
            feedback_sounds_volume: 70,
            model_unload_timeout_minutes: 0,
            meeting_autostop_enabled: true,
            meeting_autostop_minutes: 10,
            meeting_max_duration_minutes: 240,
            meeting_smart_start_enabled: true,
            meeting_smart_stop_enabled: true,
            meeting_audio_retention: MeetingAudioRetention::default(),
            meeting_transcription_language: MeetingTranscriptionLanguage::default(),
            dictation_polish_enabled: false,
            dictation_polish_template_id: crate::summary::TEMPLATE_EMAIL.to_string(),
            dictation_polish_templates: crate::summary::default_polish_templates(),
            default_summary_template_id: crate::summary::TEMPLATE_SUMMARY_DEFAULT.to_string(),
            summary_templates: crate::summary::default_summary_templates(),
            last_seen_version: String::new(),
        }
    }
}

impl AppSettings {
    pub fn pipeline_config(&self) -> crate::filter::PipelineConfig {
        let vad_model_path = if self.vad_enabled {
            crate::filter::resolve_vad_model_path()
        } else {
            None
        };

        crate::filter::PipelineConfig {
            vad_enabled: self.vad_enabled,
            vad_model_path,
            filler_removal_enabled: self.filler_removal,
            stutter_collapse_enabled: self.stutter_collapse,
            dictionary_correction_enabled: self.dictionary_correction,
        }
    }

    pub fn load(db: &Database) -> Result<Self, String> {
        let mut settings = Self::default();

        if let Some(theme) = read_json_setting::<Theme>(db, THEME_KEY)? {
            settings.theme = theme;
        }
        if let Some(locale) = read_json_setting::<String>(db, LOCALE_KEY)? {
            settings.locale = locale;
        }
        if let Some(auto_paste) = read_json_setting::<bool>(db, AUTO_PASTE_KEY)? {
            settings.auto_paste = auto_paste;
        }
        if let Some(paste_delay_ms) = read_json_setting::<u64>(db, PASTE_DELAY_MS_KEY)? {
            settings.paste_delay_ms = paste_delay_ms;
        }
        if let Some(paste_method) = read_json_setting::<PasteMethod>(db, PASTE_METHOD_KEY)? {
            settings.paste_method = paste_method;
        }
        if let Some(ollama_url) = read_json_setting::<String>(db, OLLAMA_URL_KEY)?
            && !ollama_url.trim().is_empty()
        {
            settings.ollama_url = ollama_url;
        }
        if let Some(ollama_model) = read_json_setting::<String>(db, OLLAMA_MODEL_KEY)? {
            settings.ollama_model = ollama_model;
        }
        if let Some(debug_transcription) = read_json_setting::<bool>(db, DEBUG_TRANSCRIPTION_KEY)? {
            settings.debug_transcription = debug_transcription;
        }
        if let Some(log_level) = read_json_setting::<LogLevel>(db, LOG_LEVEL_KEY)? {
            settings.log_level = log_level;
        }
        if let Some(audio_device) = read_json_setting::<String>(db, AUDIO_DEVICE_KEY)? {
            settings.audio_device = Some(audio_device);
        }
        if let Some(clamshell_audio_device) =
            read_json_setting::<String>(db, CLAMSHELL_AUDIO_DEVICE_KEY)?
        {
            settings.clamshell_audio_device = Some(clamshell_audio_device);
        }
        if let Some(transcription_engine_id) =
            read_json_setting::<String>(db, TRANSCRIPTION_ENGINE_ID_KEY)?
        {
            settings.transcription_engine_id = transcription_engine_id;
        }
        if let Some(transcription_model_id) =
            read_json_setting::<String>(db, TRANSCRIPTION_MODEL_ID_KEY)?
        {
            settings.transcription_model_id = transcription_model_id;
        }
        if let Some(transcription_backend_id) =
            read_json_setting::<String>(db, TRANSCRIPTION_BACKEND_ID_KEY)?
        {
            settings.transcription_backend_id = transcription_backend_id;
        }
        if let Some(vad_enabled) = read_json_setting::<bool>(db, VAD_ENABLED_KEY)? {
            settings.vad_enabled = vad_enabled;
        }
        if let Some(filler_removal) = read_json_setting::<bool>(db, FILLER_REMOVAL_KEY)? {
            settings.filler_removal = filler_removal;
        }
        if let Some(stutter_collapse) = read_json_setting::<bool>(db, STUTTER_COLLAPSE_KEY)? {
            settings.stutter_collapse = stutter_collapse;
        }
        if let Some(dictionary_correction) =
            read_json_setting::<bool>(db, DICTIONARY_CORRECTION_KEY)?
        {
            settings.dictionary_correction = dictionary_correction;
        }
        if let Some(capture_system_audio) = read_json_setting::<bool>(db, CAPTURE_SYSTEM_AUDIO_KEY)?
        {
            settings.capture_system_audio = capture_system_audio;
        }
        if let Some(calendar_integration_enabled) =
            read_json_setting::<bool>(db, CALENDAR_INTEGRATION_ENABLED_KEY)?
        {
            settings.calendar_integration_enabled = calendar_integration_enabled;
        }
        if let Some(calendar_selected_ids) =
            read_json_setting::<Vec<String>>(db, CALENDAR_SELECTED_IDS_KEY)?
        {
            settings.calendar_selected_ids = calendar_selected_ids;
        }
        if let Some(calendar_reminder_minutes) =
            read_json_setting::<u32>(db, CALENDAR_REMINDER_MINUTES_KEY)?
        {
            settings.calendar_reminder_minutes = calendar_reminder_minutes;
        }
        if let Some(calendar_autostart_enabled) =
            read_json_setting::<bool>(db, CALENDAR_AUTOSTART_ENABLED_KEY)?
        {
            settings.calendar_autostart_enabled = calendar_autostart_enabled;
        }
        if let Some(feedback_sounds_enabled) =
            read_json_setting::<bool>(db, FEEDBACK_SOUNDS_ENABLED_KEY)?
        {
            settings.feedback_sounds_enabled = feedback_sounds_enabled;
        }
        if let Some(feedback_sounds_volume) =
            read_json_setting::<u32>(db, FEEDBACK_SOUNDS_VOLUME_KEY)?
        {
            settings.feedback_sounds_volume = feedback_sounds_volume;
        }
        if let Some(model_unload_timeout_minutes) =
            read_json_setting::<u32>(db, MODEL_UNLOAD_TIMEOUT_MINUTES_KEY)?
        {
            settings.model_unload_timeout_minutes = model_unload_timeout_minutes;
        }
        if let Some(meeting_autostop_enabled) =
            read_json_setting::<bool>(db, MEETING_AUTOSTOP_ENABLED_KEY)?
        {
            settings.meeting_autostop_enabled = meeting_autostop_enabled;
        }
        if let Some(meeting_autostop_minutes) =
            read_json_setting::<u32>(db, MEETING_AUTOSTOP_MINUTES_KEY)?
        {
            settings.meeting_autostop_minutes = meeting_autostop_minutes;
        }
        if let Some(meeting_max_duration_minutes) =
            read_json_setting::<u32>(db, MEETING_MAX_DURATION_MINUTES_KEY)?
        {
            settings.meeting_max_duration_minutes = meeting_max_duration_minutes;
        }
        if let Some(meeting_smart_start_enabled) =
            read_json_setting::<bool>(db, MEETING_SMART_START_ENABLED_KEY)?
        {
            settings.meeting_smart_start_enabled = meeting_smart_start_enabled;
        }
        if let Some(meeting_smart_stop_enabled) =
            read_json_setting::<bool>(db, MEETING_SMART_STOP_ENABLED_KEY)?
        {
            settings.meeting_smart_stop_enabled = meeting_smart_stop_enabled;
        }
        if let Some(meeting_audio_retention) =
            read_json_setting::<MeetingAudioRetention>(db, MEETING_AUDIO_RETENTION_KEY)?
        {
            settings.meeting_audio_retention = meeting_audio_retention;
        }
        if let Some(meeting_transcription_language) =
            read_json_setting::<MeetingTranscriptionLanguage>(db, MEETING_TRANSCRIPTION_LANGUAGE_KEY)?
        {
            settings.meeting_transcription_language = meeting_transcription_language;
        }
        if let Some(dictation_polish_enabled) =
            read_json_setting::<bool>(db, DICTATION_POLISH_ENABLED_KEY)?
        {
            settings.dictation_polish_enabled = dictation_polish_enabled;
        }
        if let Some(dictation_polish_template_id) =
            read_json_setting::<String>(db, DICTATION_POLISH_TEMPLATE_ID_KEY)?
        {
            settings.dictation_polish_template_id = dictation_polish_template_id;
        }
        if let Some(dictation_polish_templates) =
            read_json_setting::<Vec<DictationPolishTemplate>>(db, DICTATION_POLISH_TEMPLATES_KEY)?
        {
            settings.dictation_polish_templates =
                crate::summary::merge_polish_templates(dictation_polish_templates);
        }
        if let Some(default_summary_template_id) =
            read_json_setting::<String>(db, DEFAULT_SUMMARY_TEMPLATE_ID_KEY)?
        {
            settings.default_summary_template_id = default_summary_template_id;
        }
        if let Some(summary_templates) =
            read_json_setting::<Vec<SummaryTemplate>>(db, SUMMARY_TEMPLATES_KEY)?
        {
            settings.summary_templates = crate::summary::merge_summary_templates(summary_templates);
        }
        if let Some(last_seen_version) = read_json_setting::<String>(db, LAST_SEEN_VERSION_KEY)? {
            settings.last_seen_version = last_seen_version;
        }

        Ok(settings.sanitized())
    }

    pub fn sanitize_for_save(&self) -> Result<Self, String> {
        let mut normalized = self.sanitized();

        if self.ollama_url.trim().is_empty() {
            return Err("Ollama URL cannot be empty".into());
        }

        if !(50..=1000).contains(&self.paste_delay_ms) {
            return Err("Paste delay must be between 50 and 1000 ms".into());
        }

        let profile = resolve_transcription_profile(
            Some(&normalized.transcription_engine_id),
            Some(&normalized.transcription_model_id),
            Some(&normalized.transcription_backend_id),
        )?;
        normalized.transcription_engine_id = profile.engine_id;
        normalized.transcription_model_id = profile.model_id;
        normalized.transcription_backend_id = profile.backend_id;

        Ok(normalized)
    }

    fn sanitized(&self) -> Self {
        let mut normalized = self.clone();
        normalized.locale = normalized.locale.trim().to_string();
        normalized.ollama_url = normalized.ollama_url.trim().to_string();
        normalized.ollama_model = normalized.ollama_model.trim().to_string();
        normalized.transcription_engine_id = normalized.transcription_engine_id.trim().to_string();
        normalized.transcription_model_id = normalized.transcription_model_id.trim().to_string();
        normalized.transcription_backend_id =
            normalized.transcription_backend_id.trim().to_string();
        normalized.audio_device = normalized
            .audio_device
            .as_ref()
            .map(|device| device.trim().to_string())
            .filter(|device| !device.is_empty());
        normalized.clamshell_audio_device = normalized
            .clamshell_audio_device
            .as_ref()
            .map(|device| device.trim().to_string())
            .filter(|device| !device.is_empty());

        if normalized.ollama_url.is_empty() {
            normalized.ollama_url = OLLAMA_DEFAULT_URL.to_string();
        }

        if normalized.transcription_engine_id.is_empty() {
            normalized.transcription_engine_id = KYUTAI_ENGINE_ID.to_string();
        }

        if normalized.transcription_model_id.is_empty() {
            normalized.transcription_model_id = KYUTAI_MODEL_ID.to_string();
        }
        if normalized.transcription_backend_id.is_empty() {
            normalized.transcription_backend_id = CANDLE_BACKEND_ID.to_string();
        }

        if let Ok(profile) = resolve_transcription_profile(
            Some(&normalized.transcription_engine_id),
            Some(&normalized.transcription_model_id),
            Some(&normalized.transcription_backend_id),
        ) {
            normalized.transcription_engine_id = profile.engine_id;
            normalized.transcription_model_id = profile.model_id;
            normalized.transcription_backend_id = profile.backend_id;
        } else {
            normalized.transcription_engine_id = KYUTAI_ENGINE_ID.to_string();
            normalized.transcription_model_id = KYUTAI_MODEL_ID.to_string();
            normalized.transcription_backend_id = CANDLE_BACKEND_ID.to_string();
        }

        if !(50..=1000).contains(&normalized.paste_delay_ms) {
            normalized.paste_delay_ms = Self::default().paste_delay_ms;
        }

        normalized.calendar_selected_ids = {
            let mut ids: Vec<String> = normalized
                .calendar_selected_ids
                .iter()
                .map(|id| id.trim().to_string())
                .filter(|id| !id.is_empty())
                .collect();
            ids.dedup();
            ids
        };
        if !(1..=30).contains(&normalized.calendar_reminder_minutes) {
            normalized.calendar_reminder_minutes = Self::default().calendar_reminder_minutes;
        }
        if normalized.feedback_sounds_volume > 100 {
            normalized.feedback_sounds_volume = Self::default().feedback_sounds_volume;
        }

        if !ALLOWED_UNLOAD_TIMEOUT_MINUTES.contains(&normalized.model_unload_timeout_minutes) {
            normalized.model_unload_timeout_minutes = Self::default().model_unload_timeout_minutes;
        }

        if !MEETING_AUTOSTOP_MINUTES_RANGE.contains(&normalized.meeting_autostop_minutes) {
            normalized.meeting_autostop_minutes = Self::default().meeting_autostop_minutes;
        }
        if !MEETING_MAX_DURATION_MINUTES_RANGE.contains(&normalized.meeting_max_duration_minutes) {
            normalized.meeting_max_duration_minutes = Self::default().meeting_max_duration_minutes;
        }

        if !matches!(
            normalized.meeting_transcription_language,
            MeetingTranscriptionLanguage::Auto
                | MeetingTranscriptionLanguage::En
                | MeetingTranscriptionLanguage::Fr
        ) {
            normalized.meeting_transcription_language =
                Self::default().meeting_transcription_language;
        }

        normalized.dictation_polish_template_id = normalized
            .dictation_polish_template_id
            .trim()
            .to_string();
        if normalized.dictation_polish_templates.is_empty() {
            normalized.dictation_polish_templates = crate::summary::default_polish_templates();
        } else {
            normalized.dictation_polish_templates =
                crate::summary::merge_polish_templates(normalized.dictation_polish_templates.clone());
        }
        if !normalized
            .dictation_polish_templates
            .iter()
            .any(|template| template.id == normalized.dictation_polish_template_id)
        {
            normalized.dictation_polish_template_id = normalized
                .dictation_polish_templates
                .first()
                .map(|template| template.id.clone())
                .unwrap_or_else(|| crate::summary::TEMPLATE_EMAIL.to_string());
        }
        for template in &mut normalized.dictation_polish_templates {
            template.id = template.id.trim().to_string();
            template.label = template.label.trim().to_string();
            template.prompt = template.prompt.trim().to_string();
        }

        normalized.default_summary_template_id =
            normalized.default_summary_template_id.trim().to_string();
        if normalized.summary_templates.is_empty() {
            normalized.summary_templates = crate::summary::default_summary_templates();
        } else {
            normalized.summary_templates =
                crate::summary::merge_summary_templates(normalized.summary_templates.clone());
        }
        if !normalized
            .summary_templates
            .iter()
            .any(|template| template.id == normalized.default_summary_template_id)
        {
            normalized.default_summary_template_id = normalized
                .summary_templates
                .first()
                .map(|template| template.id.clone())
                .unwrap_or_else(|| crate::summary::TEMPLATE_SUMMARY_DEFAULT.to_string());
        }
        for template in &mut normalized.summary_templates {
            template.id = template.id.trim().to_string();
            template.name = template.name.trim().to_string();
            template.prompt = template.prompt.trim().to_string();
        }

        normalized
    }

    pub fn save(&self, db: &Database) -> Result<(), String> {
        let normalized = self.sanitize_for_save()?;

        write_json_setting(db, THEME_KEY, &normalized.theme)?;
        write_json_setting(db, LOCALE_KEY, &normalized.locale)?;
        write_json_setting(db, AUTO_PASTE_KEY, &normalized.auto_paste)?;
        write_json_setting(db, PASTE_DELAY_MS_KEY, &normalized.paste_delay_ms)?;
        write_json_setting(db, PASTE_METHOD_KEY, &normalized.paste_method)?;
        write_json_setting(db, OLLAMA_URL_KEY, &normalized.ollama_url)?;
        write_json_setting(db, OLLAMA_MODEL_KEY, &normalized.ollama_model)?;
        write_json_setting(
            db,
            TRANSCRIPTION_ENGINE_ID_KEY,
            &normalized.transcription_engine_id,
        )?;
        write_json_setting(
            db,
            TRANSCRIPTION_MODEL_ID_KEY,
            &normalized.transcription_model_id,
        )?;
        write_json_setting(
            db,
            TRANSCRIPTION_BACKEND_ID_KEY,
            &normalized.transcription_backend_id,
        )?;
        write_json_setting(db, DEBUG_TRANSCRIPTION_KEY, &normalized.debug_transcription)?;
        write_json_setting(db, LOG_LEVEL_KEY, &normalized.log_level)?;
        write_json_setting(db, VAD_ENABLED_KEY, &normalized.vad_enabled)?;
        write_json_setting(db, FILLER_REMOVAL_KEY, &normalized.filler_removal)?;
        write_json_setting(db, STUTTER_COLLAPSE_KEY, &normalized.stutter_collapse)?;
        write_json_setting(
            db,
            DICTIONARY_CORRECTION_KEY,
            &normalized.dictionary_correction,
        )?;
        write_json_setting(
            db,
            CAPTURE_SYSTEM_AUDIO_KEY,
            &normalized.capture_system_audio,
        )?;
        write_json_setting(
            db,
            CALENDAR_INTEGRATION_ENABLED_KEY,
            &normalized.calendar_integration_enabled,
        )?;
        write_json_setting(
            db,
            CALENDAR_SELECTED_IDS_KEY,
            &normalized.calendar_selected_ids,
        )?;
        write_json_setting(
            db,
            CALENDAR_REMINDER_MINUTES_KEY,
            &normalized.calendar_reminder_minutes,
        )?;
        write_json_setting(
            db,
            CALENDAR_AUTOSTART_ENABLED_KEY,
            &normalized.calendar_autostart_enabled,
        )?;
        write_json_setting(
            db,
            FEEDBACK_SOUNDS_ENABLED_KEY,
            &normalized.feedback_sounds_enabled,
        )?;
        write_json_setting(
            db,
            FEEDBACK_SOUNDS_VOLUME_KEY,
            &normalized.feedback_sounds_volume,
        )?;
        write_json_setting(
            db,
            MODEL_UNLOAD_TIMEOUT_MINUTES_KEY,
            &normalized.model_unload_timeout_minutes,
        )?;
        write_json_setting(
            db,
            MEETING_AUTOSTOP_ENABLED_KEY,
            &normalized.meeting_autostop_enabled,
        )?;
        write_json_setting(
            db,
            MEETING_AUTOSTOP_MINUTES_KEY,
            &normalized.meeting_autostop_minutes,
        )?;
        write_json_setting(
            db,
            MEETING_MAX_DURATION_MINUTES_KEY,
            &normalized.meeting_max_duration_minutes,
        )?;
        write_json_setting(
            db,
            MEETING_SMART_START_ENABLED_KEY,
            &normalized.meeting_smart_start_enabled,
        )?;
        write_json_setting(
            db,
            MEETING_SMART_STOP_ENABLED_KEY,
            &normalized.meeting_smart_stop_enabled,
        )?;
        write_json_setting(
            db,
            MEETING_AUDIO_RETENTION_KEY,
            &normalized.meeting_audio_retention,
        )?;
        write_json_setting(
            db,
            MEETING_TRANSCRIPTION_LANGUAGE_KEY,
            &normalized.meeting_transcription_language,
        )?;
        write_json_setting(
            db,
            DICTATION_POLISH_ENABLED_KEY,
            &normalized.dictation_polish_enabled,
        )?;
        write_json_setting(
            db,
            DICTATION_POLISH_TEMPLATE_ID_KEY,
            &normalized.dictation_polish_template_id,
        )?;
        write_json_setting(
            db,
            DICTATION_POLISH_TEMPLATES_KEY,
            &normalized.dictation_polish_templates,
        )?;
        write_json_setting(
            db,
            DEFAULT_SUMMARY_TEMPLATE_ID_KEY,
            &normalized.default_summary_template_id,
        )?;
        write_json_setting(db, SUMMARY_TEMPLATES_KEY, &normalized.summary_templates)?;
        write_json_setting(db, LAST_SEEN_VERSION_KEY, &normalized.last_seen_version)?;

        if let Some(audio_device) = normalized.audio_device.as_ref() {
            write_json_setting(db, AUDIO_DEVICE_KEY, audio_device)?;
        } else {
            db.delete_setting(AUDIO_DEVICE_KEY)?;
        }

        if let Some(clamshell_audio_device) = normalized.clamshell_audio_device.as_ref() {
            write_json_setting(db, CLAMSHELL_AUDIO_DEVICE_KEY, clamshell_audio_device)?;
        } else {
            db.delete_setting(CLAMSHELL_AUDIO_DEVICE_KEY)?;
        }

        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, specta::Type)]
pub struct ShortcutSettings {
    pub toggle: String,
    pub push_to_talk: String,
}

impl Default for ShortcutSettings {
    fn default() -> Self {
        Self {
            toggle: crate::DEFAULT_TOGGLE_SHORTCUT.to_string(),
            push_to_talk: String::new(),
        }
    }
}

impl ShortcutSettings {
    pub fn load(db: &Database) -> Result<Self, String> {
        let mut shortcuts = Self::default();

        if let Some(toggle) = read_json_setting::<String>(db, SHORTCUT_TOGGLE_KEY)? {
            shortcuts.toggle = toggle;
        }
        if let Some(push_to_talk) = read_json_setting::<String>(db, SHORTCUT_PUSH_TO_TALK_KEY)? {
            shortcuts.push_to_talk = push_to_talk;
        }

        Ok(shortcuts.sanitized())
    }

    pub fn normalize(&self) -> Result<Self, String> {
        let normalized = Self {
            toggle: self.toggle.trim().to_string(),
            push_to_talk: self.push_to_talk.trim().to_string(),
        };

        if !normalized.toggle.is_empty() && normalized.toggle == normalized.push_to_talk {
            return Err("Toggle and push-to-talk shortcuts must be different".into());
        }

        Ok(normalized)
    }

    fn sanitized(&self) -> Self {
        let mut normalized = Self {
            toggle: self.toggle.trim().to_string(),
            push_to_talk: self.push_to_talk.trim().to_string(),
        };

        if !normalized.toggle.is_empty() && normalized.toggle == normalized.push_to_talk {
            normalized.push_to_talk.clear();
        }

        normalized
    }

    pub fn save(&self, db: &Database) -> Result<(), String> {
        let normalized = self.normalize()?;
        write_json_setting(db, SHORTCUT_TOGGLE_KEY, &normalized.toggle)?;
        write_json_setting(db, SHORTCUT_PUSH_TO_TALK_KEY, &normalized.push_to_talk)?;
        Ok(())
    }
}

fn read_json_setting<T>(db: &Database, key: &str) -> Result<Option<T>, String>
where
    T: DeserializeOwned,
{
    match db.get_setting(key)? {
        Some(raw_value) => serde_json::from_str(&raw_value)
            .map(Some)
            .map_err(|e| format!("Parse setting '{key}': {e}")),
        None => Ok(None),
    }
}

fn write_json_setting<T>(db: &Database, key: &str, value: &T) -> Result<(), String>
where
    T: Serialize,
{
    let encoded =
        serde_json::to_string(value).map_err(|e| format!("Serialize setting '{key}': {e}"))?;
    db.set_setting(key, &encoded)
}

#[cfg(test)]
mod tests {
    use super::{AppSettings, MeetingAudioRetention, PasteMethod, ShortcutSettings, Theme};
    use crate::logging::LogLevel;
    use crate::constants::OLLAMA_DEFAULT_URL;
    use crate::test_helpers::fixtures::test_db;

    #[test]
    fn app_settings_round_trip() {
        let (db, _dir) = test_db();
        let settings = AppSettings {
            theme: Theme::Light,
            locale: "fr".into(),
            auto_paste: true,
            paste_delay_ms: 250,
            paste_method: PasteMethod::Type,
            ollama_url: "http://example.test:11434".into(),
            ollama_model: "qwen2.5".into(),
            debug_transcription: true,
            log_level: LogLevel::Debug,
            audio_device: Some("BlackHole".into()),
            clamshell_audio_device: Some("USB Mic".into()),
            transcription_engine_id: "kyutai".into(),
            transcription_model_id: "stt-1b-en_fr".into(),
            transcription_backend_id: "candle".into(),
            vad_enabled: true,
            filler_removal: true,
            stutter_collapse: false,
            dictionary_correction: true,
            capture_system_audio: true,
            calendar_integration_enabled: true,
            calendar_selected_ids: vec!["cal-1".into(), "cal-2".into()],
            calendar_reminder_minutes: 5,
            calendar_autostart_enabled: true,
            feedback_sounds_enabled: false,
            feedback_sounds_volume: 40,
            model_unload_timeout_minutes: 15,
            meeting_autostop_enabled: false,
            meeting_autostop_minutes: 15,
            meeting_max_duration_minutes: 120,
            meeting_smart_start_enabled: true,
            meeting_smart_stop_enabled: false,
            meeting_audio_retention: MeetingAudioRetention::Keep30d,
            meeting_transcription_language: super::MeetingTranscriptionLanguage::Fr,
            dictation_polish_enabled: true,
            dictation_polish_template_id: "email".into(),
            dictation_polish_templates: crate::summary::default_polish_templates(),
            default_summary_template_id: crate::summary::TEMPLATE_SUMMARY_BRIEF.into(),
            summary_templates: crate::summary::default_summary_templates(),
            last_seen_version: "0.0.9".into(),
        };

        settings.save(&db).expect("save settings");

        assert_eq!(AppSettings::load(&db).expect("load settings"), settings);
    }

    #[test]
    fn blank_audio_device_is_removed_on_save() {
        let (db, _dir) = test_db();
        let settings = AppSettings {
            audio_device: Some("   ".into()),
            ..AppSettings::default()
        };

        settings.save(&db).expect("save settings");

        assert_eq!(db.get_setting("audio_device").expect("get setting"), None);
    }

    #[test]
    fn blank_clamshell_audio_device_is_removed_on_save() {
        let (db, _dir) = test_db();
        let settings = AppSettings {
            clamshell_audio_device: Some("   ".into()),
            ..AppSettings::default()
        };

        settings.save(&db).expect("save settings");

        assert_eq!(
            db.get_setting("clamshell_audio_device").expect("get setting"),
            None
        );
    }

    #[test]
    fn shortcut_settings_reject_duplicate_bindings() {
        let shortcuts = ShortcutSettings {
            toggle: "CommandOrControl+Shift+Space".into(),
            push_to_talk: "CommandOrControl+Shift+Space".into(),
        };

        assert!(shortcuts.normalize().is_err());
    }

    #[test]
    fn missing_settings_use_defaults() {
        let (db, _dir) = test_db();
        let settings = AppSettings::load(&db).expect("load defaults");
        let shortcuts = ShortcutSettings::load(&db).expect("load shortcuts");

        assert_eq!(settings, AppSettings::default());
        assert_eq!(shortcuts, ShortcutSettings::default());
    }

    #[test]
    fn sanitize_rejects_empty_ollama_url() {
        let s = AppSettings {
            ollama_url: "".to_string(),
            ..AppSettings::default()
        };
        assert!(s.sanitize_for_save().is_err());
    }

    #[test]
    fn sanitize_rejects_out_of_range_paste_delay() {
        let mut s = AppSettings {
            paste_delay_ms: 49,
            ..AppSettings::default()
        };
        assert!(s.sanitize_for_save().is_err());
        s.paste_delay_ms = 1001;
        assert!(s.sanitize_for_save().is_err());
    }

    #[test]
    fn sanitize_falls_back_to_default_for_unknown_engine() {
        // sanitized() silently corrects unknown engines to the Kyutai default
        let s = AppSettings {
            transcription_engine_id: "nonexistent".to_string(),
            ..AppSettings::default()
        };
        let clean = s.sanitize_for_save().unwrap();
        assert_eq!(clean.transcription_engine_id, "kyutai");
        assert_eq!(clean.transcription_model_id, "stt-1b-en_fr");
        assert_eq!(clean.transcription_backend_id, "candle");
    }

    #[test]
    fn sanitize_accepts_whisper_engine() {
        let s = AppSettings {
            transcription_engine_id: "whisper".to_string(),
            transcription_model_id: "turbo".to_string(),
            transcription_backend_id: "whisper-rs".to_string(),
            ..AppSettings::default()
        };
        let clean = s.sanitize_for_save().unwrap();
        assert_eq!(clean.transcription_engine_id, "whisper");
        assert_eq!(clean.transcription_model_id, "turbo");
        assert_eq!(clean.transcription_backend_id, "whisper-rs");
    }

    #[test]
    fn sanitize_falls_back_to_default_for_unavailable_stub_engine() {
        let s = AppSettings {
            transcription_engine_id: "whisper".to_string(),
            transcription_model_id: "turbo".to_string(),
            transcription_backend_id: "ctranslate2".to_string(),
            ..AppSettings::default()
        };
        let clean = s.sanitize_for_save().unwrap();
        assert_eq!(clean.transcription_engine_id, "kyutai");
        assert_eq!(clean.transcription_model_id, "stt-1b-en_fr");
        assert_eq!(clean.transcription_backend_id, "candle");
    }

    #[test]
    fn calendar_reminder_minutes_out_of_range_falls_back_to_default() {
        let (db, _dir) = test_db();
        db.set_setting("calendar_reminder_minutes", "0")
            .expect("save minutes");
        let settings = AppSettings::load(&db).expect("load settings");
        assert_eq!(settings.calendar_reminder_minutes, 2);

        db.set_setting("calendar_reminder_minutes", "31")
            .expect("save minutes");
        let settings = AppSettings::load(&db).expect("load settings");
        assert_eq!(settings.calendar_reminder_minutes, 2);
    }

    #[test]
    fn model_unload_timeout_minutes_rejects_unlisted_values() {
        let (db, _dir) = test_db();
        db.set_setting("model_unload_timeout_minutes", "7")
            .expect("save minutes");
        let settings = AppSettings::load(&db).expect("load settings");
        assert_eq!(settings.model_unload_timeout_minutes, 0);

        for minutes in [0, 5, 15, 60] {
            let s = AppSettings {
                model_unload_timeout_minutes: minutes,
                ..AppSettings::default()
            };
            let clean = s.sanitize_for_save().unwrap();
            assert_eq!(clean.model_unload_timeout_minutes, minutes);
        }
    }

    #[test]
    fn meeting_autostop_minutes_out_of_range_falls_back_to_default() {
        let (db, _dir) = test_db();
        db.set_setting("meeting_autostop_minutes", "2")
            .expect("save minutes");
        let settings = AppSettings::load(&db).expect("load settings");
        assert_eq!(settings.meeting_autostop_minutes, 10);

        db.set_setting("meeting_autostop_minutes", "61")
            .expect("save minutes");
        let settings = AppSettings::load(&db).expect("load settings");
        assert_eq!(settings.meeting_autostop_minutes, 10);

        for minutes in [3, 30, 60] {
            let s = AppSettings {
                meeting_autostop_minutes: minutes,
                ..AppSettings::default()
            };
            let clean = s.sanitize_for_save().unwrap();
            assert_eq!(clean.meeting_autostop_minutes, minutes);
        }
    }

    #[test]
    fn meeting_max_duration_minutes_out_of_range_falls_back_to_default() {
        let (db, _dir) = test_db();
        db.set_setting("meeting_max_duration_minutes", "59")
            .expect("save minutes");
        let settings = AppSettings::load(&db).expect("load settings");
        assert_eq!(settings.meeting_max_duration_minutes, 240);

        db.set_setting("meeting_max_duration_minutes", "721")
            .expect("save minutes");
        let settings = AppSettings::load(&db).expect("load settings");
        assert_eq!(settings.meeting_max_duration_minutes, 240);

        for minutes in [60, 240, 480, 720] {
            let s = AppSettings {
                meeting_max_duration_minutes: minutes,
                ..AppSettings::default()
            };
            let clean = s.sanitize_for_save().unwrap();
            assert_eq!(clean.meeting_max_duration_minutes, minutes);
        }
    }

    #[test]
    fn meeting_autostop_enabled_defaults_true() {
        assert!(AppSettings::default().meeting_autostop_enabled);
    }

    #[test]
    fn calendar_selected_ids_are_trimmed_and_deduped() {
        let s = AppSettings {
            calendar_selected_ids: vec![
                " cal-1 ".into(),
                "cal-1".into(),
                "  ".into(),
                "cal-2".into(),
            ],
            ..AppSettings::default()
        };
        let clean = s.sanitize_for_save().unwrap();
        assert_eq!(clean.calendar_selected_ids, vec!["cal-1", "cal-2"]);
    }

    #[test]
    fn sanitize_normalizes_whitespace() {
        let s = AppSettings {
            ollama_url: "  http://localhost:11434  ".to_string(),
            ..AppSettings::default()
        };
        let clean = s.sanitize_for_save().unwrap();
        assert_eq!(clean.ollama_url, "http://localhost:11434");
    }

    #[test]
    fn shortcut_empty_both_valid() {
        let s = ShortcutSettings {
            toggle: String::new(),
            push_to_talk: String::new(),
        };
        assert!(s.normalize().is_ok());
    }

    #[test]
    fn shortcut_save_round_trip() {
        let (db, _dir) = test_db();
        let s = ShortcutSettings {
            toggle: "CommandOrControl+Shift+Space".to_string(),
            push_to_talk: "CommandOrControl+Shift+S".to_string(),
        };
        s.save(&db).unwrap();
        let loaded = ShortcutSettings::load(&db).unwrap();
        assert_eq!(s, loaded);
    }

    #[test]
    fn dictation_polish_settings_round_trip() {
        let (db, _dir) = test_db();
        let settings = AppSettings {
            dictation_polish_enabled: true,
            dictation_polish_template_id: "bullets".into(),
            dictation_polish_templates: vec![super::DictationPolishTemplate {
                id: "email".into(),
                label: "Email".into(),
                prompt: "Custom email prompt".into(),
            }],
            ..AppSettings::default()
        };

        settings.save(&db).expect("save settings");
        let loaded = AppSettings::load(&db).expect("load settings");

        assert!(loaded.dictation_polish_enabled);
        assert_eq!(loaded.dictation_polish_template_id, "bullets");
        assert_eq!(loaded.dictation_polish_templates.len(), 3);
        assert_eq!(
            loaded
                .dictation_polish_templates
                .iter()
                .find(|template| template.id == "email")
                .map(|template| template.prompt.as_str()),
            Some("Custom email prompt")
        );
    }

    #[test]
    fn summary_template_settings_round_trip_keeps_custom_templates() {
        let (db, _dir) = test_db();
        let settings = AppSettings {
            default_summary_template_id: "custom-1".into(),
            summary_templates: vec![
                super::SummaryTemplate {
                    id: crate::summary::TEMPLATE_SUMMARY_DEFAULT.into(),
                    name: "Default".into(),
                    prompt: "Edited built-in prompt".into(),
                },
                super::SummaryTemplate {
                    id: "custom-1".into(),
                    name: "My format".into(),
                    prompt: "Write it my way.".into(),
                },
            ],
            ..AppSettings::default()
        };

        settings.save(&db).expect("save settings");
        let loaded = AppSettings::load(&db).expect("load settings");

        assert_eq!(loaded.default_summary_template_id, "custom-1");
        // All built-ins are re-added, edited built-in keeps the user prompt,
        // and the custom template survives after the built-ins.
        assert_eq!(
            loaded.summary_templates.len(),
            crate::summary::default_summary_templates().len() + 1
        );
        assert_eq!(
            loaded
                .summary_templates
                .iter()
                .find(|t| t.id == crate::summary::TEMPLATE_SUMMARY_DEFAULT)
                .map(|t| t.prompt.as_str()),
            Some("Edited built-in prompt")
        );
        assert_eq!(
            loaded
                .summary_templates
                .iter()
                .find(|t| t.id == "custom-1")
                .map(|t| t.name.as_str()),
            Some("My format")
        );
    }

    #[test]
    fn unknown_default_summary_template_falls_back_to_first() {
        let settings = AppSettings {
            default_summary_template_id: "deleted-id".into(),
            ..AppSettings::default()
        };
        let clean = settings.sanitize_for_save().expect("sanitize");
        assert_eq!(
            clean.default_summary_template_id,
            crate::summary::TEMPLATE_SUMMARY_DEFAULT
        );
    }

    #[test]
    fn invalid_persisted_values_fall_back_to_defaults() {
        let (db, _dir) = test_db();
        db.set_setting("paste_delay_ms", "0").expect("save delay");
        db.set_setting("ollama_url", "\"   \"").expect("save url");
        db.set_setting("shortcut_toggle", "\"F6\"")
            .expect("save toggle");
        db.set_setting("shortcut_push_to_talk", "\"F6\"")
            .expect("save ptt");

        let settings = AppSettings::load(&db).expect("load settings");
        let shortcuts = ShortcutSettings::load(&db).expect("load shortcuts");

        assert_eq!(
            settings.paste_delay_ms,
            AppSettings::default().paste_delay_ms
        );
        assert_eq!(settings.ollama_url, OLLAMA_DEFAULT_URL);
        assert_eq!(shortcuts.toggle, "F6");
        assert_eq!(shortcuts.push_to_talk, "");
    }

    /// `Keep7d`/`Keep30d` pin an explicit `#[serde(rename)]` because serde's
    /// `snake_case` heuristic and specta's TS-type heuristic disagree at
    /// letter/digit boundaries (serde alone would emit "keep7d", specta
    /// would type it as "keep_7d") — this guards the wire value the
    /// frontend actually depends on against a future regression.
    #[test]
    fn meeting_audio_retention_wire_format_is_stable() {
        assert_eq!(
            serde_json::to_string(&MeetingAudioRetention::Off).unwrap(),
            "\"off\""
        );
        assert_eq!(
            serde_json::to_string(&MeetingAudioRetention::Keep7d).unwrap(),
            "\"keep_7d\""
        );
        assert_eq!(
            serde_json::to_string(&MeetingAudioRetention::Keep30d).unwrap(),
            "\"keep_30d\""
        );
        assert_eq!(
            serde_json::to_string(&MeetingAudioRetention::KeepForever).unwrap(),
            "\"keep_forever\""
        );
    }
}

