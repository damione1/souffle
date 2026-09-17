//! Native replacement for Tauri's `tray-icon` feature (SOU-191).
//!
//! `tray-icon` + `muda` are the same crates Tauri's tray feature wraps
//! internally (`tauri::tray::TrayIconBuilder` / `tauri::menu::Menu` are thin
//! forwarders over these), used directly instead of through Tauri's wrapper.

use std::sync::{Arc, OnceLock};

use tracing::{info, warn};

use crate::db::dictation::DictationEntry;
use crate::native::bridge::{self, AppView, NativeAction};
use crate::state::AppState;
use crate::state_machine::AppStateMachine;
use tray_icon::menu::{Menu, MenuEvent, MenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder, TrayIconEvent};

/// Menu items whose labels change with the recording state and locale.
struct TrayHandles {
    tray: TrayIcon,
    dictation: MenuItem,
    meeting: MenuItem,
    copy_last_transcription: MenuItem,
    pause_ptt: MenuItem,
}

// SAFETY: only ever touched from the main thread (setup, and the tray/menu
// event loop threads which immediately hop back via a plain function call —
// no cross-thread mutation of the AppKit objects themselves happens here).
unsafe impl Send for TrayHandles {}
unsafe impl Sync for TrayHandles {}

static TRAY: OnceLock<TrayHandles> = OnceLock::new();

fn decode_icon(bytes: &[u8]) -> Icon {
    let img = image::load_from_memory(bytes)
        .expect("embedded tray icon is valid PNG")
        .into_rgba8();
    let (width, height) = img.dimensions();
    Icon::from_rgba(img.into_raw(), width, height).expect("valid RGBA icon buffer")
}

/// Monochrome template icon (black + alpha — macOS recolors it).
fn idle_icon() -> Icon {
    decode_icon(include_bytes!("../icons/tray/trayTemplate.png"))
}

/// Colored recording variant (red dot) — rendered as-is, not as template.
fn recording_icon() -> Icon {
    decode_icon(include_bytes!("../icons/tray/tray-recording.png"))
}

fn is_french(state: &AppState) -> bool {
    crate::settings::AppSettings::load(&state.db)
        .map(|settings| settings.locale.starts_with("fr"))
        .unwrap_or(false)
}

