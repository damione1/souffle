//! Application bootstrap (SOU-191).
//!
//! Replaces the pre-191 split between `lib.rs::run()` (the real, Tauri-based
//! startup, used only by the now-deleted webview binary) and
//! `slint_bridge::build()` (a fake headless `tauri::App` kept alive with
//! `mem::forget` purely so `tauri::State`/`AppHandle` had something to
//! resolve against). Neither exists anymore: this is the one real bootstrap
//! path, and `AppState` is a plain `Arc`, not something behind a Tauri
//! runtime container.

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, AtomicU64};

use tracing::{info, warn};

use crate::audio::AudioCapture;
use crate::state::{AppState, AudioCommand};
use crate::{
    commands, constants, db, debug, engine, logging, permissions, pipeline, power, settings,
};

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

/// Try to become the sole running instance. Returns `false` if another
/// instance is already running (already asked to show its window) — the
/// caller must exit immediately without doing any of the startup work below.
pub fn acquire_single_instance() -> bool {
    crate::native::single_instance::acquire_or_notify_existing()
}

/// Bring up the whole application: audio thread, engine actor, database,
/// global shortcuts, tray, pill, calendar/update schedulers. Returns the
/// shared state the UI layer drives everything else through.
pub fn bootstrap() -> Arc<AppState> {
    // Must run before logging or the database open the data dir.
    constants::migrate_legacy_data_dir();
    logging::init(logging::LogLevel::Info);
    install_panic_hook();
    crate::native::notifications::request_authorization();

    let audio_rms = Arc::new(AtomicU32::new(0f32.to_bits()));
    let dropped_counter = Arc::new(AtomicU64::new(0));
    let audio_gone_reason = Arc::new(Mutex::new(None));

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

    let db_path = constants::app_data_dir().join("souffle.db");
    let database = match db::Database::open(&db_path) {
        Ok(db) => Arc::new(db),
        Err(e) => {
            tracing::error!("Fatal: Failed to open database: {e}");
            std::process::exit(1);
        }
    };
    debug::init_from_db(&database);
    match database.recover_unfinished_meetings() {
        Ok(0) => {}
        Ok(n) => info!(
            count = n,
            "Recovered unfinished meetings from a previous run"
        ),
        Err(e) => warn!("Meeting recovery failed: {e}"),
    }

    let state = Arc::new(AppState::new(cmd_tx, engine_actor, database, audio_rms));
    state.engine_actor.attach_app(Arc::clone(&state));

    // Launch Services must resolve this binary before TCC looks it up, or
    // System Settings shows the row of a previous build. Registering is not
    // a TCC request: nothing here may prompt, a permission is only ever
    // asked for on a user action.
    permissions::register_with_launch_services();

    let shortcuts = match settings::ShortcutSettings::load(&state.db) {
        Ok(shortcuts) => shortcuts,
        Err(e) => {
            warn!("Failed to load shortcuts on startup, using defaults: {e}");
            settings::ShortcutSettings::default()
        }
    };
    if let Err(e) = commands::register_shortcuts(&state, &shortcuts) {
        warn!("Failed to register shortcuts on startup: {e}");
    }
    crate::native::shortcuts::spawn_event_loop(Arc::clone(&state));

    match settings::AppSettings::load(&state.db) {
        Ok(app_settings) => {
            debug::set_transcription_debug(app_settings.debug_transcription);
            if let Err(e) = logging::set_level(app_settings.log_level) {
                warn!("Failed to apply log level: {e}");
            }
            crate::pill::restore_from_db(&state.db, app_settings.pill_hidden);
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
            // Directory walk over recordings/ can take a moment with a large
            // history; never block startup on it.
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

    // Sleep pauses an active recording cleanly instead of leaving CoreAudio
    // IO dead mid-session; wake tells the UI to offer a resume. Must run on
    // the main thread (required by NSWorkspace) — this whole function does.
    let will_sleep_state = Arc::clone(&state);
    power::install_sleep_observers(
        move || commands::handle_system_will_sleep(&will_sleep_state),
        commands::handle_system_did_wake,
    );

    #[cfg(target_os = "macos")]
    {
        use crate::audio::device_watch::{self, ChangeKind};
        use std::sync::mpsc::channel;

        let (route_tx, route_rx) = channel::<ChangeKind>();
        std::mem::forget(device_watch::start(Some(route_tx)));

        let route_db = state.db.clone();
        let route_cmd_tx = state.audio_cmd_sender.clone();
        std::thread::Builder::new()
            .name("input-route".into())
            .spawn(move || {
                commands::prime_input_route_snapshot(&route_db);
                for kind in route_rx {
                    match kind {
                        ChangeKind::DeviceList | ChangeKind::DefaultInput => {
                            if let Err(e) =
                                commands::handle_input_route_change(&route_db, &route_cmd_tx)
                            {
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

    // Native NSPanel pill (SOU-051). Owned by Swift; must run on the main
    // thread, which this function does.
    crate::pill::create_panel(&state);

    if let Err(e) = crate::tray::setup_tray(&state) {
        warn!("Failed to set up tray: {e}");
    }

    crate::calendar::scheduler::spawn(Arc::clone(&state));
    crate::update_check::scheduler::spawn(Arc::clone(&state));

    info!("Soufflé started");
    state
}
