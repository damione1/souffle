use tauri::image::Image;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Wry};
use tauri_plugin_notification::NotificationExt;
use tauri_specta::Event;
use tracing::{info, warn};

use crate::app_events::{AppView, MeetingStopRequested, Navigate, ShortcutToggle};
use crate::db::dictation::DictationEntry;
use crate::state::AppState;
use crate::state_machine::AppStateMachine;

const TRAY_ID: &str = "tray";

/// Menu items whose labels change with the recording state and locale.
struct TrayHandles {
    dictation: MenuItem<Wry>,
    meeting: MenuItem<Wry>,
    copy_last_transcription: MenuItem<Wry>,
}

/// Monochrome template icon (black + alpha — macOS recolors it).
fn idle_icon() -> Image<'static> {
    Image::from_bytes(include_bytes!("../icons/tray/trayTemplate.png"))
        .expect("embedded tray icon is valid PNG")
}

/// Colored recording variant (red dot) — rendered as-is, not as template.
fn recording_icon() -> Image<'static> {
    Image::from_bytes(include_bytes!("../icons/tray/tray-recording.png"))
        .expect("embedded tray icon is valid PNG")
}

fn is_french(app: &AppHandle) -> bool {
    let state = app.state::<AppState>();
    crate::settings::AppSettings::load(&state.db)
        .map(|settings| settings.locale.starts_with("fr"))
        .unwrap_or(false)
}

/// Whether "Copy Last Transcription" has anything to act on. Re-checked in
/// `sync` too, since a dictation completing does not by itself flip this
/// (see the doc comment on `sync`).
fn has_dictation_history(app: &AppHandle) -> bool {
    app.state::<AppState>()
        .db
        .count_dictation_entries()
        .map(|count| count > 0)
        .unwrap_or(false)
}

fn label(key: &str, fr: bool) -> &'static str {
    match (key, fr) {
        ("start_dictation", false) => "Start Dictation",
        ("start_dictation", true) => "Démarrer la dictée",
        ("stop_dictation", false) => "Stop Dictation",
        ("stop_dictation", true) => "Arrêter la dictée",
        ("start_meeting", false) => "Start Meeting Recording",
        ("start_meeting", true) => "Démarrer un meeting",
        ("stop_meeting", false) => "Stop Meeting Recording",
        ("stop_meeting", true) => "Arrêter le meeting",
        ("copy_last_transcription", false) => "Copy Last Transcription",
        ("copy_last_transcription", true) => "Copier la dernière transcription",
        ("settings", false) => "Settings",
        ("settings", true) => "Réglages",
        ("show", false) => "Show Window",
        ("show", true) => "Afficher la fenêtre",
        ("quit", false) => "Quit",
        ("quit", true) => "Quitter",
        _ => "",
    }
}

/// Outcome of a "Copy Last Transcription" attempt, driving both the
/// notification and (for failures) the log line. `Ok` carries nothing: the
/// success notification text does not vary.
#[derive(Debug, PartialEq)]
enum CopyOutcome {
    NoHistory,
    DbError,
    NotVerified,
}

/// Pure decision: which entry (if any) a copy attempt acts on. Split out from
/// `copy_last_transcription_to_clipboard` so the "no history" / "db error" /
/// "found an entry" branching is testable without a live AppHandle.
fn select_dictation_to_copy(
    entries: Result<Vec<DictationEntry>, String>,
) -> Result<DictationEntry, CopyOutcome> {
    match entries {
        Err(_) => Err(CopyOutcome::DbError),
        Ok(mut entries) => {
            if entries.is_empty() {
                Err(CopyOutcome::NoHistory)
            } else {
                Ok(entries.remove(0))
            }
        }
    }
}

