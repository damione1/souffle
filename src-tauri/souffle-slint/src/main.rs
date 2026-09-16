// SOU-187: standalone Slint shell backed by a real headless Tauri App
// (souffle_lib::slint_bridge) - no webview, but the real AppState (audio
// thread, engine actor, database). AC5 pattern: state and actions cross the
// Rust<->UI boundary as plain Slint properties and callbacks - direct
// in-process function calls, no IPC, no serialization.
slint::include_modules!();

mod timeline;

use souffle_lib::engine::{
    TranscriptionProfileSelection, TranscriptionRuntimePhase, TranscriptionSegment,
};
use souffle_lib::state::AppState;
use souffle_lib::transcript::{MeetingParticipant, MeetingTranscript};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;
use tauri::ipc::Channel;
use tauri::{AppHandle, Manager};

/// Notes autosave debounce, matching `NOTES_DEBOUNCE_MS` in
/// features/meeting/controller.svelte.ts.
const NOTES_DEBOUNCE: Duration = Duration::from_millis(800);

// Port of src/lib/utils/format.ts::formatShortcutLabel.
fn format_shortcut_label(shortcut: &str) -> String {
    if shortcut.is_empty() {
        return String::new();
    }
    shortcut
        .replace("CommandOrControl", "\u{2318}")
        .replace("Shift", "\u{21e7}")
        .replace("Alt", "\u{2325}")
        .replace('+', " ")
}

/// Re-fetches dictations + meetings from the real database and rebuilds the
/// Timeline model - mirrors features/timeline/controller.svelte.ts's
/// `refresh()`. Reads the current filter/search straight off the window
/// (already the source of truth via its in-out properties) rather than
/// threading them through as parameters.
fn refresh_timeline(window: &MainWindow, tauri_handle: &AppHandle) {
    let kind_filter = window.get_kind_filter();
    let search_query = window.get_search_query().to_string();

    let state = tauri_handle.state::<souffle_lib::state::AppState>();
    let dictations = match souffle_lib::commands::list_dictation_entries(state, Some(200)) {
        Ok(entries) => entries,
        Err(e) => {
            eprintln!("Failed to list dictation entries: {e}");
            Vec::new()
        }
    };
    let state = tauri_handle.state::<souffle_lib::state::AppState>();
    let meetings = match souffle_lib::commands::list_meetings(state) {
        Ok(meetings) => meetings,
        Err(e) => {
            eprintln!("Failed to list meetings: {e}");
            Vec::new()
        }
    };

    let is_empty = dictations.is_empty() && meetings.is_empty();
    let groups = timeline::build_groups(&dictations, &meetings, kind_filter, &search_query);
    window.set_timeline_has_matches(!groups.is_empty());
    window.set_timeline_groups(std::rc::Rc::new(slint::VecModel::from(groups)).into());
    window.set_timeline_is_empty(is_empty);
}

/// Port of the inline template in MeetingHeaderSection.svelte:
/// `{name}{is_organizer ? " (organisateur)" : ""}{is_current_user ? " (vous)" : ""}`.
fn participant_label(p: &MeetingParticipant) -> String {
    let mut label = p.name.clone();
    if p.is_organizer {
        label.push_str(" (organisateur)");
    }
    if p.is_current_user {
        label.push_str(" (vous)");
    }
    label
}

/// Port of the meta line in MeetingHeaderSection.svelte (date, duration,
/// segment count, session count) for the completed-meeting case only - the
/// live-recording case is out of scope here, see meeting_detail.slint.
/// `formatDate`'s `new Date(iso).toLocaleString()` is locale/OS-dependent;
/// this uses a fixed French `dd/mm/yyyy hh:mm` instead of trying to
/// replicate that, consistent with the rest of this port (day_label etc.
/// already hardcode French).
fn meta_line(meeting: &MeetingTranscript) -> String {
    let date = meeting
        .started_at
        .with_timezone(&chrono::Local)
        .format("%d/%m/%Y %H:%M");
    let duration = timeline::format_duration(meeting.duration_seconds);
    let segments = meeting.segments.len();
    let mut line = format!("{date} \u{b7} {duration} \u{b7} {segments} segments");
    let sessions = meeting.recording_sessions.len();
    if sessions > 1 {
        line.push_str(&format!(" \u{b7} {sessions} sessions"));
    }
    line
}

