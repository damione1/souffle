use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager,
};
use tracing::info;

use crate::state::{AppState, AudioCommand, RecordingMode};

/// Set up the system tray with menu items
pub fn setup_tray(app: &AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    let toggle_dictation =
        MenuItem::with_id(app, "toggle_dictation", "Start Dictation", true, None::<&str>)?;
    let toggle_meeting =
        MenuItem::with_id(app, "toggle_meeting", "Start Meeting Recording", true, None::<&str>)?;
    let separator = MenuItem::with_id(app, "sep", "─────────", false, None::<&str>)?;
    let settings = MenuItem::with_id(app, "settings", "Settings", true, None::<&str>)?;
    let show = MenuItem::with_id(app, "show", "Show Window", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;

    let menu = Menu::with_items(
        app,
        &[&toggle_dictation, &toggle_meeting, &separator, &settings, &show, &quit],
    )?;

    TrayIconBuilder::new()
        .menu(&menu)
        .tooltip("Souffle")
        .icon(app.default_window_icon().unwrap().clone())
        .icon_as_template(true)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "toggle_dictation" => {
                let state = app.state::<AppState>();
                let mut is_recording = state.is_recording.lock().unwrap();
                let mode = *state.recording_mode.lock().unwrap();

                if mode == RecordingMode::Dictation {
                    let _ = state.audio_cmd_sender.send(AudioCommand::Stop);
                    *is_recording = false;
                    *state.recording_mode.lock().unwrap() = RecordingMode::Idle;
                    let _ = app.emit("recording-stopped", ());
                    info!("Dictation stopped via tray");
                } else if !*is_recording {
                    let _ = state.audio_cmd_sender.send(AudioCommand::Start);
                    *is_recording = true;
                    *state.recording_mode.lock().unwrap() = RecordingMode::Dictation;
                    let _ = app.emit("recording-started", ());
                    info!("Dictation started via tray");
                }
            }
            "toggle_meeting" => {
                // Meeting start/stop requires the full pipeline setup,
                // so we just show the window on the recordings tab
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.set_focus();
                    let _ = app.emit("navigate", "recordings");
                }
            }
            "settings" => {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.set_focus();
                    let _ = app.emit("navigate", "settings");
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