fn copy_notification_text(
    result: &Result<(), CopyOutcome>,
    fr: bool,
) -> (&'static str, &'static str) {
    match result {
        Ok(()) => (
            if fr {
                "Copié dans le presse-papiers"
            } else {
                "Copied to clipboard"
            },
            if fr {
                "Votre dernière dictée a été copiée dans le presse-papiers."
            } else {
                "Your last dictation was copied to the clipboard."
            },
        ),
        Err(CopyOutcome::NoHistory) => (
            if fr {
                "Rien à copier"
            } else {
                "Nothing to copy"
            },
            if fr {
                "Aucune dictée n'a encore été enregistrée."
            } else {
                "No dictation has been recorded yet."
            },
        ),
        Err(CopyOutcome::DbError) => (
            if fr {
                "Échec de la copie"
            } else {
                "Copy failed"
            },
            if fr {
                "Impossible de lire l'historique des dictées."
            } else {
                "Could not read your dictation history."
            },
        ),
        Err(CopyOutcome::NotVerified) => (
            if fr {
                "Échec de la copie"
            } else {
                "Copy failed"
            },
            if fr {
                "Le presse-papiers n'a pas été mis à jour. Réessayez."
            } else {
                "The clipboard did not update. Please try again."
            },
        ),
    }
}

fn notify_copy_result(app: &AppHandle, fr: bool, result: &Result<(), CopyOutcome>) {
    let (title, body) = copy_notification_text(result, fr);
    if let Err(e) = app.notification().builder().title(title).body(body).show() {
        warn!("Copy last transcription notification failed: {e}");
    }
}

/// Copy the newest dictation to the clipboard (tray "Copy Last
/// Transcription"). Dictations only, meeting transcripts are out of scope,
/// per the ticket. Always notifies: a tray action with no visible result is
/// confusing, and staying silent on failure defeats the point of a feature
/// whose whole purpose is to recover from a bad paste (SOU-010) without
/// opening the app.
fn copy_last_transcription_to_clipboard(app: &AppHandle) {
    let fr = is_french(app);
    let entries = app.state::<AppState>().db.list_dictation_entries(1);
    if let Err(e) = &entries {
        warn!("Copy last transcription: dictation history read failed: {e}");
    }

    let result = select_dictation_to_copy(entries).and_then(|entry| {
        crate::clipboard::copy_text(&entry.text).map_err(|e| {
            warn!("Copy last transcription: clipboard write failed: {e}");
            CopyOutcome::NotVerified
        })
    });

    if result.is_ok() {
        info!("Copied last dictation to clipboard via tray");
    }
    notify_copy_result(app, fr, &result);
}

/// Bring the main window to the front. The recording pill is a visible
/// NSPanel on every Space while dictating (and AppKit can keep counting it
/// after `orderOut`), so a dock click's `has_visible_windows` is not a
/// reliable stand-in for "the user can see Soufflé". Always restore `main`.
pub fn show_main_window(app: &AppHandle) {
    let Some(window) = app.get_webview_window("main") else {
        warn!("Main window is gone; cannot bring Soufflé to the front");
        return;
    };
    // Unhide the app first (⌘H / close-to-hide), then the window. `set_focus`
    // alone will not switch Spaces onto a hidden/miniaturized window, and
    // it will not win against a non-activating overlay panel.
    #[cfg(target_os = "macos")]
    {
        let _ = app.show();
        activate_app();
    }
    let _ = window.unminimize();
    let _ = window.show();
    let _ = window.set_focus();
}

/// Dock reopen must restore `main` even when AppKit reports other visible
/// windows — the native pill NSPanel is one. Pure so a regression that gates
/// on `has_visible_windows` fails a unit test instead of a dock click.
pub fn should_restore_main_on_reopen(_has_visible_windows: bool) -> bool {
    true
}

#[cfg(target_os = "macos")]
pub(crate) fn activate_app() {
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSApplication;

    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let app = NSApplication::sharedApplication(mtm);
    app.unhide(None);
    // Cooperative `activate` is a no-op when another app is frontmost, which
    // is exactly the "stuck in the background" case. The deprecated API is
    // still the one that actually steals focus.
    #[allow(deprecated)]
    app.activateIgnoringOtherApps(true);
}

