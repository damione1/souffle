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
                } else if let Some(window) = app.get_webview_window("main") {
                    // Starting needs the main window; show the home screen.
                    let _ = window.show();
                    let _ = window.set_focus();
                    let _ = Navigate(AppView::Home).emit(app);
                }
            }
            "settings" => {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.set_focus();
                    let _ = Navigate(AppView::Settings).emit(app);
                }
            }
            "show" => {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
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
                let app = tray.app_handle();
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
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