/// Loads a meeting and pushes it into MeetingDetail's properties - mirrors
/// `controller.svelte.ts`'s `openMeeting`/`loadMeeting` effect.
fn populate_meeting_detail(window: &MainWindow, meeting: &MeetingTranscript) {
    window.set_active_meeting_id(meeting.id.clone().into());
    window.set_meeting_detail_title(meeting.title.clone().into());
    window.set_meeting_detail_meta(meta_line(meeting).into());
    window.set_meeting_detail_model_label(meeting.transcription_profile.model_label.clone().into());
    let participants: Vec<slint::SharedString> = meeting
        .participants
        .iter()
        .map(|p| participant_label(p).into())
        .collect();
    window.set_meeting_detail_participants(
        std::rc::Rc::new(slint::VecModel::from(participants)).into(),
    );
    window.set_meeting_detail_notes(meeting.notes.clone().unwrap_or_default().into());
    window.set_meeting_detail_notes_save_state(NotesSaveState::Idle);
}

/// Mirrors `ensureModelLoaded` in transcription/runtime.ts: ready is a no-op,
/// load_required loads it, download_required is refused rather than
/// triggering a real (multi-GB, network-bound) download from here - that
/// flow belongs to SOU-190 (onboarding/dialogs), not this ticket.
async fn ensure_model_ready(handle: &AppHandle) -> Result<(), String> {
    let selection = TranscriptionProfileSelection::default();
    let state = handle.state::<AppState>();
    let status = souffle_lib::commands::get_model_status(state, selection)?;
    match status.phase {
        TranscriptionRuntimePhase::Ready => Ok(()),
        TranscriptionRuntimePhase::LoadRequired => {
            let handle = handle.clone();
            tauri::async_runtime::spawn_blocking(move || {
                let state = handle.state::<AppState>();
                souffle_lib::commands::load_model(state, TranscriptionProfileSelection::default())
            })
            .await
            .map_err(|e| format!("Join load_model task: {e}"))?
        }
        TranscriptionRuntimePhase::DownloadRequired => {
            Err("Modèle non téléchargé - ouvrez Réglages pour le télécharger.".into())
        }
    }
}

/// Pushes each transcribed segment into the window's `live-text`/
/// `live-tentative` properties. Runs on the engine-actor thread, not the
/// Slint main thread, so every update is marshaled via
/// `invoke_from_event_loop` - the same reasoning as `run_on_main_thread`,
/// just fire-and-forget instead of awaited. Milestone 5 scope: a single
/// running text block for both dictation and meetings, not the full
/// paragraph-grouped/speaker-lane rendering LiveSessionCard.svelte does -
/// that's real, separate work (windowing, speaker lanes, inline edit),
/// deliberately deferred and noted here rather than half-built.
fn live_segment_channel(weak: slint::Weak<MainWindow>) -> Channel<TranscriptionSegment> {
    Channel::new(move |body| {
        // Channel::send serializes via serde_json regardless of the type
        // parameter (see IpcResponse's blanket impl) - the callback always
        // receives the raw IPC body, typed or not.
        let tauri::ipc::InvokeResponseBody::Json(json) = body else {
            return Ok(());
        };
        let Ok(segment) = serde_json::from_str::<TranscriptionSegment>(&json) else {
            return Ok(());
        };
        let weak = weak.clone();
        let _ = slint::invoke_from_event_loop(move || {
            let Some(window) = weak.upgrade() else {
                return;
            };
            if segment.is_final {
                let mut text = window.get_live_text().to_string();
                let trimmed = segment.text.trim();
                if !trimmed.is_empty() {
                    if !text.is_empty() {
                        text.push(' ');
                    }
                    text.push_str(trimmed);
                }
                window.set_live_text(text.into());
                window.set_live_tentative("".into());
            } else {
                window.set_live_tentative(segment.text.into());
            }
        });
        Ok(())
    })
}

