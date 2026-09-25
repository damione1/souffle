//! SOU-188: Settings shell. Mirrors `SettingsView.svelte` + its 17 sections,
//! one tab/section at a time as each milestone lands (see the ticket
//! journal for which). `AppSettings` is the same contract the real backend
//! (and the Svelte UI) already reads/writes - this never redeclares bounds
//! or option lists locally (AC3): `SettingsOptions::current()` is the single
//! source for those once a section needs them.

use crate::{
    AccessState, AppLocale, AppTheme, AudioRetention as SlintAudioRetention, CalendarRow,
    LogLevel as SlintLogLevel, MainWindow, MeetingLanguage, PasteMethod as SlintPasteMethod,
    PermissionKind as SlintPermissionKind,
};
use souffle_lib::calendar::CalendarInfo;
use souffle_lib::logging::LogLevel;
use souffle_lib::permissions::{PermState, PermissionKind};
use souffle_lib::settings::{
    AppSettings, MeetingAudioRetention, MeetingTranscriptionLanguage, PasteMethod, SettingsOptions,
    ShortcutSettings, Theme,
};

/// Pushes `settings` into the Slint properties this shell currently wires.
/// Mirrors `controller.svelte.ts` setting `app.settings` after
/// `getSettings()`. Grows with each milestone; the "Système" and
/// "Interface" tabs' fields are populated so far.
pub fn populate(window: &MainWindow, settings: &AppSettings) {
    window.set_settings_autostart_enabled(settings.autostart_enabled);
    window.set_settings_debug_transcription(settings.debug_transcription);
    window.set_settings_log_level(log_level_to_slint(settings.log_level));

    window.set_settings_theme(theme_to_slint(settings.theme));
    window.set_settings_locale(locale_to_slint(&settings.locale));
    window.set_settings_auto_paste(settings.auto_paste);
    window.set_settings_paste_method(paste_method_to_slint(settings.paste_method));
    window.set_settings_dictation_learn_from_edit(settings.dictation_learn_from_edit);
    window.set_settings_paste_delay_ms(settings.paste_delay_ms as i32);
    window.set_settings_pill_hidden(settings.pill_hidden);
    window.set_settings_feedback_sounds_enabled(settings.feedback_sounds_enabled);
    window.set_settings_feedback_sounds_volume(settings.feedback_sounds_volume as i32);

    window.set_settings_calendar_enabled(settings.calendar_integration_enabled);
    window.set_settings_calendar_autostart_enabled(settings.calendar_autostart_enabled);
    window.set_settings_calendar_reminder_minutes(settings.calendar_reminder_minutes as i32);

    window.set_settings_allow_bluetooth_mic(settings.allow_bluetooth_mic);
    window.set_settings_capture_system_audio(settings.capture_system_audio);
    window.set_settings_meeting_transcription_language(meeting_language_to_slint(
        settings.meeting_transcription_language,
    ));
    window.set_settings_meeting_autostop_enabled(settings.meeting_autostop_enabled);
    window.set_settings_meeting_autostop_label(
        crate::audio_ui::minute_label(settings.meeting_autostop_minutes).into(),
    );
    window.set_settings_meeting_max_duration_label(
        crate::audio_ui::minute_label(settings.meeting_max_duration_minutes).into(),
    );
    window.set_settings_vad_enabled(settings.vad_enabled);
    window.set_settings_filler_removal(settings.filler_removal);
    window.set_settings_stutter_collapse(settings.stutter_collapse);
    window.set_settings_dictionary_correction(settings.dictionary_correction);

    let bounds = SettingsOptions::current();
    window.set_settings_paste_delay_min(bounds.paste_delay_ms_min as i32);
    window.set_settings_paste_delay_max(bounds.paste_delay_ms_max as i32);
    window.set_settings_feedback_sounds_volume_min(bounds.feedback_sounds_volume_min as i32);
    window.set_settings_feedback_sounds_volume_max(bounds.feedback_sounds_volume_max as i32);
    window.set_settings_calendar_reminder_minutes_min(bounds.calendar_reminder_minutes_min as i32);
    window.set_settings_calendar_reminder_minutes_max(bounds.calendar_reminder_minutes_max as i32);
    window.set_settings_meeting_autostop_labels(shared_string_model(
        &crate::audio_ui::minute_labels(&bounds.meeting_autostop_minutes),
    ));
    window.set_settings_meeting_max_duration_labels(shared_string_model(
        &crate::audio_ui::minute_labels(&bounds.meeting_max_duration_minutes),
    ));
}

pub fn populate_platform_capabilities(
    window: &MainWindow,
    is_laptop: bool,
    system_audio_supported: bool,
) {
    window.set_settings_is_laptop(is_laptop);
    window.set_settings_system_audio_supported(system_audio_supported);
}

fn shared_string_model(values: &[String]) -> slint::ModelRc<slint::SharedString> {
    let values: Vec<slint::SharedString> = values.iter().map(|v| v.as_str().into()).collect();
    std::rc::Rc::new(slint::VecModel::from(values)).into()
}

