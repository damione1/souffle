// SOU-187: standalone Slint shell backed by a real headless Tauri App
// (souffle_lib::slint_bridge) - no webview, but the real AppState (audio
// thread, engine actor, database). AC5 pattern: state and actions cross the
// Rust<->UI boundary as plain Slint properties and callbacks - direct
// in-process function calls, no IPC, no serialization.
slint::include_modules!();

mod audio_player;
mod audio_ui;
mod data_ui;
mod ia_ui;
mod lists_ui;
mod markdown;
mod microphone_list;
mod model_ui;
mod onboarding_flags;
mod onboarding_ui;
mod settings_ui;
mod shortcut_capture;
mod summary;
mod timeline;
mod transcript;

use slint::Model;
use souffle_lib::audio::AudioInputDevice;
use souffle_lib::engine::{
    TranscriptionProfileSelection, TranscriptionRuntimePhase, TranscriptionSegment,
};
use souffle_lib::permissions::{PermState, PermissionKind};
use souffle_lib::settings::{AppSettings, ShortcutSettings};
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

/// Matches `TranscriptSection`'s fixed `height: 260px` Rectangle - the
/// windowing math below only needs to be approximately right (it pads with
/// `TRANSCRIPT_SCROLL_MARGIN` on each side), not pixel-exact.
const TRANSCRIPT_VIEWPORT_HEIGHT: f32 = 260.0;
const TRANSCRIPT_SCROLL_MARGIN: f32 = 3.0 * TRANSCRIPT_VIEWPORT_HEIGHT;
/// How often to re-check the transcript's scroll position and possibly
/// mount a different slice (SOU-187 milestone 8b, AC15). Same idea as
/// `start_audio_progress_timer`'s 150ms poll below, just faster since
/// scrolling is more time-sensitive than a playback position label.
const TRANSCRIPT_SCROLL_POLL: Duration = Duration::from_millis(80);

/// Matches `DiagnosticsSettingsSection.svelte`'s `TAIL_LINES`/`POLL_MS`.
const SETTINGS_LOG_TAIL_LINES: u32 = 80;
const SETTINGS_LOG_POLL: Duration = Duration::from_millis(2000);

/// Backing data for the virtualized transcript list: the full block list
/// plus precomputed cumulative height estimates (see
/// `transcript::compute_offsets`). `MeetingDetail`'s `transcript-blocks`
/// property only ever holds the current visible slice of this, never the
/// whole thing - that's the actual virtualization.
struct TranscriptState {
    blocks: Vec<TranscriptBlock>,
    offsets: Vec<f32>,
    mounted_start: usize,
    mounted_end: usize,
}

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

/// Shared by the diagnostics and MCP snippet "Copy" buttons - no webview
/// clipboard API to fall back on in this headless shell (see the `arboard`
/// dependency comment in Cargo.toml).
fn copy_to_clipboard(text: &str) {
    match arboard::Clipboard::new() {
        Ok(mut clipboard) => {
            if let Err(e) = clipboard.set_text(text) {
                eprintln!("Failed to write to clipboard: {e}");
            }
        }
        Err(e) => eprintln!("Failed to open clipboard: {e}"),
    }
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
    window.set_meeting_detail_transcript_segment_count(meeting.segments.len() as i32);

    let summary_text = meeting.summary.clone().unwrap_or_default();
    let key_points: Vec<slint::SharedString> = summary::extract_key_points(&summary_text)
        .into_iter()
        .map(Into::into)
        .collect();
    window.set_meeting_detail_summary(summary_text.into());
    window.set_meeting_detail_summary_is_stale(meeting.summary_is_stale);
    window.set_meeting_detail_summary_model_label(
        meeting.summary_model.clone().unwrap_or_default().into(),
    );
    window.set_meeting_detail_summary_key_points(
        std::rc::Rc::new(slint::VecModel::from(key_points)).into(),
    );
    let structured = meeting.structured_summary.clone().unwrap_or_default();
    let decisions: Vec<slint::SharedString> =
        structured.decisions.into_iter().map(Into::into).collect();
    let action_items: Vec<StructuredActionItem> = structured
        .action_items
        .into_iter()
        .map(|item| StructuredActionItem {
            text: item.text.into(),
            owner: item.owner.unwrap_or_default().into(),
        })
        .collect();
    let open_questions: Vec<slint::SharedString> = structured
        .open_questions
        .into_iter()
        .map(Into::into)
        .collect();
    window.set_meeting_detail_summary_decisions(
        std::rc::Rc::new(slint::VecModel::from(decisions)).into(),
    );
    window.set_meeting_detail_summary_action_items(
        std::rc::Rc::new(slint::VecModel::from(action_items)).into(),
    );
    window.set_meeting_detail_summary_open_questions(
        std::rc::Rc::new(slint::VecModel::from(open_questions)).into(),
    );
}

/// Sets `transcript_state` to `meeting`'s full block list + offsets, mounts
/// the initial (scroll-top) slice, and (re)starts the scroll-poll timer
/// that keeps the mounted slice matched to `scroll-top` while the meeting
/// is open. Separate from `populate_meeting_detail` because it owns
/// mutable shared state the header/notes population doesn't need.
fn load_meeting_transcript_window(
    window: &MainWindow,
    meeting: &MeetingTranscript,
    transcript_state: &Rc<RefCell<Option<TranscriptState>>>,
    transcript_timer: &Rc<RefCell<Option<slint::Timer>>>,
    weak: slint::Weak<MainWindow>,
) {
    let blocks =
        transcript::build_transcript_blocks(&meeting.segments, &meeting.recording_sessions);
    let offsets = transcript::compute_offsets(&blocks);
    *transcript_state.borrow_mut() = Some(TranscriptState {
        blocks,
        offsets,
        mounted_start: usize::MAX,
        mounted_end: usize::MAX,
    });
    window.invoke_reset_meeting_detail_transcript_scroll();
    update_transcript_window(window, transcript_state, 0.0);
    *transcript_timer.borrow_mut() = Some(start_transcript_scroll_timer(
        weak,
        transcript_state.clone(),
    ));
}

/// Recomputes which slice of `transcript_state`'s blocks should be mounted
/// for `scroll_top` (px scrolled down from the top) and pushes it into
/// `MeetingDetail`'s properties, but only when the slice actually changed -
/// rebuilding the Slint model on every poll tick even while stationary
/// would be wasted work.
fn update_transcript_window(
    window: &MainWindow,
    transcript_state: &Rc<RefCell<Option<TranscriptState>>>,
    scroll_top: f32,
) {
    let mut guard = transcript_state.borrow_mut();
    let Some(state) = guard.as_mut() else {
        return;
    };
    let win = transcript::visible_window(
        &state.offsets,
        scroll_top,
        TRANSCRIPT_VIEWPORT_HEIGHT,
        TRANSCRIPT_SCROLL_MARGIN,
    );
    if win.start == state.mounted_start && win.end == state.mounted_end {
        return;
    }
    state.mounted_start = win.start;
    state.mounted_end = win.end;
    let slice = state.blocks[win.start..win.end].to_vec();
    let mounted = slice.len();
    let total = state.blocks.len();
    window.set_meeting_detail_transcript_blocks(
        std::rc::Rc::new(slint::VecModel::from(slice)).into(),
    );
    window.set_meeting_detail_transcript_spacer_before(win.spacer_before);
    window.set_meeting_detail_transcript_spacer_after(win.spacer_after);
    // AC15's "active node counter" evidence: this stays small and bounded
    // even for a meeting with thousands of paragraphs - see
    // `transcript::tests::visible_window_slices_a_huge_transcript_to_a_bounded_count`
    // for the automated version of this same claim.
    eprintln!("transcript window: {mounted}/{total} blocks mounted (SOU-187 AC15)");
}

/// Repeatedly (80ms) reads the real Flickable scroll position out of
/// `MeetingDetail` and re-windows the transcript if it moved - same
/// Rust-polls-Slint pattern as `start_audio_progress_timer` below, needed
/// because Slint's expression language can't do the offset/binary-search
/// math `visible_window` does.
fn start_transcript_scroll_timer(
    weak: slint::Weak<MainWindow>,
    transcript_state: Rc<RefCell<Option<TranscriptState>>>,
) -> slint::Timer {
    let timer = slint::Timer::default();
    timer.start(
        slint::TimerMode::Repeated,
        TRANSCRIPT_SCROLL_POLL,
        move || {
            let Some(window) = weak.upgrade() else {
                return;
            };
            // Flickable's viewport-y (and its `-px` mirror) is negative-going-
            // down; scroll_top here is the usual positive "distance scrolled
            // from the top".
            let scroll_top = -window.get_meeting_detail_transcript_scroll_top_px();
            update_transcript_window(&window, &transcript_state, scroll_top);
        },
    );
    timer
}

/// Drops the transcript window state/timer - mirrors `stop_audio_player`.
/// Called before loading a different meeting's transcript and when leaving
/// MeetingDetail, so a stale huge block list never lingers in memory and
/// the poll timer never fires against a slice that no longer applies.
fn stop_transcript_window(
    transcript_state: &Rc<RefCell<Option<TranscriptState>>>,
    transcript_timer: &Rc<RefCell<Option<slint::Timer>>>,
) {
    *transcript_timer.borrow_mut() = None;
    *transcript_state.borrow_mut() = None;
}

/// Stops and drops any currently loaded audio player/progress timer -
/// dropping `AudioPlayer` stops its `cpal` stream. Called before loading a
/// different meeting's audio and when leaving MeetingDetail, so switching
/// meetings never leaves a stale stream playing in the background.
fn stop_audio_player(
    player: &Rc<RefCell<Option<audio_player::AudioPlayer>>>,
    progress_timer: &Rc<RefCell<Option<slint::Timer>>>,
) {
    *progress_timer.borrow_mut() = None;
    *player.borrow_mut() = None;
}

/// Repeatedly (150ms) reflects the loaded player's real position/playing
/// state into the window's properties - mirrors the `<audio>` element's
/// `timeupdate` event that `MeetingAudioPlayerSection.svelte` listens to.
/// Left running for as long as MeetingDetail with audio is open (not just
/// while playing): simpler and safe (no self-drop from inside its own
/// callback) at the cost of a harmless no-op tick every 150ms while paused.
fn start_audio_progress_timer(
    weak: slint::Weak<MainWindow>,
    player: Rc<RefCell<Option<audio_player::AudioPlayer>>>,
) -> slint::Timer {
    let timer = slint::Timer::default();
    timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(150),
        move || {
            let Some(window) = weak.upgrade() else {
                return;
            };
            let guard = player.borrow();
            let Some(p) = guard.as_ref() else {
                return;
            };
            window.set_meeting_detail_audio_progress(p.progress());
            window.set_meeting_detail_audio_position_label(
                timeline::format_duration(p.position_seconds()).into(),
            );
            window.set_meeting_detail_audio_is_playing(p.is_playing());
        },
    );
    timer
}

/// Re-fetches the log tail and pushes it into the Settings window - mirrors
/// `DiagnosticsSettingsSection.svelte`'s `refreshTail()`.
fn refresh_settings_log_tail(window: &MainWindow) {
    match souffle_lib::commands::get_log_tail(SETTINGS_LOG_TAIL_LINES) {
        Ok(tail) => window.set_settings_log_tail(tail.into()),
        Err(e) => eprintln!("Failed to read log tail: {e}"),
    }
}

/// Polls the log tail every 2s while Settings is open - mirrors the Svelte
/// section's `setInterval`. Stopped on `settings-closed` (see
/// `stop_settings_log_timer`), not left running once the sheet is gone.
fn start_settings_log_timer(weak: slint::Weak<MainWindow>) -> slint::Timer {
    let timer = slint::Timer::default();
    timer.start(slint::TimerMode::Repeated, SETTINGS_LOG_POLL, move || {
        let Some(window) = weak.upgrade() else {
            return;
        };
        refresh_settings_log_tail(&window);
    });
    timer
}

/// Populates the calendar picker without prompting - mirrors
/// `loadCalendars()`'s own guard (only queries EventKit when the
/// integration is already on, which implies access was granted before).
fn load_calendars_if_enabled(
    window: &MainWindow,
    settings_state: &Rc<RefCell<Option<AppSettings>>>,
) {
    let (enabled, selected_ids) = {
        let guard = settings_state.borrow();
        match guard.as_ref() {
            Some(settings) => (
                settings.calendar_integration_enabled,
                settings.calendar_selected_ids.clone(),
            ),
            None => return,
        }
    };
    if !enabled {
        settings_ui::populate_calendars(window, &[], &[], PermState::Unknown);
        return;
    }
    match souffle_lib::calendar::list_calendars() {
        Ok(calendars) => {
            settings_ui::populate_calendars(window, &calendars, &selected_ids, PermState::Granted)
        }
        Err(_) => settings_ui::populate_calendars(window, &[], &selected_ids, PermState::Denied),
    }
}

/// Refreshes the audio device list, both device pickers, and the
/// microphone priority list, then kicks off an async sample-rate fetch for
/// whichever device `resolve_sample_rate_device_uid` picks - mirrors
/// `refreshDevices()` + the `$effect` that calls `refreshInputSampleRate()`
/// on device/selection change in controller.svelte.ts.
/// Runs the real `check_summary_providers()` network/availability check and
/// repopulates the whole IA tab from the result - shared by settings-open
/// and the "Retry" button, since both need the same round trip. `slint::
/// spawn_local`, not `tauri::async_runtime::spawn`: this closes over
/// `Rc<RefCell<..>>` state, which is not `Send`.
fn refresh_summary_providers(
    weak: slint::Weak<MainWindow>,
    handle: AppHandle,
    settings_state: Rc<RefCell<Option<AppSettings>>>,
    summary_status_state: Rc<RefCell<Option<souffle_lib::summary::SummaryProvidersStatus>>>,
    summary_template_editing: Rc<RefCell<String>>,
) {
    slint::spawn_local(async move {
        let state = handle.state::<AppState>();
        let status = souffle_lib::commands::check_summary_providers(state).await;
        let Some(window) = weak.upgrade() else {
            return;
        };
        let Ok(status) = status else {
            return;
        };
        let guard = settings_state.borrow();
        let Some(settings) = guard.as_ref() else {
            return;
        };
        ia_ui::populate_intelligence(&window, settings, &status);
        let provider_available = window.get_settings_summary_unusable_message().is_empty();
        ia_ui::populate_dictation_polish(&window, settings, provider_available);
        let editing_id = summary_template_editing.borrow().clone();
        ia_ui::populate_summary_templates(&window, settings, &editing_id);
        drop(guard);
        *summary_status_state.borrow_mut() = Some(status);
    })
    .expect("slint event loop not running");
}

