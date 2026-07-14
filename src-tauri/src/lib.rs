pub mod app_events;
pub mod archive;
pub mod audio;
pub mod calendar;
pub mod cli;
pub mod clipboard;
pub mod commands;
pub mod constants;
pub mod db;
pub mod debug;
pub mod diagnostics;
pub mod engine;
pub mod errors;
pub mod export;
pub mod filter;
pub mod lid;
pub mod lock_ext;
pub mod logging;
pub mod models;
pub mod apple_intelligence;
pub mod summary;
pub mod ort_runtime;
pub mod permissions;
pub mod pill;
pub mod pipeline;
pub mod platform;
pub mod power;
pub mod settings;
pub mod state;
pub mod state_machine;
pub mod thread_qos;
pub mod transcript;
pub mod tray;
pub mod update_check;

#[cfg(test)]
pub mod test_helpers;

use std::sync::Arc;

use audio::AudioCapture;
use audio::system_activity::SystemAudioActivity;
use state::AppState;
use tauri::Manager;
use tauri_specta::{Builder, collect_commands, collect_events};
use tracing::info;

/// Default shortcut strings
pub const DEFAULT_TOGGLE_SHORTCUT: &str = "CommandOrControl+Shift+Space";

/// Initialize logging — delegates to `logging` module.
fn init_logging() {
    logging::init(logging::LogLevel::Info);
}

/// Log every panic (thread, message, location) before it unwinds. The macOS
/// crash report only says "abort() called" with no Rust context, so without
/// this a pipeline-thread panic is undiagnosable. Chains to the default hook.
fn install_panic_hook() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current();
        let name = thread.name().unwrap_or("<unnamed>");
        let location = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "<unknown location>".to_string());
        let message = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "<non-string panic payload>".to_string());
        tracing::error!(thread = name, location = %location, "PANIC: {message}");
        default(info);
    }));
}

fn specta_builder() -> Builder<tauri::Wry> {
    Builder::<tauri::Wry>::new()
        .commands(collect_commands![
            commands::get_transcription_catalog,
            commands::get_model_status,
            commands::download_model,
            commands::delete_model,
            commands::load_model,
            commands::start_transcription,
            commands::stop_transcription,
            commands::list_audio_devices,
            commands::select_audio_device,
            commands::is_laptop,
            commands::test_transcribe_wav,
            commands::paste_text,
            commands::start_meeting_recording,
            commands::resume_meeting_recording,
            commands::stop_meeting_recording,
            commands::take_sleep_paused_meeting,
            commands::list_meetings,
            commands::get_meeting,
            commands::delete_meeting,
            commands::get_meeting_audio,
            commands::rename_meeting,
            commands::save_meeting_notes,
            commands::save_edited_transcript,
            commands::apply_live_paragraph_edit,
            commands::add_session_correction,
            commands::export_meeting_preview,
            commands::export_meeting_filename,
            commands::export_meeting_to_file,
            commands::check_summary_providers,
            commands::summarize_meeting,
            commands::search_text,
            commands::list_dictation_entries,
            commands::add_dictation_entry,
            commands::delete_dictation_entry,
            commands::clear_dictation_history,
            commands::polish_dictation,
            commands::pill_hold,
            commands::pill_release,
            commands::pill_resize,
            commands::get_settings,
            commands::save_settings,
            commands::save_shortcuts,
            commands::get_shortcuts,
            commands::get_system_audio_support,
            commands::debug_record_system_audio,
            commands::get_machine_state,
            commands::recover_state,
            commands::list_dictionary,
            commands::add_dictionary_entry,
            commands::update_dictionary_entry,
            commands::delete_dictionary_entry,
            commands::clear_dictionary,
            commands::get_permission_status,
            commands::request_permission,
            commands::repair_accessibility_permission,
            commands::list_calendars,
            commands::list_todays_calendar_events,
            commands::export_archive,
            commands::get_data_stats,
            commands::reveal_data_dir,
            commands::get_mcp_setup_info,
            commands::test_mcp_connection,
            commands::get_log_tail,
            commands::get_diagnostics_bundle,
            commands::get_diagnostics_text,
            commands::check_for_updates,
            commands::get_release_notes_for_version,
            commands::get_app_version,
            commands::open_release_page,
        ])
        .events(collect_events![
            app_events::Navigate,
            app_events::ShortcutToggle,
            app_events::ShortcutPttStart,
            app_events::ShortcutPttStop,
            app_events::StateChanged,
            app_events::TranscriptionHealth,
            app_events::PipelineError,
            app_events::SystemAudioStatus,
            app_events::AudioLevel,
            app_events::MeetingStopRequested,
            app_events::MeetingFinalized,
            app_events::UpcomingMeeting,
            app_events::MeetingIdle,
            app_events::SystemWokeUp,
            app_events::ArchiveExportProgress,
            app_events::PillHoldChanged,
            app_events::DictationLiveText,
        ])
}