/// Runs `f` on the real Slint/OS main thread and returns its result.
///
/// Found the hard way (milestone 4): `start_transcription` internally calls
/// `dictation_cancel::sync`, which touches `app.global_shortcut()` to arm the
/// Escape-cancels-dictation binding. That registration needs a thread with a
/// live run loop; a background tokio worker thread (where a plain
/// `tauri::async_runtime::spawn`ed task runs) has none, and the call hangs
/// forever - confirmed by bisecting with temporary eprintln!s, not guessed.
/// The real OS main thread (Slint's own, via `window.run()`) does have one.
/// Model loading (the slow part, seconds) stays off-thread via
/// `ensure_model_ready`'s `spawn_blocking`; only the fast (tens to hundreds
/// of ms once the model is loaded) plugin-touching command call itself needs
/// this - a brief, acceptable main-thread pause, not a multi-second freeze.
async fn run_on_main_thread<F, T>(f: F) -> T
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    let (tx, rx) = tokio::sync::oneshot::channel();
    slint::invoke_from_event_loop(move || {
        let _ = tx.send(f());
    })
    .expect("Slint event loop is gone");
    rx.await.expect("main-thread task dropped its result")
}

async fn start_dictation(handle: AppHandle, weak: slint::Weak<MainWindow>) -> Result<(), String> {
    ensure_model_ready(&handle).await?;
    run_on_main_thread(move || {
        tauri::async_runtime::block_on(async move {
            let state = handle.state::<AppState>();
            souffle_lib::commands::start_transcription(state, live_segment_channel(weak), true)
                .await
        })
    })
    .await
}

async fn start_meeting(handle: AppHandle, weak: slint::Weak<MainWindow>) -> Result<(), String> {
    ensure_model_ready(&handle).await?;
    // Mirrors defaultMeetingTitle() in meeting/controller.svelte.ts.
    let title = format!("Meeting {}", default_meeting_date());
    run_on_main_thread(move || {
        tauri::async_runtime::block_on(async move {
            let state = handle.state::<AppState>();
            souffle_lib::commands::start_meeting_recording(
                state,
                title,
                None,
                live_segment_channel(weak),
            )
            .await
        })
    })
    .await
}

/// `M/D/YYYY`, matching `new Date().toLocaleDateString()`'s en-US default
/// used by `defaultMeetingTitle()`.
fn default_meeting_date() -> String {
    let local = chrono::Local::now();
    format!(
        "{}/{}/{}",
        local.format("%-m"),
        local.format("%-d"),
        local.format("%Y")
    )
}

async fn stop_dictation(handle: AppHandle) -> Result<(), String> {
    run_on_main_thread(move || {
        tauri::async_runtime::block_on(async move {
            let state = handle.state::<AppState>();
            souffle_lib::commands::stop_transcription(state).await
        })
    })
    .await
}

async fn stop_meeting(handle: AppHandle) -> Result<(), String> {
    run_on_main_thread(move || {
        tauri::async_runtime::block_on(async move {
            let state = handle.state::<AppState>();
            souffle_lib::commands::stop_meeting_recording(state).await
        })
    })
    .await
    .map(|_meeting_id| ())
}