/// Pushes the model picker's option list + current status - AC2's own
/// state (download/load) is read via `get_model_status`, never
/// reimplemented; this only formats what it returns.
fn load_transcription_model_state(
    window: &MainWindow,
    handle: &AppHandle,
    settings_state: &Rc<RefCell<Option<AppSettings>>>,
    model_options_state: &Rc<RefCell<Vec<model_ui::FlatModelOption>>>,
) {
    let state = handle.state::<AppState>();
    let catalog = match souffle_lib::commands::get_transcription_catalog(state) {
        Ok(catalog) => catalog,
        Err(e) => {
            eprintln!("Failed to load transcription catalog: {e}");
            return;
        }
    };
    let unload_timeout_minutes = settings_state
        .borrow()
        .as_ref()
        .map(|s| s.model_unload_timeout_minutes)
        .unwrap_or(0);
    let unload_timeout_options =
        souffle_lib::settings::SettingsOptions::current().model_unload_timeout_minutes;
    model_ui::populate_options(
        window,
        &catalog,
        unload_timeout_minutes,
        &unload_timeout_options,
    );
    *model_options_state.borrow_mut() = model_ui::list_available_model_options(&catalog);

    let selection = TranscriptionProfileSelection {
        engine_id: catalog.selected_engine_id.clone(),
        model_id: catalog.selected_model_id.clone(),
        backend_id: catalog.selected_backend_id.clone(),
    };
    let state = handle.state::<AppState>();
    match souffle_lib::commands::get_model_status(state, selection) {
        Ok(status) => {
            window.set_settings_model_status_text(model_ui::phase_label(status.phase).into());
            window.set_settings_model_downloading(false);
            window.set_settings_model_error_message("".into());
        }
        Err(e) => window.set_settings_model_error_message(e.into()),
    }
}

fn load_audio_devices(
    window: &MainWindow,
    settings_state: &Rc<RefCell<Option<AppSettings>>>,
    audio_devices_state: &Rc<RefCell<Vec<AudioInputDevice>>>,
) {
    let devices = souffle_lib::commands::list_audio_devices().unwrap_or_else(|e| {
        eprintln!("Failed to list audio devices: {e}");
        Vec::new()
    });
    let (selected, clamshell, priority) = {
        let guard = settings_state.borrow();
        match guard.as_ref() {
            Some(settings) => (
                settings.audio_device.clone().unwrap_or_default(),
                settings.clamshell_audio_device.clone(),
                settings.input_priority.clone(),
            ),
            None => return,
        }
    };
    audio_ui::populate_device_pickers(window, &devices, &selected, clamshell.as_deref());
    let list = microphone_list::build_microphone_list(&devices, &priority);
    audio_ui::populate_microphones(window, &list);

    let rate_uid = audio_ui::resolve_sample_rate_device_uid(&selected, &devices).map(String::from);
    *audio_devices_state.borrow_mut() = devices;

    window.set_settings_sample_rate_label("".into());
    window.set_settings_sample_rate_high(false);
    if let Some(uid) = rate_uid {
        let weak = window.as_weak();
        slint::spawn_local(async move {
            if let Ok(hz) = souffle_lib::commands::get_input_sample_rate(uid).await
                && let Some(window) = weak.upgrade()
            {
                window.set_settings_sample_rate_label(audio_ui::format_sample_rate_hz(hz).into());
                window.set_settings_sample_rate_high(audio_ui::sample_rate_blocks_conferencing(hz));
            }
        })
        .expect("slint event loop not running");
    }
}

/// Loads (decodes + opens a paused output stream for) the first recorded
/// session of `meeting_id`, if any - mirrors `getMeetingAudio` populating
/// `MeetingAudioPlayerSection`. Multiple recording sessions (a meeting
/// resumed after being stopped) are real but out of scope here: only the
/// first session plays, same honest v1 boundary as the rest of this
/// milestone, not silently wrong for the common single-session case.
fn load_meeting_audio(
    window: &MainWindow,
    meeting_id: &str,
    player: &Rc<RefCell<Option<audio_player::AudioPlayer>>>,
    progress_timer: &Rc<RefCell<Option<slint::Timer>>>,
    weak: slint::Weak<MainWindow>,
) {
    let sessions = souffle_lib::commands::get_meeting_audio(meeting_id.to_string())
        .inspect_err(|e| eprintln!("Failed to list meeting audio: {e}"))
        .unwrap_or_default();
    let Some(session) = sessions.first() else {
        window.set_meeting_detail_has_audio(false);
        window.set_meeting_detail_audio_peaks(
            std::rc::Rc::new(slint::VecModel::from(Vec::<f32>::new())).into(),
        );
        return;
    };
    let path = std::path::PathBuf::from(&session.path);
    match audio_player::load(&path) {
        Ok((loaded, peaks)) => {
            window.set_meeting_detail_has_audio(true);
            window.set_meeting_detail_audio_peaks(
                std::rc::Rc::new(slint::VecModel::from(peaks)).into(),
            );
            window.set_meeting_detail_audio_duration_label(
                timeline::format_duration(loaded.duration_seconds()).into(),
            );
            window.set_meeting_detail_audio_position_label(timeline::format_duration(0.0).into());
            window.set_meeting_detail_audio_progress(0.0);
            window.set_meeting_detail_audio_is_playing(false);
            *player.borrow_mut() = Some(loaded);
            *progress_timer.borrow_mut() = Some(start_audio_progress_timer(weak, player.clone()));
        }
        Err(e) => {
            eprintln!("Failed to load meeting audio: {e}");
            window.set_meeting_detail_has_audio(false);
        }
    }
}