/// Set up the system tray with menu items
pub fn setup_tray(app: &AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    let fr = is_french(app);

    let toggle_dictation = MenuItem::with_id(
        app,
        "toggle_dictation",
        label("start_dictation", fr),
        true,
        None::<&str>,
    )?;
    let toggle_meeting = MenuItem::with_id(
        app,
        "toggle_meeting",
        label("start_meeting", fr),
        true,
        None::<&str>,
    )?;
    let copy_last_transcription = MenuItem::with_id(
        app,
        "copy_last_transcription",
        label("copy_last_transcription", fr),
        has_dictation_history(app),
        None::<&str>,
    )?;
    let separator = MenuItem::with_id(app, "sep", "─────────", false, None::<&str>)?;
    let settings = MenuItem::with_id(app, "settings", label("settings", fr), true, None::<&str>)?;
    let show = MenuItem::with_id(app, "show", label("show", fr), true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", label("quit", fr), true, None::<&str>)?;

    let menu = Menu::with_items(
        app,
        &[
            &toggle_dictation,
            &toggle_meeting,
            &copy_last_transcription,
            &separator,
            &settings,
            &show,
            &quit,
        ],
    )?;

    app.manage(TrayHandles {
        dictation: toggle_dictation,
        meeting: toggle_meeting,
        copy_last_transcription,
    });

    TrayIconBuilder::with_id(TRAY_ID)
        .menu(&menu)
        .tooltip("Soufflé")
        .icon(idle_icon())
        .icon_as_template(true)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "toggle_dictation" => {
                // Emit same event as keyboard shortcut — frontend handles full pipeline
                let _ = ShortcutToggle.emit(app);
                info!("Dictation toggle via tray");
            }
            "toggle_meeting" => {
                let recording_meeting = app
                    .state::<AppState>()
                    .current_machine_state()
                    .map(|machine| matches!(machine, AppStateMachine::RecordingMeeting { .. }))
                    .unwrap_or(false);
                if recording_meeting {
                    let _ = MeetingStopRequested.emit(app);
                    info!("Meeting stop via tray");
                } else {
                    // Starting needs the main window; show the home screen.
                    show_main_window(app);
                    let _ = Navigate(AppView::Home).emit(app);
                }
            }
            "copy_last_transcription" => {
                copy_last_transcription_to_clipboard(app);
            }
            "settings" => {
                show_main_window(app);
                let _ = Navigate(AppView::Settings).emit(app);
            }
            "show" => {
                show_main_window(app);
            }
            "quit" => {
                info!("Quit requested from tray");
                app.exit(0);
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main_window(tray.app_handle());
            }
        })
        .build(app)?;

    info!("System tray initialized");
    Ok(())
}

/// Reflect the machine state in the menu bar: recording shows the red-dot
/// icon and Stop labels. Also called after settings save so a locale change
/// relabels the menu, and after `add_dictation_entry` so "Copy Last
/// Transcription" enables itself as soon as the entry is actually in the
/// database. The state-machine transition back to Idle fires before that
/// write happens (the frontend saves history only after the stop command
/// resolves), so relying on it alone would leave the item disabled until
/// some unrelated later sync. Never re-acquires the machine lock (the caller
/// may hold it) — the state is passed in.
pub fn sync(app: &AppHandle, machine: &AppStateMachine) {
    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        return;
    };
    let fr = is_french(app);

    let dictating = matches!(machine, AppStateMachine::RecordingDictation { .. });
    let meeting = matches!(machine, AppStateMachine::RecordingMeeting { .. });

    let result = if dictating || meeting {
        tray.set_icon(Some(recording_icon()))
            .and_then(|()| tray.set_icon_as_template(false))
    } else {
        tray.set_icon(Some(idle_icon()))
            .and_then(|()| tray.set_icon_as_template(true))
    };
    if let Err(e) = result {
        warn!("Tray icon sync failed: {e}");
    }

    if let Some(handles) = app.try_state::<TrayHandles>() {
        let _ = handles.dictation.set_text(label(
            if dictating {
                "stop_dictation"
            } else {
                "start_dictation"
            },
            fr,
        ));
        // A meeting owns the recording session; this item must not offer to
        // start dictation on top of it (SOU-044).
        let _ = handles.dictation.set_enabled(!meeting);
        let _ = handles.meeting.set_text(label(
            if meeting {
                "stop_meeting"
            } else {
                "start_meeting"
            },
            fr,
        ));
        let _ = handles
            .copy_last_transcription
            .set_text(label("copy_last_transcription", fr));
        let _ = handles
            .copy_last_transcription
            .set_enabled(has_dictation_history(app));
    }
}

