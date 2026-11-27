use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::audio::device::migrate_stored_device_id;
use crate::audio::{AudioInputDevice, InputPriority};
use crate::constants::OLLAMA_DEFAULT_URL;
use crate::db::Database;
use crate::engine::{
    CANDLE_BACKEND_ID, KYUTAI_ENGINE_ID, KYUTAI_MODEL_ID, TranscriptionProfileSelection,
    resolve_transcription_profile,
};
use crate::summary::SummaryProviderChoice;

const THEME_KEY: &str = "theme";
const WINDOW_WIDTH_KEY: &str = "window_width";
const WINDOW_HEIGHT_KEY: &str = "window_height";
const AUTO_PASTE_KEY: &str = "auto_paste";
const PASTE_DELAY_MS_KEY: &str = "paste_delay_ms";
const OLLAMA_URL_KEY: &str = "ollama_url";
const OLLAMA_MODEL_KEY: &str = "ollama_model";
const SUMMARY_PROVIDER_KEY: &str = "summary_provider";
const DETAILED_LOGGING_KEY: &str = "detailed_logging";
// Read once by `migrate_pinned_device`, then deleted.
const PINNED_DEVICE_MIGRATION_KEY: &str = "audio_device";
const CLAMSHELL_AUDIO_DEVICE_KEY: &str = "clamshell_audio_device";
const INPUT_PRIORITY_KEY: &str = "input_priority";
const TRANSCRIPTION_ENGINE_ID_KEY: &str = "transcription_engine_id";
const TRANSCRIPTION_MODEL_ID_KEY: &str = "transcription_model_id";
const TRANSCRIPTION_BACKEND_ID_KEY: &str = "transcription_backend_id";
const FILLER_REMOVAL_KEY: &str = "filler_removal";
const DICTIONARY_CORRECTION_KEY: &str = "dictionary_correction";
const CAPTURE_SYSTEM_AUDIO_KEY: &str = "capture_system_audio";
const CALENDAR_INTEGRATION_ENABLED_KEY: &str = "calendar_integration_enabled";
const CALENDAR_SELECTED_IDS_KEY: &str = "calendar_selected_ids";
const CALENDAR_REMINDER_MINUTES_KEY: &str = "calendar_reminder_minutes";
const FEEDBACK_SOUNDS_ENABLED_KEY: &str = "feedback_sounds_enabled";
const PILL_HIDDEN_KEY: &str = "pill_hidden";
const MODEL_UNLOAD_TIMEOUT_MINUTES_KEY: &str = "model_unload_timeout_minutes";
const AUTO_UPDATE_CHECK_ENABLED_KEY: &str = "auto_update_check_enabled";
/// Unix seconds of the last completed check. Not part of `AppSettings`: it is
/// bookkeeping, not a preference, and nothing in the UI shows it.
pub const LAST_UPDATE_CHECK_AT_KEY: &str = "last_update_check_at";
/// Last user-dragged pill origin (JSON `[x, y]` in AppKit points). Not a
/// preference: the overlay re-clamps it when the screen changes.
pub const PILL_POSITION_KEY: &str = "pill_position";
const MEETING_AUTOSTOP_ENABLED_KEY: &str = "meeting_autostop_enabled";
const AUTOSTART_ENABLED_KEY: &str = "autostart_enabled";
const LOCALE_KEY: &str = "locale";
const SHORTCUT_TOGGLE_KEY: &str = "shortcut_toggle";
const SHORTCUT_PUSH_TO_TALK_KEY: &str = "shortcut_push_to_talk";
const DICTATION_POLISH_ENABLED_KEY: &str = "dictation_polish_enabled";
const DICTATION_POLISH_TEMPLATE_ID_KEY: &str = "dictation_polish_template_id";
const DICTATION_POLISH_TEMPLATES_KEY: &str = "dictation_polish_templates";
const DICTATION_APP_BRANCHES_KEY: &str = "dictation_app_branches";
const DEFAULT_SUMMARY_TEMPLATE_ID_KEY: &str = "default_summary_template_id";
const SUMMARY_TEMPLATES_KEY: &str = "summary_templates";
const PASTE_METHOD_KEY: &str = "paste_method";
const LAST_SEEN_VERSION_KEY: &str = "last_seen_version";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PreparedSettingsSaveError {
    ModelChangeRejected,
    Persistence { message: String },
}

impl From<String> for PreparedSettingsSaveError {
    fn from(message: String) -> Self {
        Self::Persistence { message }
    }
}
const DICTATION_LEARN_FROM_EDIT_KEY: &str = "dictation_learn_from_edit";
const DICTIONARY_LEARNING_MODE_KEY: &str = "dictionary_learning_mode";
const MEETING_AUDIO_RETENTION_KEY: &str = "meeting_audio_retention";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum PasteMethod {
    #[default]
    Clipboard,
    Type,
    /// Set the focused element's selected text via Accessibility.
    Ax,
}

/// Persistence policy for admissible recognition corrections.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum DictionaryLearningMode {
    Disabled,
    Suggestions,
    #[default]
    Automatic,
}

impl DictionaryLearningMode {
    pub const ALL: [Self; 3] = [Self::Disabled, Self::Suggestions, Self::Automatic];
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DictationPolishTemplate {
    pub id: String,
    pub label: String,
    pub prompt: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(transparent)]
pub struct AppBranchId(pub String);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(transparent)]
pub struct PolishTemplateId(pub String);

/// Global follows the current global selection, including later changes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", content = "template_id", rename_all = "snake_case")]
pub enum AppBranchTarget {
    Global,
    Template(PolishTemplateId),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AppBranchRule {
    pub id: AppBranchId,
    pub app_pattern: String,
    pub enabled: bool,
    pub target: AppBranchTarget,
}

impl AppBranchRule {
    pub fn new(app_pattern: String, target: AppBranchTarget) -> Self {
        Self {
            id: AppBranchId(uuid::Uuid::new_v4().to_string()),
            app_pattern: app_pattern.trim().to_string(),
            enabled: false,
            target,
        }
    }
}

/// Offered only when the setting does not yet exist. An explicitly empty
/// stored list means the user deleted the suggestions and stays empty.
pub fn default_app_branches() -> Vec<AppBranchRule> {
    [
        ("seed-slack", "Slack", crate::summary::TEMPLATE_CHAT),
        ("seed-mail", "Mail", crate::summary::TEMPLATE_EMAIL),
        (
            "seed-code",
            "Visual Studio Code",
            crate::summary::TEMPLATE_CODE,
        ),
        ("seed-xcode", "Xcode", crate::summary::TEMPLATE_CODE),
    ]
    .into_iter()
    .map(|(id, pattern, template)| AppBranchRule {
        id: AppBranchId(id.into()),
        app_pattern: pattern.into(),
        enabled: false,
        target: AppBranchTarget::Template(PolishTemplateId(template.into())),
    })
    .collect()
}

/// A meeting-summary template: `prompt` replaces the final-pass system
/// prompt only (map/merge prompts stay fixed). Built-ins ship with
/// well-known ids and are non-deletable; the user can also add their own.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SummaryTemplate {
    pub id: String,
    pub name: String,
    pub prompt: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    #[default]
    Dark,
    Light,
    System,
}