pub fn meeting_language_to_slint(value: MeetingTranscriptionLanguage) -> MeetingLanguage {
    match value {
        MeetingTranscriptionLanguage::Auto => MeetingLanguage::Auto,
        MeetingTranscriptionLanguage::En => MeetingLanguage::En,
        MeetingTranscriptionLanguage::Fr => MeetingLanguage::Fr,
    }
}

pub fn meeting_language_from_slint(value: MeetingLanguage) -> MeetingTranscriptionLanguage {
    match value {
        MeetingLanguage::Auto => MeetingTranscriptionLanguage::Auto,
        MeetingLanguage::En => MeetingTranscriptionLanguage::En,
        MeetingLanguage::Fr => MeetingTranscriptionLanguage::Fr,
    }
}

/// Pushes the calendar picker's list + permission state - kept separate
/// from `populate()` since it needs a real EventKit query
/// (`souffle_lib::calendar::list_calendars`), not just `AppSettings`, and is
/// only ever loaded when `calendar_integration_enabled` is already on
/// (mirrors `loadCalendars()`'s own guard in controller.svelte.ts).
pub fn populate_calendars(
    window: &MainWindow,
    calendars: &[CalendarInfo],
    selected_ids: &[String],
    permission: PermState,
) {
    window.set_settings_calendar_permission(perm_state_to_slint(permission));

    let mut sorted: Vec<&CalendarInfo> = calendars.iter().collect();
    sorted.sort_by(|a, b| {
        a.source_title
            .as_deref()
            .unwrap_or("")
            .cmp(b.source_title.as_deref().unwrap_or(""))
            .then_with(|| a.title.cmp(&b.title))
    });

    let mut last_source: Option<&str> = None;
    let rows: Vec<CalendarRow> = sorted
        .into_iter()
        .map(|calendar| {
            let source = calendar.source_title.as_deref().unwrap_or("");
            let show_header = last_source != Some(source);
            last_source = Some(source);
            CalendarRow {
                id: calendar.id.as_str().into(),
                title: calendar.title.as_str().into(),
                source_title: source.into(),
                selected: selected_ids.is_empty()
                    || selected_ids.iter().any(|id| id == &calendar.id),
                show_header,
            }
        })
        .collect();
    window.set_settings_calendars(std::rc::Rc::new(slint::VecModel::from(rows)).into());
}

pub(crate) fn perm_state_to_slint(state: PermState) -> AccessState {
    match state {
        PermState::Granted => AccessState::Granted,
        PermState::Denied => AccessState::Denied,
        PermState::Unknown => AccessState::Unknown,
        PermState::Unsupported => AccessState::Unsupported,
        PermState::NoDevice => AccessState::NoDevice,
    }
}

pub fn permission_kind_to_slint(kind: PermissionKind) -> SlintPermissionKind {
    match kind {
        PermissionKind::Microphone => SlintPermissionKind::Microphone,
        PermissionKind::SystemAudio => SlintPermissionKind::SystemAudio,
        PermissionKind::Accessibility => SlintPermissionKind::Accessibility,
        PermissionKind::Calendar => SlintPermissionKind::Calendar,
    }
}

pub fn permission_kind_from_slint(kind: SlintPermissionKind) -> PermissionKind {
    match kind {
        SlintPermissionKind::Microphone => PermissionKind::Microphone,
        SlintPermissionKind::SystemAudio => PermissionKind::SystemAudio,
        SlintPermissionKind::Accessibility => PermissionKind::Accessibility,
        SlintPermissionKind::Calendar => PermissionKind::Calendar,
    }
}

/// Pushes shortcut state into the properties `interface_section.slint`
/// reads - kept separate from `populate()` since it comes from
/// `get_shortcuts`/`get_native_shortcuts`/`get_modifier_tap_status`, not
/// `AppSettings`.
pub fn populate_shortcuts(
    window: &MainWindow,
    shortcuts: &ShortcutSettings,
    native_shortcuts: &[String],
    tap_installed: Option<bool>,
) {
    let toggle_label = crate::format_shortcut_label(&shortcuts.toggle);
    window.set_settings_toggle_shortcut_label(toggle_label.as_str().into());
    // ActionHero reads the root property, not the Settings label. Keep both
    // projections in one successful-load/save path so returning home never
    // shows the startup shortcut after it has changed.
    window.set_dictation_shortcut(toggle_label.into());
    window.set_settings_ptt_shortcut_label(
        crate::format_shortcut_label(&shortcuts.push_to_talk).into(),
    );

    let native_bound =
        |value: &str| !value.is_empty() && native_shortcuts.iter().any(|n| n == value);
    let warning_visible = tap_installed == Some(false)
        && (native_bound(&shortcuts.toggle) || native_bound(&shortcuts.push_to_talk));
    window.set_settings_native_tap_warning_visible(warning_visible);
}

pub fn log_level_to_slint(value: LogLevel) -> SlintLogLevel {
    match value {
        LogLevel::Error => SlintLogLevel::Error,
        LogLevel::Warn => SlintLogLevel::Warn,
        LogLevel::Info => SlintLogLevel::Info,
        LogLevel::Debug => SlintLogLevel::Debug,
        LogLevel::Trace => SlintLogLevel::Trace,
    }
}