#[cfg(test)]
mod tests {
    use super::{
        copy_notification_text, label, select_dictation_to_copy, should_restore_main_on_reopen,
        CopyOutcome, DictationEntry,
    };

    #[test]
    fn dock_reopen_restores_main_even_when_the_pill_counts_as_visible() {
        assert!(
            should_restore_main_on_reopen(true),
            "the overlay panel is a visible NSWindow; gating on has_visible_windows leaves the main UI stuck behind"
        );
        assert!(should_restore_main_on_reopen(false));
    }

    fn entry(text: &str) -> DictationEntry {
        DictationEntry {
            id: "d1".to_string(),
            text: text.to_string(),
            timestamp: "2024-01-01T00:00:00Z".to_string(),
        }
    }

    #[test]
    fn no_dictation_history_selects_the_no_history_outcome() {
        assert_eq!(
            select_dictation_to_copy(Ok(vec![])),
            Err(CopyOutcome::NoHistory)
        );
    }

    #[test]
    fn a_database_error_selects_the_db_error_outcome() {
        assert_eq!(
            select_dictation_to_copy(Err("disk full".to_string())),
            Err(CopyOutcome::DbError)
        );
    }

    #[test]
    fn the_newest_entry_is_selected_when_present() {
        let selected = select_dictation_to_copy(Ok(vec![entry("hello"), entry("older")]))
            .expect("an entry was found");
        assert_eq!(selected.text, "hello");
    }

    #[test]
    fn copy_menu_label_matches_locale() {
        assert_eq!(
            label("copy_last_transcription", false),
            "Copy Last Transcription"
        );
        assert_eq!(
            label("copy_last_transcription", true),
            "Copier la dernière transcription"
        );
    }

    #[test]
    fn copy_notification_text_covers_every_outcome_in_both_locales() {
        let cases: [(Result<(), CopyOutcome>, bool, &str, &str); 8] = [
            (
                Ok(()),
                false,
                "Copied to clipboard",
                "Your last dictation was copied to the clipboard.",
            ),
            (
                Ok(()),
                true,
                "Copié dans le presse-papiers",
                "Votre dernière dictée a été copiée dans le presse-papiers.",
            ),
            (
                Err(CopyOutcome::NoHistory),
                false,
                "Nothing to copy",
                "No dictation has been recorded yet.",
            ),
            (
                Err(CopyOutcome::NoHistory),
                true,
                "Rien à copier",
                "Aucune dictée n'a encore été enregistrée.",
            ),
            (
                Err(CopyOutcome::DbError),
                false,
                "Copy failed",
                "Could not read your dictation history.",
            ),
            (
                Err(CopyOutcome::DbError),
                true,
                "Échec de la copie",
                "Impossible de lire l'historique des dictées.",
            ),
            (
                Err(CopyOutcome::NotVerified),
                false,
                "Copy failed",
                "The clipboard did not update. Please try again.",
            ),
            (
                Err(CopyOutcome::NotVerified),
                true,
                "Échec de la copie",
                "Le presse-papiers n'a pas été mis à jour. Réessayez.",
            ),
        ];
        for (result, fr, title, body) in cases {
            assert_eq!(copy_notification_text(&result, fr), (title, body));
        }
    }
}
