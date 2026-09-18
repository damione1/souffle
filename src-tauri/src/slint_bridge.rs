//! Real headless Tauri bootstrap for the Slint shell (SOU-187).
//!
//! Builds a full `tauri::App` with the same `AppState` the production app
//! uses (real audio thread, engine actor, database) but zero windows: only
//! `.build()` is ever called, never `.run()`. Backend commands are plain
//! functions taking `tauri::State`/`tauri::AppHandle` (see
//! `commands::meetings::list_meetings` for example), so the Slint UI calls
//! them directly instead of going through the webview IPC layer.
//!
//! This coexistence (Tauri's runtime alive with Slint owning the actual OS
//! event loop) was verified empirically before this module was written; see
//! SOU-187's Journal and the SOU-186 spike report for the verification.
//!
//! `.setup()` in `lib.rs::run()` never fires on this path, so its
//! load-bearing steps are replayed manually in `replay_setup` below. The
//! NSPanel pill, the tray icon, and the calendar/update-check schedulers are
//! deliberately not replayed: out of SOU-187's scope (see the ticket's Hors
//! périmètre), not an oversight.

use std::sync::atomic::{AtomicU32, AtomicU64};
use std::sync::{Arc, Mutex};

use tauri::Manager;
use tracing::{info, warn};

use crate::audio::AudioCapture;
use crate::state::{AppState, AudioCommand};
use crate::{commands, constants, db, debug, engine, logging, pipeline, power, settings};

/// Builds and returns the headless `App`. The caller must keep it alive
/// (e.g. `std::mem::forget`, or hold it for the process lifetime) for its
/// `AppHandle` to keep working.
pub fn build() -> tauri::App<tauri::Wry> {
    // Must run before logging or the database open the data dir - same
    // ordering constraint as lib.rs::run().
    constants::migrate_legacy_data_dir();
    logging::init(logging::LogLevel::Info);

    let audio_rms = Arc::new(AtomicU32::new(0f32.to_bits()));
    let dropped_counter = Arc::new(AtomicU64::new(0));
    let audio_gone_reason = Arc::new(Mutex::new(None));

    let (cmd_tx, audio_rx) = AudioCapture::spawn(
        Arc::clone(&audio_rms),
        Arc::clone(&dropped_counter),
        Arc::clone(&audio_gone_reason),
    )
    .expect("failed to spawn audio capture");

    let engine_actor = Arc::new(
        pipeline::EngineActorHandle::spawn(
            audio_rx,
            dropped_counter,
            audio_gone_reason,
            Box::new(engine::create_engine),
        )
        .expect("failed to spawn engine actor"),
    );

    let db_path = constants::app_data_dir().join("souffle.db");
    let database = Arc::new(db::Database::open(&db_path).expect("failed to open database"));
    debug::init_from_db(&database);
    match database.recover_unfinished_meetings() {
        Ok(0) => {}
        Ok(n) => info!(
            count = n,
            "Recovered unfinished meetings from a previous run"
        ),
        Err(e) => warn!("Meeting recovery failed: {e}"),
    }

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .manage(AppState::new(cmd_tx, engine_actor, database, audio_rms))
        .build(crate::tauri_context())
        .expect("failed to build headless tauri app");

    // Command bodies emit tauri-specta events (StateChanged, etc.) regardless
    // of whether a webview is listening. Without this, the first such emit
    // panics: "EventRegistry not found in Tauri state" - `run()`'s `.setup()`
    // calls this too, but that closure never fires on this headless path.
    crate::specta_builder().mount_events(&app);

    replay_setup(&app);
    app
}

/// Replays the load-bearing parts of `lib.rs::run()`'s `.setup()` closure.
fn replay_setup(app: &tauri::App<tauri::Wry>) {
    let state = app.state::<AppState>();
    {
        let mut handle_guard = state.app_handle.lock().unwrap_or_else(|e| e.into_inner());
        *handle_guard = Some(app.handle().clone());
    }
    state.engine_actor.attach_app(app.handle().clone());
    let _ = state
        .audio_cmd_sender
        .send(AudioCommand::AttachApp(app.handle().clone()));

    let shortcuts = match settings::ShortcutSettings::load(&state.db) {
        Ok(shortcuts) => shortcuts,
        Err(e) => {
            warn!("Failed to load shortcuts on startup, using defaults: {e}");
            settings::ShortcutSettings::default()
        }
    };
    if let Err(e) = commands::register_shortcuts(app.handle(), &shortcuts) {
        warn!("Failed to register shortcuts on startup: {e}");
    }

    match settings::AppSettings::load(&state.db) {
        Ok(app_settings) => {
            debug::set_transcription_debug(app_settings.debug_transcription);
            if let Err(e) = logging::set_level(app_settings.log_level) {
                warn!("Failed to apply log level: {e}");
            }
            state
                .engine_actor
                .set_unload_timeout(app_settings.model_unload_timeout_minutes);
            let _ = state
                .audio_cmd_sender
                .send(AudioCommand::SetClamshellDevice(
                    app_settings.clamshell_audio_device,
                ));
            let _ = state.audio_cmd_sender.send(AudioCommand::SetInputPolicy {
                priority: app_settings.input_priority,
                allow_bluetooth_mic: app_settings.allow_bluetooth_mic,
            });
            let retention = app_settings.meeting_audio_retention;
            std::thread::spawn(move || {
                crate::audio::retention::sweep_expired_recordings(
                    retention,
                    std::time::SystemTime::now(),
                );
            });
        }
        Err(e) => warn!("Failed to load settings on startup: {e}"),
    }

    let will_sleep_app = app.handle().clone();
    let did_wake_app = app.handle().clone();
    power::install_sleep_observers(
        move || commands::handle_system_will_sleep(&will_sleep_app),
        move || commands::handle_system_did_wake(&did_wake_app),
    );

    #[cfg(target_os = "macos")]
    {
        use crate::audio::device_watch::{self, ChangeKind};
        use std::sync::mpsc::channel;

        let (route_tx, route_rx) = channel::<ChangeKind>();
        std::mem::forget(device_watch::start(Some(route_tx)));

        let route_state = app.state::<AppState>();
        let route_cmd_tx = route_state.audio_cmd_sender.clone();
        let route_db = route_state.db.clone();
        let route_app = app.handle().clone();
        std::thread::Builder::new()
            .name("input-route".into())
            .spawn(move || {
                commands::prime_input_route_snapshot(&route_db);
                for kind in route_rx {
                    match kind {
                        ChangeKind::DeviceList | ChangeKind::DefaultInput => {
                            if let Err(e) = commands::handle_input_route_change(
                                &route_db,
                                &route_cmd_tx,
                                &route_app,
                            ) {
                                warn!("Input route refresh failed: {e}");
                            }
                        }
                        ChangeKind::DefaultOutput => {}
                    }
                }
            })
            .expect("failed to spawn input-route thread");

        device_watch::destroy_orphaned_souffle_taps();
    }

    info!("Souffle (Slint) headless bridge started");
}