/// Opens a meeting in MeetingDetail: stops whatever audio was previously
/// loaded, fetches the meeting, and populates header/notes/transcript/audio.
/// Shared by "open from Timeline" and "just stopped this meeting recording",
/// the latter of which must land the user on the meeting they just
/// finished, not back on the Timeline.
#[allow(clippy::too_many_arguments)]
fn open_meeting_detail(
    window: &MainWindow,
    handle: &AppHandle,
    meeting_id: &str,
    player: &Rc<RefCell<Option<audio_player::AudioPlayer>>>,
    progress_timer: &Rc<RefCell<Option<slint::Timer>>>,
    transcript_state: &Rc<RefCell<Option<TranscriptState>>>,
    transcript_timer: &Rc<RefCell<Option<slint::Timer>>>,
    weak: slint::Weak<MainWindow>,
) {
    stop_audio_player(player, progress_timer);
    stop_transcript_window(transcript_state, transcript_timer);
    let state = handle.state::<AppState>();
    match souffle_lib::commands::get_meeting(state, meeting_id.to_string()) {
        Ok(meeting) => {
            populate_meeting_detail(window, &meeting);
            load_meeting_audio(window, &meeting.id, player, progress_timer, weak.clone());
            load_meeting_transcript_window(
                window,
                &meeting,
                transcript_state,
                transcript_timer,
                weak,
            );
        }
        Err(e) => eprintln!("Failed to load meeting {meeting_id}: {e}"),
    }
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

/// Pushes each `OllamaPullProgress` update into the settings window's
/// pull-status properties - same `Channel` + `invoke_from_event_loop`
/// pattern as `live_segment_channel`, for the real download triggered by
/// "Download recommended model" in the IA tab.
fn ollama_pull_channel(
    weak: slint::Weak<MainWindow>,
) -> Channel<souffle_lib::summary::OllamaPullProgress> {
    Channel::new(move |body| {
        let tauri::ipc::InvokeResponseBody::Json(json) = body else {
            return Ok(());
        };
        let Ok(progress) = serde_json::from_str::<souffle_lib::summary::OllamaPullProgress>(&json)
        else {
            return Ok(());
        };
        let weak = weak.clone();
        let _ = slint::invoke_from_event_loop(move || {
            let Some(window) = weak.upgrade() else {
                return;
            };
            let status = if let Some(total) = progress.total_bytes.filter(|t| *t > 0) {
                format!(
                    "{} \u{2014} {:.0}%",
                    progress.status,
                    (progress.downloaded_bytes as f64 / total as f64) * 100.0
                )
            } else {
                progress.status.clone()
            };
            window.set_settings_ollama_pull_status(status.into());
            if let Some(error) = progress.error {
                window.set_settings_ollama_pull_error(error.into());
            }
        });
        Ok(())
    })
}

/// Drives one model transition (AC2): reads the real `get_model_status`,
/// then either does nothing (Ready), loads in the background (LoadRequired),
/// or starts a real tracked download that itself triggers the load once
/// `DownloadStatus::Complete` arrives (DownloadRequired) - mirrors
/// `selectModelOption()`'s `refreshRuntimeStatus()` branch, using the same
/// commands, never a client-side re-derivation of the state machine.
fn start_model_transition(
    weak: slint::Weak<MainWindow>,
    handle: AppHandle,
    selection: TranscriptionProfileSelection,
) {
    let state = handle.state::<AppState>();
    let status = match souffle_lib::commands::get_model_status(state, selection.clone()) {
        Ok(status) => status,
        Err(e) => {
            if let Some(window) = weak.upgrade() {
                window.set_settings_model_error_message(e.into());
            }
            return;
        }
    };
    let Some(window) = weak.upgrade() else {
        return;
    };
    window.set_settings_model_error_message("".into());
    match status.phase {
        TranscriptionRuntimePhase::Ready => {
            window.set_settings_model_status_text(model_ui::phase_label(status.phase).into());
        }
        TranscriptionRuntimePhase::LoadRequired => {
            window.set_settings_model_status_text("Chargement\u{2026}".into());
            load_model_in_background(weak, handle, selection);
        }
        TranscriptionRuntimePhase::DownloadRequired => {
            window.set_settings_model_downloading(true);
            window.set_settings_model_download_progress_label("".into());
            window.set_settings_model_download_progress_fraction(0.0);
            let state = handle.state::<AppState>();
            let channel = model_download_channel(weak.clone(), handle.clone(), selection.clone());
            if let Err(e) = souffle_lib::commands::download_model(state, selection, channel) {
                window.set_settings_model_downloading(false);
                window.set_settings_model_error_message(e.into());
            }
        }
    }
}

/// Off the Slint event-loop thread (weights loading takes seconds) - same
/// `spawn_blocking` + await pattern `ensure_model_ready` already uses for
/// the recording flow's own load step.
fn load_model_in_background(
    weak: slint::Weak<MainWindow>,
    handle: AppHandle,
    selection: TranscriptionProfileSelection,
) {
    slint::spawn_local(async move {
        let handle_for_load = handle.clone();
        let selection_for_load = selection.clone();
        let result = tauri::async_runtime::spawn_blocking(move || {
            let state = handle_for_load.state::<AppState>();
            souffle_lib::commands::load_model(state, selection_for_load)
        })
        .await
        .map_err(|e| format!("Join load_model task: {e}"))
        .and_then(|r| r);
        if let Some(window) = weak.upgrade() {
            match result {
                Ok(()) => window.set_settings_model_status_text("Pr\u{ea}t".into()),
                Err(e) => window.set_settings_model_error_message(e.into()),
            }
        }
    })
    .expect("slint event loop not running");
}

/// Pushes `DownloadProgress` updates into the model tab's progress
/// properties, and triggers the load step itself once the download
/// reports `Complete` - same `Channel` + `invoke_from_event_loop` pattern
/// as `live_segment_channel`/`ollama_pull_channel`.
fn model_download_channel(
    weak: slint::Weak<MainWindow>,
    handle: AppHandle,
    selection: TranscriptionProfileSelection,
) -> Channel<souffle_lib::models::DownloadProgress> {
    Channel::new(move |body| {
        let tauri::ipc::InvokeResponseBody::Json(json) = body else {
            return Ok(());
        };
        let Ok(progress) = serde_json::from_str::<souffle_lib::models::DownloadProgress>(&json)
        else {
            return Ok(());
        };
        let weak = weak.clone();
        let handle = handle.clone();
        let selection = selection.clone();
        let _ = slint::invoke_from_event_loop(move || {
            let Some(window) = weak.upgrade() else {
                return;
            };
            match &progress.status {
                souffle_lib::models::DownloadStatus::Starting
                | souffle_lib::models::DownloadStatus::Downloading => {
                    let fraction = progress
                        .total_bytes
                        .filter(|total| *total > 0)
                        .map(|total| progress.downloaded_bytes as f32 / total as f32)
                        .unwrap_or(0.0);
                    window.set_settings_model_download_progress_fraction(fraction);
                    window
                        .set_settings_model_download_progress_label(progress.file.as_str().into());
                }
                souffle_lib::models::DownloadStatus::Complete => {
                    window.set_settings_model_downloading(false);
                    window.set_settings_model_status_text("Chargement\u{2026}".into());
                    load_model_in_background(weak.clone(), handle.clone(), selection.clone());
                }
                souffle_lib::models::DownloadStatus::Error(e) => {
                    window.set_settings_model_downloading(false);
                    window.set_settings_model_error_message(e.as_str().into());
                }
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

async fn stop_meeting(handle: AppHandle) -> Result<String, String> {
    run_on_main_thread(move || {
        tauri::async_runtime::block_on(async move {
            let state = handle.state::<AppState>();
            souffle_lib::commands::stop_meeting_recording(state).await
        })
    })
    .await
}

/// All mutable state the onboarding wizard needs across its 4 steps, in one
/// place rather than a dozen separate `Rc<RefCell<..>>`s - unlike
/// `wire_callbacks`'s per-field caches, this state is only ever touched
/// while the wizard is open, so bundling it doesn't create the same
/// cross-feature borrow-conflict risk.
struct OnboardingState {
    steps: Vec<&'static str>,
    step_index: usize,
    recovery_only: bool,
    permission_status: souffle_lib::permissions::PermissionStatus,
    permission_busy: [bool; 3],
    devices: Vec<AudioInputDevice>,
    selected_device: String,
    model_options: Vec<model_ui::FlatModelOption>,
    selected_model_index: Option<usize>,
    toggle_shortcut: String,
    pending_modifier: Option<String>,
    auto_paste: bool,
}

impl Default for OnboardingState {
    fn default() -> Self {
        Self {
            steps: Vec::new(),
            step_index: 0,
            recovery_only: false,
            permission_status: souffle_lib::permissions::PermissionStatus {
                microphone: PermState::Unknown,
                system_audio: PermState::Unknown,
                accessibility: PermState::Unknown,
                calendar: PermState::Unknown,
            },
            permission_busy: [false; 3],
            devices: Vec::new(),
            selected_device: String::new(),
            model_options: Vec::new(),
            selected_model_index: None,
            toggle_shortcut: String::new(),
            pending_modifier: None,
            auto_paste: false,
        }
    }
}

fn device_option_label(device: &AudioInputDevice) -> String {
    if device.is_default {
        format!("{} (par défaut)", device.name)
    } else {
        device.name.clone()
    }
}

/// Renders whatever step `ob.step_index` currently points at - shared by
/// wizard-open, "Back", and every successful "Continue"/transition.
fn show_onboarding_step(
    window: &MainWindow,
    handle: &AppHandle,
    ob: &Rc<RefCell<OnboardingState>>,
) {
    let (step, step_index, step_count) = {
        let guard = ob.borrow();
        (
            guard.steps.get(guard.step_index).copied().unwrap_or(""),
            guard.step_index as i32,
            guard.steps.len() as i32,
        )
    };
    window.set_onboarding_step(step.into());
    window.set_onboarding_step_index(step_index);
    window.set_onboarding_step_count(step_count);
    window.set_onboarding_title(onboarding_ui::step_title(step).into());
    window.set_onboarding_subtitle(onboarding_ui::step_subtitle(step).into());
    window.set_onboarding_busy(false);
    window.set_onboarding_continue_enabled(true);
    window.set_onboarding_continue_label("Continuer".into());

    match step {
        "permissions" => {
            let (status, busy) = {
                let guard = ob.borrow();
                (guard.permission_status.clone(), guard.permission_busy)
            };
            onboarding_ui::populate_permission_rows(window, &status, busy);
        }
        "microphone" => {
            let guard = ob.borrow();
            let mut labels: Vec<slint::SharedString> = vec!["Automatique".into()];
            labels.extend(guard.devices.iter().map(|d| device_option_label(d).into()));
            window.set_onboarding_device_labels(
                std::rc::Rc::new(slint::VecModel::from(labels)).into(),
            );
            let selected_label = guard
                .devices
                .iter()
                .find(|d| d.uid == guard.selected_device)
                .map(device_option_label)
                .unwrap_or_else(|| "Automatique".to_string());
            window.set_onboarding_selected_device_label(selected_label.into());
        }
        "model" => {
            let guard = ob.borrow();
            let labels: Vec<slint::SharedString> = guard
                .model_options
                .iter()
                .map(|o| o.label.as_str().into())
                .collect();
            window.set_onboarding_model_labels(
                std::rc::Rc::new(slint::VecModel::from(labels)).into(),
            );
            let selected_index = guard.selected_model_index.unwrap_or(0);
            let selected_label = guard
                .model_options
                .get(selected_index)
                .map(|o| o.label.clone())
                .unwrap_or_default();
            window.set_onboarding_selected_model_label(selected_label.into());
            let selection = guard
                .model_options
                .get(selected_index)
                .map(|o| o.selection());
            drop(guard);
            let phase = selection.and_then(|selection| {
                souffle_lib::commands::get_model_status(handle.state::<AppState>(), selection).ok()
            });
            let phase_str = match phase.map(|s| s.phase) {
                Some(TranscriptionRuntimePhase::Ready) => "ready",
                Some(TranscriptionRuntimePhase::LoadRequired) => "load_required",
                _ => "pick",
            };
            window.set_onboarding_model_phase(if phase_str == "load_required" {
                "pick".into()
            } else {
                phase_str.into()
            });
            if phase_str == "ready" {
                window.set_onboarding_continue_label("Continuer".into());
            } else {
                window.set_onboarding_continue_label("Télécharger et continuer".into());
            }
        }
        "shortcut" => {
            let guard = ob.borrow();
            window.set_onboarding_toggle_shortcut_label(
                format_shortcut_label(&guard.toggle_shortcut).into(),
            );
            window.set_onboarding_auto_paste(guard.auto_paste);
            window.set_onboarding_accessibility_granted(
                guard.permission_status.accessibility == PermState::Granted,
            );
            window.set_onboarding_continue_label("Terminer".into());
        }
        _ => {}
    }
}

/// Async permission refresh for the permissions step - `get_permission_status`
/// is a real TCC query, never a value fixed at wizard-open (AC7).
fn refresh_onboarding_permissions(weak: slint::Weak<MainWindow>, ob: Rc<RefCell<OnboardingState>>) {
    slint::spawn_local(async move {
        let status = souffle_lib::commands::get_permission_status().await;
        let Some(window) = weak.upgrade() else {
            return;
        };
        if window.get_onboarding_step() != "permissions" {
            return;
        }
        if let Ok(status) = status {
            let busy = {
                let mut guard = ob.borrow_mut();
                guard.permission_status = status.clone();
                guard.permission_busy
            };
            onboarding_ui::populate_permission_rows(&window, &status, busy);
        }
    })
    .expect("slint event loop not running");
}

fn start_onboarding_permission_poll(
    weak: slint::Weak<MainWindow>,
    ob: Rc<RefCell<OnboardingState>>,
) -> slint::Timer {
    let timer = slint::Timer::default();
    timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(600),
        move || {
            if ob.borrow().permission_busy.iter().any(|b| *b) {
                return;
            }
            refresh_onboarding_permissions(weak.clone(), ob.clone());
        },
    );
    timer
}

/// Sets up the whole onboarding overlay: decides whether to show it at
/// startup (`decide_show_setup_wizard`, never a hardcoded "first launch"
/// flag), then wires every step's callbacks. Model download/load reuse the
/// same commands `wire_callbacks`'s Settings-tab model selector calls
/// (`get_model_status`/`download_model`/`load_model`); shortcut capture
/// reuses `shortcut_capture.rs` the same way the Interface tab does.
fn wire_onboarding_callbacks(window: &MainWindow, tauri_handle: AppHandle) -> bool {
    let ob: Rc<RefCell<OnboardingState>> = Rc::new(RefCell::new(OnboardingState::default()));
    let onboarding_poll_timer: Rc<RefCell<Option<slint::Timer>>> = Rc::new(RefCell::new(None));

    let handle = tauri_handle.clone();
    let state = handle.state::<AppState>();
    let default_selection = TranscriptionProfileSelection::default();
    let phase = souffle_lib::commands::get_model_status(state, default_selection)
        .map(|s| s.phase)
        .unwrap_or(TranscriptionRuntimePhase::DownloadRequired);
    let machine_state = handle
        .state::<AppState>()
        .current_machine_state()
        .unwrap_or(souffle_lib::state_machine::AppStateMachine::Idle);
    let flags = onboarding_flags::read_setup_flags();
    let should_show = onboarding_flags::decide_show_setup_wizard(phase, flags, &machine_state);

    if should_show {
        let mut guard = ob.borrow_mut();
        guard.steps = onboarding_flags::wizard_steps(flags);
        guard.recovery_only = flags.setup_done;
        guard.devices = souffle_lib::commands::list_audio_devices().unwrap_or_default();
        let state = handle.state::<AppState>();
        if let Ok(settings) = souffle_lib::commands::get_settings(state) {
            guard.selected_device = settings.audio_device.unwrap_or_default();
            guard.auto_paste = settings.auto_paste;
        }
        let state = handle.state::<AppState>();
        if let Ok(catalog) = souffle_lib::commands::get_transcription_catalog(state) {
            guard.model_options = model_ui::list_available_model_options(&catalog);
            guard.selected_model_index = guard.model_options.iter().position(|o| {
                o.engine_id == catalog.selected_engine_id && o.model_id == catalog.selected_model_id
            });
        }
        let state = handle.state::<AppState>();
        if let Ok(shortcuts) = souffle_lib::commands::get_shortcuts(state) {
            guard.toggle_shortcut = shortcuts.toggle;
        }
        drop(guard);
        window.set_onboarding_open(true);
        show_onboarding_step(window, &handle, &ob);
        if ob.borrow().steps.first() == Some(&"permissions") {
            *onboarding_poll_timer.borrow_mut() = Some(start_onboarding_permission_poll(
                window.as_weak(),
                ob.clone(),
            ));
        }
    }

    let handle = tauri_handle.clone();
    let weak = window.as_weak();
    window.on_onboarding_locale_changed(move |locale| {
        if let Err(e) = (|| -> Result<(), String> {
            let state = handle.state::<AppState>();
            let mut settings = souffle_lib::commands::get_settings(state)?;
            settings.locale = locale.to_string();
            let state = handle.state::<AppState>();
            souffle_lib::commands::save_settings(handle.clone(), state, settings)
        })() {
            eprintln!("Failed to save onboarding locale: {e}");
        }
        if let Some(window) = weak.upgrade() {
            window.set_onboarding_locale(locale);
        }
    });

    let weak = window.as_weak();
    let ob_for_grant = ob.clone();
    window.on_onboarding_grant_requested(move |kind_str| {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let Some(index) = onboarding_ui::row_index(&kind_str) else {
            return;
        };
        let kind = match kind_str.as_str() {
            "microphone" => PermissionKind::Microphone,
            "system_audio" => PermissionKind::SystemAudio,
            "accessibility" => PermissionKind::Accessibility,
            _ => return,
        };
        ob_for_grant.borrow_mut().permission_busy[index] = true;
        {
            let (status, busy) = {
                let guard = ob_for_grant.borrow();
                (guard.permission_status.clone(), guard.permission_busy)
            };
            onboarding_ui::populate_permission_rows(&window, &status, busy);
        }
        let weak = weak.clone();
        let ob = ob_for_grant.clone();
        slint::spawn_local(async move {
            let result = souffle_lib::commands::request_permission(kind).await;
            let Some(window) = weak.upgrade() else {
                return;
            };
            let mut guard = ob.borrow_mut();
            guard.permission_busy[index] = false;
            if let Ok(state) = result {
                match kind {
                    PermissionKind::Microphone => guard.permission_status.microphone = state,
                    PermissionKind::SystemAudio => guard.permission_status.system_audio = state,
                    PermissionKind::Accessibility => guard.permission_status.accessibility = state,
                    PermissionKind::Calendar => {}
                }
            }
            let (status, busy) = (guard.permission_status.clone(), guard.permission_busy);
            drop(guard);
            onboarding_ui::populate_permission_rows(&window, &status, busy);
        })
        .expect("slint event loop not running");
    });

    window.on_onboarding_hint_action_requested(move |_kind| {
        souffle_lib::commands::open_apple_intelligence_settings();
        let _ = std::process::Command::new("open")
            .arg("x-apple.systempreferences:com.apple.preference.security")
            .spawn();
    });

    let weak = window.as_weak();
    window.on_onboarding_repair_requested(move || {
        let Some(window) = weak.upgrade() else {
            return;
        };
        window.set_onboarding_repair_busy(true);
        let weak = weak.clone();
        slint::spawn_local(async move {
            let result = souffle_lib::commands::repair_accessibility_permission().await;
            if let Some(window) = weak.upgrade() {
                window.set_onboarding_repair_busy(false);
                match result {
                    Ok(_) => {
                        window.set_onboarding_repair_success(true);
                        window.set_onboarding_repair_cooldown(true);
                        let weak_for_cooldown = weak.clone();
                        let timer = slint::Timer::default();
                        timer.start(
                            slint::TimerMode::SingleShot,
                            Duration::from_millis(2500),
                            move || {
                                if let Some(window) = weak_for_cooldown.upgrade() {
                                    window.set_onboarding_repair_cooldown(false);
                                }
                            },
                        );
                        std::mem::forget(timer);
                    }
                    Err(e) => eprintln!("Accessibility repair failed: {e}"),
                }
            }
        })
        .expect("slint event loop not running");
    });

    let ob_for_device = ob.clone();
    window.on_onboarding_device_changed(move |label| {
        let mut guard = ob_for_device.borrow_mut();
        guard.selected_device = if label == "Automatique" {
            String::new()
        } else {
            guard
                .devices
                .iter()
                .find(|d| device_option_label(d) == label.as_str())
                .map(|d| d.uid.clone())
                .unwrap_or_default()
        };
    });

    let weak = window.as_weak();
    let handle = tauri_handle.clone();
    let ob_for_refresh = ob.clone();
    window.on_onboarding_refresh_devices_requested(move || {
        let Some(window) = weak.upgrade() else {
            return;
        };
        ob_for_refresh.borrow_mut().devices =
            souffle_lib::commands::list_audio_devices().unwrap_or_default();
        show_onboarding_step(&window, &handle, &ob_for_refresh);
    });

    let ob_for_model_pick = ob.clone();
    window.on_onboarding_model_picked(move |label| {
        let mut guard = ob_for_model_pick.borrow_mut();
        guard.selected_model_index = guard
            .model_options
            .iter()
            .position(|o| o.label == label.as_str());
    });

    let weak = window.as_weak();
    let handle = tauri_handle.clone();
    let ob_for_shortcut_record = ob.clone();
    window.on_onboarding_shortcut_record_requested(move || {
        let _ = &handle;
        let _ = &ob_for_shortcut_record;
        if let Some(window) = weak.upgrade() {
            window.set_onboarding_shortcut_recording(true);
            window.set_onboarding_shortcut_error("".into());
        }
    });

    let weak = window.as_weak();
    let handle = tauri_handle.clone();
    let ob_for_capture = ob.clone();
    window.on_onboarding_shortcut_captured(move |text, ctrl, shift, alt, meta| {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let modifiers = shortcut_capture::Modifiers { control: ctrl, shift, alt, meta };
        if let Some(name) = shortcut_capture::modifier_only_shortcut(&text) {
            ob_for_capture.borrow_mut().pending_modifier = Some(name.to_string());
            return;
        }
        ob_for_capture.borrow_mut().pending_modifier = None;
        if shortcut_capture::missing_modifier(&text, modifiers) {
            window.set_onboarding_shortcut_error(
                "Le raccourci doit inclure une touche de modification (Cmd, Ctrl, Maj, Alt) ou être une touche de fonction.".into(),
            );
            return;
        }
        let Some(value) = shortcut_capture::format_combo(&text, modifiers) else {
            return;
        };
        apply_onboarding_shortcut(&handle, &window, &ob_for_capture, value);
    });

    let weak = window.as_weak();
    let handle = tauri_handle.clone();
    let ob_for_release = ob.clone();
    window.on_onboarding_shortcut_released(move |text, _ctrl, _shift, _alt, _meta| {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let Some(name) = shortcut_capture::modifier_only_shortcut(&text) else {
            return;
        };
        let mut pending = ob_for_release.borrow_mut();
        if pending.pending_modifier.as_deref() != Some(name) {
            return;
        }
        pending.pending_modifier = None;
        drop(pending);
        apply_onboarding_shortcut(&handle, &window, &ob_for_release, name.to_string());
    });

    let weak = window.as_weak();
    let ob_for_clear = ob.clone();
    let handle = tauri_handle.clone();
    window.on_onboarding_shortcut_cleared(move || {
        let Some(window) = weak.upgrade() else {
            return;
        };
        apply_onboarding_shortcut(&handle, &window, &ob_for_clear, String::new());
    });

    let weak = window.as_weak();
    let ob_for_cancel = ob.clone();
    window.on_onboarding_shortcut_cancelled(move || {
        ob_for_cancel.borrow_mut().pending_modifier = None;
        if let Some(window) = weak.upgrade() {
            window.set_onboarding_shortcut_recording(false);
            window.set_onboarding_shortcut_error("".into());
        }
    });

    let ob_for_auto_paste = ob.clone();
    window.on_onboarding_auto_paste_changed(move |enabled| {
        ob_for_auto_paste.borrow_mut().auto_paste = enabled;
    });

    window.on_onboarding_review_accessibility_requested(move || {
        let _ = std::process::Command::new("open")
            .arg("x-apple.systempreferences:com.apple.preference.security")
            .spawn();
    });

    let weak = window.as_weak();
    let handle = tauri_handle.clone();
    let ob_for_back = ob.clone();
    window.on_onboarding_back_requested(move || {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let mut guard = ob_for_back.borrow_mut();
        if guard.step_index > 0 {
            guard.step_index -= 1;
        }
        drop(guard);
        show_onboarding_step(&window, &handle, &ob_for_back);
    });

    let weak = window.as_weak();
    let handle = tauri_handle.clone();
    let ob_for_continue = ob.clone();
    let onboarding_poll_timer_for_continue = onboarding_poll_timer.clone();
    window.on_onboarding_continue_requested(move || {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let step = window.get_onboarding_step().to_string();
        match step.as_str() {
            "permissions" => {
                onboarding_flags::mark_permissions_done();
                advance_onboarding_step(
                    &window,
                    &handle,
                    &ob_for_continue,
                    &onboarding_poll_timer_for_continue,
                );
            }
            "microphone" => {
                let uid = ob_for_continue.borrow().selected_device.clone();
                let state = handle.state::<AppState>();
                let _ =
                    souffle_lib::commands::select_audio_device(handle.clone(), state, uid.clone());
                if let Ok(mut settings) =
                    souffle_lib::commands::get_settings(handle.state::<AppState>())
                {
                    settings.audio_device = if uid.is_empty() { None } else { Some(uid) };
                    let state = handle.state::<AppState>();
                    let _ = souffle_lib::commands::save_settings(handle.clone(), state, settings);
                }
                advance_onboarding_step(
                    &window,
                    &handle,
                    &ob_for_continue,
                    &onboarding_poll_timer_for_continue,
                );
            }
            "model" => {
                let phase = window.get_onboarding_model_phase();
                if phase == "ready" {
                    advance_onboarding_step(
                        &window,
                        &handle,
                        &ob_for_continue,
                        &onboarding_poll_timer_for_continue,
                    );
                    return;
                }
                let selection = {
                    let guard = ob_for_continue.borrow();
                    guard
                        .selected_model_index
                        .and_then(|i| guard.model_options.get(i))
                        .map(|o| {
                            (
                                o.engine_id.clone(),
                                o.model_id.clone(),
                                o.backend_id.clone(),
                                o.selection(),
                            )
                        })
                };
                let Some((engine_id, model_id, backend_id, selection)) = selection else {
                    return;
                };
                if let Ok(mut settings) =
                    souffle_lib::commands::get_settings(handle.state::<AppState>())
                {
                    settings.transcription_engine_id = engine_id;
                    settings.transcription_model_id = model_id;
                    settings.transcription_backend_id = backend_id;
                    let state = handle.state::<AppState>();
                    let _ = souffle_lib::commands::save_settings(handle.clone(), state, settings);
                }
                window.set_onboarding_busy(true);
                window.set_onboarding_continue_enabled(false);
                start_onboarding_model_transition(weak.clone(), handle.clone(), selection);
            }
            "shortcut" => {
                let (auto_paste, recovery_only) = {
                    let guard = ob_for_continue.borrow();
                    (guard.auto_paste, guard.recovery_only)
                };
                if let Ok(mut settings) =
                    souffle_lib::commands::get_settings(handle.state::<AppState>())
                {
                    settings.auto_paste = auto_paste;
                    settings.autostart_enabled = onboarding_flags::decide_autostart_on_finish(
                        recovery_only,
                        settings.autostart_enabled,
                    );
                    let state = handle.state::<AppState>();
                    let _ = souffle_lib::commands::save_settings(handle.clone(), state, settings);
                }
                onboarding_flags::mark_setup_complete();
                *onboarding_poll_timer_for_continue.borrow_mut() = None;
                window.set_onboarding_open(false);
            }
            _ => {}
        }
    });

    should_show
}

/// Advances to the next step, stopping/starting the permissions poll timer
/// as the wizard enters/leaves that step.
fn advance_onboarding_step(
    window: &MainWindow,
    handle: &AppHandle,
    ob: &Rc<RefCell<OnboardingState>>,
    poll_timer: &Rc<RefCell<Option<slint::Timer>>>,
) {
    {
        let mut guard = ob.borrow_mut();
        if guard.step_index + 1 < guard.steps.len() {
            guard.step_index += 1;
        }
    }
    show_onboarding_step(window, handle, ob);
    let now_on_permissions = {
        let guard = ob.borrow();
        guard.steps.get(guard.step_index) == Some(&"permissions")
    };
    if now_on_permissions {
        *poll_timer.borrow_mut() = Some(start_onboarding_permission_poll(
            window.as_weak(),
            ob.clone(),
        ));
    } else {
        *poll_timer.borrow_mut() = None;
    }
}

fn apply_onboarding_shortcut(
    handle: &AppHandle,
    window: &MainWindow,
    ob: &Rc<RefCell<OnboardingState>>,
    value: String,
) {
    window.set_onboarding_shortcut_recording(false);
    ob.borrow_mut().toggle_shortcut = value.clone();
    let state = handle.state::<AppState>();
    let mut shortcuts = souffle_lib::commands::get_shortcuts(state).unwrap_or_default();
    shortcuts.toggle = value;
    let state = handle.state::<AppState>();
    match souffle_lib::commands::save_shortcuts(handle.clone(), state, shortcuts) {
        Ok(()) => {
            window.set_onboarding_shortcut_error("".into());
            let guard = ob.borrow();
            window.set_onboarding_toggle_shortcut_label(
                format_shortcut_label(&guard.toggle_shortcut).into(),
            );
        }
        Err(e) => window.set_onboarding_shortcut_error(e.into()),
    }
}

/// Onboarding-scoped mirror of `start_model_transition` - same commands,
/// separate `onboarding-*` window properties so this never touches the
/// Settings tab's model state (the two overlays are never open together,
/// but nothing here assumes that).
fn start_onboarding_model_transition(
    weak: slint::Weak<MainWindow>,
    handle: AppHandle,
    selection: TranscriptionProfileSelection,
) {
    let state = handle.state::<AppState>();
    let status = match souffle_lib::commands::get_model_status(state, selection.clone()) {
        Ok(status) => status,
        Err(e) => {
            if let Some(window) = weak.upgrade() {
                window.set_onboarding_busy(false);
                window.set_onboarding_continue_enabled(true);
                window.set_onboarding_status_message(e.into());
            }
            return;
        }
    };
    let Some(window) = weak.upgrade() else {
        return;
    };
    match status.phase {
        TranscriptionRuntimePhase::Ready => {
            window.set_onboarding_model_phase("ready".into());
            window.set_onboarding_busy(false);
            window.set_onboarding_continue_enabled(true);
        }
        TranscriptionRuntimePhase::LoadRequired => {
            window.set_onboarding_model_phase("loading".into());
            let weak2 = weak.clone();
            let handle2 = handle.clone();
            let selection2 = selection.clone();
            slint::spawn_local(async move {
                let handle3 = handle2.clone();
                let result = tauri::async_runtime::spawn_blocking(move || {
                    let state = handle3.state::<AppState>();
                    souffle_lib::commands::load_model(state, selection2)
                })
                .await
                .map_err(|e| format!("Join load_model task: {e}"))
                .and_then(|r| r);
                if let Some(window) = weak2.upgrade() {
                    window.set_onboarding_busy(false);
                    window.set_onboarding_continue_enabled(true);
                    match result {
                        Ok(()) => window.set_onboarding_model_phase("ready".into()),
                        Err(e) => window.set_onboarding_status_message(e.into()),
                    }
                }
            })
            .expect("slint event loop not running");
        }
        TranscriptionRuntimePhase::DownloadRequired => {
            window.set_onboarding_model_phase("downloading".into());
            window.set_onboarding_download_progress_label("".into());
            window.set_onboarding_download_progress_fraction(0.0);
            let state = handle.state::<AppState>();
            let channel =
                onboarding_model_download_channel(weak.clone(), handle.clone(), selection.clone());
            if let Err(e) = souffle_lib::commands::download_model(state, selection, channel) {
                window.set_onboarding_busy(false);
                window.set_onboarding_continue_enabled(true);
                window.set_onboarding_status_message(e.into());
            }
        }
    }
}

fn onboarding_model_download_channel(
    weak: slint::Weak<MainWindow>,
    handle: AppHandle,
    selection: TranscriptionProfileSelection,
) -> Channel<souffle_lib::models::DownloadProgress> {
    Channel::new(move |body| {
        let tauri::ipc::InvokeResponseBody::Json(json) = body else {
            return Ok(());
        };
        let Ok(progress) = serde_json::from_str::<souffle_lib::models::DownloadProgress>(&json)
        else {
            return Ok(());
        };
        let weak = weak.clone();
        let handle = handle.clone();
        let selection = selection.clone();
        let _ = slint::invoke_from_event_loop(move || {
            let Some(window) = weak.upgrade() else {
                return;
            };
            match &progress.status {
                souffle_lib::models::DownloadStatus::Starting
                | souffle_lib::models::DownloadStatus::Downloading => {
                    let fraction = progress
                        .total_bytes
                        .filter(|total| *total > 0)
                        .map(|total| progress.downloaded_bytes as f32 / total as f32)
                        .unwrap_or(0.0);
                    window.set_onboarding_download_progress_fraction(fraction);
                    window.set_onboarding_download_progress_label(progress.file.as_str().into());
                }
                souffle_lib::models::DownloadStatus::Complete => {
                    window.set_onboarding_model_phase("loading".into());
                    start_onboarding_model_transition(
                        weak.clone(),
                        handle.clone(),
                        selection.clone(),
                    );
                }
                souffle_lib::models::DownloadStatus::Error(e) => {
                    window.set_onboarding_busy(false);
                    window.set_onboarding_continue_enabled(true);
                    window.set_onboarding_status_message(e.as_str().into());
                }
            }
        });
        Ok(())
    })
}

/// "What's New" (a version bump since the last launch) + the automatic
/// "Update Available" startup check - port of `bootstrap.ts`'s
/// `whatsNew`/auto-check logic. Never stacks either dialog on top of the
/// onboarding wizard (`onboarding_open`), matching `bootstrap.ts`'s own
/// "never stacks on the wizard" comment.
fn wire_update_dialogs(window: &MainWindow, tauri_handle: AppHandle, onboarding_open: bool) {
    let handle = tauri_handle.clone();
    window.on_whats_new_dismissed(move || {
        let state = handle.state::<AppState>();
        if let Ok(mut settings) = souffle_lib::commands::get_settings(state) {
            settings.last_seen_version = souffle_lib::commands::get_app_version().version;
            let state = handle.state::<AppState>();
            let _ = souffle_lib::commands::save_settings(handle.clone(), state, settings);
        }
    });

    let weak = window.as_weak();
    let handle = tauri_handle.clone();
    window.on_update_download_requested(move || {
        let Some(window) = weak.upgrade() else {
            return;
        };
        window.set_update_phase("downloading".into());
        window.set_update_error_message("".into());
        let weak = weak.clone();
        let handle = handle.clone();
        slint::spawn_local(async move {
            let result = souffle_lib::commands::download_update(handle).await;
            let Some(window) = weak.upgrade() else {
                return;
            };
            match result {
                Ok(status) if status.error.is_none() => window.set_update_phase("ready".into()),
                Ok(status) => {
                    window.set_update_phase("failed".into());
                    window.set_update_error_message(status.error.unwrap_or_default().into());
                }
                Err(e) => {
                    window.set_update_phase("failed".into());
                    window.set_update_error_message(e.into());
                }
            }
        })
        .expect("slint event loop not running");
    });

    let weak = window.as_weak();
    let handle = tauri_handle.clone();
    window.on_update_install_requested(move || {
        let weak = weak.clone();
        let handle = handle.clone();
        slint::spawn_local(async move {
            let state = handle.state::<AppState>();
            if let Err(e) = souffle_lib::commands::install_update(handle.clone(), state).await
                && let Some(window) = weak.upgrade()
            {
                window.set_update_error_message(e.into());
            }
            // On success the process restarts itself; nothing left to update here.
        })
        .expect("slint event loop not running");
    });

    let weak = window.as_weak();
    window.on_update_open_release_requested(move || {
        if let Some(window) = weak.upgrade() {
            let url = window.get_update_release_url().to_string();
            if !url.is_empty() {
                let _ = souffle_lib::commands::open_release_page(url);
            }
        }
    });

    window.on_update_dismiss_requested(move || {});

    // Only `https://github.com/...` links are ever emitted by real content
    // (whole-line links from `check_for_updates`/`whatsNewFallback`), same
    // constraint `open_release_page` enforces server-side - mirrors the
    // Svelte version's own comment on why this restriction exists.
    window.on_whats_new_link_clicked(move |url| {
        if url.starts_with("https://github.com/") {
            let _ = souffle_lib::commands::open_release_page(url.to_string());
        }
    });
    window.on_update_link_clicked(move |url| {
        if url.starts_with("https://github.com/") {
            let _ = souffle_lib::commands::open_release_page(url.to_string());
        }
    });

    if onboarding_open {
        return;
    }

    let handle = tauri_handle;
    let weak = window.as_weak();
    let Ok(settings) = souffle_lib::commands::get_settings(handle.state::<AppState>()) else {
        return;
    };
    let app_version = souffle_lib::commands::get_app_version();
    let current_version = app_version.version.clone();
    let setup_done = onboarding_flags::read_setup_flags().setup_done;

    if app_version.is_local_build {
        // No release notes to show for a local checkout build.
    } else if !setup_done || settings.last_seen_version.is_empty() {
        // First launch / unfinished setup: stamp silently so the changelog
        // never stacks on the wizard and doesn't pop right after it either.
        if settings.last_seen_version != current_version {
            let mut next = settings.clone();
            next.last_seen_version = current_version.clone();
            let state = handle.state::<AppState>();
            let _ = souffle_lib::commands::save_settings(handle.clone(), state, next);
        }
    } else if settings.last_seen_version != current_version {
        window.set_whats_new_version(current_version.clone().into());
        let blocks = markdown::render_blocks(&format!("Mis à jour vers v{current_version}."));
        window.set_whats_new_content_blocks(std::rc::Rc::new(slint::VecModel::from(blocks)).into());
        window.set_whats_new_open(true);
    }

    if settings.auto_update_check_enabled && !window.get_whats_new_open() {
        slint::spawn_local(async move {
            let Ok(result) = souffle_lib::commands::check_for_updates().await else {
                return;
            };
            let Some(window) = weak.upgrade() else {
                return;
            };
            if result.update_available && result.check_error.is_none() {
                window.set_update_latest_version(result.latest_version.unwrap_or_default().into());
                let notes = result
                    .release_notes
                    .unwrap_or_else(|| "Voir les notes de version sur GitHub.".to_string());
                let blocks = markdown::render_blocks(&notes);
                window.set_update_release_notes_blocks(
                    std::rc::Rc::new(slint::VecModel::from(blocks)).into(),
                );
                window.set_update_release_url(result.release_url.unwrap_or_default().into());
                window.set_update_phase("idle".into());
                let state = handle.state::<AppState>();
                let blocked = souffle_lib::commands::get_update_install_block(state)
                    .ok()
                    .flatten();
                window.set_update_install_blocked_reason(
                    blocked
                        .map(|r| r.as_str().to_string())
                        .unwrap_or_default()
                        .into(),
                );
                window.set_update_available_open(true);
            }
        })
        .expect("slint event loop not running");
    }
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
    // Shared with load_meeting_audio/stop_audio_player/open_meeting_detail
    // and the play-pause/seek callbacks below - one loaded player at a
    // time, for whichever meeting is currently open in MeetingDetail.
    let player: Rc<RefCell<Option<audio_player::AudioPlayer>>> = Rc::new(RefCell::new(None));
    let progress_timer: Rc<RefCell<Option<slint::Timer>>> = Rc::new(RefCell::new(None));
    // Same sharing pattern, for the virtualized transcript list (AC15).
    let transcript_state: Rc<RefCell<Option<TranscriptState>>> = Rc::new(RefCell::new(None));
    let transcript_timer: Rc<RefCell<Option<slint::Timer>>> = Rc::new(RefCell::new(None));

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
            if mode == RecordingMode::Meeting {
                let result = stop_meeting(handle.clone()).await;
                // Send-safe on purpose: this closure cannot capture the
                // Rc<RefCell<AudioPlayer>> player state (Rc isn't Send, and
                // upgrade_in_event_loop requires it) - re-invoking the
                // already-registered timeline-item-opened callback (which
                // does capture it, as a plain same-thread closure) reuses
                // the real open-meeting path instead of duplicating it.
                if let Err(e) = weak.upgrade_in_event_loop(move |window| {
                    window.set_live_text("".into());
                    window.set_live_tentative("".into());
                    match result {
                        // A stopped meeting recording lands the user back on
                        // that meeting's detail, not the Timeline - they
                        // were just looking at it live.
                        Ok(meeting_id) => {
                            window.set_recording_mode(RecordingMode::Idle);
                            window.invoke_timeline_item_opened(
                                TimelineKind::Meeting,
                                meeting_id.into(),
                            );
                        }
                        Err(e) => {
                            eprintln!("Failed to stop meeting: {e}");
                            window.set_recording_mode(RecordingMode::Idle);
                            refresh_timeline(&window, &handle_for_refresh);
                        }
                    }
                }) {
                    eprintln!("upgrade_in_event_loop failed (stop meeting): {e}");
                }
            } else {
                let result = stop_dictation(handle).await;
                if let Err(e) = weak.upgrade_in_event_loop(move |window| {
                    if let Err(e) = result {
                        eprintln!("Failed to stop dictation: {e}");
                    }
                    window.set_recording_mode(RecordingMode::Idle);
                    window.set_live_text("".into());
                    window.set_live_tentative("".into());
                    refresh_timeline(&window, &handle_for_refresh);
                }) {
                    eprintln!("upgrade_in_event_loop failed (stop dictation): {e}");
                }
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
    let player_for_open = player.clone();
    let progress_timer_for_open = progress_timer.clone();
    let transcript_state_for_open = transcript_state.clone();
    let transcript_timer_for_open = transcript_timer.clone();
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
        open_meeting_detail(
            &window,
            &handle,
            &id,
            &player_for_open,
            &progress_timer_for_open,
            &transcript_state_for_open,
            &transcript_timer_for_open,
            weak.clone(),
        );
    });

    let weak = window.as_weak();
    let player_for_back = player.clone();
    let progress_timer_for_back = progress_timer.clone();
    let transcript_state_for_back = transcript_state.clone();
    let transcript_timer_for_back = transcript_timer.clone();
    window.on_meeting_detail_back(move || {
        if let Some(window) = weak.upgrade() {
            window.set_active_meeting_id("".into());
        }
        stop_audio_player(&player_for_back, &progress_timer_for_back);
        stop_transcript_window(&transcript_state_for_back, &transcript_timer_for_back);
    });

    let weak = window.as_weak();
    let player_for_play = player.clone();
    window.on_meeting_detail_audio_play_pause_requested(move || {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let guard = player_for_play.borrow();
        let Some(p) = guard.as_ref() else {
            return;
        };
        if p.is_playing() {
            p.pause();
        } else {
            p.play();
        }
        window.set_meeting_detail_audio_is_playing(p.is_playing());
    });

    let player_for_seek = player.clone();
    window.on_meeting_detail_audio_seek_requested(move |fraction| {
        if let Some(p) = player_for_seek.borrow().as_ref() {
            p.seek_to(fraction);
        }
    });

    let player_for_paragraph = player.clone();
    window.on_meeting_detail_transcript_paragraph_clicked(move |_session_index, start_time| {
        if let Some(p) = player_for_paragraph.borrow().as_ref() {
            p.seek_to_seconds(f64::from(start_time));
        }
    });

    let handle = tauri_handle.clone();
    window.on_meeting_detail_transcript_alias_save_requested(move |term, pronunciation| {
        let term = term.trim().to_string();
        if term.is_empty() {
            return;
        }
        let pronunciation = pronunciation.trim();
        let pronunciation = (!pronunciation.is_empty()).then(|| pronunciation.to_string());
        let state = handle.state::<AppState>();
        if let Err(e) =
            souffle_lib::commands::add_dictionary_entry(state, term, pronunciation, None)
        {
            eprintln!("Failed to add dictionary alias: {e}");
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

    // SOU-188: Settings shell. `settings_state` caches the full `AppSettings`
    // struct while the sheet is open, since `save_settings` always writes
    // the whole object back (matching `saveSettings()` in
    // controller.svelte.ts) - every field-level callback below mutates this
    // cache and re-saves it, it never redeclares state on the Slint side.
    let settings_state: Rc<RefCell<Option<AppSettings>>> = Rc::new(RefCell::new(None));
    let settings_log_timer: Rc<RefCell<Option<slint::Timer>>> = Rc::new(RefCell::new(None));
    // Not part of `AppSettings` (see `get_shortcuts`/`save_shortcuts`), so it
    // gets its own cache next to `settings_state` rather than folding into it.
    let shortcuts_state: Rc<RefCell<Option<ShortcutSettings>>> = Rc::new(RefCell::new(None));
    // The modifier text (e.g. "MetaLeft") of a bare modifier key press still
    // waiting to see whether it is released alone (-> commit) or followed by
    // a real key (-> that combo wins instead) - mirrors `modifierDownEvent`
    // in controller.svelte.ts's `handleKeyDown`/`handleKeyUp`.
    let pending_modifier: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
    // Cached so device-picker/microphone-list callbacks (move/hide/remove,
    // label->uid resolution) don't each re-query CoreAudio.
    let audio_devices_state: Rc<RefCell<Vec<AudioInputDevice>>> = Rc::new(RefCell::new(Vec::new()));
    // Cached so the model picker/download button don't each re-query Ollama.
    let summary_status_state: Rc<RefCell<Option<souffle_lib::summary::SummaryProvidersStatus>>> =
        Rc::new(RefCell::new(None));
    // Which summary template's name/prompt are shown in the edit fields -
    // mirrors `editingTemplateId` in controller.svelte.ts (local UI state,
    // not part of `AppSettings`).
    let summary_template_editing: Rc<RefCell<String>> = Rc::new(RefCell::new(String::new()));
    // Cached so a model switch resolves the clicked label without re-querying
    // the catalog (a pure static list, but avoids an extra command round trip
    // on every pick).
    let model_options_state: Rc<RefCell<Vec<model_ui::FlatModelOption>>> =
        Rc::new(RefCell::new(Vec::new()));
    let snippets_list_state: Rc<RefCell<Vec<souffle_lib::db::snippets::SnippetEntry>>> =
        Rc::new(RefCell::new(Vec::new()));
    // Which snippet is shown as an inline edit form - mirrors `editingId` in
    // SnippetsSettingsSection.svelte (local UI state, not persisted).
    let snippet_editing: Rc<RefCell<Option<i64>>> = Rc::new(RefCell::new(None));

    let weak = window.as_weak();
    let handle = tauri_handle.clone();
    let settings_state_for_open = settings_state.clone();
    let shortcuts_state_for_open = shortcuts_state.clone();
    let audio_devices_state_for_open = audio_devices_state.clone();
    let summary_status_state_for_open = summary_status_state.clone();
    let summary_template_editing_for_open = summary_template_editing.clone();
    let model_options_state_for_open = model_options_state.clone();
    let snippets_list_state_for_open = snippets_list_state.clone();
    let snippet_editing_for_open = snippet_editing.clone();
    let settings_log_timer_for_open = settings_log_timer.clone();
    window.on_settings_requested(move || {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let state = handle.state::<AppState>();
        match souffle_lib::commands::get_settings(state) {
            Ok(settings) => {
                settings_ui::populate(&window, &settings);
                data_ui::populate(&window, &settings);
                *settings_state_for_open.borrow_mut() = Some(settings);
                window.set_settings_open(true);
                refresh_settings_log_tail(&window);
                *settings_log_timer_for_open.borrow_mut() =
                    Some(start_settings_log_timer(weak.clone()));
            }
            Err(e) => eprintln!("Failed to load settings: {e}"),
        }
        match souffle_lib::commands::get_data_stats(handle.state::<AppState>()) {
            Ok(stats) => data_ui::populate_stats(&window, &stats),
            Err(e) => eprintln!("Failed to load data stats: {e}"),
        }
        match souffle_lib::commands::get_mcp_setup_info() {
            Ok(info) => data_ui::populate_mcp(&window, &info),
            Err(e) => eprintln!("Failed to load MCP setup info: {e}"),
        }
        let state = handle.state::<AppState>();
        match souffle_lib::commands::get_shortcuts(state) {
            Ok(shortcuts) => {
                let natives = souffle_lib::commands::get_native_shortcuts();
                let tap_installed =
                    souffle_lib::commands::get_modifier_tap_status().map(|s| s.installed);
                settings_ui::populate_shortcuts(&window, &shortcuts, &natives, tap_installed);
                *shortcuts_state_for_open.borrow_mut() = Some(shortcuts);
            }
            Err(e) => eprintln!("Failed to load shortcuts: {e}"),
        }
        load_calendars_if_enabled(&window, &settings_state_for_open);
        load_audio_devices(
            &window,
            &settings_state_for_open,
            &audio_devices_state_for_open,
        );
        refresh_summary_providers(
            weak.clone(),
            handle.clone(),
            settings_state_for_open.clone(),
            summary_status_state_for_open.clone(),
            summary_template_editing_for_open.clone(),
        );
        load_transcription_model_state(
            &window,
            &handle,
            &settings_state_for_open,
            &model_options_state_for_open,
        );
        match souffle_lib::commands::list_dictionary(handle.state::<AppState>()) {
            Ok(entries) => lists_ui::populate_dictionary(&window, &entries),
            Err(e) => eprintln!("Failed to load dictionary: {e}"),
        }
        *snippet_editing_for_open.borrow_mut() = None;
        match souffle_lib::commands::list_snippets(handle.state::<AppState>()) {
            Ok(entries) => {
                lists_ui::populate_snippets(&window, &entries, None);
                *snippets_list_state_for_open.borrow_mut() = entries;
            }
            Err(e) => eprintln!("Failed to load snippets: {e}"),
        }
    });

    let weak = window.as_weak();
    let settings_log_timer_for_close = settings_log_timer.clone();
    let pending_modifier_for_close = pending_modifier.clone();
    window.on_settings_closed(move || {
        *settings_log_timer_for_close.borrow_mut() = None;
        *pending_modifier_for_close.borrow_mut() = None;
        if let Some(window) = weak.upgrade() {
            window.set_settings_recording_field("".into());
            window.set_settings_shortcut_error("".into());
        }
    });

    // Mutates `settings_state`'s cached `AppSettings` with `mutate`, saves
    // the whole object, and reports failure the same honest way every other
    // command in this file does - `eprintln!`, no popup UI yet. On success
    // the Slint property is left as the optimistic value the two-way
    // binding already applied; on failure it stays optimistic too (matches
    // `notes-changed`'s existing precedent in this file) since none of the
    // fields wired so far can fail for a reason the user could act on.
    fn save_settings_field(
        handle: &AppHandle,
        settings_state: &Rc<RefCell<Option<AppSettings>>>,
        mutate: impl FnOnce(&mut AppSettings),
    ) {
        let mut guard = settings_state.borrow_mut();
        let Some(settings) = guard.as_mut() else {
            return;
        };
        mutate(settings);
        let state = handle.state::<AppState>();
        if let Err(e) =
            souffle_lib::commands::save_settings(handle.clone(), state, settings.clone())
        {
            eprintln!("Failed to save settings: {e}");
        }
    }

    // Mirrors `applyShortcutValue()` + `saveShortcutSettings()`: writes
    // `value` into the cached `ShortcutSettings` under `field` ("toggle" or
    // "ptt"), saves the whole thing (this also re-registers the global
    // shortcut and syncs the native modifier tap, see
    // `commands::settings::save_shortcuts`), and always clears the
    // recording UI state regardless of outcome - only the error banner
    // differs between success and failure, matching the Svelte controller.
    fn apply_shortcut(
        handle: &AppHandle,
        shortcuts_state: &Rc<RefCell<Option<ShortcutSettings>>>,
        window: &MainWindow,
        field: &str,
        value: String,
    ) {
        window.set_settings_recording_field("".into());
        let mut guard = shortcuts_state.borrow_mut();
        let Some(shortcuts) = guard.as_mut() else {
            return;
        };
        match field {
            "toggle" => shortcuts.toggle = value,
            "ptt" => shortcuts.push_to_talk = value,
            _ => return,
        }
        let state = handle.state::<AppState>();
        match souffle_lib::commands::save_shortcuts(handle.clone(), state, shortcuts.clone()) {
            Ok(()) => {
                window.set_settings_shortcut_error("".into());
                let natives = souffle_lib::commands::get_native_shortcuts();
                let tap_installed =
                    souffle_lib::commands::get_modifier_tap_status().map(|s| s.installed);
                settings_ui::populate_shortcuts(window, shortcuts, &natives, tap_installed);
            }
            Err(e) => window.set_settings_shortcut_error(e.into()),
        }
    }

    let handle = tauri_handle.clone();
    let settings_state_for_autostart = settings_state.clone();
    window.on_settings_autostart_changed(move |enabled| {
        save_settings_field(&handle, &settings_state_for_autostart, |settings| {
            settings.autostart_enabled = enabled;
        });
    });

    let handle = tauri_handle.clone();
    let settings_state_for_debug = settings_state.clone();
    window.on_settings_debug_transcription_changed(move |enabled| {
        save_settings_field(&handle, &settings_state_for_debug, |settings| {
            settings.debug_transcription = enabled;
        });
    });

    let weak = window.as_weak();
    let handle = tauri_handle.clone();
    let settings_state_for_log_level = settings_state.clone();
    window.on_settings_log_level_changed(move |value| {
        let level = settings_ui::log_level_from_str(&value);
        save_settings_field(&handle, &settings_state_for_log_level, |settings| {
            settings.log_level = level;
        });
        if let Some(window) = weak.upgrade() {
            window.set_settings_log_level(value);
        }
    });

    let handle = tauri_handle.clone();
    let settings_state_for_theme = settings_state.clone();
    window.on_settings_theme_changed(move |value| {
        let theme = settings_ui::theme_from_str(&value);
        save_settings_field(&handle, &settings_state_for_theme, |settings| {
            settings.theme = theme;
        });
    });

    let handle = tauri_handle.clone();
    let settings_state_for_locale = settings_state.clone();
    window.on_settings_locale_changed(move |value| {
        save_settings_field(&handle, &settings_state_for_locale, |settings| {
            settings.locale = value.to_string();
        });
    });

    let handle = tauri_handle.clone();
    let settings_state_for_auto_paste = settings_state.clone();
    window.on_settings_auto_paste_changed(move |enabled| {
        save_settings_field(&handle, &settings_state_for_auto_paste, |settings| {
            settings.auto_paste = enabled;
        });
    });

    let handle = tauri_handle.clone();
    let settings_state_for_paste_method = settings_state.clone();
    window.on_settings_paste_method_changed(move |value| {
        let method = settings_ui::paste_method_from_str(&value);
        save_settings_field(&handle, &settings_state_for_paste_method, |settings| {
            settings.paste_method = method;
        });
    });

    let handle = tauri_handle.clone();
    let settings_state_for_paste_delay = settings_state.clone();
    window.on_settings_paste_delay_changed(move |value| {
        save_settings_field(&handle, &settings_state_for_paste_delay, |settings| {
            settings.paste_delay_ms = value.max(0) as u64;
        });
    });

    let handle = tauri_handle.clone();
    let settings_state_for_pill_hidden = settings_state.clone();
    window.on_settings_pill_hidden_changed(move |hidden| {
        save_settings_field(&handle, &settings_state_for_pill_hidden, |settings| {
            settings.pill_hidden = hidden;
        });
    });

    let handle = tauri_handle.clone();
    let settings_state_for_feedback_enabled = settings_state.clone();
    window.on_settings_feedback_sounds_enabled_changed(move |enabled| {
        save_settings_field(&handle, &settings_state_for_feedback_enabled, |settings| {
            settings.feedback_sounds_enabled = enabled;
        });
    });

    let handle = tauri_handle.clone();
    let settings_state_for_feedback_volume = settings_state.clone();
    window.on_settings_feedback_sounds_volume_changed(move |value| {
        save_settings_field(&handle, &settings_state_for_feedback_volume, |settings| {
            settings.feedback_sounds_volume = value.clamp(0, 100) as u32;
        });
    });

    let weak = window.as_weak();
    let handle = tauri_handle.clone();
    let settings_state_for_calendar_enabled = settings_state.clone();
    window.on_settings_calendar_enabled_changed(move |enabled| {
        if !enabled {
            save_settings_field(&handle, &settings_state_for_calendar_enabled, |settings| {
                settings.calendar_integration_enabled = false;
            });
            if let Some(window) = weak.upgrade() {
                settings_ui::populate_calendars(&window, &[], &[], PermState::Unknown);
            }
            return;
        }
        let Some(window) = weak.upgrade() else {
            return;
        };
        let handle = handle.clone();
        let settings_state = settings_state_for_calendar_enabled.clone();
        // Not `tauri::async_runtime::spawn`: this closes over an
        // `Rc<RefCell<..>>`, which is not `Send`. `request_permission`
        // blocks on the native TCC prompt internally (off its own thread via
        // `spawn_blocking`), so awaiting it on Slint's single-threaded local
        // executor is exactly what it's for.
        slint::spawn_local(async move {
            let permission = souffle_lib::commands::request_permission(PermissionKind::Calendar)
                .await
                .unwrap_or(PermState::Denied);
            if permission != PermState::Granted {
                window.set_settings_calendar_enabled(false);
                settings_ui::populate_calendars(&window, &[], &[], permission);
                return;
            }
            save_settings_field(&handle, &settings_state, |settings| {
                settings.calendar_integration_enabled = true;
            });
            window.set_settings_calendar_enabled(true);
            let selected_ids = settings_state
                .borrow()
                .as_ref()
                .map(|s| s.calendar_selected_ids.clone())
                .unwrap_or_default();
            match souffle_lib::calendar::list_calendars() {
                Ok(list) => settings_ui::populate_calendars(
                    &window,
                    &list,
                    &selected_ids,
                    PermState::Granted,
                ),
                Err(_) => settings_ui::populate_calendars(&window, &[], &[], PermState::Denied),
            }
        })
        .expect("slint event loop not running");
    });

    let handle = tauri_handle.clone();
    let settings_state_for_calendar_autostart = settings_state.clone();
    window.on_settings_calendar_autostart_enabled_changed(move |enabled| {
        save_settings_field(
            &handle,
            &settings_state_for_calendar_autostart,
            |settings| {
                settings.calendar_autostart_enabled = enabled;
            },
        );
    });

    let handle = tauri_handle.clone();
    let settings_state_for_calendar_reminder = settings_state.clone();
    window.on_settings_calendar_reminder_minutes_changed(move |value| {
        save_settings_field(&handle, &settings_state_for_calendar_reminder, |settings| {
            settings.calendar_reminder_minutes = value.clamp(1, 30) as u32;
        });
    });

    let weak = window.as_weak();
    let handle = tauri_handle.clone();
    let settings_state_for_calendar_toggle = settings_state.clone();
    window.on_settings_calendar_toggled(move |id| {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let model = window.get_settings_calendars();
        let all_ids: Vec<String> = (0..model.row_count())
            .filter_map(|i| model.row_data(i))
            .map(|row| row.id.to_string())
            .collect();
        let mut guard = settings_state_for_calendar_toggle.borrow_mut();
        let Some(settings) = guard.as_mut() else {
            return;
        };
        let effective = if settings.calendar_selected_ids.is_empty() {
            all_ids.clone()
        } else {
            settings.calendar_selected_ids.clone()
        };
        let id_string = id.to_string();
        let mut next = effective.clone();
        if let Some(pos) = next.iter().position(|existing| existing == &id_string) {
            next.remove(pos);
        } else {
            next.push(id_string);
        }
        if next.is_empty() {
            // Mirrors `toggleCalendarSelected()`: the last checked calendar
            // cannot be unchecked.
            return;
        }
        settings.calendar_selected_ids = if next.len() == all_ids.len() {
            Vec::new()
        } else {
            next
        };
        let selected_ids = settings.calendar_selected_ids.clone();
        let state = handle.state::<AppState>();
        if let Err(e) =
            souffle_lib::commands::save_settings(handle.clone(), state, settings.clone())
        {
            eprintln!("Failed to save settings: {e}");
        }
        drop(guard);
        match souffle_lib::calendar::list_calendars() {
            Ok(calendars) => settings_ui::populate_calendars(
                &window,
                &calendars,
                &selected_ids,
                PermState::Granted,
            ),
            Err(_) => eprintln!("Failed to reload calendars after toggle"),
        }
    });

    window.on_settings_calendar_open_system_settings_requested(move || {
        souffle_lib::commands::open_calendar_settings();
    });

    let weak = window.as_weak();
    let handle = tauri_handle.clone();
    let settings_state_for_device = settings_state.clone();
    let audio_devices_state_for_device = audio_devices_state.clone();
    window.on_settings_device_changed(move |label| {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let uid = audio_ui::resolve_device_uid(&audio_devices_state_for_device.borrow(), &label)
            .unwrap_or_default();
        let state = handle.state::<AppState>();
        if let Err(e) =
            souffle_lib::commands::select_audio_device(handle.clone(), state, uid.clone())
        {
            eprintln!("Failed to select audio device: {e}");
        }
        save_settings_field(&handle, &settings_state_for_device, |settings| {
            settings.audio_device = if uid.is_empty() { None } else { Some(uid) };
        });
        load_audio_devices(
            &window,
            &settings_state_for_device,
            &audio_devices_state_for_device,
        );
    });

    let weak = window.as_weak();
    let settings_state_for_refresh = settings_state.clone();
    let audio_devices_state_for_refresh = audio_devices_state.clone();
    window.on_settings_refresh_devices_requested(move || {
        if let Some(window) = weak.upgrade() {
            load_audio_devices(
                &window,
                &settings_state_for_refresh,
                &audio_devices_state_for_refresh,
            );
        }
    });

    let handle = tauri_handle.clone();
    let settings_state_for_bt = settings_state.clone();
    window.on_settings_allow_bluetooth_mic_changed(move |allowed| {
        save_settings_field(&handle, &settings_state_for_bt, |settings| {
            settings.allow_bluetooth_mic = allowed;
        });
    });

    let handle = tauri_handle.clone();
    let settings_state_for_clamshell = settings_state.clone();
    let audio_devices_state_for_clamshell = audio_devices_state.clone();
    window.on_settings_clamshell_device_changed(move |label| {
        let uid = audio_ui::resolve_device_uid(&audio_devices_state_for_clamshell.borrow(), &label);
        save_settings_field(&handle, &settings_state_for_clamshell, |settings| {
            settings.clamshell_audio_device = uid;
        });
    });

    let handle = tauri_handle.clone();
    let settings_state_for_system_audio = settings_state.clone();
    window.on_settings_capture_system_audio_changed(move |enabled| {
        save_settings_field(&handle, &settings_state_for_system_audio, |settings| {
            settings.capture_system_audio = enabled;
        });
    });

    let handle = tauri_handle.clone();
    let settings_state_for_meeting_lang = settings_state.clone();
    window.on_settings_meeting_transcription_language_changed(move |value| {
        let language = settings_ui::meeting_transcription_language_from_str(&value);
        save_settings_field(&handle, &settings_state_for_meeting_lang, |settings| {
            settings.meeting_transcription_language = language;
        });
    });

    let handle = tauri_handle.clone();
    let settings_state_for_autostop_enabled = settings_state.clone();
    window.on_settings_meeting_autostop_enabled_changed(move |enabled| {
        save_settings_field(&handle, &settings_state_for_autostop_enabled, |settings| {
            settings.meeting_autostop_enabled = enabled;
        });
    });

    let handle = tauri_handle.clone();
    let settings_state_for_autostop_minutes = settings_state.clone();
    window.on_settings_meeting_autostop_changed(move |label| {
        let Some(minutes) = audio_ui::parse_minute_label(&label) else {
            return;
        };
        save_settings_field(&handle, &settings_state_for_autostop_minutes, |settings| {
            settings.meeting_autostop_minutes = minutes;
        });
    });

    let handle = tauri_handle.clone();
    let settings_state_for_max_duration = settings_state.clone();
    window.on_settings_meeting_max_duration_changed(move |label| {
        let Some(minutes) = audio_ui::parse_minute_label(&label) else {
            return;
        };
        save_settings_field(&handle, &settings_state_for_max_duration, |settings| {
            settings.meeting_max_duration_minutes = minutes;
        });
    });

    let handle = tauri_handle.clone();
    let settings_state_for_vad = settings_state.clone();
    window.on_settings_vad_enabled_changed(move |enabled| {
        save_settings_field(&handle, &settings_state_for_vad, |settings| {
            settings.vad_enabled = enabled;
        });
    });

    let handle = tauri_handle.clone();
    let settings_state_for_filler = settings_state.clone();
    window.on_settings_filler_removal_changed(move |enabled| {
        save_settings_field(&handle, &settings_state_for_filler, |settings| {
            settings.filler_removal = enabled;
        });
    });

    let handle = tauri_handle.clone();
    let settings_state_for_stutter = settings_state.clone();
    window.on_settings_stutter_collapse_changed(move |enabled| {
        save_settings_field(&handle, &settings_state_for_stutter, |settings| {
            settings.stutter_collapse = enabled;
        });
    });

    let handle = tauri_handle.clone();
    let settings_state_for_dictionary = settings_state.clone();
    window.on_settings_dictionary_correction_changed(move |enabled| {
        save_settings_field(&handle, &settings_state_for_dictionary, |settings| {
            settings.dictionary_correction = enabled;
        });
    });

    let weak = window.as_weak();
    let settings_state_for_move = settings_state.clone();
    let audio_devices_state_for_move = audio_devices_state.clone();
    let handle = tauri_handle.clone();
    window.on_settings_move_device_requested(move |uid, direction| {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let devices = audio_devices_state_for_move.borrow().clone();
        let mut guard = settings_state_for_move.borrow_mut();
        let Some(settings) = guard.as_mut() else {
            return;
        };
        let list = microphone_list::build_microphone_list(&devices, &settings.input_priority);
        let Some(next) = microphone_list::reorder_microphone_list(&list, &uid, direction) else {
            return;
        };
        settings.input_priority.priorities = next;
        let state = handle.state::<AppState>();
        if let Err(e) =
            souffle_lib::commands::save_settings(handle.clone(), state, settings.clone())
        {
            eprintln!("Failed to save settings: {e}");
        }
        drop(guard);
        load_audio_devices(
            &window,
            &settings_state_for_move,
            &audio_devices_state_for_move,
        );
    });

    let weak = window.as_weak();
    let settings_state_for_hide = settings_state.clone();
    let audio_devices_state_for_hide = audio_devices_state.clone();
    let handle = tauri_handle.clone();
    window.on_settings_toggle_hidden_requested(move |uid, hidden| {
        let Some(window) = weak.upgrade() else {
            return;
        };
        save_settings_field(&handle, &settings_state_for_hide, |settings| {
            let hidden_set = &mut settings.input_priority.hidden;
            hidden_set.retain(|existing| existing.as_str() != uid.as_str());
            if hidden {
                hidden_set.push(uid.to_string());
            }
        });
        load_audio_devices(
            &window,
            &settings_state_for_hide,
            &audio_devices_state_for_hide,
        );
    });

    let weak = window.as_weak();
    let settings_state_for_remove = settings_state.clone();
    let audio_devices_state_for_remove = audio_devices_state.clone();
    let handle = tauri_handle.clone();
    window.on_settings_remove_device_requested(move |uid| {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let state = handle.state::<AppState>();
        let mut guard = settings_state_for_remove.borrow_mut();
        let Some(settings) = guard.as_mut() else {
            return;
        };
        settings.input_priority =
            microphone_list::remove_known_device(&settings.input_priority, &uid);
        if settings.audio_device.as_deref() == Some(uid.as_str()) {
            settings.audio_device = None;
        }
        if settings.clamshell_audio_device.as_deref() == Some(uid.as_str()) {
            settings.clamshell_audio_device = None;
        }
        if let Err(e) =
            souffle_lib::commands::save_settings(handle.clone(), state, settings.clone())
        {
            eprintln!("Failed to save settings: {e}");
        }
        drop(guard);
        load_audio_devices(
            &window,
            &settings_state_for_remove,
            &audio_devices_state_for_remove,
        );
    });

    let weak = window.as_weak();
    let settings_state_for_reset_devices = settings_state.clone();
    let audio_devices_state_for_reset_devices = audio_devices_state.clone();
    let handle = tauri_handle.clone();
    window.on_settings_reset_devices_requested(move || {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let connected = souffle_lib::commands::list_audio_devices().unwrap_or_default();
        let connected_uids: Vec<String> = connected.iter().map(|d| d.uid.clone()).collect();
        save_settings_field(&handle, &settings_state_for_reset_devices, |settings| {
            settings
                .input_priority
                .priorities
                .retain(|uid| connected_uids.contains(uid));
            settings
                .input_priority
                .hidden
                .retain(|uid| connected_uids.contains(uid));
            settings
                .input_priority
                .known
                .retain(|entry| connected_uids.contains(&entry.uid));
        });
        load_audio_devices(
            &window,
            &settings_state_for_reset_devices,
            &audio_devices_state_for_reset_devices,
        );
    });

    let weak = window.as_weak();
    let settings_state_for_reset_rate = settings_state.clone();
    let audio_devices_state_for_reset_rate = audio_devices_state.clone();
    window.on_settings_reset_sample_rate_requested(move || {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let selected = settings_state_for_reset_rate
            .borrow()
            .as_ref()
            .and_then(|s| s.audio_device.clone())
            .unwrap_or_default();
        let devices = audio_devices_state_for_reset_rate.borrow().clone();
        let Some(uid) =
            audio_ui::resolve_sample_rate_device_uid(&selected, &devices).map(String::from)
        else {
            return;
        };
        window.set_settings_resetting_sample_rate(true);
        let weak = weak.clone();
        slint::spawn_local(async move {
            let result = souffle_lib::commands::reset_input_sample_rate(uid).await;
            if let Some(window) = weak.upgrade() {
                window.set_settings_resetting_sample_rate(false);
                match result {
                    Ok(hz) => {
                        window.set_settings_sample_rate_label(
                            audio_ui::format_sample_rate_hz(hz).into(),
                        );
                        window.set_settings_sample_rate_high(
                            audio_ui::sample_rate_blocks_conferencing(hz),
                        );
                    }
                    Err(e) => eprintln!("Failed to reset sample rate: {e}"),
                }
            }
        })
        .expect("slint event loop not running");
    });

    let handle = tauri_handle.clone();
    let settings_state_for_retention = settings_state.clone();
    window.on_settings_meeting_audio_retention_changed(move |value| {
        let retention = data_ui::meeting_audio_retention_from_str(&value);
        save_settings_field(&handle, &settings_state_for_retention, |settings| {
            settings.meeting_audio_retention = retention;
        });
    });

    let weak = window.as_weak();
    let handle = tauri_handle.clone();
    window.on_settings_data_export_requested(move || {
        let Some(window) = weak.upgrade() else {
            return;
        };
        // Native macOS folder picker, not the `@tauri-apps/plugin-dialog`
        // the Svelte version uses - see `data_section.slint`'s doc comment.
        let output = std::process::Command::new("osascript")
            .arg("-e")
            .arg("POSIX path of (choose folder with prompt \"Choisir un dossier d'export\")")
            .output();
        let dir = match output {
            Ok(out) if out.status.success() => {
                String::from_utf8_lossy(&out.stdout).trim().to_string()
            }
            _ => return, // cancelled or picker failed
        };
        window.set_settings_data_exporting(true);
        window.set_settings_data_export_status("".into());
        let state = handle.state::<AppState>();
        let result = souffle_lib::commands::export_archive(handle.clone(), state, dir.clone());
        window.set_settings_data_exporting(false);
        match result {
            Ok(()) => window.set_settings_data_export_status(
                format!("Export démarré vers {dir}. Il continue en arrière-plan.").into(),
            ),
            Err(e) => {
                window.set_settings_data_export_status(e.into());
                window.set_settings_data_export_status_is_error(true);
                return;
            }
        }
        window.set_settings_data_export_status_is_error(false);
    });

    window.on_settings_data_reveal_requested(move || {
        if let Err(e) = souffle_lib::commands::reveal_data_dir() {
            eprintln!("Failed to reveal data directory: {e}");
        }
    });

    let weak = window.as_weak();
    window.on_settings_mcp_copy_desktop_snippet_requested(move || {
        let Some(window) = weak.upgrade() else {
            return;
        };
        copy_to_clipboard(&window.get_settings_mcp_claude_desktop_snippet());
    });

    let weak = window.as_weak();
    window.on_settings_mcp_copy_code_command_requested(move || {
        let Some(window) = weak.upgrade() else {
            return;
        };
        copy_to_clipboard(&window.get_settings_mcp_claude_code_command());
    });

    let weak = window.as_weak();
    window.on_settings_mcp_test_requested(move || {
        let Some(window) = weak.upgrade() else {
            return;
        };
        window.set_settings_testing_mcp(true);
        window.set_settings_mcp_test_status("".into());
        match souffle_lib::commands::test_mcp_connection() {
            Ok(tools) => {
                window.set_settings_mcp_test_status(format!("Connexion réussie ({tools}).").into())
            }
            Err(e) => window.set_settings_mcp_test_status(e.into()),
        }
        window.set_settings_testing_mcp(false);
    });

    let handle = tauri_handle.clone();
    let settings_state_for_auto_update = settings_state.clone();
    window.on_settings_auto_update_check_changed(move |enabled| {
        save_settings_field(&handle, &settings_state_for_auto_update, |settings| {
            settings.auto_update_check_enabled = enabled;
        });
    });

    let weak = window.as_weak();
    window.on_settings_check_updates_requested(move || {
        let Some(window) = weak.upgrade() else {
            return;
        };
        window.set_settings_checking_updates(true);
        window.set_settings_update_status("".into());
        slint::spawn_local(async move {
            let result = souffle_lib::commands::check_for_updates().await;
            window.set_settings_checking_updates(false);
            match result {
                Ok(update) => {
                    let status = if let Some(err) = update.check_error {
                        err
                    } else if update.update_available {
                        format!(
                            "Mise à jour disponible : v{}",
                            update.latest_version.unwrap_or_default()
                        )
                    } else {
                        "À jour.".to_string()
                    };
                    window.set_settings_update_status(status.into());
                }
                Err(e) => window.set_settings_update_status(e.into()),
            }
        })
        .expect("slint event loop not running");
    });

    let weak = window.as_weak();
    let handle = tauri_handle.clone();
    let settings_state_for_provider = settings_state.clone();
    let summary_status_state_for_provider = summary_status_state.clone();
    window.on_settings_summary_provider_changed(move |value| {
        let provider = ia_ui::summary_provider_from_str(&value);
        save_settings_field(&handle, &settings_state_for_provider, |settings| {
            settings.summary_provider = provider;
        });
        let Some(window) = weak.upgrade() else {
            return;
        };
        let guard = settings_state_for_provider.borrow();
        let status_guard = summary_status_state_for_provider.borrow();
        if let (Some(settings), Some(status)) = (guard.as_ref(), status_guard.as_ref()) {
            ia_ui::populate_intelligence(&window, settings, status);
            let provider_available = window.get_settings_summary_unusable_message().is_empty();
            ia_ui::populate_dictation_polish(&window, settings, provider_available);
        }
    });

    let handle = tauri_handle.clone();
    let settings_state_for_ollama_url = settings_state.clone();
    window.on_settings_ollama_url_changed(move |value| {
        save_settings_field(&handle, &settings_state_for_ollama_url, |settings| {
            settings.ollama_url = value.to_string();
        });
    });

    let handle = tauri_handle.clone();
    let settings_state_for_ollama_model = settings_state.clone();
    let summary_status_state_for_ollama_model = summary_status_state.clone();
    window.on_settings_ollama_model_changed(move |label| {
        let Some(model_id) = summary_status_state_for_ollama_model
            .borrow()
            .as_ref()
            .and_then(|status| ia_ui::resolve_summary_model_id(&status.models, &label))
        else {
            return;
        };
        save_settings_field(&handle, &settings_state_for_ollama_model, |settings| {
            settings.ollama_model = model_id;
        });
    });

    let weak = window.as_weak();
    let handle = tauri_handle.clone();
    let settings_state_for_retry = settings_state.clone();
    let summary_status_state_for_retry = summary_status_state.clone();
    let summary_template_editing_for_retry = summary_template_editing.clone();
    window.on_settings_summary_providers_retry_requested(move || {
        refresh_summary_providers(
            weak.clone(),
            handle.clone(),
            settings_state_for_retry.clone(),
            summary_status_state_for_retry.clone(),
            summary_template_editing_for_retry.clone(),
        );
    });

    let weak = window.as_weak();
    let handle = tauri_handle.clone();
    let settings_state_for_pull = settings_state.clone();
    let summary_status_state_for_pull = summary_status_state.clone();
    let summary_template_editing_for_pull = summary_template_editing.clone();
    window.on_settings_download_recommended_ollama_model_requested(move || {
        let Some(window) = weak.upgrade() else {
            return;
        };
        window.set_settings_ollama_pulling(true);
        window.set_settings_ollama_pull_error("".into());
        let channel = ollama_pull_channel(weak.clone());
        let weak = weak.clone();
        let handle = handle.clone();
        let settings_state_for_pull = settings_state_for_pull.clone();
        let summary_status_state_for_pull = summary_status_state_for_pull.clone();
        let summary_template_editing_for_pull = summary_template_editing_for_pull.clone();
        slint::spawn_local(async move {
            let state = handle.state::<AppState>();
            let result = souffle_lib::commands::pull_recommended_ollama_model(state, channel).await;
            let Some(window) = weak.upgrade() else {
                return;
            };
            window.set_settings_ollama_pulling(false);
            match result {
                Ok(_) => refresh_summary_providers(
                    weak,
                    handle,
                    settings_state_for_pull,
                    summary_status_state_for_pull,
                    summary_template_editing_for_pull,
                ),
                Err(e) => window.set_settings_ollama_pull_error(e.into()),
            }
        })
        .expect("slint event loop not running");
    });

    window.on_settings_open_apple_intelligence_settings_requested(move || {
        souffle_lib::commands::open_apple_intelligence_settings();
    });

    let handle = tauri_handle.clone();
    let weak = window.as_weak();
    let settings_state_for_polish_enabled = settings_state.clone();
    window.on_settings_dictation_polish_enabled_changed(move |enabled| {
        save_settings_field(&handle, &settings_state_for_polish_enabled, |settings| {
            settings.dictation_polish_enabled = enabled;
        });
        if let Some(window) = weak.upgrade() {
            let guard = settings_state_for_polish_enabled.borrow();
            if let Some(settings) = guard.as_ref() {
                let provider_available = window.get_settings_summary_unusable_message().is_empty();
                ia_ui::populate_dictation_polish(&window, settings, provider_available);
            }
        }
    });

    let weak = window.as_weak();
    let handle = tauri_handle.clone();
    let settings_state_for_polish_template = settings_state.clone();
    window.on_settings_dictation_polish_template_changed(move |label| {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let Some(template_id) = settings_state_for_polish_template
            .borrow()
            .as_ref()
            .and_then(|settings| {
                ia_ui::resolve_dictation_polish_id(&settings.dictation_polish_templates, &label)
            })
        else {
            return;
        };
        save_settings_field(&handle, &settings_state_for_polish_template, |settings| {
            settings.dictation_polish_template_id = template_id;
        });
        let guard = settings_state_for_polish_template.borrow();
        if let Some(settings) = guard.as_ref() {
            let provider_available = window.get_settings_summary_unusable_message().is_empty();
            ia_ui::populate_dictation_polish(&window, settings, provider_available);
        }
    });

    let weak = window.as_weak();
    let handle = tauri_handle.clone();
    let settings_state_for_polish_prompt = settings_state.clone();
    window.on_settings_dictation_polish_prompt_changed(move |text| {
        save_settings_field(&handle, &settings_state_for_polish_prompt, |settings| {
            let active_id = settings.dictation_polish_template_id.clone();
            if let Some(template) = settings
                .dictation_polish_templates
                .iter_mut()
                .find(|t| t.id == active_id)
            {
                template.prompt = text.to_string();
            }
        });
        if let Some(window) = weak.upgrade() {
            let guard = settings_state_for_polish_prompt.borrow();
            if let Some(settings) = guard.as_ref() {
                let provider_available = window.get_settings_summary_unusable_message().is_empty();
                ia_ui::populate_dictation_polish(&window, settings, provider_available);
            }
        }
    });

    let weak = window.as_weak();
    let handle = tauri_handle.clone();
    let settings_state_for_template_default = settings_state.clone();
    let summary_template_editing_for_default = summary_template_editing.clone();
    window.on_settings_summary_template_default_changed(move |label| {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let Some(template_id) = settings_state_for_template_default
            .borrow()
            .as_ref()
            .and_then(|settings| {
                ia_ui::resolve_summary_template_id(&settings.summary_templates, &label)
            })
        else {
            return;
        };
        save_settings_field(&handle, &settings_state_for_template_default, |settings| {
            settings.default_summary_template_id = template_id;
        });
        let guard = settings_state_for_template_default.borrow();
        if let Some(settings) = guard.as_ref() {
            let editing_id = summary_template_editing_for_default.borrow().clone();
            ia_ui::populate_summary_templates(&window, settings, &editing_id);
        }
    });

    let weak = window.as_weak();
    let settings_state_for_edit_target = settings_state.clone();
    let summary_template_editing_for_edit_target = summary_template_editing.clone();
    window.on_settings_summary_template_edit_target_changed(move |label| {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let guard = settings_state_for_edit_target.borrow();
        let Some(settings) = guard.as_ref() else {
            return;
        };
        let Some(template_id) =
            ia_ui::resolve_summary_template_id(&settings.summary_templates, &label)
        else {
            return;
        };
        *summary_template_editing_for_edit_target.borrow_mut() = template_id.clone();
        ia_ui::populate_summary_templates(&window, settings, &template_id);
    });

    let weak = window.as_weak();
    let handle = tauri_handle.clone();
    let settings_state_for_template_delete = settings_state.clone();
    let summary_template_editing_for_delete = summary_template_editing.clone();
    window.on_settings_summary_template_delete_requested(move || {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let mut editing_id = summary_template_editing_for_delete.borrow().clone();
        if editing_id.is_empty() {
            editing_id = settings_state_for_template_delete
                .borrow()
                .as_ref()
                .map(|s| s.default_summary_template_id.clone())
                .unwrap_or_default();
        }
        if editing_id.is_empty() || ia_ui::is_builtin_summary_template(&editing_id) {
            return;
        }
        save_settings_field(&handle, &settings_state_for_template_delete, |settings| {
            settings.summary_templates.retain(|t| t.id != editing_id);
            if settings.default_summary_template_id == editing_id {
                settings.default_summary_template_id = settings
                    .summary_templates
                    .first()
                    .map(|t| t.id.clone())
                    .unwrap_or_else(|| "default".to_string());
            }
        });
        *summary_template_editing_for_delete.borrow_mut() = String::new();
        let guard = settings_state_for_template_delete.borrow();
        if let Some(settings) = guard.as_ref() {
            ia_ui::populate_summary_templates(&window, settings, "");
        }
    });

    let weak = window.as_weak();
    let handle = tauri_handle.clone();
    let settings_state_for_template_name = settings_state.clone();
    let summary_template_editing_for_name = summary_template_editing.clone();
    window.on_settings_summary_template_name_changed(move |text| {
        let editing_id = summary_template_editing_for_name.borrow().clone();
        save_settings_field(&handle, &settings_state_for_template_name, |settings| {
            if let Some(template) = settings
                .summary_templates
                .iter_mut()
                .find(|t| t.id == editing_id)
            {
                template.name = text.to_string();
            }
        });
        if let Some(window) = weak.upgrade() {
            let guard = settings_state_for_template_name.borrow();
            if let Some(settings) = guard.as_ref() {
                let editing_id = summary_template_editing_for_name.borrow().clone();
                ia_ui::populate_summary_templates(&window, settings, &editing_id);
            }
        }
    });

    let weak = window.as_weak();
    let handle = tauri_handle.clone();
    let settings_state_for_template_prompt = settings_state.clone();
    let summary_template_editing_for_prompt = summary_template_editing.clone();
    window.on_settings_summary_template_prompt_changed(move |text| {
        let editing_id = summary_template_editing_for_prompt.borrow().clone();
        save_settings_field(&handle, &settings_state_for_template_prompt, |settings| {
            if let Some(template) = settings
                .summary_templates
                .iter_mut()
                .find(|t| t.id == editing_id)
            {
                template.prompt = text.to_string();
            }
        });
        if let Some(window) = weak.upgrade() {
            let guard = settings_state_for_template_prompt.borrow();
            if let Some(settings) = guard.as_ref() {
                let editing_id = summary_template_editing_for_prompt.borrow().clone();
                ia_ui::populate_summary_templates(&window, settings, &editing_id);
            }
        }
    });

    let weak = window.as_weak();
    let handle = tauri_handle.clone();
    let settings_state_for_template_add = settings_state.clone();
    let summary_template_editing_for_add = summary_template_editing.clone();
    window.on_settings_summary_template_add_requested(move |name| {
        let name = name.trim().to_string();
        if name.is_empty() {
            return;
        }
        let new_id = uuid::Uuid::new_v4().to_string();
        let new_id_for_editing = new_id.clone();
        save_settings_field(&handle, &settings_state_for_template_add, |settings| {
            let base_prompt = settings
                .summary_templates
                .iter()
                .find(|t| t.id == "default")
                .map(|t| t.prompt.clone())
                .unwrap_or_default();
            settings
                .summary_templates
                .push(souffle_lib::settings::SummaryTemplate {
                    id: new_id.clone(),
                    name: name.clone(),
                    prompt: base_prompt,
                });
        });
        *summary_template_editing_for_add.borrow_mut() = new_id_for_editing.clone();
        if let Some(window) = weak.upgrade() {
            let guard = settings_state_for_template_add.borrow();
            if let Some(settings) = guard.as_ref() {
                ia_ui::populate_summary_templates(&window, settings, &new_id_for_editing);
            }
        }
    });

    let weak = window.as_weak();
    let handle = tauri_handle.clone();
    let settings_state_for_model = settings_state.clone();
    let model_options_state_for_model = model_options_state.clone();
    window.on_settings_model_changed(move |label| {
        let Some(window) = weak.upgrade() else {
            return;
        };
        if window.get_recording_mode() != RecordingMode::Idle {
            window.set_settings_model_error_message(
                "Impossible de changer de modèle pendant un enregistrement.".into(),
            );
            // Unlike the Svelte version's `finally { target.value =
            // selectedKey; }`, the combobox's own displayed value is not
            // forced back here: Slint's binding only reasserts on a real
            // value change, and the rejected pick never became the real
            // selection. Documented known gap - the error message above is
            // the source of truth, the visible combobox label may lag it
            // until the next real selection.
            return;
        }
        let Some(option) = model_ui::find_option(&model_options_state_for_model.borrow(), &label)
            .map(|o| {
                (
                    o.engine_id.clone(),
                    o.model_id.clone(),
                    o.backend_id.clone(),
                    o.selection(),
                )
            })
        else {
            return;
        };
        let (engine_id, model_id, backend_id, selection) = option;
        save_settings_field(&handle, &settings_state_for_model, |settings| {
            settings.transcription_engine_id = engine_id;
            settings.transcription_model_id = model_id;
            settings.transcription_backend_id = backend_id;
        });
        window.set_settings_selected_model_label(label);
        start_model_transition(weak.clone(), handle.clone(), selection);
    });

    let handle = tauri_handle.clone();
    let settings_state_for_unload_timeout = settings_state.clone();
    window.on_settings_unload_timeout_changed(move |label| {
        let Some(minutes) = model_ui::parse_unload_timeout_label(&label) else {
            return;
        };
        save_settings_field(&handle, &settings_state_for_unload_timeout, |settings| {
            settings.model_unload_timeout_minutes = minutes;
        });
    });

    let handle = tauri_handle.clone();
    let settings_state_for_learn = settings_state.clone();
    window.on_settings_dictation_learn_from_edit_changed(move |enabled| {
        save_settings_field(&handle, &settings_state_for_learn, |settings| {
            settings.dictation_learn_from_edit = enabled;
        });
    });

    let weak = window.as_weak();
    let handle = tauri_handle.clone();
    window.on_settings_dictionary_add_requested(move |term, pronunciation, category| {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let state = handle.state::<AppState>();
        let pronunciation = (!pronunciation.is_empty()).then(|| pronunciation.to_string());
        let category = (!category.is_empty()).then(|| category.to_string());
        if let Err(e) = souffle_lib::commands::add_dictionary_entry(
            state,
            term.to_string(),
            pronunciation,
            category,
        ) {
            window.set_settings_dictionary_add_error(e.into());
            return;
        }
        window.set_settings_dictionary_add_error("".into());
        let state = handle.state::<AppState>();
        match souffle_lib::commands::list_dictionary(state) {
            Ok(entries) => lists_ui::populate_dictionary(&window, &entries),
            Err(e) => eprintln!("Failed to reload dictionary: {e}"),
        }
    });

    let weak = window.as_weak();
    let handle = tauri_handle.clone();
    window.on_settings_dictionary_delete_requested(move |id| {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let state = handle.state::<AppState>();
        if let Err(e) = souffle_lib::commands::delete_dictionary_entry(state, id as i64) {
            eprintln!("Failed to delete dictionary entry: {e}");
            return;
        }
        let state = handle.state::<AppState>();
        match souffle_lib::commands::list_dictionary(state) {
            Ok(entries) => lists_ui::populate_dictionary(&window, &entries),
            Err(e) => eprintln!("Failed to reload dictionary: {e}"),
        }
    });

    let weak = window.as_weak();
    let handle = tauri_handle.clone();
    let snippets_list_state_for_add = snippets_list_state.clone();
    window.on_settings_snippet_add_requested(move |trigger, expansion| {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let state = handle.state::<AppState>();
        if let Err(e) = souffle_lib::commands::add_snippet(state, &trigger, &expansion) {
            window.set_settings_snippet_add_error(e.into());
            return;
        }
        window.set_settings_snippet_add_error("".into());
        let state = handle.state::<AppState>();
        match souffle_lib::commands::list_snippets(state) {
            Ok(entries) => {
                lists_ui::populate_snippets(&window, &entries, None);
                *snippets_list_state_for_add.borrow_mut() = entries;
            }
            Err(e) => eprintln!("Failed to reload snippets: {e}"),
        }
    });

    let weak = window.as_weak();
    let handle = tauri_handle.clone();
    let snippets_list_state_for_delete = snippets_list_state.clone();
    let snippet_editing_for_delete = snippet_editing.clone();
    window.on_settings_snippet_delete_requested(move |id| {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let state = handle.state::<AppState>();
        if let Err(e) = souffle_lib::commands::delete_snippet(state, id as i64) {
            eprintln!("Failed to delete snippet: {e}");
            return;
        }
        let state = handle.state::<AppState>();
        match souffle_lib::commands::list_snippets(state) {
            Ok(entries) => {
                let editing = *snippet_editing_for_delete.borrow();
                lists_ui::populate_snippets(&window, &entries, editing);
                *snippets_list_state_for_delete.borrow_mut() = entries;
            }
            Err(e) => eprintln!("Failed to reload snippets: {e}"),
        }
    });

    let weak = window.as_weak();
    let snippets_list_state_for_edit = snippets_list_state.clone();
    let snippet_editing_for_edit = snippet_editing.clone();
    window.on_settings_snippet_edit_requested(move |id| {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let id = id as i64;
        let entries = snippets_list_state_for_edit.borrow();
        let Some(entry) = entries.iter().find(|e| e.id == id) else {
            return;
        };
        window.set_settings_edit_snippet_trigger_draft(entry.trigger.as_str().into());
        window.set_settings_edit_snippet_expansion_draft(entry.expansion.as_str().into());
        window.set_settings_snippet_update_error("".into());
        *snippet_editing_for_edit.borrow_mut() = Some(id);
        lists_ui::populate_snippets(&window, &entries, Some(id));
    });

    let weak = window.as_weak();
    let snippets_list_state_for_cancel = snippets_list_state.clone();
    let snippet_editing_for_cancel = snippet_editing.clone();
    window.on_settings_snippet_cancel_edit_requested(move || {
        *snippet_editing_for_cancel.borrow_mut() = None;
        if let Some(window) = weak.upgrade() {
            let entries = snippets_list_state_for_cancel.borrow();
            lists_ui::populate_snippets(&window, &entries, None);
        }
    });

    let weak = window.as_weak();
    let handle = tauri_handle.clone();
    let snippets_list_state_for_save = snippets_list_state.clone();
    let snippet_editing_for_save = snippet_editing.clone();
    window.on_settings_snippet_save_edit_requested(move |id, trigger, expansion| {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let state = handle.state::<AppState>();
        if let Err(e) =
            souffle_lib::commands::update_snippet(state, id as i64, &trigger, &expansion)
        {
            window.set_settings_snippet_update_error(e.into());
            return;
        }
        window.set_settings_snippet_update_error("".into());
        *snippet_editing_for_save.borrow_mut() = None;
        let state = handle.state::<AppState>();
        match souffle_lib::commands::list_snippets(state) {
            Ok(entries) => {
                lists_ui::populate_snippets(&window, &entries, None);
                *snippets_list_state_for_save.borrow_mut() = entries;
            }
            Err(e) => eprintln!("Failed to reload snippets: {e}"),
        }
    });

    let weak = window.as_weak();
    window.on_settings_shortcut_record_requested(move |field| {
        if let Some(window) = weak.upgrade() {
            window.set_settings_recording_field(field);
            window.set_settings_shortcut_error("".into());
        }
    });

    let weak = window.as_weak();
    let handle = tauri_handle.clone();
    let shortcuts_state_for_capture = shortcuts_state.clone();
    let pending_modifier_for_capture = pending_modifier.clone();
    window.on_settings_shortcut_captured(move |field, text, ctrl, shift, alt, meta| {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let modifiers = shortcut_capture::Modifiers {
            control: ctrl,
            shift,
            alt,
            meta,
        };
        if let Some(name) = shortcut_capture::modifier_only_shortcut(&text) {
            // Wait for the matching key-released event (see
            // `on_settings_shortcut_released`) instead of committing now -
            // a real key pressed while this is still held wins instead.
            *pending_modifier_for_capture.borrow_mut() = Some(name.to_string());
            return;
        }
        *pending_modifier_for_capture.borrow_mut() = None;
        if shortcut_capture::missing_modifier(&text, modifiers) {
            window.set_settings_shortcut_error(
                "Le raccourci doit inclure une touche de modification (Cmd, Ctrl, Maj, Alt) ou être une touche de fonction.".into(),
            );
            return;
        }
        let Some(value) = shortcut_capture::format_combo(&text, modifiers) else {
            return;
        };
        apply_shortcut(&handle, &shortcuts_state_for_capture, &window, &field, value);
    });

    let weak = window.as_weak();
    let handle = tauri_handle.clone();
    let shortcuts_state_for_release = shortcuts_state.clone();
    let pending_modifier_for_release = pending_modifier.clone();
    window.on_settings_shortcut_released(move |field, text, _ctrl, _shift, _alt, _meta| {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let Some(name) = shortcut_capture::modifier_only_shortcut(&text) else {
            return;
        };
        let mut pending = pending_modifier_for_release.borrow_mut();
        if pending.as_deref() != Some(name) {
            return;
        }
        *pending = None;
        drop(pending);
        apply_shortcut(
            &handle,
            &shortcuts_state_for_release,
            &window,
            &field,
            name.to_string(),
        );
    });

    let weak = window.as_weak();
    let handle = tauri_handle.clone();
    let shortcuts_state_for_clear = shortcuts_state.clone();
    let pending_modifier_for_clear = pending_modifier.clone();
    window.on_settings_shortcut_cleared(move |field| {
        let Some(window) = weak.upgrade() else {
            return;
        };
        *pending_modifier_for_clear.borrow_mut() = None;
        apply_shortcut(
            &handle,
            &shortcuts_state_for_clear,
            &window,
            &field,
            String::new(),
        );
    });

    let weak = window.as_weak();
    let pending_modifier_for_cancel = pending_modifier.clone();
    window.on_settings_shortcut_cancelled(move || {
        *pending_modifier_for_cancel.borrow_mut() = None;
        if let Some(window) = weak.upgrade() {
            window.set_settings_recording_field("".into());
            window.set_settings_shortcut_error("".into());
        }
    });

    window.on_settings_permissions_review_requested(move || {
        let _ = std::process::Command::new("open")
            .arg("x-apple.systempreferences:com.apple.preference.security")
            .spawn();
    });

    let weak = window.as_weak();
    let handle = tauri_handle.clone();
    window.on_settings_copy_diagnostics_requested(move || {
        let Some(window) = weak.upgrade() else {
            return;
        };
        window.set_settings_copying_diagnostics(true);
        let state = handle.state::<AppState>();
        let text = souffle_lib::commands::get_diagnostics_text(state);
        window.set_settings_copying_diagnostics(false);
        match text {
            Ok(text) => copy_to_clipboard(&text),
            Err(e) => eprintln!("Failed to build diagnostics text: {e}"),
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
    window.set_settings_app_version(souffle_lib::commands::get_app_version().version.into());

    refresh_timeline(&window, &tauri_handle);
    let onboarding_open = wire_onboarding_callbacks(&window, tauri_handle.clone());
    wire_update_dialogs(&window, tauri_handle.clone(), onboarding_open);
    wire_callbacks(&window, tauri_handle);

    window.run().expect("event loop failed");
}