/// How long recorded meeting audio is kept on disk before the startup sweep
/// deletes it. Opt-in: recording itself only happens when this is not `Off`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AppSettings {
    pub window_width: u32,
    pub window_height: u32,
    pub theme: Theme,
    pub locale: String,
    pub auto_paste: bool,
    pub paste_delay_ms: u64,
    /// How dictation text is inserted: clipboard Cmd+V, simulated keystrokes,
    /// or the focused AX field.
    pub paste_method: PasteMethod,
    pub ollama_url: String,
    /// Which provider runs summaries and dictation polish. `ollama_model` only
    /// applies when this resolves to Ollama.
    pub summary_provider: SummaryProviderChoice,
    pub ollama_model: String,
    /// Opt-in for experimental meeting summarization and compaction in the app.
    /// Dictation polish and explicit CLI/read-only MCP access stay independent.
    #[serde(default)]
    pub meeting_summaries_enabled: bool,
    #[serde(default)]
    pub summary_compression_enabled: bool,
    /// Debug level logging, transcript lines in the log, and the short
    /// engine input capture that lives for the length of a session.
    pub detailed_logging: bool,
    /// Preferred microphone UID while the lid is closed with an external display
    /// attached (clamshell mode). `None` means just follow whatever macOS
    /// reports as the default input, the previous behavior.
    pub clamshell_audio_device: Option<String>,
    /// Ordered input-device preferences and remembered devices.
    pub input_priority: InputPriority,
    pub transcription_engine_id: String,
    pub transcription_model_id: String,
    pub transcription_backend_id: String,
    pub filler_removal: bool,
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
    /// Ask GitHub once a day whether a newer release exists, and show a
    /// dialog when there is one. Never downloads or installs anything.
    pub auto_update_check_enabled: bool,
    /// Audible start/stop cues for dictation sessions.
    pub feedback_sounds_enabled: bool,
    /// When true, the floating recording HUD is not shown.
    pub pill_hidden: bool,
    /// Unload the transcription model after this many idle minutes to
    /// reclaim RAM; 0 means never unload. The next recording reloads it
    /// through the normal load flow.
    pub model_unload_timeout_minutes: u32,
    /// Detect a meeting that has probably ended (no speech for a while, or
    /// the max-duration failsafe) and offer/auto-stop. A session snapshots
    /// this setting at start; a change applies from the next meeting.
    pub meeting_autostop_enabled: bool,
    /// Launch Soufflé at login (SMAppService login item, SOU-036). The stored
    /// value is a fallback only: `get_settings` overwrites it with the state
    /// the system reports, and an absent key means "never asked", not "on".
    pub autostart_enabled: bool,
    /// Opt-in recording of meeting audio to compressed files on disk, and
    /// for how long they're kept. Off by default.
    pub meeting_audio_retention: MeetingAudioRetention,
    /// Optional LLM post-processing applied to dictation before paste/history.
    pub dictation_polish_enabled: bool,
    /// Active polish template id (clean, email, bullets, no_fillers).
    pub dictation_polish_template_id: String,
    /// User-editable polish prompt templates.
    pub dictation_polish_templates: Vec<DictationPolishTemplate>,
    /// How recognition corrections enter the persistent dictionary.
    pub dictionary_learning_mode: DictionaryLearningMode,
    /// Ordered app rules. Their position is their persisted priority.
    #[serde(default = "default_app_branches")]
    pub dictation_app_branches: Vec<AppBranchRule>,
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

const CALENDAR_REMINDER_MINUTES_RANGE: std::ops::RangeInclusive<u32> = 1..=30;

pub const MIN_PASTE_DELAY_MS: u64 = 50;
pub const MAX_PASTE_DELAY_MS: u64 = 1000;

/// The numeric choices the settings UI may offer, served from the same file
/// that validates them. `sanitize_for_save` alone decides what is acceptable;
/// this is how the UI finds out instead of restating it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SettingsOptions {
    pub model_unload_timeout_minutes: Vec<u32>,
    pub paste_delay_ms_min: u64,
    pub paste_delay_ms_max: u64,
    pub calendar_reminder_minutes_min: u32,
    pub calendar_reminder_minutes_max: u32,
    pub default_shortcuts: ShortcutSettings,
}

impl SettingsOptions {
    /// Return the settings constraints and defaults exposed through the IPC contract.
    pub fn current() -> Self {
        Self {
            model_unload_timeout_minutes: ALLOWED_UNLOAD_TIMEOUT_MINUTES.to_vec(),
            paste_delay_ms_min: MIN_PASTE_DELAY_MS,
            paste_delay_ms_max: MAX_PASTE_DELAY_MS,
            calendar_reminder_minutes_min: *CALENDAR_REMINDER_MINUTES_RANGE.start(),
            calendar_reminder_minutes_max: *CALENDAR_REMINDER_MINUTES_RANGE.end(),
            default_shortcuts: ShortcutSettings::default(),
        }
    }
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            // SOU-242: bumped from the original Tauri window (860x1040).
            // Onboarding/permissions and the home view rendered visibly
            // cramped at the old default on a fresh install (no saved
            // window_width/window_height row yet - see
            // `main.rs::main()`'s `initial_settings` read). Keep every
            // occurrence of this pair in sync; the test literals in this
            // file were already redundant with `..AppSettings::default()`
            // and were removed instead of updated (SOU-242).
            window_width: 1040,
            window_height: 1280,
            theme: Theme::Light,
            locale: String::new(),
            auto_paste: false,
            paste_delay_ms: 100,
            paste_method: PasteMethod::default(),
            ollama_url: OLLAMA_DEFAULT_URL.to_string(),
            summary_provider: SummaryProviderChoice::default(),
            ollama_model: String::new(),
            meeting_summaries_enabled: false,
            summary_compression_enabled: false,
            detailed_logging: false,
            clamshell_audio_device: None,
            input_priority: InputPriority::default(),
            transcription_engine_id: KYUTAI_ENGINE_ID.to_string(),
            transcription_model_id: KYUTAI_MODEL_ID.to_string(),
            transcription_backend_id: CANDLE_BACKEND_ID.to_string(),
            filler_removal: true,
            dictionary_correction: true,
            capture_system_audio: true,
            calendar_integration_enabled: false,
            calendar_selected_ids: Vec::new(),
            calendar_reminder_minutes: 2,
            auto_update_check_enabled: true,
            feedback_sounds_enabled: true,
            pill_hidden: false,
            model_unload_timeout_minutes: 60,
            meeting_autostop_enabled: true,
            autostart_enabled: false,
            meeting_audio_retention: MeetingAudioRetention::default(),
            dictation_polish_enabled: true,
            dictation_polish_template_id: crate::summary::TEMPLATE_CLEAN.to_string(),
            dictation_polish_templates: crate::summary::default_polish_templates(),
            dictionary_learning_mode: DictionaryLearningMode::Automatic,
            dictation_app_branches: default_app_branches(),
            default_summary_template_id: crate::summary::TEMPLATE_SUMMARY_DEFAULT.to_string(),
            summary_templates: crate::summary::default_summary_templates(),
            last_seen_version: String::new(),
        }
    }
}

impl AppSettings {
    /// `vad_model_path` is the Silero model the session gates audio with;
    /// `None` runs the session without a voice gate.
    pub fn pipeline_config(
        &self,
        vad_model_path: Option<std::path::PathBuf>,
    ) -> crate::filter::PipelineConfig {
        crate::filter::PipelineConfig {
            vad_model_path,
            filler_removal_enabled: self.filler_removal,
            dictionary_correction_enabled: self.dictionary_correction,
        }
    }

    pub fn load(db: &Database) -> Result<Self, String> {
        let mut settings = Self::load_read_only(db)?;
        Self::migrate_audio_device_preferences(&mut settings, db)?;
        Ok(settings)
    }

