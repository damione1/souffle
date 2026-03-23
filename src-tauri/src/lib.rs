pub mod audio;
pub mod clipboard;
pub mod commands;
pub mod constants;
pub mod db;
pub mod debug;
pub mod engine;
pub mod errors;
pub mod lock_ext;
pub mod models;
pub mod ollama;
pub mod pipeline;
pub mod platform;
pub mod state;
pub mod transcript;
pub mod tray;

use std::sync::Arc;

use audio::AudioCapture;
use state::AppState;
use tauri::Manager;
use tracing::info;

/// Default shortcut strings
pub const DEFAULT_TOGGLE_SHORTCUT: &str = "CommandOrControl+Shift+Space";

pub fn run() {
    // Create the shared audio RMS level
    let audio_rms = Arc::new(std::sync::atomic::AtomicU32::new(0f32.to_bits()));

    // Spawn the audio thread before Tauri starts (cpal Stream is !Send on macOS)
    let (cmd_tx, audio_rx) = match AudioCapture::spawn(Arc::clone(&audio_rms)) {
        Ok(channels) => channels,
        Err(e) => {
            tracing::error!("Fatal: {e}");
            std::process::exit(1);
        }
    };

    // Initialize SQLite database
    let db_path = constants::app_data_dir().join("souffle.db");

    let database = match db::Database::open(&db_path) {
        Ok(db) => Arc::new(db),
        Err(e) => {
            tracing::error!("Fatal: Failed to open database: {e}");
            std::process::exit(1);
        }
    };
    debug::init_from_db(&database);

    tauri::Builder::default()
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(
            tauri_plugin_log::Builder::new()
                .target(tauri_plugin_log::Target::new(
                    tauri_plugin_log::TargetKind::Stderr,
                ))
                .level(log::LevelFilter::Info)
                .build(),
        )
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.set_focus();
            }
        }))
        .manage(AppState::new(cmd_tx, audio_rx, database, audio_rms))
        .invoke_handler(tauri::generate_handler![
            commands::start_recording,
            commands::stop_recording,
            commands::get_model_status,
            commands::download_model,
            commands::load_model,
            commands::start_transcription,
            commands::stop_transcription,
            commands::list_audio_devices,
            commands::select_audio_device,
            commands::test_transcribe_wav,
            commands::paste_text,
            commands::start_meeting_recording,
            commands::stop_meeting_recording,
            commands::list_meetings,
            commands::get_meeting,
            commands::delete_meeting,
            commands::check_ollama,
            commands::summarize_meeting,
            commands::list_dictation_entries,
            commands::add_dictation_entry,
            commands::delete_dictation_entry,
            commands::clear_dictation_history,
            commands::get_settings,
            commands::save_setting,
            commands::update_shortcuts,
            commands::get_shortcuts,
            commands::get_audio_level,
        ])
        .setup(|app| {
            // Load shortcut settings from DB and register
            let state = app.state::<AppState>();
            let toggle = state
                .db
                .get_setting("shortcut_toggle")
                .ok()
                .flatten()
                .and_then(|v| serde_json::from_str::<String>(&v).ok())
                .unwrap_or_else(|| DEFAULT_TOGGLE_SHORTCUT.to_string());
            let ptt = state
                .db
                .get_setting("shortcut_push_to_talk")
                .ok()
                .flatten()
                .and_then(|v| serde_json::from_str::<String>(&v).ok())
                .unwrap_or_default();

            if let Err(e) = commands::register_shortcuts(app.handle(), &toggle, &ptt) {
                tracing::warn!("Failed to register shortcuts on startup: {e}");
            }

            tray::setup_tray(app.handle())?;
            info!("Souffle started");
            Ok(())
        })
        .run(tauri::generate_context!())
        .unwrap_or_else(|e| {
            tracing::error!("Tauri runtime error: {e}");
            std::process::exit(1);
        });
}