pub fn run() {
    // Must run before logging or the database open the data dir.
    constants::migrate_legacy_data_dir();
    init_logging();
    install_panic_hook();

    // Create the shared audio RMS level
    let audio_rms = Arc::new(std::sync::atomic::AtomicU32::new(0f32.to_bits()));
    let system_audio_activity = Arc::new(SystemAudioActivity::default());
    // Chunks dropped by the capture callback — read by the actor for health reporting
    let dropped_counter = Arc::new(std::sync::atomic::AtomicU64::new(0));
    // Reason the capture thread sets right before it exits after an
    // unrecoverable failure (e.g. mic loss with no other source); read by
    // the actor's AudioGone handler.
    let audio_gone_reason = Arc::new(std::sync::Mutex::new(None));

    // Spawn the audio thread before Tauri starts (cpal Stream is !Send on macOS)
    let (cmd_tx, audio_rx) = match AudioCapture::spawn(
        Arc::clone(&audio_rms),
        Arc::clone(&dropped_counter),
        Arc::clone(&audio_gone_reason),
    ) {
        Ok(channels) => channels,
        Err(e) => {
            tracing::error!("Fatal: {e}");
            std::process::exit(1);
        }
    };

    // Spawn the engine actor — the single thread that owns the transcription
    // engine and consumes captured audio.
    let engine_actor = match pipeline::EngineActorHandle::spawn(
        audio_rx,
        dropped_counter,
        audio_gone_reason,
        Box::new(engine::create_engine),
    ) {
        Ok(actor) => Arc::new(actor),
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

    // Finalize any meeting left mid-recording by a previous crash, so the
    // incrementally-persisted segments show up cleanly in history.
    match database.recover_unfinished_meetings() {
        Ok(0) => {}
        Ok(n) => info!(
            count = n,
            "Recovered unfinished meetings from a previous run"
        ),
        Err(e) => tracing::warn!("Meeting recovery failed: {e}"),
    }

    let specta = specta_builder();

    tauri::Builder::default()
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_dialog::init())
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
        .manage(AppState::new(
            cmd_tx,
            engine_actor,
            database,
            audio_rms,
            system_audio_activity,
        ))
        .invoke_handler(specta.invoke_handler())
        .setup(move |app| {
            specta.mount_events(app);

            // Store the AppHandle so state transitions can emit events
            let state = app.state::<AppState>();
            {
                let mut handle_guard = state
                    .app_handle
                    .lock()
                    .map_err(|e| format!("Lock poisoned: {e}"))?;
                *handle_guard = Some(app.handle().clone());
            }
            state.engine_actor.attach_app(app.handle().clone());
            let _ = state
                .audio_cmd_sender
                .send(state::AudioCommand::AttachApp(app.handle().clone()));

            // Load shortcut settings from DB and register
            let state = app.state::<AppState>();
            let shortcuts = match settings::ShortcutSettings::load(&state.db) {
                Ok(shortcuts) => shortcuts,
                Err(e) => {
                    tracing::warn!("Failed to load shortcuts on startup, using defaults: {e}");
                    settings::ShortcutSettings::default()
                }
            };

            if let Err(e) = commands::register_shortcuts(app.handle(), &shortcuts) {
                tracing::warn!("Failed to register shortcuts on startup: {e}");
            }

            match settings::AppSettings::load(&state.db) {
                Ok(app_settings) => {
                    debug::set_transcription_debug(app_settings.debug_transcription);
                    if let Err(e) = logging::set_level(app_settings.log_level) {
                        tracing::warn!("Failed to apply log level: {e}");
                    }
                    state
                        .engine_actor
                        .set_unload_timeout(app_settings.model_unload_timeout_minutes);
                    let _ = state.audio_cmd_sender.send(state::AudioCommand::SetClamshellDevice(
                        app_settings.clamshell_audio_device,
                    ));
                    // Directory walk over recordings/ can take a moment with
                    // a large history; never block startup on it.
                    let retention = app_settings.meeting_audio_retention;
                    std::thread::spawn(move || {
                        audio::retention::sweep_expired_recordings(retention, std::time::SystemTime::now());
                    });
                }
                Err(e) => {
                    tracing::warn!("Failed to load settings on startup: {e}");
                }
            }

            // Sleep pauses an active recording cleanly instead of leaving
            // CoreAudio IO dead mid-session; wake tells the frontend to offer
            // a resume. Registered here (main thread, required by NSWorkspace)
            // rather than in AudioCapture::spawn's dedicated thread.
            let will_sleep_app = app.handle().clone();
            let did_wake_app = app.handle().clone();
            power::install_sleep_observers(
                move || commands::handle_system_will_sleep(&will_sleep_app),
                move || commands::handle_system_did_wake(&did_wake_app),
            );

            // Diagnostic device-change logging for the Bluetooth headset /
            // input-priority work. No natural long-lived owner here, so the
            // handle is deliberately leaked to keep the listeners alive for
            // the app's lifetime, same as the sleep observer tokens above.
            #[cfg(target_os = "macos")]
            std::mem::forget(audio::device_watch::start());

            tray::setup_tray(app.handle())?;
            calendar::scheduler::spawn(app.handle().clone());
            info!("Souffle started");
            Ok(())
        })
        .build(tauri::generate_context!())
        .unwrap_or_else(|e| {
            tracing::error!("Tauri build error: {e}");
            std::process::exit(1);
        })
        .run(|app, event| {
            if let tauri::RunEvent::ExitRequested { .. } = event {
                // Shut down the engine actor before process exit. It unloads
                // and drops the engine on its own thread — whisper.cpp's Metal
                // residency sets and Candle's Metal objects must be freed
                // before the Metal device is destroyed, otherwise C++ static
                // destructor order causes a ggml_metal_rsets_free SIGABRT.
                let state = app.state::<AppState>();
                if let Err(e) = state.engine_actor.shutdown() {
                    tracing::warn!("Engine actor shutdown failed: {e}");
                }
            }
        });
}

#[cfg(test)]
mod tests {
    use super::specta_builder;
    use specta_typescript::{BigIntExportBehavior, Typescript};

    #[test]
    fn export_typescript_bindings() {
        let builder = specta_builder();
        builder
            .export(
                Typescript::default().bigint(BigIntExportBehavior::Number),
                "../src/lib/types/generated.ts",
            )
            .expect("Failed to export TypeScript bindings");
    }
}
