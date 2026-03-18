pub mod audio;
pub mod commands;
pub mod engine;
pub mod errors;
pub mod state;
pub mod tray;

use audio::AudioCapture;
use state::{AppState, AudioCommand};
use tauri::{Emitter, Manager};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};
use tracing::info;

pub fn run() {
    // Spawn the audio thread before Tauri starts (cpal Stream is !Send on macOS)
    let (cmd_tx, audio_rx) = AudioCapture::spawn();

    let dictation_shortcut = Shortcut::new(Some(Modifiers::SUPER | Modifiers::SHIFT), Code::Space);

    tauri::Builder::default()
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(move |app, shortcut, event| {
                    if shortcut == &dictation_shortcut && event.state() == ShortcutState::Pressed {
                        let state = app.state::<AppState>();
                        let mut is_recording = state.is_recording.lock().unwrap();

                        if *is_recording {
                            let _ = state.audio_cmd_sender.send(AudioCommand::Stop);
                            *is_recording = false;
                            info!("Dictation stopped via hotkey");
                            // Emit event to frontend
                            let _ = app.emit("recording-stopped", ());
                        } else {
                            let _ = state.audio_cmd_sender.send(AudioCommand::Start);
                            *is_recording = true;
                            info!("Dictation started via hotkey");
                            let _ = app.emit("recording-started", ());
                        }
                    }
                })
                .build(),
        )
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_store::Builder::default().build())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_log::Builder::new().build())
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.set_focus();
            }
        }))
        .manage(AppState::new(cmd_tx, audio_rx))
        .invoke_handler(tauri::generate_handler![
            commands::start_recording,
            commands::stop_recording,
        ])
        .setup(|app| {
            // Register global hotkey: Cmd+Shift+Space for dictation toggle
            let shortcut =
                Shortcut::new(Some(Modifiers::SUPER | Modifiers::SHIFT), Code::Space);
            app.global_shortcut().register(shortcut)?;
            info!("Global shortcut registered: Cmd+Shift+Space");

            tray::setup_tray(app.handle())?;
            info!("Souffle started");
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Souffle");
}
