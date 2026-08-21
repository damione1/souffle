use tauri::image::Image;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Wry};
use tauri_specta::Event;
use tracing::{info, warn};

use crate::app_events::{AppView, MeetingStopRequested, Navigate, ShortcutToggle};
use crate::state::AppState;
use crate::state_machine::AppStateMachine;

const TRAY_ID: &str = "tray";

/// Menu items whose labels change with the recording state and locale.
struct TrayHandles {
    dictation: MenuItem<Wry>,
    meeting: MenuItem<Wry>,
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
        ("settings", false) => "Settings",
        ("settings", true) => "Réglages",
        ("show", false) => "Show Window",
        ("show", true) => "Afficher la fenêtre",
        ("quit", false) => "Quit",
        ("quit", true) => "Quitter",
        _ => "",
    }
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
/// windows — the pill overlay is one. Pure so a regression that gates on
/// `has_visible_windows` fails a unit test instead of a dock click.
pub fn should_restore_main_on_reopen(_has_visible_windows: bool) -> bool {
    true
}

#[cfg(target_os = "macos")]
fn activate_app() {
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
    let separator = MenuItem::with_id(app, "sep", "─────────", false, None::<&str>)?;
    let settings = MenuItem::with_id(app, "settings", label("settings", fr), true, None::<&str>)?;
    let show = MenuItem::with_id(app, "show", label("show", fr), true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", label("quit", fr), true, None::<&str>)?;

    let menu = Menu::with_items(
        app,
        &[
            &toggle_dictation,
            &toggle_meeting,
            &separator,
            &settings,
            &show,
            &quit,
        ],
    )?;

    app.manage(TrayHandles {
        dictation: toggle_dictation,
        meeting: toggle_meeting,
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
/// relabels the menu. Never re-acquires the machine lock (the caller may
/// hold it) — the state is passed in.
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
        let _ = handles.meeting.set_text(label(
            if meeting {
                "stop_meeting"
            } else {
                "start_meeting"
            },
            fr,
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::should_restore_main_on_reopen;

    #[test]
    fn dock_reopen_restores_main_even_when_the_pill_counts_as_visible() {
        assert!(
            should_restore_main_on_reopen(true),
            "the overlay panel is a visible NSWindow; gating on has_visible_windows leaves the main UI stuck behind"
        );
        assert!(should_restore_main_on_reopen(false));
    }
}