pub fn log_level_from_slint(value: SlintLogLevel) -> LogLevel {
    match value {
        SlintLogLevel::Error => LogLevel::Error,
        SlintLogLevel::Warn => LogLevel::Warn,
        SlintLogLevel::Info => LogLevel::Info,
        SlintLogLevel::Debug => LogLevel::Debug,
        SlintLogLevel::Trace => LogLevel::Trace,
    }
}

pub fn theme_to_slint(theme: Theme) -> AppTheme {
    match theme {
        Theme::Dark => AppTheme::Dark,
        Theme::Light => AppTheme::Light,
        Theme::System => AppTheme::System,
    }
}

pub fn theme_from_slint(theme: AppTheme) -> Theme {
    match theme {
        AppTheme::Dark => Theme::Dark,
        AppTheme::Light => Theme::Light,
        AppTheme::System => Theme::System,
    }
}

pub fn locale_to_slint(locale: &str) -> AppLocale {
    match locale {
        "fr" => AppLocale::Fr,
        "en" => AppLocale::En,
        "" => AppLocale::En,
        other => {
            eprintln!("unknown locale {other:?}, falling back to en");
            AppLocale::En
        }
    }
}

pub fn locale_from_slint(locale: AppLocale) -> &'static str {
    match locale {
        AppLocale::En => "en",
        AppLocale::Fr => "fr",
    }
}

/// Port of App.svelte's `isLightTheme` (`theme === "light" || (theme ===
/// "system" && !prefersDark)`), inverted to match `Theme.slint`'s `dark`
/// flag. `"system"` resolves against the real macOS appearance
/// (`native::appearance::is_system_dark`) rather than the browser's
/// `matchMedia` - see that function's doc comment for what "resolves"
/// means (a one-shot query, not a live OS-appearance subscription).
pub fn resolve_dark(theme: Theme) -> bool {
    match theme {
        Theme::Dark => true,
        Theme::Light => false,
        Theme::System => souffle_lib::native::appearance::is_system_dark(),
    }
}

pub fn paste_method_to_slint(method: PasteMethod) -> SlintPasteMethod {
    match method {
        PasteMethod::Clipboard => SlintPasteMethod::Clipboard,
        PasteMethod::Type => SlintPasteMethod::Typing,
        PasteMethod::Ax => SlintPasteMethod::Ax,
    }
}

pub fn paste_method_from_slint(method: SlintPasteMethod) -> PasteMethod {
    match method {
        SlintPasteMethod::Clipboard => PasteMethod::Clipboard,
        SlintPasteMethod::Typing => PasteMethod::Type,
        SlintPasteMethod::Ax => PasteMethod::Ax,
    }
}

pub fn audio_retention_to_slint(value: MeetingAudioRetention) -> SlintAudioRetention {
    match value {
        MeetingAudioRetention::Off => SlintAudioRetention::Off,
        MeetingAudioRetention::Keep7d => SlintAudioRetention::Keep7d,
        MeetingAudioRetention::Keep30d => SlintAudioRetention::Keep30d,
        MeetingAudioRetention::KeepForever => SlintAudioRetention::KeepForever,
    }
}

pub fn audio_retention_from_slint(value: SlintAudioRetention) -> MeetingAudioRetention {
    match value {
        SlintAudioRetention::Off => MeetingAudioRetention::Off,
        SlintAudioRetention::Keep7d => MeetingAudioRetention::Keep7d,
        SlintAudioRetention::Keep30d => MeetingAudioRetention::Keep30d,
        SlintAudioRetention::KeepForever => MeetingAudioRetention::KeepForever,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use slint::platform::{Platform, WindowAdapter, software_renderer::MinimalSoftwareWindow};
    use std::rc::Rc;

    struct TestPlatform;

    impl Platform for TestPlatform {
        fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
            Ok(MinimalSoftwareWindow::new(Default::default()))
        }
    }

    #[test]
    fn permission_kind_conversions_cover_every_variant() {
        let variants = [
            PermissionKind::Microphone,
            PermissionKind::SystemAudio,
            PermissionKind::Accessibility,
            PermissionKind::Calendar,
        ];

        for variant in variants {
            assert_eq!(
                permission_kind_from_slint(permission_kind_to_slint(variant)),
                variant
            );
        }
    }

    #[test]
    fn shortcut_projection_updates_settings_and_the_home_hint_together() {
        let _ = slint::platform::set_platform(Box::new(TestPlatform));
        let window = MainWindow::new().unwrap();
        let shortcuts = ShortcutSettings {
            toggle: "CommandOrControl+Shift+Space".into(),
            push_to_talk: "Alt+Space".into(),
        };

        populate_shortcuts(&window, &shortcuts, &[], Some(true));

        assert_eq!(window.get_settings_toggle_shortcut_label(), "⌘ ⇧ Space");
        assert_eq!(window.get_dictation_shortcut(), "⌘ ⇧ Space");
        assert_eq!(window.get_settings_ptt_shortcut_label(), "⌥ Space");
    }
}
