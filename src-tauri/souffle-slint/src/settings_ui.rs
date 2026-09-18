//! SOU-188: Settings shell. Mirrors `SettingsView.svelte` + its 17 sections,
//! one tab/section at a time as each milestone lands (see the ticket
//! journal for which). `AppSettings` is the same contract the real backend
//! (and the Svelte UI) already reads/writes - this never redeclares bounds
//! or option lists locally (AC3): `SettingsOptions::current()` is the single
//! source for those once a section needs them.

use crate::{CalendarRow, MainWindow};
use souffle_lib::calendar::CalendarInfo;
use souffle_lib::logging::LogLevel;
use souffle_lib::permissions::PermState;
use souffle_lib::settings::{
    AppSettings, MeetingTranscriptionLanguage, PasteMethod, SettingsOptions, ShortcutSettings,
    Theme,
};

/// Pushes `settings` into the Slint properties this shell currently wires.
/// Mirrors `controller.svelte.ts` setting `app.settings` after
/// `getSettings()`. Grows with each milestone; the "Système" and
/// "Interface" tabs' fields are populated so far.
pub fn populate(window: &MainWindow, settings: &AppSettings) {
    window.set_settings_autostart_enabled(settings.autostart_enabled);
    window.set_settings_debug_transcription(settings.debug_transcription);
    window.set_settings_log_level(settings.log_level.as_str().into());

    window.set_settings_theme(theme_to_str(&settings.theme).into());
    window.set_settings_locale(settings.locale.as_str().into());
    window.set_settings_auto_paste(settings.auto_paste);
    window.set_settings_paste_method(paste_method_to_str(&settings.paste_method).into());
    window.set_settings_paste_delay_ms(settings.paste_delay_ms as i32);
    window.set_settings_pill_hidden(settings.pill_hidden);
    window.set_settings_feedback_sounds_enabled(settings.feedback_sounds_enabled);
    window.set_settings_feedback_sounds_volume(settings.feedback_sounds_volume as i32);

    window.set_settings_calendar_enabled(settings.calendar_integration_enabled);
    window.set_settings_calendar_autostart_enabled(settings.calendar_autostart_enabled);
    window.set_settings_calendar_reminder_minutes(settings.calendar_reminder_minutes as i32);

    window.set_settings_is_laptop(souffle_lib::commands::is_laptop());
    window.set_settings_system_audio_supported(souffle_lib::commands::get_system_audio_support());
    window.set_settings_allow_bluetooth_mic(settings.allow_bluetooth_mic);
    window.set_settings_capture_system_audio(settings.capture_system_audio);
    window.set_settings_meeting_transcription_language(
        meeting_transcription_language_to_str(&settings.meeting_transcription_language).into(),
    );
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
    window.set_settings_meeting_autostop_labels(shared_string_model(
        &crate::audio_ui::minute_labels(&bounds.meeting_autostop_minutes),
    ));
    window.set_settings_meeting_max_duration_labels(shared_string_model(
        &crate::audio_ui::minute_labels(&bounds.meeting_max_duration_minutes),
    ));
}

fn shared_string_model(values: &[String]) -> slint::ModelRc<slint::SharedString> {
    let values: Vec<slint::SharedString> = values.iter().map(|v| v.as_str().into()).collect();
    std::rc::Rc::new(slint::VecModel::from(values)).into()
}

fn meeting_transcription_language_to_str(value: &MeetingTranscriptionLanguage) -> &'static str {
    match value {
        MeetingTranscriptionLanguage::Auto => "auto",
        MeetingTranscriptionLanguage::En => "en",
        MeetingTranscriptionLanguage::Fr => "fr",
    }
}

pub fn meeting_transcription_language_from_str(value: &str) -> MeetingTranscriptionLanguage {
    match value {
        "en" => MeetingTranscriptionLanguage::En,
        "fr" => MeetingTranscriptionLanguage::Fr,
        _ => MeetingTranscriptionLanguage::Auto,
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
    window.set_settings_calendar_permission(perm_state_to_str(permission).into());

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

fn perm_state_to_str(state: PermState) -> &'static str {
    match state {
        PermState::Granted => "granted",
        PermState::Denied => "denied",
        PermState::Unknown => "unknown",
        PermState::Unsupported => "unsupported",
        PermState::NoDevice => "unknown",
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
    window
        .set_settings_toggle_shortcut_label(crate::format_shortcut_label(&shortcuts.toggle).into());
    window.set_settings_ptt_shortcut_label(
        crate::format_shortcut_label(&shortcuts.push_to_talk).into(),
    );

    let native_bound =
        |value: &str| !value.is_empty() && native_shortcuts.iter().any(|n| n == value);
    let warning_visible = tap_installed == Some(false)
        && (native_bound(&shortcuts.toggle) || native_bound(&shortcuts.push_to_talk));
    window.set_settings_native_tap_warning_visible(warning_visible);
}

/// Inverse of `LogLevel::as_str()` - the combobox in `diagnostics_section.slint`
/// only ever sends back one of these five values.
pub fn log_level_from_str(value: &str) -> LogLevel {
    match value {
        "error" => LogLevel::Error,
        "warn" => LogLevel::Warn,
        "debug" => LogLevel::Debug,
        "trace" => LogLevel::Trace,
        _ => LogLevel::Info,
    }
}

fn theme_to_str(theme: &Theme) -> &'static str {
    match theme {
        Theme::Dark => "dark",
        Theme::Light => "light",
        Theme::System => "system",
    }
}

pub fn theme_from_str(value: &str) -> Theme {
    match value {
        "light" => Theme::Light,
        "system" => Theme::System,
        _ => Theme::Dark,
    }
}

fn paste_method_to_str(method: &PasteMethod) -> &'static str {
    match method {
        PasteMethod::Clipboard => "clipboard",
        PasteMethod::Type => "type",
        PasteMethod::Ax => "ax",
    }
}

pub fn paste_method_from_str(value: &str) -> PasteMethod {
    match value {
        "type" => PasteMethod::Type,
        "ax" => PasteMethod::Ax,
        _ => PasteMethod::Clipboard,
    }
}
