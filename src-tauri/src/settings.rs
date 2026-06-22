use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::constants::OLLAMA_DEFAULT_URL;
use crate::db::Database;
use crate::engine::{
    CANDLE_BACKEND_ID, KYUTAI_ENGINE_ID, KYUTAI_MODEL_ID, resolve_transcription_profile,
};

const THEME_KEY: &str = "theme";
const AUTO_PASTE_KEY: &str = "auto_paste";
const PASTE_DELAY_MS_KEY: &str = "paste_delay_ms";
const OLLAMA_URL_KEY: &str = "ollama_url";
const OLLAMA_MODEL_KEY: &str = "ollama_model";
const DEBUG_TRANSCRIPTION_KEY: &str = "debug_transcription";
const AUDIO_DEVICE_KEY: &str = "audio_device";
const TRANSCRIPTION_ENGINE_ID_KEY: &str = "transcription_engine_id";
const TRANSCRIPTION_MODEL_ID_KEY: &str = "transcription_model_id";
const TRANSCRIPTION_BACKEND_ID_KEY: &str = "transcription_backend_id";
const VAD_ENABLED_KEY: &str = "vad_enabled";
const FILLER_REMOVAL_KEY: &str = "filler_removal";
const STUTTER_COLLAPSE_KEY: &str = "stutter_collapse";
const DICTIONARY_CORRECTION_KEY: &str = "dictionary_correction";
const CAPTURE_SYSTEM_AUDIO_KEY: &str = "capture_system_audio";
const LOCALE_KEY: &str = "locale";
const SHORTCUT_TOGGLE_KEY: &str = "shortcut_toggle";
const SHORTCUT_PUSH_TO_TALK_KEY: &str = "shortcut_push_to_talk";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default, specta::Type)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    #[default]
    Dark,
    Light,
    System,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, specta::Type)]
pub struct AppSettings {
    pub theme: Theme,
    pub locale: String,
    pub auto_paste: bool,
    pub paste_delay_ms: u64,
    pub ollama_url: String,
    pub ollama_model: String,
    pub debug_transcription: bool,
    pub audio_device: Option<String>,
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
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            theme: Theme::Dark,
            locale: String::new(),
            auto_paste: false,
            paste_delay_ms: 100,
            ollama_url: OLLAMA_DEFAULT_URL.to_string(),
            ollama_model: String::new(),
            debug_transcription: false,
            audio_device: None,
            transcription_engine_id: KYUTAI_ENGINE_ID.to_string(),
            transcription_model_id: KYUTAI_MODEL_ID.to_string(),
            transcription_backend_id: CANDLE_BACKEND_ID.to_string(),
            vad_enabled: true,
            filler_removal: true,
            stutter_collapse: false,
            dictionary_correction: true,
            capture_system_audio: true,
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
        if let Some(audio_device) = read_json_setting::<String>(db, AUDIO_DEVICE_KEY)? {
            settings.audio_device = Some(audio_device);
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

        normalized
    }

    pub fn save(&self, db: &Database) -> Result<(), String> {
        let normalized = self.sanitize_for_save()?;

        write_json_setting(db, THEME_KEY, &normalized.theme)?;
        write_json_setting(db, LOCALE_KEY, &normalized.locale)?;
        write_json_setting(db, AUTO_PASTE_KEY, &normalized.auto_paste)?;
        write_json_setting(db, PASTE_DELAY_MS_KEY, &normalized.paste_delay_ms)?;
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

        if let Some(audio_device) = normalized.audio_device.as_ref() {
            write_json_setting(db, AUDIO_DEVICE_KEY, audio_device)?;
        } else {
            db.delete_setting(AUDIO_DEVICE_KEY)?;
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
    use super::{AppSettings, ShortcutSettings, Theme};
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
            ollama_url: "http://example.test:11434".into(),
            ollama_model: "qwen2.5".into(),
            debug_transcription: true,
            audio_device: Some("BlackHole".into()),
            transcription_engine_id: "kyutai".into(),
            transcription_model_id: "stt-1b-en_fr".into(),
            transcription_backend_id: "candle".into(),
            vad_enabled: true,
            filler_removal: true,
            stutter_collapse: false,
            dictionary_correction: true,
            capture_system_audio: true,
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
}