/// Milestone 3 wires the real Timeline (this function); milestones 4-6 wire
/// dictate/meeting start and opening a meeting's detail. Until then those
/// two callbacks only log - no fabricated state change on click.
///
/// Plain `eprintln!`, not `tracing`: this dev shell's own diagnostics are
/// unrelated to the production log file/filter (`SOUFFLE_LOG`, scoped to the
/// `souffle` lib crate's own targets), which was the wrong tool here and
/// silently swallowed these lines during milestone 2 verification.
fn wire_callbacks(window: &MainWindow, tauri_handle: AppHandle) {
    let weak = window.as_weak();
    let handle = tauri_handle.clone();
    window.on_dictate_requested(move || {
        let weak = weak.clone();
        let handle = handle.clone();
        tauri::async_runtime::spawn(async move {
            let result = start_dictation(handle, weak.clone()).await;
            if let Err(e) = weak.upgrade_in_event_loop(move |window| match result {
                Ok(()) => {
                    window.set_live_text("".into());
                    window.set_live_tentative("".into());
                    window.set_recording_mode(RecordingMode::Dictation);
                }
                Err(e) => window.set_transcription_status_message(e.into()),
            }) {
                eprintln!("upgrade_in_event_loop failed (dictate): {e}");
            }
        });
    });

    let weak = window.as_weak();
    let handle = tauri_handle.clone();
    window.on_meeting_requested(move || {
        let weak = weak.clone();
        let handle = handle.clone();
        tauri::async_runtime::spawn(async move {
            let result = start_meeting(handle, weak.clone()).await;
            if let Err(e) = weak.upgrade_in_event_loop(move |window| match result {
                Ok(()) => {
                    window.set_live_text("".into());
                    window.set_live_tentative("".into());
                    window.set_recording_mode(RecordingMode::Meeting);
                }
                Err(e) => window.set_meeting_status_message(e.into()),
            }) {
                eprintln!("upgrade_in_event_loop failed (meeting): {e}");
            }
        });
    });

    let weak = window.as_weak();
    let handle = tauri_handle.clone();
    window.on_stop_requested(move || {
        let Some(current) = weak.upgrade() else {
            return;
        };
        let mode = current.get_recording_mode();
        let weak = weak.clone();
        let handle = handle.clone();
        tauri::async_runtime::spawn(async move {
            let handle_for_refresh = handle.clone();
            let result = if mode == RecordingMode::Meeting {
                stop_meeting(handle).await
            } else {
                stop_dictation(handle).await
            };
            if let Err(e) = weak.upgrade_in_event_loop(move |window| {
                if let Err(e) = result {
                    eprintln!("Failed to stop {mode:?}: {e}");
                }
                window.set_recording_mode(RecordingMode::Idle);
                window.set_live_text("".into());
                window.set_live_tentative("".into());
                refresh_timeline(&window, &handle_for_refresh);
            }) {
                eprintln!("upgrade_in_event_loop failed (stop): {e}");
            }
        });
    });

    let weak = window.as_weak();
    let handle = tauri_handle.clone();
    window.on_filter_changed(move |kind| {
        if let Some(window) = weak.upgrade() {
            window.set_kind_filter(kind);
            refresh_timeline(&window, &handle);
        }
    });

    let weak = window.as_weak();
    let handle = tauri_handle.clone();
    window.on_search_changed(move |_query| {
        if let Some(window) = weak.upgrade() {
            refresh_timeline(&window, &handle);
        }
    });

    let weak = window.as_weak();
    let handle = tauri_handle.clone();
    window.on_timeline_item_opened(move |kind, id| {
        if kind != TimelineKind::Meeting {
            // Dictation inline-expand has no Slint equivalent yet - real
            // action, just not ported.
            eprintln!("timeline-item-opened: dictation {id} (inline-expand not wired yet)");
            return;
        }
        let Some(window) = weak.upgrade() else {
            return;
        };
        let state = handle.state::<AppState>();
        match souffle_lib::commands::get_meeting(state, id.to_string()) {
            Ok(meeting) => populate_meeting_detail(&window, &meeting),
            Err(e) => eprintln!("Failed to load meeting {id}: {e}"),
        }
    });

    let weak = window.as_weak();
    window.on_meeting_detail_back(move || {
        if let Some(window) = weak.upgrade() {
            window.set_active_meeting_id("".into());
        }
    });

    let weak = window.as_weak();
    let handle = tauri_handle.clone();
    window.on_meeting_detail_rename(move |new_title| {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let id = window.get_active_meeting_id().to_string();
        let state = handle.state::<AppState>();
        match souffle_lib::commands::rename_meeting(state, id, new_title.to_string()) {
            Ok(()) => {
                window.set_meeting_detail_title(new_title);
                refresh_timeline(&window, &handle);
            }
            Err(e) => eprintln!("Failed to rename meeting: {e}"),
        }
    });

    // Debounced autosave: each edit restarts the timer, dropping the
    // previous one (a live `slint::Timer` cancels on drop) - mirrors
    // `onNotesChange`/`flushNotes` in controller.svelte.ts.
    let notes_timer: Rc<RefCell<Option<slint::Timer>>> = Rc::new(RefCell::new(None));
    let weak = window.as_weak();
    let handle = tauri_handle.clone();
    window.on_meeting_detail_notes_changed(move |value| {
        let Some(window) = weak.upgrade() else {
            return;
        };
        window.set_meeting_detail_notes_save_state(NotesSaveState::Pending);
        let meeting_id = window.get_active_meeting_id().to_string();
        let value = value.to_string();
        let handle = handle.clone();
        let weak = weak.clone();
        let timer = slint::Timer::default();
        timer.start(slint::TimerMode::SingleShot, NOTES_DEBOUNCE, move || {
            let state = handle.state::<AppState>();
            let result = souffle_lib::commands::save_meeting_notes(
                state,
                meeting_id.clone(),
                Some(value.clone()),
            );
            if let Some(window) = weak.upgrade() {
                match result {
                    Ok(()) => window.set_meeting_detail_notes_save_state(NotesSaveState::Saved),
                    Err(e) => {
                        eprintln!("Failed to save meeting notes: {e}");
                        window.set_meeting_detail_notes_save_state(NotesSaveState::Idle);
                    }
                }
            }
        });
        *notes_timer.borrow_mut() = Some(timer);
    });

    let weak = window.as_weak();
    let handle = tauri_handle.clone();
    window.on_timeline_item_removed(move |kind, id| {
        let state = handle.state::<souffle_lib::state::AppState>();
        let result = if kind == TimelineKind::Dictation {
            souffle_lib::commands::delete_dictation_entry(state, id.to_string())
        } else {
            souffle_lib::commands::delete_meeting(state, id.to_string())
        };
        if let Err(e) = result {
            eprintln!("Failed to delete {kind:?} {id}: {e}");
        }
        if let Some(window) = weak.upgrade() {
            refresh_timeline(&window, &handle);
        }
    });

    let weak = window.as_weak();
    window.on_dismiss_transcription_status(move || {
        if let Some(window) = weak.upgrade() {
            window.set_transcription_status_message("".into());
        }
    });
    let weak = window.as_weak();
    window.on_dismiss_meeting_status(move || {
        if let Some(window) = weak.upgrade() {
            window.set_meeting_status_message("".into());
        }
    });
}

fn main() {
    // Real bootstrap (audio thread, engine actor, DB, replayed .setup()) -
    // see slint_bridge.rs. Must run before Slint's own window/event loop.
    let tauri_app = souffle_lib::slint_bridge::build();
    let tauri_handle = tauri_app.handle().clone();
    // Never call .run() on this App; it must simply stay alive so the
    // AppHandle above keeps working while Slint owns the OS event loop.
    std::mem::forget(tauri_app);

    let window = MainWindow::new().expect("failed to create Slint window");

    // Real shortcut setting, not a placeholder - mirrors HomeView.svelte's
    // onMount getShortcuts() call.
    let state = tauri_handle.state::<souffle_lib::state::AppState>();
    match souffle_lib::commands::get_shortcuts(state) {
        Ok(shortcuts) => {
            window.set_dictation_shortcut(format_shortcut_label(&shortcuts.toggle).into());
        }
        Err(e) => eprintln!("Failed to load shortcuts: {e}"),
    }

    refresh_timeline(&window, &tauri_handle);
    wire_callbacks(&window, tauri_handle);

    window.run().expect("event loop failed");
}