/// Whether "Copy Last Transcription" has anything to act on. Re-checked in
/// `sync` too, since a dictation completing does not by itself flip this
/// (see the doc comment on `sync`).
fn has_dictation_history(state: &AppState) -> bool {
    state
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
        ("pause_1h", false) => "Pause Shortcut (1h)",
        ("pause_1h", true) => "Pause raccourci (1 h)",
        ("resume_ptt", false) => "Resume Shortcut",
        ("resume_ptt", true) => "Reprendre",
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
/// "found an entry" branching is testable without a live tray.
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

fn notify_copy_result(fr: bool, result: &Result<(), CopyOutcome>) {
    let (title, body) = copy_notification_text(result, fr);
    crate::native::notifications::notify(title, body);
}

/// Copy the newest dictation to the clipboard (tray "Copy Last
/// Transcription"). Dictations only, meeting transcripts are out of scope,
/// per the ticket. Always notifies: a tray action with no visible result is
/// confusing, and staying silent on failure defeats the point of a feature
/// whose whole purpose is to recover from a bad paste (SOU-010) without
/// opening the app.
fn copy_last_transcription_to_clipboard(state: &AppState) {
    let fr = is_french(state);
    let entries = state.db.list_dictation_entries(1);
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
    notify_copy_result(fr, &result);
}

/// Bring the main window to the front: activate the app locally, then ask
/// `souffle-slint` (which owns the actual Slint window) to show it.
pub fn show_main_window() {
    #[cfg(target_os = "macos")]
    activate_app();
    bridge::dispatch(NativeAction::ShowMainWindow);
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

/// Set up the system tray with menu items.
pub fn setup_tray(state: &Arc<AppState>) -> Result<(), Box<dyn std::error::Error>> {
    let fr = is_french(state);

    let toggle_dictation =
        MenuItem::with_id("toggle_dictation", label("start_dictation", fr), true, None);
    let toggle_meeting =
        MenuItem::with_id("toggle_meeting", label("start_meeting", fr), true, None);
    let copy_last_transcription = MenuItem::with_id(
        "copy_last_transcription",
        label("copy_last_transcription", fr),
        has_dictation_history(state),
        None,
    );
    let is_paused = state.ptt_is_paused();
    let pause_ptt = MenuItem::with_id(
        "pause_ptt",
        label(if is_paused { "resume_ptt" } else { "pause_1h" }, fr),
        true,
        None,
    );
    let settings = MenuItem::with_id("settings", label("settings", fr), true, None);
    let show = MenuItem::with_id("show", label("show", fr), true, None);
    let quit = MenuItem::with_id("quit", label("quit", fr), true, None);

    let menu = Menu::new();
    menu.append(&toggle_dictation)?;
    menu.append(&toggle_meeting)?;
    menu.append(&copy_last_transcription)?;
    menu.append(&pause_ptt)?;
    menu.append(&tray_icon::menu::PredefinedMenuItem::separator())?;
    menu.append(&settings)?;
    menu.append(&show)?;
    menu.append(&quit)?;

    let tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("Soufflé")
        .with_icon(idle_icon())
        .with_icon_as_template(true)
        .build()?;

    TRAY.set(TrayHandles {
        tray,
        dictation: toggle_dictation,
        meeting: toggle_meeting,
        copy_last_transcription,
        pause_ptt,
    })
    .ok();

    spawn_event_loops(Arc::clone(state));

    info!("System tray initialized");
    Ok(())
}

fn spawn_event_loops(state: Arc<AppState>) {
    let menu_state = Arc::clone(&state);
    std::thread::Builder::new()
        .name("tray-menu-events".into())
        .spawn(move || {
            for event in MenuEvent::receiver() {
                handle_menu_event(&menu_state, event.id.as_ref());
            }
        })
        .expect("failed to spawn tray menu event loop");

    std::thread::Builder::new()
        .name("tray-icon-events".into())
        .spawn(move || {
            for event in TrayIconEvent::receiver() {
                if let TrayIconEvent::Click {
                    button: tray_icon::MouseButton::Left,
                    button_state: tray_icon::MouseButtonState::Up,
                    ..
                } = event
                {
                    show_main_window();
                }
            }
        })
        .expect("failed to spawn tray icon event loop");
}

fn handle_menu_event(state: &Arc<AppState>, id: &str) {
    match id {
        "toggle_dictation" => {
            bridge::dispatch(NativeAction::ToggleDictation);
            info!("Dictation toggle via tray");
        }
        "toggle_meeting" => {
            let recording_meeting = state
                .current_machine_state()
                .map(|machine| matches!(machine, AppStateMachine::RecordingMeeting { .. }))
                .unwrap_or(false);
            if recording_meeting {
                bridge::dispatch(NativeAction::StopMeeting);
                info!("Meeting stop via tray");
            } else {
                show_main_window();
                bridge::dispatch(NativeAction::Navigate(AppView::Home));
            }
        }
        "copy_last_transcription" => {
            copy_last_transcription_to_clipboard(state);
        }
        "pause_ptt" => {
            // Drop the pause mutex before current_machine_state/sync: sync()
            // re-locks ptt_paused_until, and std::sync::Mutex is not reentrant.
            state.toggle_ptt_pause();
            if let Ok(machine) = state.current_machine_state() {
                sync(state, &machine);
            }
        }
        "settings" => {
            show_main_window();
            bridge::dispatch(NativeAction::Navigate(AppView::Settings));
        }
        "show" => {
            show_main_window();
        }
        "quit" => {
            info!("Quit requested from tray");
            std::process::exit(0);
        }
        _ => {}
    }
}

/// Reflect the machine state in the menu bar: recording shows the red-dot
/// icon and Stop labels. Also called after settings save so a locale change
/// relabels the menu, and after `add_dictation_entry` so "Copy Last
/// Transcription" enables itself as soon as the entry is actually in the
/// database. The state-machine transition back to Idle fires before that
/// write happens, so relying on it alone would leave the item disabled until
/// some unrelated later sync.
/// `TrayHandles`'s AppKit objects are main-thread-only; the caller isn't
/// always on it (model download/load run in background threads), so the
/// actual work hops via `main_thread::on_main`. Safe from a `cargo test`
/// binary too: `TRAY` is only populated by the real bootstrapped app, so
/// the early return below fires before the hop ever would.
pub fn sync(state: &AppState, machine: &AppStateMachine) {
    let Some(handles) = TRAY.get() else {
        return;
    };
    let fr = is_french(state);

    let dictating = matches!(machine, AppStateMachine::RecordingDictation { .. });
    let meeting = matches!(machine, AppStateMachine::RecordingMeeting { .. });
    let has_history = has_dictation_history(state);
    let is_paused = state.ptt_is_paused();

    crate::main_thread::on_main(move || {
        let icon_result = if dictating || meeting {
            handles.tray.set_icon(Some(recording_icon()))
        } else {
            handles.tray.set_icon(Some(idle_icon()))
        };
        if let Err(e) = icon_result {
            warn!("Tray icon sync failed: {e}");
        }
        handles.tray.set_icon_as_template(!(dictating || meeting));

        handles.dictation.set_text(label(
            if dictating {
                "stop_dictation"
            } else {
                "start_dictation"
            },
            fr,
        ));
        // A meeting owns the recording session; this item must not offer to
        // start dictation on top of it (SOU-044).
        handles.dictation.set_enabled(!meeting);
        handles.meeting.set_text(label(
            if meeting {
                "stop_meeting"
            } else {
                "start_meeting"
            },
            fr,
        ));
        handles
            .copy_last_transcription
            .set_text(label("copy_last_transcription", fr));
        handles.copy_last_transcription.set_enabled(has_history);
        handles
            .pause_ptt
            .set_text(label(if is_paused { "resume_ptt" } else { "pause_1h" }, fr));
    });
}

#[cfg(test)]
mod tests {
    use super::{
        CopyOutcome, DictationEntry, copy_notification_text, label, select_dictation_to_copy,
    };

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