    /// Read and normalize the persisted snapshot without running migrations or
    /// writing any key. Save preflight and post-save observation must use this
    /// path so a rejected or failed save cannot mutate the database indirectly.
    pub fn load_read_only(db: &Database) -> Result<Self, String> {
        let mut settings = Self::default();
        if let Some(w) = read_json_setting::<u32>(db, WINDOW_WIDTH_KEY)? {
            settings.window_width = w;
        }
        if let Some(h) = read_json_setting::<u32>(db, WINDOW_HEIGHT_KEY)? {
            settings.window_height = h;
        }

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
        let stored_provider = read_json_setting::<SummaryProviderChoice>(db, SUMMARY_PROVIDER_KEY)?;
        settings.meeting_summaries_enabled =
            read_json_setting(db, "meeting_summaries_enabled")?.unwrap_or(false);
        settings.summary_compression_enabled =
            read_json_setting(db, "summary_compression_enabled")?.unwrap_or(false);
        settings.summary_provider =
            SummaryProviderChoice::from_stored(stored_provider, &settings.ollama_model);
        if let Some(detailed_logging) = read_json_setting::<bool>(db, DETAILED_LOGGING_KEY)? {
            settings.detailed_logging = detailed_logging;
        }
        if let Some(clamshell_audio_device) =
            read_json_setting::<String>(db, CLAMSHELL_AUDIO_DEVICE_KEY)?
        {
            settings.clamshell_audio_device = Some(clamshell_audio_device);
        }
        if let Some(input_priority) = read_json_setting::<InputPriority>(db, INPUT_PRIORITY_KEY)? {
            settings.input_priority = input_priority;
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
        if let Some(filler_removal) = read_json_setting::<bool>(db, FILLER_REMOVAL_KEY)? {
            settings.filler_removal = filler_removal;
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
        if let Some(feedback_sounds_enabled) =
            read_json_setting::<bool>(db, FEEDBACK_SOUNDS_ENABLED_KEY)?
        {
            settings.feedback_sounds_enabled = feedback_sounds_enabled;
        }
        if let Some(pill_hidden) = read_json_setting::<bool>(db, PILL_HIDDEN_KEY)? {
            settings.pill_hidden = pill_hidden;
        }
        if let Some(auto_update_check_enabled) =
            read_json_setting::<bool>(db, AUTO_UPDATE_CHECK_ENABLED_KEY)?
        {
            settings.auto_update_check_enabled = auto_update_check_enabled;
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
        if let Some(autostart_enabled) = read_json_setting::<bool>(db, AUTOSTART_ENABLED_KEY)? {
            settings.autostart_enabled = autostart_enabled;
        }
        if let Some(meeting_audio_retention) =
            read_json_setting::<MeetingAudioRetention>(db, MEETING_AUDIO_RETENTION_KEY)?
        {
            settings.meeting_audio_retention = meeting_audio_retention;
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
        settings.dictionary_learning_mode =
            match read_json_setting::<DictionaryLearningMode>(db, DICTIONARY_LEARNING_MODE_KEY)? {
                Some(mode) => mode,
                None => match read_json_setting::<bool>(db, DICTATION_LEARN_FROM_EDIT_KEY)? {
                    Some(false) => DictionaryLearningMode::Disabled,
                    Some(true) | None => DictionaryLearningMode::Automatic,
                },
            };
        if let Some(rules) =
            read_json_setting::<Vec<AppBranchRule>>(db, DICTATION_APP_BRANCHES_KEY)?
        {
            settings.dictation_app_branches = rules;
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

    fn migrate_audio_device_preferences(
        settings: &mut AppSettings,
        db: &Database,
    ) -> Result<(), String> {
        let devices = connected_input_devices();
        Self::migrate_pinned_device(settings, db, &devices)?;
        Self::migrate_clamshell_device_name(settings, db, &devices)
    }

    /// One-shot upgrade from the retired pinned microphone: it becomes the
    /// top of the preference order. The old row is deleted in the same
    /// transaction, so a later load never promotes it again.
    fn migrate_pinned_device(
        settings: &mut AppSettings,
        db: &Database,
        devices: &[AudioInputDevice],
    ) -> Result<(), String> {
        let Some(stored) = read_json_setting::<String>(db, PINNED_DEVICE_MIGRATION_KEY)? else {
            return Ok(());
        };
        let stored = stored.trim();
        if !stored.is_empty() {
            // Old pins can be display names.
            let (uid, _) = migrate_stored_device_id(stored, devices);
            settings.input_priority.promote(&uid);
        }
        let priority = &settings.input_priority;
        db.with_settings_transaction(|transaction| {
            write_json_setting_in_transaction(transaction, INPUT_PRIORITY_KEY, priority)?;
            delete_setting_in_transaction(transaction, PINNED_DEVICE_MIGRATION_KEY)
        })
    }

    /// Best-effort upgrade from a legacy name to a stable CoreAudio UID.
    fn migrate_clamshell_device_name(
        settings: &mut AppSettings,
        db: &Database,
        devices: &[AudioInputDevice],
    ) -> Result<(), String> {
        let Some(stored) = settings.clamshell_audio_device.as_ref() else {
            return Ok(());
        };
        let (migrated, changed) = migrate_stored_device_id(stored, devices);
        if changed {
            write_json_setting(db, CLAMSHELL_AUDIO_DEVICE_KEY, &migrated)?;
            settings.clamshell_audio_device = Some(migrated);
        }
        Ok(())
    }

    /// Normalize settings for persistence and reject values that cannot be corrected safely.
    pub fn sanitize_for_save(&self) -> Result<Self, String> {
        let mut normalized = self.sanitized();

        if self.ollama_url.trim().is_empty() {
            return Err("Ollama URL cannot be empty".into());
        }

        if !(MIN_PASTE_DELAY_MS..=MAX_PASTE_DELAY_MS).contains(&self.paste_delay_ms) {
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

    /// Normalize recoverable settings values, replacing invalid choices with defaults.
    fn sanitized(&self) -> Self {
        let mut normalized = self.clone();
        normalized.locale = normalized.locale.trim().to_string();
        normalized.ollama_url = normalized.ollama_url.trim().to_string();
        normalized.ollama_model = normalized.ollama_model.trim().to_string();
        normalized.transcription_engine_id = normalized.transcription_engine_id.trim().to_string();
        normalized.transcription_model_id = normalized.transcription_model_id.trim().to_string();
        normalized.transcription_backend_id =
            normalized.transcription_backend_id.trim().to_string();
        normalized.clamshell_audio_device = normalized
            .clamshell_audio_device
            .as_ref()
            .map(|device| device.trim().to_string())
            .filter(|device| !device.is_empty());

        normalized.input_priority.sanitize();

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

        if !(MIN_PASTE_DELAY_MS..=MAX_PASTE_DELAY_MS).contains(&normalized.paste_delay_ms) {
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
        if !CALENDAR_REMINDER_MINUTES_RANGE.contains(&normalized.calendar_reminder_minutes) {
            normalized.calendar_reminder_minutes = Self::default().calendar_reminder_minutes;
        }

        if !ALLOWED_UNLOAD_TIMEOUT_MINUTES.contains(&normalized.model_unload_timeout_minutes) {
            normalized.model_unload_timeout_minutes = Self::default().model_unload_timeout_minutes;
        }

        normalized.dictation_polish_template_id =
            normalized.dictation_polish_template_id.trim().to_string();
        if normalized.dictation_polish_templates.is_empty() {
            normalized.dictation_polish_templates = crate::summary::default_polish_templates();
        } else {
            normalized.dictation_polish_templates = crate::summary::merge_polish_templates(
                normalized.dictation_polish_templates.clone(),
            );
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
                .unwrap_or_else(|| crate::summary::TEMPLATE_CLEAN.to_string());
        }
        for template in &mut normalized.dictation_polish_templates {
            template.id = template.id.trim().to_string();
            template.label = template.label.trim().to_string();
            template.prompt = template.prompt.trim().to_string();
        }
        let mut branch_ids = std::collections::HashSet::new();
        normalized.dictation_app_branches.retain_mut(|rule| {
            rule.app_pattern = rule.app_pattern.trim().to_string();
            if rule.app_pattern.is_empty() {
                return false;
            }
            if rule.id.0.trim().is_empty() || !branch_ids.insert(rule.id.0.clone()) {
                rule.id = AppBranchId(uuid::Uuid::new_v4().to_string());
                branch_ids.insert(rule.id.0.clone());
            }
            true
        });

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

    /// Refresh `known` from the current connected-device snapshot and persist
    /// only the input-priority JSON. Used when CoreAudio reports a device
    /// list or default-input change.
    pub fn sync_input_priority_from_devices(db: &Database) -> Result<InputPriority, String> {
        // Run the one-shot migrations before touching the key they write.
        Self::load(db)?;
        Self::touch_known_devices(db, &connected_input_devices())
    }

    // The key is read and written inside one transaction: a preference order
    // saved from Settings meanwhile must not be overwritten by a stale copy.
    fn touch_known_devices(
        db: &Database,
        devices: &[AudioInputDevice],
    ) -> Result<InputPriority, String> {
        let mut synced = InputPriority::default();
        db.with_settings_transaction(|transaction| {
            let mut priority: InputPriority =
                read_json_setting_in_transaction(transaction, INPUT_PRIORITY_KEY)?
                    .unwrap_or_default();
            crate::audio::touch_known(&mut priority, devices);
            priority.sanitize();
            write_json_setting_in_transaction(transaction, INPUT_PRIORITY_KEY, &priority)?;
            synced = priority;
            Ok::<(), String>(())
        })?;
        Ok(synced)
    }

    /// Fully validate and normalize the snapshot, including the current audio
    /// device catalogue, before any database or machine-state lock is held.
    pub(crate) fn prepare_for_save(&self) -> Result<Self, String> {
        let mut normalized = self.sanitize_for_save()?;
        crate::audio::touch_known(&mut normalized.input_priority, &connected_input_devices());
        Ok(normalized)
    }

    pub fn save(&self, db: &Database) -> Result<(), String> {
        self.prepare_for_save()?.save_prepared(db)
    }

    /// Persist one snapshot already returned by `prepare_for_save`.
    pub(crate) fn save_prepared(&self, db: &Database) -> Result<(), String> {
        self.save_prepared_with_recording_guard(db, false)
            .map_err(|error| match error {
                PreparedSettingsSaveError::ModelChangeRejected => {
                    "Unexpected model-change rejection outside recording".into()
                }
                PreparedSettingsSaveError::Persistence { message } => message,
            })
    }

    /// Persist one prepared snapshot while atomically refusing a model change
    /// if the canonical state machine is recording. The durable selection is
    /// read inside the same SQLite transaction as the writes, so another save
    /// cannot open a time-of-check/time-of-use window.
    pub(crate) fn save_prepared_with_recording_guard(
        &self,
        db: &Database,
        is_recording: bool,
    ) -> Result<(), PreparedSettingsSaveError> {
        db.with_settings_transaction(|transaction| {
            if is_recording {
                let stored_selection = read_transcription_selection_in_transaction(transaction)?;
                if self.transcription_selection() != stored_selection {
                    return Err(PreparedSettingsSaveError::ModelChangeRejected);
                }
            }

            let normalized = self;
            write_json_setting_in_transaction(
                transaction,
                WINDOW_WIDTH_KEY,
                &normalized.window_width,
            )?;
            write_json_setting_in_transaction(
                transaction,
                WINDOW_HEIGHT_KEY,
                &normalized.window_height,
            )?;
            write_json_setting_in_transaction(transaction, THEME_KEY, &normalized.theme)?;
            write_json_setting_in_transaction(transaction, LOCALE_KEY, &normalized.locale)?;
            write_json_setting_in_transaction(transaction, AUTO_PASTE_KEY, &normalized.auto_paste)?;
            write_json_setting_in_transaction(
                transaction,
                PASTE_DELAY_MS_KEY,
                &normalized.paste_delay_ms,
            )?;
            write_json_setting_in_transaction(
                transaction,
                PASTE_METHOD_KEY,
                &normalized.paste_method,
            )?;
            write_json_setting_in_transaction(transaction, OLLAMA_URL_KEY, &normalized.ollama_url)?;
            write_json_setting_in_transaction(
                transaction,
                "meeting_summaries_enabled",
                &normalized.meeting_summaries_enabled,
            )?;
            write_json_setting_in_transaction(
                transaction,
                "summary_compression_enabled",
                &normalized.summary_compression_enabled,
            )?;
            write_json_setting_in_transaction(
                transaction,
                SUMMARY_PROVIDER_KEY,
                &normalized.summary_provider,
            )?;
            write_json_setting_in_transaction(
                transaction,
                OLLAMA_MODEL_KEY,
                &normalized.ollama_model,
            )?;
            write_json_setting_in_transaction(
                transaction,
                TRANSCRIPTION_ENGINE_ID_KEY,
                &normalized.transcription_engine_id,
            )?;
            write_json_setting_in_transaction(
                transaction,
                TRANSCRIPTION_MODEL_ID_KEY,
                &normalized.transcription_model_id,
            )?;
            write_json_setting_in_transaction(
                transaction,
                TRANSCRIPTION_BACKEND_ID_KEY,
                &normalized.transcription_backend_id,
            )?;
            write_json_setting_in_transaction(
                transaction,
                DETAILED_LOGGING_KEY,
                &normalized.detailed_logging,
            )?;
            write_json_setting_in_transaction(
                transaction,
                FILLER_REMOVAL_KEY,
                &normalized.filler_removal,
            )?;
            write_json_setting_in_transaction(
                transaction,
                DICTIONARY_CORRECTION_KEY,
                &normalized.dictionary_correction,
            )?;
            write_json_setting_in_transaction(
                transaction,
                CAPTURE_SYSTEM_AUDIO_KEY,
                &normalized.capture_system_audio,
            )?;
            write_json_setting_in_transaction(
                transaction,
                CALENDAR_INTEGRATION_ENABLED_KEY,
                &normalized.calendar_integration_enabled,
            )?;
            write_json_setting_in_transaction(
                transaction,
                CALENDAR_SELECTED_IDS_KEY,
                &normalized.calendar_selected_ids,
            )?;
            write_json_setting_in_transaction(
                transaction,
                CALENDAR_REMINDER_MINUTES_KEY,
                &normalized.calendar_reminder_minutes,
            )?;
            write_json_setting_in_transaction(
                transaction,
                FEEDBACK_SOUNDS_ENABLED_KEY,
                &normalized.feedback_sounds_enabled,
            )?;
            write_json_setting_in_transaction(
                transaction,
                PILL_HIDDEN_KEY,
                &normalized.pill_hidden,
            )?;
            write_json_setting_in_transaction(
                transaction,
                AUTO_UPDATE_CHECK_ENABLED_KEY,
                &normalized.auto_update_check_enabled,
            )?;
            write_json_setting_in_transaction(
                transaction,
                MODEL_UNLOAD_TIMEOUT_MINUTES_KEY,
                &normalized.model_unload_timeout_minutes,
            )?;
            write_json_setting_in_transaction(
                transaction,
                MEETING_AUTOSTOP_ENABLED_KEY,
                &normalized.meeting_autostop_enabled,
            )?;
            write_json_setting_in_transaction(
                transaction,
                AUTOSTART_ENABLED_KEY,
                &normalized.autostart_enabled,
            )?;
            write_json_setting_in_transaction(
                transaction,
                MEETING_AUDIO_RETENTION_KEY,
                &normalized.meeting_audio_retention,
            )?;
            write_json_setting_in_transaction(
                transaction,
                DICTATION_POLISH_ENABLED_KEY,
                &normalized.dictation_polish_enabled,
            )?;
            write_json_setting_in_transaction(
                transaction,
                DICTATION_POLISH_TEMPLATE_ID_KEY,
                &normalized.dictation_polish_template_id,
            )?;
            write_json_setting_in_transaction(
                transaction,
                DICTATION_POLISH_TEMPLATES_KEY,
                &normalized.dictation_polish_templates,
            )?;
            write_json_setting_in_transaction(
                transaction,
                DICTIONARY_LEARNING_MODE_KEY,
                &normalized.dictionary_learning_mode,
            )?;
            write_json_setting_in_transaction(
                transaction,
                DICTATION_APP_BRANCHES_KEY,
                &normalized.dictation_app_branches,
            )?;
            write_json_setting_in_transaction(
                transaction,
                DEFAULT_SUMMARY_TEMPLATE_ID_KEY,
                &normalized.default_summary_template_id,
            )?;
            write_json_setting_in_transaction(
                transaction,
                SUMMARY_TEMPLATES_KEY,
                &normalized.summary_templates,
            )?;
            write_json_setting_in_transaction(
                transaction,
                LAST_SEEN_VERSION_KEY,
                &normalized.last_seen_version,
            )?;

            if let Some(clamshell_audio_device) = normalized.clamshell_audio_device.as_ref() {
                write_json_setting_in_transaction(
                    transaction,
                    CLAMSHELL_AUDIO_DEVICE_KEY,
                    clamshell_audio_device,
                )?;
            } else {
                delete_setting_in_transaction(transaction, CLAMSHELL_AUDIO_DEVICE_KEY)?;
            }

            write_json_setting_in_transaction(
                transaction,
                INPUT_PRIORITY_KEY,
                &normalized.input_priority,
            )?;
            Ok(())
        })
    }

    fn transcription_selection(&self) -> TranscriptionProfileSelection {
        TranscriptionProfileSelection {
            engine_id: self.transcription_engine_id.clone(),
            model_id: self.transcription_model_id.clone(),
            backend_id: self.transcription_backend_id.clone(),
        }
    }
}

fn connected_input_devices() -> Vec<AudioInputDevice> {
    #[cfg(target_os = "macos")]
    {
        crate::audio::device_watch::list_devices()
    }
    #[cfg(not(target_os = "macos"))]
    {
        Vec::new()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
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

        if conflicting_pair(&normalized.toggle, &normalized.push_to_talk) {
            return Err("Dictation shortcuts must be different".into());
        }

        Ok(normalized)
    }

    fn sanitized(&self) -> Self {
        let mut normalized = Self {
            toggle: self.toggle.trim().to_string(),
            push_to_talk: self.push_to_talk.trim().to_string(),
        };

        if normalized.toggle == "Fn" {
            normalized.toggle = crate::DEFAULT_TOGGLE_SHORTCUT.to_string();
        }
        if normalized.push_to_talk == "Fn" {
            normalized.push_to_talk = String::new();
        }

        if conflicting_pair(&normalized.toggle, &normalized.push_to_talk) {
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

/// Whether the UI locale is French, for the native tray menu and pill panel
/// labels. Seeded at bootstrap and refreshed by the settings-save effects,
/// so a state transition can relabel both without re-reading every setting
/// from SQLite (SOU-285).
static UI_LOCALE_FR: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Record the saved locale for [`ui_locale_is_french`].
pub fn remember_ui_locale(locale: &str) {
    UI_LOCALE_FR.store(
        locale_is_french(locale),
        std::sync::atomic::Ordering::Relaxed,
    );
}

/// The locale last passed to [`remember_ui_locale`], as "is it French".
pub fn ui_locale_is_french() -> bool {
    UI_LOCALE_FR.load(std::sync::atomic::Ordering::Relaxed)
}

fn locale_is_french(locale: &str) -> bool {
    locale.starts_with("fr")
}

/// The calendar-integration toggle alone: one key read, no migration. The
/// calendar scheduler checks this before anything else so a disabled
/// integration costs a single SELECT per settings save (SOU-285).
pub fn calendar_integration_enabled(db: &Database) -> Result<bool, String> {
    Ok(
        read_json_setting::<bool>(db, CALENDAR_INTEGRATION_ENABLED_KEY)?
            .unwrap_or(AppSettings::default().calendar_integration_enabled),
    )
}

fn conflicting_pair(left: &str, right: &str) -> bool {
    !left.is_empty() && left == right
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

fn read_json_setting_in_transaction<T>(
    transaction: &rusqlite::Transaction<'_>,
    key: &str,
) -> Result<Option<T>, String>
where
    T: DeserializeOwned,
{
    let result = transaction.query_row(
        "SELECT value FROM settings WHERE key = ?1",
        rusqlite::params![key],
        |row| row.get::<_, String>(0),
    );
    match result {
        Ok(raw_value) => serde_json::from_str(&raw_value)
            .map(Some)
            .map_err(|e| format!("Parse setting '{key}': {e}")),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(format!("Query setting '{key}': {e}")),
    }
}

fn read_transcription_selection_in_transaction(
    transaction: &rusqlite::Transaction<'_>,
) -> Result<TranscriptionProfileSelection, PreparedSettingsSaveError> {
    let mut settings = AppSettings::default();
    if let Some(engine_id) =
        read_json_setting_in_transaction::<String>(transaction, TRANSCRIPTION_ENGINE_ID_KEY)?
    {
        settings.transcription_engine_id = engine_id;
    }
    if let Some(model_id) =
        read_json_setting_in_transaction::<String>(transaction, TRANSCRIPTION_MODEL_ID_KEY)?
    {
        settings.transcription_model_id = model_id;
    }
    if let Some(backend_id) =
        read_json_setting_in_transaction::<String>(transaction, TRANSCRIPTION_BACKEND_ID_KEY)?
    {
        settings.transcription_backend_id = backend_id;
    }

    Ok(settings.sanitized().transcription_selection())
}

fn write_json_setting<T>(db: &Database, key: &str, value: &T) -> Result<(), String>
where
    T: Serialize,
{
    let encoded =
        serde_json::to_string(value).map_err(|e| format!("Serialize setting '{key}': {e}"))?;
    db.set_setting(key, &encoded)
}

fn write_json_setting_in_transaction<T>(
    transaction: &rusqlite::Transaction<'_>,
    key: &str,
    value: &T,
) -> Result<(), String>
where
    T: Serialize,
{
    let encoded =
        serde_json::to_string(value).map_err(|e| format!("Serialize setting '{key}': {e}"))?;
    transaction
        .execute(
            "INSERT OR REPLACE INTO settings (key, value) VALUES (?1, ?2)",
            rusqlite::params![key, encoded],
        )
        .map_err(|e| format!("Set setting: {e}"))?;
    Ok(())
}

fn delete_setting_in_transaction(
    transaction: &rusqlite::Transaction<'_>,
    key: &str,
) -> Result<(), String> {
    transaction
        .execute(
            "DELETE FROM settings WHERE key = ?1",
            rusqlite::params![key],
        )
        .map_err(|e| format!("Delete setting: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::DictionaryLearningMode;
    #[test]
    fn dictionary_learning_migrates_legacy_choice_and_keeps_new_mode() {
        let (db, _dir) = crate::test_helpers::fixtures::test_db();
        for (legacy, expected) in [("true", "automatic"), ("false", "disabled")] {
            db.set_setting("dictation_learn_from_edit", legacy).unwrap();
            let settings = super::AppSettings::load(&db).unwrap();
            assert_eq!(
                serde_json::to_value(settings).unwrap()["dictionary_learning_mode"],
                expected
            );
        }
        db.set_setting("dictionary_learning_mode", "\"suggestions\"")
            .unwrap();
        let settings = super::AppSettings::load(&db).unwrap();
        settings.save(&db).unwrap();
        assert_eq!(
            serde_json::to_value(super::AppSettings::load(&db).unwrap()).unwrap()["dictionary_learning_mode"],
            "suggestions"
        );
    }

    use super::{
        AppSettings, CLAMSHELL_AUDIO_DEVICE_KEY, INPUT_PRIORITY_KEY, MeetingAudioRetention,
        PINNED_DEVICE_MIGRATION_KEY, PasteMethod, SettingsOptions, ShortcutSettings,
        SummaryProviderChoice, Theme,
    };
    use crate::audio::InputPriority;
    use crate::constants::OLLAMA_DEFAULT_URL;
    use crate::test_helpers::fixtures::test_db;

    /// AC2: every step the settings UI is told to offer must survive
    /// `sanitize_for_save` untouched. Narrowing a range, or editing an option
    /// list out of its range, fails here instead of silently folding the
    /// user's choice back to the default at the next save.
    #[test]
    fn offered_options_survive_sanitize() {
        let options = SettingsOptions::current();

        for minutes in options.model_unload_timeout_minutes {
            let normalized = AppSettings {
                model_unload_timeout_minutes: minutes,
                ..AppSettings::default()
            }
            .sanitize_for_save()
            .unwrap();
            assert_eq!(normalized.model_unload_timeout_minutes, minutes);
        }
    }

    /// AC3: the menus the user sees are unchanged by this refactor.
    #[test]
    fn offered_options_are_unchanged() {
        let options = SettingsOptions::current();
        assert_eq!(options.model_unload_timeout_minutes, vec![0, 5, 15, 60]);
    }

    #[test]
    fn app_settings_round_trip() {
        let (db, _dir) = test_db();
        let settings = AppSettings {
            window_width: 1040,
            window_height: 1280,
            auto_update_check_enabled: false,
            theme: Theme::Light,
            locale: "fr".into(),
            auto_paste: true,
            paste_delay_ms: 250,
            paste_method: PasteMethod::Type,
            ollama_url: "http://example.test:11434".into(),
            summary_provider: SummaryProviderChoice::Ollama,
            ollama_model: "qwen2.5".into(),
            meeting_summaries_enabled: true,
            summary_compression_enabled: false,
            detailed_logging: true,
            clamshell_audio_device: Some("USB Mic".into()),
            input_priority: InputPriority::default(),
            transcription_engine_id: "kyutai".into(),
            transcription_model_id: "stt-1b-en_fr".into(),
            transcription_backend_id: "candle".into(),
            filler_removal: true,
            dictionary_correction: true,
            capture_system_audio: true,
            calendar_integration_enabled: true,
            calendar_selected_ids: vec!["cal-1".into(), "cal-2".into()],
            calendar_reminder_minutes: 5,
            feedback_sounds_enabled: false,
            pill_hidden: true,
            model_unload_timeout_minutes: 15,
            meeting_autostop_enabled: false,
            autostart_enabled: true,
            meeting_audio_retention: MeetingAudioRetention::Keep30d,
            dictation_polish_enabled: true,
            dictation_polish_template_id: "email".into(),
            dictation_polish_templates: crate::summary::default_polish_templates(),
            dictionary_learning_mode: DictionaryLearningMode::Automatic,
            dictation_app_branches: super::default_app_branches(),
            default_summary_template_id: crate::summary::TEMPLATE_SUMMARY_BRIEF.into(),
            summary_templates: crate::summary::default_summary_templates(),
            last_seen_version: "0.0.9".into(),
        };

        settings.save(&db).expect("save settings");

        let loaded = AppSettings::load(&db).expect("load settings");
        let mut expected = settings;
        expected.input_priority.known = loaded.input_priority.known.clone();
        assert_eq!(loaded, expected);
        #[cfg(target_os = "macos")]
        assert!(!loaded.input_priority.known.is_empty());
    }

    #[test]
    fn hud_activation_persists() {
        let (db, _dir) = test_db();
        assert!(
            !AppSettings::default().pill_hidden,
            "the HUD is on by default"
        );

        for pill_hidden in [true, false] {
            AppSettings {
                pill_hidden,
                ..AppSettings::default()
            }
            .save(&db)
            .expect("save settings");
            let loaded = AppSettings::load(&db).expect("load settings");
            assert_eq!(loaded.pill_hidden, pill_hidden);
        }
    }

    #[test]
    fn app_settings_save_rolls_back_every_key_when_a_middle_write_fails() {
        let (db, _dir) = test_db();
        let initial = AppSettings::default();
        initial.save(&db).expect("save initial settings");
        let mut before = db.get_all_settings().expect("read initial settings");
        before.sort();

        db.execute_settings_sql_for_test(
            r#"
            CREATE TRIGGER fail_calendar_integration_write
            BEFORE INSERT ON settings
            WHEN NEW.key = 'calendar_integration_enabled'
            BEGIN
                SELECT RAISE(ABORT, 'injected middle settings write failure');
            END;
            "#,
        )
        .expect("install failure trigger");

        let candidate = AppSettings {
            theme: Theme::Dark,
            calendar_integration_enabled: true,
            calendar_reminder_minutes: 17,
            ..initial
        };
        let error = candidate.save(&db).expect_err("injected save must fail");
        assert!(error.contains("injected middle settings write failure"));

        let mut after = db.get_all_settings().expect("read settings after failure");
        after.sort();
        assert_eq!(after, before, "no AppSettings key may change partially");
    }

    #[test]
    fn read_only_snapshot_never_runs_audio_migration_writes() {
        let (db, _dir) = test_db();
        db.set_setting(PINNED_DEVICE_MIGRATION_KEY, r#""Legacy Microphone""#)
            .expect("save legacy pinned device");
        db.set_setting(
            CLAMSHELL_AUDIO_DEVICE_KEY,
            r#""Legacy Clamshell Microphone""#,
        )
        .expect("save legacy clamshell device");
        let mut before = db.get_all_settings().expect("read legacy settings");
        before.sort();
        db.execute_settings_sql_for_test(
            r#"
            CREATE TRIGGER reject_any_settings_write_during_read
            BEFORE INSERT ON settings
            BEGIN
                SELECT RAISE(ABORT, 'read attempted to write settings');
            END;
            "#,
        )
        .expect("install read-only trigger");

        let settings = AppSettings::load_read_only(&db).expect("load read-only snapshot");
        assert!(settings.input_priority.priorities.is_empty());
        assert_eq!(
            settings.clamshell_audio_device.as_deref(),
            Some("Legacy Clamshell Microphone")
        );
        let mut after = db.get_all_settings().expect("read settings after snapshot");
        after.sort();
        assert_eq!(after, before, "a read-only snapshot must change no key");
    }

    // SOU-036: an install that never saw the setting has no key at all. That
    // must load as "off" — the login item is only ever registered after a
    // deliberate gesture (toggle or fresh-install wizard), never by default.
    #[test]
    fn autostart_absent_key_loads_as_disabled() {
        let (db, _dir) = test_db();
        assert_eq!(
            db.get_setting("autostart_enabled").expect("get setting"),
            None
        );

        let loaded = AppSettings::load(&db).expect("load settings");
        assert!(!loaded.autostart_enabled);
    }

    #[test]
    fn autostart_explicit_value_round_trips() {
        let (db, _dir) = test_db();
        let settings = AppSettings {
            autostart_enabled: true,
            ..AppSettings::default()
        };
        settings.save(&db).expect("save settings");
        assert!(AppSettings::load(&db).expect("load").autostart_enabled);

        // An explicit `false` is written too, so it stays distinguishable
        // from the never-written key above.
        let settings = AppSettings {
            autostart_enabled: false,
            ..AppSettings::default()
        };
        settings.save(&db).expect("save settings");
        assert_eq!(
            db.get_setting("autostart_enabled").expect("get setting"),
            Some("false".into())
        );
        assert!(!AppSettings::load(&db).expect("load").autostart_enabled);
    }

    fn input_device(uid: &str, name: &str) -> crate::audio::AudioInputDevice {
        crate::audio::AudioInputDevice {
            uid: uid.into(),
            name: name.into(),
            transport: crate::audio::TransportType::Usb,
            is_default: false,
        }
    }

    fn stored_priority(db: &crate::db::Database) -> InputPriority {
        let raw = db
            .get_setting(INPUT_PRIORITY_KEY)
            .expect("read input priority")
            .expect("input priority is stored");
        serde_json::from_str(&raw).expect("parse input priority")
    }

    fn seed_pin(db: &crate::db::Database, pin: &str, priorities: &[&str]) {
        db.set_setting(
            PINNED_DEVICE_MIGRATION_KEY,
            &serde_json::to_string(pin).unwrap(),
        )
        .expect("seed the pinned device");
        seed_priorities(db, priorities);
    }

    fn seed_priorities(db: &crate::db::Database, priorities: &[&str]) {
        let priority = InputPriority {
            priorities: priorities.iter().map(|uid| uid.to_string()).collect(),
            known: Vec::new(),
        };
        db.set_setting(
            INPUT_PRIORITY_KEY,
            &serde_json::to_string(&priority).unwrap(),
        )
        .expect("seed the preference order");
    }

    fn migrate_pin(
        db: &crate::db::Database,
        devices: &[crate::audio::AudioInputDevice],
    ) -> AppSettings {
        let mut settings = AppSettings::load_read_only(db).expect("load stored settings");
        AppSettings::migrate_pinned_device(&mut settings, db, devices).expect("migrate the pin");
        settings
    }

    #[test]
    fn the_microphone_chosen_in_the_wizard_is_saved_on_top_of_the_order() {
        for ranked in [&[][..], &["builtin-uid", "usb-uid"][..]] {
            let (db, _dir) = test_db();
            seed_priorities(&db, ranked);
            let mut settings = AppSettings::load(&db).expect("load");

            settings.input_priority.promote("usb-uid");
            settings
                .prepare_for_save()
                .expect("prepare")
                .save(&db)
                .expect("save");

            let loaded = AppSettings::load(&db).expect("reload");
            assert_eq!(loaded.input_priority.priorities.first().unwrap(), "usb-uid");
            assert_eq!(loaded.input_priority.priorities.len(), ranked.len().max(1));
        }
    }

    #[test]
    fn a_pinned_microphone_becomes_the_top_of_the_preference_order() {
        let (db, _dir) = test_db();
        seed_pin(&db, "usb-uid", &["builtin-uid"]);

        let settings = migrate_pin(&db, &[input_device("usb-uid", "USB Mic")]);

        assert_eq!(
            settings.input_priority.priorities,
            ["usb-uid", "builtin-uid"]
        );
        assert_eq!(stored_priority(&db).priorities, ["usb-uid", "builtin-uid"]);
        assert_eq!(db.get_setting(PINNED_DEVICE_MIGRATION_KEY).unwrap(), None);
    }

    #[test]
    fn a_microphone_pinned_by_name_is_promoted_by_its_uid() {
        let (db, _dir) = test_db();
        seed_pin(&db, "USB Mic", &["builtin-uid"]);

        migrate_pin(&db, &[input_device("usb-uid", "USB Mic")]);

        assert_eq!(stored_priority(&db).priorities, ["usb-uid", "builtin-uid"]);
    }

    #[test]
    fn a_pinned_microphone_already_ranked_moves_up_without_a_duplicate() {
        let (db, _dir) = test_db();
        seed_pin(&db, "usb-uid", &["builtin-uid", "usb-uid", "webcam-uid"]);

        migrate_pin(&db, &[input_device("usb-uid", "USB Mic")]);

        assert_eq!(
            stored_priority(&db).priorities,
            ["usb-uid", "builtin-uid", "webcam-uid"]
        );
    }

    #[test]
    fn the_pin_migration_runs_once() {
        let (db, _dir) = test_db();
        seed_pin(&db, "usb-uid", &["builtin-uid"]);
        AppSettings::load(&db).expect("first load migrates");

        // The user then ranks another microphone first.
        let mut reordered = stored_priority(&db);
        reordered.promote("builtin-uid");
        db.set_setting(
            INPUT_PRIORITY_KEY,
            &serde_json::to_string(&reordered).unwrap(),
        )
        .unwrap();

        let loaded = AppSettings::load(&db).expect("second load");
        assert_eq!(loaded.input_priority.priorities, ["builtin-uid", "usb-uid"]);
        assert_eq!(stored_priority(&db).priorities, ["builtin-uid", "usb-uid"]);
    }

    #[test]
    fn a_device_event_keeps_an_order_saved_since_the_last_load() {
        let (db, _dir) = test_db();
        seed_priorities(&db, &["builtin-uid", "usb-uid"]);

        let synced = AppSettings::touch_known_devices(&db, &[input_device("usb-uid", "USB Mic")])
            .expect("sync known devices");

        assert_eq!(synced.priorities, ["builtin-uid", "usb-uid"]);
        assert_eq!(synced.known.len(), 1);
        assert_eq!(synced.known[0].uid, "usb-uid");
        assert_eq!(stored_priority(&db), synced);
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
            db.get_setting("clamshell_audio_device")
                .expect("get setting"),
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
    fn shortcut_settings_reject_duplicate_native_bindings() {
        let shortcuts = ShortcutSettings {
            toggle: "ShiftLeft".into(),
            push_to_talk: "ShiftLeft".into(),
        };

        assert_eq!(
            shortcuts.normalize().unwrap_err(),
            "Dictation shortcuts must be different"
        );
    }

    #[test]
    fn native_toggle_shortcut_persists_across_load() {
        let (db, _dir) = test_db();
        let s = ShortcutSettings {
            toggle: "ShiftLeft".into(),
            push_to_talk: "MetaRight".into(),
        };
        s.save(&db).unwrap();
        let loaded = ShortcutSettings::load(&db).unwrap();
        assert_eq!(loaded.toggle, "ShiftLeft");
        assert_eq!(loaded.push_to_talk, "MetaRight");
    }

    #[test]
    fn fn_shortcut_normalizes_to_default_on_load() {
        let (db, _dir) = test_db();

        // Can't use s.save(&db) directly because normalize() would error on duplicates.
        // We bypass it to test the load time migration.
        super::write_json_setting(&db, super::SHORTCUT_TOGGLE_KEY, &"Fn".to_string()).unwrap();
        super::write_json_setting(&db, super::SHORTCUT_PUSH_TO_TALK_KEY, &"Fn".to_string())
            .unwrap();

        let loaded = ShortcutSettings::load(&db).unwrap();
        assert_eq!(loaded.toggle, crate::DEFAULT_TOGGLE_SHORTCUT.to_string());
        assert_eq!(loaded.push_to_talk, String::new());
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

    /// Loading must never rewrite what is stored: the one-time correction of
    /// the old "never unload" default is a schema migration, not a load-time
    /// side effect (`load` runs from 19 call sites, the headless CLI included).
    #[test]
    fn loading_never_rewrites_a_stored_unload_timeout() {
        for minutes in [0u32, 5, 15, 60] {
            let (db, _dir) = test_db();
            db.set_setting("model_unload_timeout_minutes", &minutes.to_string())
                .expect("save minutes");
            let settings = AppSettings::load(&db).expect("load settings");
            assert_eq!(settings.model_unload_timeout_minutes, minutes);
            assert_eq!(
                db.get_setting("model_unload_timeout_minutes")
                    .expect("read back"),
                Some(minutes.to_string())
            );
        }
    }

    #[test]
    fn a_chosen_unload_timeout_is_never_corrected() {
        for minutes in [5u32, 15, 60] {
            let (db, _dir) = test_db();
            db.set_setting("model_unload_timeout_minutes", &minutes.to_string())
                .expect("save minutes");
            let settings = AppSettings::load(&db).expect("load settings");
            assert_eq!(settings.model_unload_timeout_minutes, minutes);
        }
    }

    #[test]
    fn model_unload_timeout_minutes_rejects_unlisted_values() {
        let (db, _dir) = test_db();
        db.set_setting("model_unload_timeout_minutes", "7")
            .expect("save minutes");
        let settings = AppSettings::load(&db).expect("load settings");
        assert_eq!(
            settings.model_unload_timeout_minutes,
            AppSettings::default().model_unload_timeout_minutes
        );

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

    // SOU-131: `dictionary_interview_done` was dropped with the onboarding
    // interview, so every install that ran the old wizard still carries that
    // row. Loading must ignore it instead of failing on it.
    #[test]
    fn load_ignores_a_setting_key_that_no_longer_exists() {
        let (db, _dir) = test_db();
        let retired = [
            ("dictionary_interview_done", "true"),
            ("allow_bluetooth_mic", "true"),
            ("vad_enabled", "false"),
            ("stutter_collapse", "true"),
            ("dictation_ceiling_seconds", "120"),
            ("meeting_transcription_language", "\"fr\""),
            ("meeting_autostop_minutes", "30"),
            ("meeting_max_duration_minutes", "480"),
            ("meeting_audio_diagnostic", "true"),
            ("log_level", "\"debug\""),
            ("debug_transcription", "true"),
            ("compact_transcript_enabled", "true"),
            ("summary_compression_rate", "0.7"),
            ("calendar_autostart_enabled", "false"),
            ("feedback_sounds_volume", "15"),
            ("pill_hidden_in_captures", "false"),
        ];
        for (key, value) in retired {
            db.set_setting(key, value).expect("seed the retired key");
        }

        let loaded = AppSettings::load(&db).expect("load settings");

        assert_eq!(loaded, AppSettings::default());
        // Saving does not resurrect the keys, and leaves the stale rows alone.
        loaded.save(&db).expect("save settings");
        for (key, value) in retired {
            assert_eq!(db.get_setting(key).unwrap(), Some(value.to_string()));
        }
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
        assert_eq!(loaded.dictation_polish_templates.len(), 6);
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
    fn app_branches_round_trip_preserves_identity_order_activation_and_targets() {
        use super::{AppBranchRule, AppBranchTarget, PolishTemplateId};
        let (db, _dir) = test_db();
        let mut settings = AppSettings::default();
        let mut first = AppBranchRule::new("MAIL".into(), AppBranchTarget::Global);
        first.enabled = true;
        let second = AppBranchRule::new(
            "Slack".into(),
            AppBranchTarget::Template(PolishTemplateId("chat".into())),
        );
        settings.dictation_app_branches = vec![second, first];
        settings.save(&db).unwrap();
        let loaded = AppSettings::load(&db).unwrap();
        assert_eq!(
            loaded.dictation_app_branches,
            settings.dictation_app_branches
        );
        settings.dictation_app_branches.clear();
        settings.save(&db).unwrap();
        assert!(
            AppSettings::load(&db)
                .unwrap()
                .dictation_app_branches
                .is_empty()
        );
    }

    #[test]
    fn app_branch_custom_shared_template_survives_restart_and_duplicate_ids() {
        use super::{AppBranchRule, AppBranchTarget, DictationPolishTemplate, PolishTemplateId};
        let (db, _dir) = test_db();
        let mut settings = AppSettings {
            dictation_polish_template_id: "custom-note".into(),
            ..AppSettings::default()
        };
        let builtin_count = settings.dictation_polish_templates.len();
        settings
            .dictation_polish_templates
            .iter_mut()
            .find(|template| template.id == crate::summary::TEMPLATE_EMAIL)
            .unwrap()
            .prompt = "Edited built-in instruction".into();
        settings.dictation_polish_templates.extend([
            DictationPolishTemplate {
                id: " custom-note ".into(),
                label: "My note".into(),
                prompt: "Shared custom instruction".into(),
            },
            DictationPolishTemplate {
                id: "custom-note".into(),
                label: "Duplicate".into(),
                prompt: "Must not replace the first instruction".into(),
            },
            DictationPolishTemplate {
                id: "another-custom".into(),
                label: "Second custom".into(),
                prompt: "Second instruction".into(),
            },
            DictationPolishTemplate {
                id: " email ".into(),
                label: "Duplicate built-in".into(),
                prompt: "Must not replace the edited built-in".into(),
            },
        ]);
        let mut rule = AppBranchRule::new(
            "Notes".into(),
            AppBranchTarget::Template(PolishTemplateId("custom-note".into())),
        );
        rule.enabled = true;
        settings.dictation_app_branches = vec![rule];
        settings.save(&db).unwrap();
        let restored = AppSettings::load(&db).unwrap();
        assert_eq!(restored.dictation_polish_template_id, "custom-note");
        let customs: Vec<_> = restored.dictation_polish_templates[builtin_count..]
            .iter()
            .map(|template| template.id.as_str())
            .collect();
        assert_eq!(customs, ["custom-note", "another-custom"]);
        assert_eq!(
            restored
                .dictation_polish_templates
                .iter()
                .find(|template| template.id == crate::summary::TEMPLATE_EMAIL)
                .unwrap()
                .prompt,
            "Edited built-in instruction"
        );
        let template = crate::summary::resolve_app_branch(&restored, Some("Apple Notes"))
            .template()
            .unwrap();
        assert_eq!(template.id, "custom-note");
        assert_eq!(template.prompt, "Shared custom instruction");
        restored.save(&db).unwrap();
        assert_eq!(
            AppSettings::load(&db).unwrap().dictation_polish_templates,
            restored.dictation_polish_templates
        );
    }

    #[test]
    fn app_branch_upgrade_adds_disabled_seeds_and_preserves_global_and_prompts() {
        let (db, _dir) = test_db();
        db.set_setting("dictation_polish_template_id", "\"email\"")
            .unwrap();
        db.set_setting(
            "dictation_polish_templates",
            &serde_json::to_string(&vec![super::DictationPolishTemplate {
                id: "email".into(),
                label: "My email".into(),
                prompt: "User instruction".into(),
            }])
            .unwrap(),
        )
        .unwrap();
        let loaded = AppSettings::load(&db).unwrap();
        assert_eq!(loaded.dictation_polish_template_id, "email");
        assert_eq!(
            loaded
                .dictation_polish_templates
                .iter()
                .find(|template| template.id == "email")
                .unwrap()
                .prompt,
            "User instruction"
        );
        assert!(!loaded.dictation_app_branches.is_empty());
        assert!(
            loaded
                .dictation_app_branches
                .iter()
                .all(|rule| !rule.enabled)
        );
        loaded.save(&db).unwrap();
        assert_eq!(
            AppSettings::load(&db).unwrap().dictation_app_branches,
            loaded.dictation_app_branches
        );
    }

    #[test]
    fn app_branch_save_rejects_blank_patterns_and_repairs_duplicate_ids() {
        let mut settings = AppSettings::default();
        let mut duplicate = settings.dictation_app_branches[0].clone();
        duplicate.app_pattern = "   ".into();
        settings.dictation_app_branches.push(duplicate);
        settings
            .dictation_app_branches
            .push(settings.dictation_app_branches[0].clone());
        let saved = settings.sanitize_for_save().unwrap();
        assert!(
            saved
                .dictation_app_branches
                .iter()
                .all(|rule| !rule.app_pattern.is_empty())
        );
        let ids: std::collections::HashSet<_> = saved
            .dictation_app_branches
            .iter()
            .map(|rule| &rule.id)
            .collect();
        assert_eq!(ids.len(), saved.dictation_app_branches.len());
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

    /// Pre-provider-setting installs stored the engine in `ollama_model`.
    /// Loading that row with no `summary_provider` key must not default to
    /// Auto, or those users would silently start hitting Apple Intelligence.
    #[test]
    fn missing_summary_provider_with_an_ollama_model_migrates_to_ollama() {
        let (db, _dir) = test_db();
        db.set_setting("ollama_model", "\"qwen2.5:7b\"")
            .expect("save model");

        let settings = AppSettings::load(&db).expect("load settings");
        assert_eq!(settings.summary_provider, SummaryProviderChoice::Ollama);
        assert_eq!(settings.ollama_model, "qwen2.5:7b");
    }

    #[test]
    fn missing_summary_provider_without_a_model_stays_auto() {
        let (db, _dir) = test_db();
        let settings = AppSettings::load(&db).expect("load settings");
        assert_eq!(settings.summary_provider, SummaryProviderChoice::Auto);
    }

    #[test]
    fn explicit_auto_is_kept_even_with_a_stored_ollama_model() {
        let (db, _dir) = test_db();
        db.set_setting("summary_provider", "\"auto\"")
            .expect("save provider");
        db.set_setting("ollama_model", "\"qwen2.5:7b\"")
            .expect("save model");

        let settings = AppSettings::load(&db).expect("load settings");
        assert_eq!(settings.summary_provider, SummaryProviderChoice::Auto);
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

    /// Ensure the frontend's published paste-delay limits match backend sanitization.
    #[test]
    fn paste_delay_bounds_match_settings_options() {
        let opts = SettingsOptions::current();
        assert_eq!(opts.paste_delay_ms_min, super::MIN_PASTE_DELAY_MS);
        assert_eq!(opts.paste_delay_ms_max, super::MAX_PASTE_DELAY_MS);

        let settings_min = AppSettings {
            paste_delay_ms: opts.paste_delay_ms_min,
            ..Default::default()
        };
        assert_eq!(
            settings_min.sanitized().paste_delay_ms,
            opts.paste_delay_ms_min
        );

        let settings_max = AppSettings {
            paste_delay_ms: opts.paste_delay_ms_max,
            ..Default::default()
        };
        assert_eq!(
            settings_max.sanitized().paste_delay_ms,
            opts.paste_delay_ms_max
        );
    }

    #[test]
    fn reminder_bounds_match_settings_options() {
        let options = SettingsOptions::current();
        assert_eq!(
            options.calendar_reminder_minutes_min,
            *super::CALENDAR_REMINDER_MINUTES_RANGE.start()
        );
        assert_eq!(
            options.calendar_reminder_minutes_max,
            *super::CALENDAR_REMINDER_MINUTES_RANGE.end()
        );
    }

    /// SOU-285: the scheduler's one-key read agrees with the full snapshot,
    /// including the default when the key was never written.
    #[test]
    fn calendar_integration_enabled_reads_the_single_key() {
        let (db, _dir) = test_db();
        assert!(!super::calendar_integration_enabled(&db).expect("default read"));
        db.set_setting(super::CALENDAR_INTEGRATION_ENABLED_KEY, "true")
            .expect("write toggle");
        assert!(super::calendar_integration_enabled(&db).expect("enabled read"));
        assert!(
            AppSettings::load_read_only(&db)
                .expect("snapshot")
                .calendar_integration_enabled
        );
    }

    #[test]
    fn french_locale_detection() {
        assert!(super::locale_is_french("fr"));
        assert!(super::locale_is_french("fr-CA"));
        assert!(!super::locale_is_french("en"));
        assert!(!super::locale_is_french(""));
    }
}

#[cfg(test)]
mod compression_tests {
    use super::*;
    #[test]
    fn compression_defaults_and_roundtrip() {
        let (db, _dir) = crate::test_helpers::fixtures::test_db();
        let mut s = AppSettings::default();
        assert!(!s.meeting_summaries_enabled && !s.summary_compression_enabled);
        assert!(
            !AppSettings::load_read_only(&db)
                .unwrap()
                .meeting_summaries_enabled
        );
        s.meeting_summaries_enabled = true;
        s.summary_compression_enabled = true;
        s.save(&db).unwrap();
        let loaded = AppSettings::load_read_only(&db).unwrap();
        assert!(loaded.meeting_summaries_enabled && loaded.summary_compression_enabled);
        s.meeting_summaries_enabled = false;
        s.save(&db).unwrap();
        let disabled = AppSettings::load_read_only(&db).unwrap();
        assert!(!disabled.meeting_summaries_enabled);
        assert!(disabled.summary_compression_enabled);
    }
}
