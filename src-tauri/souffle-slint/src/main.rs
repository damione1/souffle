// SOU-187/191: standalone Slint shell backed by the real `AppState` (audio
// thread, engine actor, database) via `souffle_lib::bootstrap::bootstrap()` -
// no webview, no Tauri runtime. AC5 pattern: state and actions cross the
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
mod permissions_ui;
mod settings_drafts;
mod settings_io;
mod settings_ui;
mod settings_values;
mod shortcut_capture;
mod summary;
mod timeline;
mod transcript;

use settings_values::SettingsCache;
use slint::Model;
use souffle_lib::audio::AudioInputDevice;
use souffle_lib::calendar::CalendarEvent;
use souffle_lib::commands::SettingsSaveOutcome;
use souffle_lib::engine::{
    Speaker, TranscriptionProfileSelection, TranscriptionRuntimePhase, TranscriptionSegment,
};
use souffle_lib::native::bridge::{AppView, NativeAction};
use souffle_lib::permissions::{PermState, PermissionKind as DomainPermissionKind};
use souffle_lib::progress::ProgressChannel;
use souffle_lib::settings::{AppSettings, ShortcutSettings};
use souffle_lib::state::AppState;
use souffle_lib::transcript::{MeetingCalendarContext, MeetingParticipant, MeetingTranscript};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

/// `AppHandle` was Tauri's cloneable, 'static, `Send`+`Sync` handle used to
/// reach `AppState` from any thread/task. `Arc<AppState>` has exactly the
/// same properties directly, so this alias keeps every `handle: AppHandle`
/// parameter below unchanged, while removing the Tauri dependency itself.
type AppHandle = Arc<AppState>;

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
const ARCHIVE_EXPORT_POLL: Duration = Duration::from_millis(150);

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

fn choose_archive_export_folder() -> Result<Option<String>, String> {
    let output = std::process::Command::new("osascript")
        .arg("-e")
        .arg("POSIX path of (choose folder with prompt \"Choisir un dossier d'export\")")
        .output()
        .map_err(|error| format!("Ouvrir le sélecteur de dossier : {error}"))?;

    if output.status.success() {
        let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
        return Ok((!path.is_empty()).then_some(path));
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    if stderr.contains("-128") || stderr.contains("User canceled") {
        Ok(None)
    } else {
        Err(stderr.trim().to_string())
    }
}

fn monitor_archive_export(weak: slint::Weak<MainWindow>, destination: String) {
    let worker = souffle_lib::async_runtime::spawn(async move {
        loop {
            if let Some(progress) = souffle_lib::commands::get_archive_export_progress()
                && progress.finished
            {
                return progress;
            }
            tokio::time::sleep(ARCHIVE_EXPORT_POLL).await;
        }
    });

    slint::spawn_local(async move {
        let result = worker
            .await
            .map_err(|error| format!("Suivi de l'export interrompu : {error}"));
        let Some(window) = weak.upgrade() else {
            return;
        };
        window.set_settings_data_exporting(false);
        match result {
            Ok(progress) => match progress.error {
                Some(error) => {
                    window.set_settings_data_export_status(error.into());
                    window.set_settings_data_export_status_is_error(true);
                }
                None => {
                    window.set_settings_data_export_status(
                        format!("Export terminé vers {destination}.").into(),
                    );
                    window.set_settings_data_export_status_is_error(false);
                }
            },
            Err(error) => {
                window.set_settings_data_export_status(error.into());
                window.set_settings_data_export_status_is_error(true);
            }
        }
    })
    .expect("slint event loop not running");
}

fn log_settings_save_outcome(context: &str, outcome: &SettingsSaveOutcome) {
    match outcome {
        SettingsSaveOutcome::Observed { result, .. } => {
            if let Err(error) = result {
                eprintln!("Failed to save {context}: {error}");
            }
        }
        SettingsSaveOutcome::Unavailable { result, read_error } => {
            eprintln!("{context} state unavailable after save {result:?}: {read_error}");
        }
    }
}

fn settings_save_failure_message(outcome: &SettingsSaveOutcome) -> String {
    let result = match outcome {
        SettingsSaveOutcome::Observed { result, .. }
        | SettingsSaveOutcome::Unavailable { result, .. } => result,
    };
    match result {
        Ok(()) => String::new(),
        Err(error) => error.user_message(),
    }
}

fn settle_onboarding_completion(
    outcome: &SettingsSaveOutcome,
    committed: impl FnOnce(),
    retained: impl FnOnce(String),
) {
    match settings_values::save_outcome_commit_status(outcome) {
        settings_values::SettingsCommitStatus::Committed => committed(),
        settings_values::SettingsCommitStatus::NotCommitted => {
            retained(settings_save_failure_message(outcome))
        }
    }
}

fn should_clear_committed_summary_add_draft(
    submitted_revision: u64,
    latest_revision: u64,
    submitted: &str,
    current: &str,
) -> bool {
    submitted_revision == latest_revision && submitted == current
}

fn clear_committed_summary_add_draft(
    window: &MainWindow,
    submitted_revision: u64,
    latest_revision: u64,
    submitted: &str,
) {
    if should_clear_committed_summary_add_draft(
        submitted_revision,
        latest_revision,
        submitted,
        window.get_settings_new_summary_template_draft().as_str(),
    ) {
        window.set_settings_new_summary_template_draft("".into());
    }
}

fn prime_summary_template_editor(
    window: &MainWindow,
    settings: &AppSettings,
    editing: &Rc<RefCell<String>>,
    drafts: &Rc<settings_drafts::SettingsDraftController>,
) {
    let current = editing.borrow().clone();
    let editing_id = settings_drafts::summary_editing_id(&current, settings);
    *editing.borrow_mut() = editing_id.clone();
    ia_ui::populate_summary_templates(window, settings, &editing_id);
    drafts.reapply_summary_template(window, &editing_id);
}

fn wire_summary_template_edit_callbacks(
    window: &MainWindow,
    settings_state: SettingsCache,
    editing: Rc<RefCell<String>>,
    drafts: Rc<settings_drafts::SettingsDraftController>,
) {
    let weak = window.as_weak();
    let settings_state_for_name = settings_state;
    let editing_for_name = editing.clone();
    let drafts_for_name = drafts.clone();
    window.on_settings_summary_template_name_changed(move |text| {
        let editing_id = editing_for_name.borrow().clone();
        if editing_id.is_empty() {
            return;
        }
        let weak = weak.clone();
        let settings_state = settings_state_for_name.clone();
        drafts_for_name.edit_summary_name(editing_id.clone(), text.to_string(), move |result| {
            match result {
                settings_drafts::DraftFlushResult::Committed => {
                    if let Some(window) = weak.upgrade() {
                        let guard = settings_state.borrow();
                        if let Some(settings) = guard.as_ref() {
                            ia_ui::populate_summary_templates(&window, settings, &editing_id);
                        }
                    }
                }
                settings_drafts::DraftFlushResult::NothingPending
                | settings_drafts::DraftFlushResult::RetainedAfterFailure => {}
            }
        });
    });

    let editing_for_prompt = editing;
    let drafts_for_prompt = drafts;
    window.on_settings_summary_template_prompt_changed(move |text| {
        let editing_id = editing_for_prompt.borrow().clone();
        if !editing_id.is_empty() {
            drafts_for_prompt.edit_summary_prompt(editing_id, text.to_string());
        }
    });
}

/// Re-fetches dictations + meetings from the real database and rebuilds the
/// Timeline model - mirrors features/timeline/controller.svelte.ts's
/// `refresh()`. Reads the current filter/search straight off the window
/// (already the source of truth via its in-out properties) rather than
/// threading them through as parameters.
fn refresh_timeline(window: &MainWindow, tauri_handle: &AppHandle) {
    let kind_filter = window.get_kind_filter();
    let search_query = window.get_search_query().to_string();

    let state = Arc::clone(tauri_handle);
    let dictations = match souffle_lib::commands::list_dictation_entries(state, Some(200)) {
        Ok(entries) => entries,
        Err(e) => {
            eprintln!("Failed to list dictation entries: {e}");
            Vec::new()
        }
    };
    let state = Arc::clone(tauri_handle);
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
fn refresh_settings_log_tail(
    window: &MainWindow,
    settings_io: Rc<settings_io::SettingsIoCoordinator>,
) {
    let Some(token) = settings_io.current_load_token() else {
        return;
    };
    let weak = window.as_weak();
    let worker = souffle_lib::async_runtime::spawn_blocking(move || {
        souffle_lib::commands::get_log_tail(SETTINGS_LOG_TAIL_LINES)
    });
    slint::spawn_local(async move {
        match worker.await {
            Ok(Ok(tail)) => {
                if settings_io.accepts_load(token)
                    && let Some(window) = weak.upgrade()
                {
                    window.set_settings_log_tail(tail.into());
                }
            }
            Ok(Err(error)) => eprintln!("Failed to read log tail: {error}"),
            Err(error) => eprintln!("Failed to join log tail worker: {error}"),
        }
    })
    .expect("slint event loop not running");
}

/// Polls the log tail every 2s while Settings is open - mirrors the Svelte
/// section's `setInterval`. Stopped on `settings-closed` (see
/// `stop_settings_log_timer`), not left running once the sheet is gone.
fn start_settings_log_timer(
    weak: slint::Weak<MainWindow>,
    settings_io: Rc<settings_io::SettingsIoCoordinator>,
) -> slint::Timer {
    let timer = slint::Timer::default();
    timer.start(slint::TimerMode::Repeated, SETTINGS_LOG_POLL, move || {
        let Some(window) = weak.upgrade() else {
            return;
        };
        refresh_settings_log_tail(&window, settings_io.clone());
    });
    timer
}

fn spawn_settings_load_stage<T: Send + 'static>(
    weak: slint::Weak<MainWindow>,
    settings_io: Rc<settings_io::SettingsIoCoordinator>,
    token: settings_io::SettingsLoadToken,
    label: &'static str,
    worker: tokio::task::JoinHandle<Result<T, String>>,
    publish: impl FnOnce(&MainWindow, T) + 'static,
) {
    slint::spawn_local(async move {
        let result = match worker.await {
            Ok(result) => result,
            Err(error) => Err(format!("Failed to join {label} worker: {error}")),
        };
        if !settings_io.accepts_load(token) {
            return;
        }
        match result {
            Ok(value) => {
                if let Some(window) = weak.upgrade() {
                    publish(&window, value);
                }
            }
            Err(error) => eprintln!("Failed to load {label}: {error}"),
        }
    })
    .expect("slint event loop not running");
}

/// Populates the calendar picker without prompting - mirrors
/// `loadCalendars()`'s own guard (only queries EventKit when the
/// integration is already on, which implies access was granted before).
fn load_calendars_if_enabled(
    window: &MainWindow,
    settings_state: &SettingsCache,
    settings_io: Rc<settings_io::SettingsIoCoordinator>,
) {
    let Some(token) = settings_io.current_load_token() else {
        return;
    };
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
        settings_ui::populate_calendars(
            window,
            &[],
            &[],
            souffle_lib::calendar::authorization_state(),
        );
        return;
    }
    let weak = window.as_weak();
    let worker = souffle_lib::async_runtime::spawn_blocking(souffle_lib::calendar::list_calendars);
    slint::spawn_local(async move {
        let result = worker.await;
        if !settings_io.accepts_load(token) {
            return;
        }
        let Some(window) = weak.upgrade() else {
            return;
        };
        match result {
            Ok(Ok(calendars)) => settings_ui::populate_calendars(
                &window,
                &calendars,
                &selected_ids,
                PermState::Granted,
            ),
            Ok(Err(_)) | Err(_) => {
                settings_ui::populate_calendars(&window, &[], &selected_ids, PermState::Denied)
            }
        }
    })
    .expect("slint event loop not running");
}

fn apply_upcoming(
    window: &MainWindow,
    events: &[CalendarEvent],
    cache: &Rc<RefCell<Vec<CalendarEvent>>>,
) {
    *cache.borrow_mut() = events.to_vec();
    let rows = timeline::upcoming_rows(events, chrono::Utc::now());
    window.set_upcoming_events(std::rc::Rc::new(slint::VecModel::from(rows)).into());
}

/// Port of calendar/controller.svelte.ts `refresh()`. EventKit stays off
/// the UI thread (`list_todays_calendar_events` is spawn_blocking inside).
fn refresh_upcoming(
    window: &MainWindow,
    handle: &AppHandle,
    cache: Rc<RefCell<Vec<CalendarEvent>>>,
) {
    if !window.get_settings_calendar_enabled() {
        apply_upcoming(window, &[], &cache);
        return;
    }
    let handle = handle.clone();
    let weak = window.as_weak();
    slint::spawn_local(async move {
        match souffle_lib::commands::list_todays_calendar_events(handle).await {
            Ok(today) => {
                if let Some(window) = weak.upgrade() {
                    window.set_settings_calendar_permission(settings_ui::perm_state_to_slint(
                        today.permission,
                    ));
                    apply_upcoming(&window, &today.events, &cache);
                }
            }
            Err(e) => eprintln!("Failed to load today's calendar: {e}"),
        }
    })
    .expect("slint event loop not running");
}

async fn start_meeting_from_event(
    handle: AppHandle,
    weak: slint::Weak<MainWindow>,
    event: CalendarEvent,
) -> Result<(), String> {
    ensure_model_ready(&handle).await?;
    let title = event.title.clone();
    let context = MeetingCalendarContext {
        event_id: event.id.clone(),
        participants: event.participants.clone(),
        description: event.description.clone(),
    };
    run_on_main_thread(move || {
        souffle_lib::async_runtime::block_on(async move {
            let state = Arc::clone(&handle);
            souffle_lib::commands::start_meeting_recording(
                state,
                title,
                Some(context),
                live_segment_channel(weak),
            )
            .await
        })
    })
    .await
}

/// Refreshes the audio device list, both device pickers, and the
/// microphone priority list, then kicks off an async sample-rate fetch for
/// whichever device `resolve_sample_rate_device_uid` picks - mirrors
/// `refreshDevices()` + the `$effect` that calls `refreshInputSampleRate()`
/// on device/selection change in controller.svelte.ts.
/// Runs the real `check_summary_providers()` network/availability check and
/// repopulates the whole IA tab from the result - shared by settings-open
/// and the "Retry" button, since both need the same round trip. `slint::
/// spawn_local`, not `souffle_lib::async_runtime::spawn`: this closes over
/// `Rc<RefCell<..>>` state, which is not `Send`.
struct SummaryRefreshState {
    generation: Cell<u64>,
    url_edited: Cell<bool>,
    status: Rc<RefCell<Option<souffle_lib::summary::SummaryProvidersStatus>>>,
}

impl SummaryRefreshState {
    fn new(status: Rc<RefCell<Option<souffle_lib::summary::SummaryProvidersStatus>>>) -> Self {
        Self {
            generation: Cell::new(0),
            url_edited: Cell::new(false),
            status,
        }
    }

    fn begin_session(&self) {
        let _ = self.invalidate();
        self.url_edited.set(false);
    }

    fn mark_url_edited(&self) -> u64 {
        self.url_edited.set(true);
        self.invalidate()
    }

    fn project_url_if_pristine(&self, window: &MainWindow, url: &str) {
        if !self.url_edited.get() {
            window.set_settings_ollama_url(url.into());
        }
    }

    fn begin(&self) -> u64 {
        let generation = self.generation.get().wrapping_add(1);
        self.generation.set(generation);
        generation
    }

    fn invalidate(&self) -> u64 {
        self.begin()
    }

    fn accepts(&self, generation: u64) -> bool {
        self.generation.get() == generation
    }
}

fn summary_refresh_after_settings_response(order: settings_io::SettingsResponseOrder) -> bool {
    match order {
        settings_io::SettingsResponseOrder::LatestVisible
        | settings_io::SettingsResponseOrder::Intermediate => true,
        settings_io::SettingsResponseOrder::LatestHidden
        | settings_io::SettingsResponseOrder::Stale => false,
    }
}

fn refresh_summary_providers(
    weak: slint::Weak<MainWindow>,
    handle: AppHandle,
    settings_state: SettingsCache,
    refresh_state: Rc<SummaryRefreshState>,
    summary_template_editing: Rc<RefCell<String>>,
    settings_drafts: Rc<settings_drafts::SettingsDraftController>,
    settings_io: Rc<settings_io::SettingsIoCoordinator>,
) {
    let Some(token) = settings_io.current_load_token() else {
        return;
    };
    let generation = refresh_state.begin();
    if let Some(window) = weak.upgrade() {
        window.set_settings_ollama_checking(true);
        window.set_settings_summary_refresh_error("".into());
    }
    slint::spawn_local(async move {
        let handle_for_check = handle.clone();
        // `check_summary_providers` awaits a reqwest call, which needs an
        // ambient Tokio reactor; `slint::spawn_local`'s own executor (the
        // Slint event loop) isn't one, so awaiting it directly here panics
        // with "there is no reactor running" the first time Settings opens.
        // Route it through the global Tokio runtime instead, same as
        // `load_model_in_background` does for its own blocking work.
        let status = souffle_lib::async_runtime::spawn(async move {
            let state = Arc::clone(&handle_for_check);
            souffle_lib::commands::check_summary_providers(state).await
        })
        .await
        .map_err(|e| format!("Join check_summary_providers task: {e}"))
        .and_then(|r| r);
        if !settings_io.accepts_load(token) || !refresh_state.accepts(generation) {
            return;
        }
        let Some(window) = weak.upgrade() else {
            return;
        };
        window.set_settings_ollama_checking(false);
        let status = match status {
            Ok(status) => status,
            Err(error) => {
                window.set_settings_summary_refresh_error(error.into());
                return;
            }
        };
        let guard = settings_state.borrow();
        let Some(settings) = guard.as_ref() else {
            return;
        };
        ia_ui::populate_intelligence(&window, settings, &status);
        let provider_available = window.get_settings_summary_unusable_message().is_empty();
        ia_ui::populate_dictation_polish(&window, settings, provider_available);
        settings_drafts.reapply_polish_prompt(&window, &settings.dictation_polish_template_id);
        let current_editing_id = summary_template_editing.borrow().clone();
        let editing_id = settings_drafts::summary_editing_id(&current_editing_id, settings);
        *summary_template_editing.borrow_mut() = editing_id.clone();
        ia_ui::populate_summary_templates(&window, settings, &editing_id);
        settings_drafts.reapply_summary_template(&window, &editing_id);
        drop(guard);
        *refresh_state.status.borrow_mut() = Some(status);
    })
    .expect("slint event loop not running");
}

/// Pushes the model picker's option list + current status - AC2's own
/// state (download/load) is read via `get_model_status`, never
/// reimplemented; this only formats what it returns.
fn load_transcription_model_state(
    window: &MainWindow,
    handle: &AppHandle,
    settings_state: &SettingsCache,
    model_options_state: &Rc<RefCell<Vec<model_ui::FlatModelOption>>>,
    settings_io: Rc<settings_io::SettingsIoCoordinator>,
) {
    let Some(token) = settings_io.current_load_token() else {
        return;
    };
    let unload_timeout_minutes = settings_state
        .borrow()
        .as_ref()
        .map(|s| s.model_unload_timeout_minutes)
        .unwrap_or(0);
    let unload_timeout_options =
        souffle_lib::settings::SettingsOptions::current().model_unload_timeout_minutes;
    let weak = window.as_weak();
    let model_options_state = model_options_state.clone();
    let worker_handle = handle.clone();
    let worker = souffle_lib::async_runtime::spawn_blocking(move || {
        let catalog = souffle_lib::commands::get_transcription_catalog(worker_handle.clone())?;
        let status = souffle_lib::commands::get_model_status(
            worker_handle.clone(),
            model_ui::selected_profile(&catalog),
        )?;
        let machine = worker_handle.current_machine_state();
        Ok::<_, String>((catalog, status, machine))
    });
    slint::spawn_local(async move {
        let result = match worker.await {
            Ok(result) => result,
            Err(error) => Err(format!("Failed to join model state worker: {error}")),
        };
        if !settings_io.accepts_load(token) {
            return;
        }
        let Some(window) = weak.upgrade() else {
            return;
        };
        match result {
            Ok((catalog, status, machine)) => {
                model_ui::populate_options(
                    &window,
                    &catalog,
                    unload_timeout_minutes,
                    &unload_timeout_options,
                );
                window
                    .set_header_model_label(model_ui::selected_model_short_label(&catalog).into());
                *model_options_state.borrow_mut() =
                    model_ui::list_available_model_options(&catalog);
                model_ui::populate_runtime(&window, status.phase);
                if let Ok(souffle_lib::state_machine::AppStateMachine::Error { message, .. }) =
                    machine
                {
                    window.set_settings_model_error_message(message.into());
                }
            }
            Err(error) => window.set_settings_model_error_message(error.into()),
        }
    })
    .expect("slint event loop not running");
}

/// Only this projection writes the main window's model phase. Opening Settings,
/// startup and native transitions all read the same backend snapshot, including
/// background loads and idle unloads. No poll timer or optimistic ready flag.
fn refresh_model_runtime(weak: slint::Weak<MainWindow>, handle: AppHandle) {
    let worker_handle = handle.clone();
    let worker = souffle_lib::async_runtime::spawn_blocking(move || {
        let catalog = souffle_lib::commands::get_transcription_catalog(worker_handle.clone())?;
        let status = souffle_lib::commands::get_model_status(
            worker_handle.clone(),
            model_ui::selected_profile(&catalog),
        )?;
        Ok::<_, String>((catalog, status, worker_handle.current_machine_state()))
    });
    slint::spawn_local(async move {
        let result = worker
            .await
            .map_err(|error| format!("Failed to join model refresh worker: {error}"))
            .and_then(|result| result);
        let Some(window) = weak.upgrade() else {
            return;
        };
        match result {
            Ok((catalog, status, machine)) => {
                window
                    .set_header_model_label(model_ui::selected_model_short_label(&catalog).into());
                model_ui::populate_runtime(&window, status.phase);
                if let Ok(souffle_lib::state_machine::AppStateMachine::Error { message, .. }) =
                    machine
                {
                    window.set_settings_model_error_message(message.into());
                }
            }
            Err(error) => window.set_settings_model_error_message(error.into()),
        }
    })
    .expect("slint event loop not running");
}

/// Reuse the selected-profile transition used by Settings after first-run
/// onboarding has made its choices. A configured installation never needs to
/// reselect its model just to warm the engine on a new process launch.
fn initialize_model_at_startup(
    window: &MainWindow,
    handle: AppHandle,
    onboarding_open: bool,
    catalog: &souffle_lib::engine::TranscriptionCatalog,
    phase: TranscriptionRuntimePhase,
) {
    window.set_header_model_label(model_ui::selected_model_short_label(catalog).into());
    model_ui::populate_runtime(window, phase);
    if onboarding_open {
        return;
    }
    start_model_transition(
        window.as_weak(),
        handle,
        model_ui::selected_profile(catalog),
    );
}

fn project_model_selection_snapshot(
    window: &MainWindow,
    settings: &AppSettings,
    model_options_state: &Rc<RefCell<Vec<model_ui::FlatModelOption>>>,
) -> Result<TranscriptionProfileSelection, String> {
    let catalog = souffle_lib::commands::transcription_catalog_from_settings(settings)?;
    let unload_timeout_options =
        souffle_lib::settings::SettingsOptions::current().model_unload_timeout_minutes;
    model_ui::populate_options(
        window,
        &catalog,
        settings.model_unload_timeout_minutes,
        &unload_timeout_options,
    );
    window.set_header_model_label(model_ui::selected_model_short_label(&catalog).into());
    *model_options_state.borrow_mut() = model_ui::list_available_model_options(&catalog);
    Ok(model_ui::selected_profile(&catalog))
}

fn settle_model_selection_save(
    outcome: &SettingsSaveOutcome,
    fallback: &Option<AppSettings>,
    requested: TranscriptionProfileSelection,
    project_canonical: impl FnOnce(&AppSettings) -> Result<TranscriptionProfileSelection, String>,
    committed: impl FnOnce(TranscriptionProfileSelection),
) -> Result<(), String> {
    match (
        outcome,
        settings_values::save_outcome_commit_status(outcome),
    ) {
        (
            SettingsSaveOutcome::Observed { settings, .. },
            settings_values::SettingsCommitStatus::Committed,
        ) => committed(project_canonical(settings)?),
        (
            SettingsSaveOutcome::Observed { settings, .. },
            settings_values::SettingsCommitStatus::NotCommitted,
        ) => {
            project_canonical(settings)?;
        }
        (
            SettingsSaveOutcome::Unavailable { .. },
            settings_values::SettingsCommitStatus::Committed,
        ) => committed(requested),
        (
            SettingsSaveOutcome::Unavailable { .. },
            settings_values::SettingsCommitStatus::NotCommitted,
        ) => {
            if let Some(settings) = fallback {
                project_canonical(settings)?;
            }
        }
    }
    Ok(())
}

enum AudioDeviceSaveSettlement {
    Committed {
        uid: String,
        canonical: Option<Box<AppSettings>>,
    },
    Rejected {
        canonical: Option<Box<AppSettings>>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum AudioDeviceProjectionState {
    Canonical { generation: u64 },
    PendingSave { generation: u64, uid: String },
    CommittedUnknown { generation: u64, uid: String },
}

impl AudioDeviceProjectionState {
    fn generation(&self) -> u64 {
        match self {
            Self::Canonical { generation }
            | Self::PendingSave { generation, .. }
            | Self::CommittedUnknown { generation, .. } => *generation,
        }
    }

    fn begin_pending(&mut self, uid: String) {
        *self = Self::PendingSave {
            generation: self.generation().wrapping_add(1),
            uid,
        };
    }

    fn commit_unknown(&mut self, uid: String) {
        let generation = match self {
            Self::PendingSave { generation, .. } => *generation,
            Self::Canonical { .. } | Self::CommittedUnknown { .. } => {
                self.generation().wrapping_add(1)
            }
        };
        *self = Self::CommittedUnknown { generation, uid };
    }

    fn observe_canonical(&mut self) {
        *self = Self::Canonical {
            generation: self.generation().wrapping_add(1),
        };
    }
}

impl AudioDeviceSaveSettlement {
    fn has_observed_canonical(&self) -> bool {
        match self {
            Self::Committed { canonical, .. } | Self::Rejected { canonical } => canonical.is_some(),
        }
    }
}

fn apply_audio_device_projection_settlement(
    settlement: &AudioDeviceSaveSettlement,
    projection: &mut AudioDeviceProjectionState,
) {
    match settlement {
        AudioDeviceSaveSettlement::Committed {
            uid,
            canonical: None,
        } => projection.commit_unknown(uid.clone()),
        AudioDeviceSaveSettlement::Committed {
            canonical: Some(_), ..
        }
        | AudioDeviceSaveSettlement::Rejected { .. } => projection.observe_canonical(),
    }
}

fn settle_audio_device_save(
    outcome: &SettingsSaveOutcome,
    fallback: &Option<AppSettings>,
    submitted_uid: &str,
) -> AudioDeviceSaveSettlement {
    match (
        outcome,
        settings_values::save_outcome_commit_status(outcome),
    ) {
        (
            SettingsSaveOutcome::Observed { settings, .. },
            settings_values::SettingsCommitStatus::Committed,
        ) => AudioDeviceSaveSettlement::Committed {
            uid: settings.audio_device.clone().unwrap_or_default(),
            canonical: Some(settings.clone()),
        },
        (
            SettingsSaveOutcome::Unavailable { .. },
            settings_values::SettingsCommitStatus::Committed,
        ) => AudioDeviceSaveSettlement::Committed {
            uid: submitted_uid.to_string(),
            canonical: None,
        },
        (
            SettingsSaveOutcome::Observed { settings, .. },
            settings_values::SettingsCommitStatus::NotCommitted,
        ) => AudioDeviceSaveSettlement::Rejected {
            canonical: Some(settings.clone()),
        },
        (
            SettingsSaveOutcome::Unavailable { .. },
            settings_values::SettingsCommitStatus::NotCommitted,
        ) => AudioDeviceSaveSettlement::Rejected {
            canonical: fallback.clone().map(Box::new),
        },
    }
}

fn audio_device_load_configuration(
    settings_state: &SettingsCache,
    projection_state: &Rc<RefCell<AudioDeviceProjectionState>>,
) -> Option<(
    u64,
    String,
    Option<String>,
    souffle_lib::audio::InputPriority,
)> {
    let known = settings_state.known_snapshot();
    let last_observed = settings_state.borrow().clone();
    let settings = known.as_ref().or(last_observed.as_ref())?;
    let mut projection = projection_state.borrow_mut();
    let selected = match projection.clone() {
        AudioDeviceProjectionState::Canonical { .. } => {
            settings.audio_device.clone().unwrap_or_default()
        }
        AudioDeviceProjectionState::PendingSave { uid, .. } => uid,
        AudioDeviceProjectionState::CommittedUnknown { uid, .. } => {
            if known.is_some() {
                projection.observe_canonical();
                settings.audio_device.clone().unwrap_or_default()
            } else {
                uid
            }
        }
    };
    Some((
        projection.generation(),
        selected,
        settings.clamshell_audio_device.clone(),
        settings.input_priority.clone(),
    ))
}

fn load_audio_devices(
    window: &MainWindow,
    settings_state: &SettingsCache,
    audio_devices_state: &Rc<RefCell<Vec<AudioInputDevice>>>,
    projection_state: &Rc<RefCell<AudioDeviceProjectionState>>,
    settings_io: Rc<settings_io::SettingsIoCoordinator>,
) {
    let Some(token) = settings_io.current_load_token() else {
        return;
    };
    let weak = window.as_weak();
    let devices_state = audio_devices_state.clone();
    let settings_state = settings_state.clone();
    let projection_state = projection_state.clone();
    let worker =
        souffle_lib::async_runtime::spawn_blocking(souffle_lib::commands::list_audio_devices);
    slint::spawn_local(async move {
        let devices = match worker.await {
            Ok(Ok(devices)) => devices,
            Ok(Err(error)) => {
                eprintln!("Failed to list audio devices: {error}");
                Vec::new()
            }
            Err(error) => {
                eprintln!("Failed to join audio device worker: {error}");
                Vec::new()
            }
        };
        if !settings_io.accepts_load(token) {
            return;
        }
        // Resolve the selected UID only after the CoreAudio wait. A device
        // save may have settled while the worker was blocked; capturing this
        // before `await` would let the old UID overwrite the newer picker.
        let Some((generation, selected, clamshell, priority)) =
            audio_device_load_configuration(&settings_state, &projection_state)
        else {
            return;
        };
        let Some(window) = weak.upgrade() else {
            return;
        };
        audio_ui::populate_device_pickers(&window, &devices, &selected, clamshell.as_deref());
        let list = microphone_list::build_microphone_list(&devices, &priority);
        audio_ui::populate_microphones(&window, &list);
        let rate_uid =
            audio_ui::resolve_sample_rate_device_uid(&selected, &devices).map(String::from);
        *devices_state.borrow_mut() = devices;
        window.set_settings_sample_rate_label("".into());
        window.set_settings_sample_rate_high(false);
        drop(window);
        if let Some(uid) = rate_uid {
            let rate = souffle_lib::commands::get_input_sample_rate(uid).await;
            if !settings_io.accepts_load(token) {
                return;
            }
            if projection_state.borrow().generation() != generation {
                return;
            }
            if let Ok(hz) = rate
                && let Some(window) = weak.upgrade()
            {
                window.set_settings_sample_rate_label(audio_ui::format_sample_rate_hz(hz).into());
                window.set_settings_sample_rate_high(audio_ui::sample_rate_blocks_conferencing(hz));
            }
        }
    })
    .expect("slint event loop not running");
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
    let state = Arc::clone(handle);
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
    let worker_handle = handle.clone();
    souffle_lib::async_runtime::spawn_blocking(move || {
        let catalog = souffle_lib::commands::get_transcription_catalog(worker_handle.clone())?;
        let selection = model_ui::selected_profile(&catalog);
        let status =
            souffle_lib::commands::get_model_status(worker_handle.clone(), selection.clone())?;
        match status.phase {
            TranscriptionRuntimePhase::Ready => Ok(()),
            TranscriptionRuntimePhase::LoadRequired => {
                souffle_lib::commands::load_model(worker_handle, selection)
            }
            TranscriptionRuntimePhase::DownloadRequired => {
                Err("Modèle non téléchargé - ouvrez Réglages pour le télécharger.".into())
            }
            TranscriptionRuntimePhase::Downloading
            | TranscriptionRuntimePhase::Loading
            | TranscriptionRuntimePhase::Unloading => {
                Err("Le modèle est en cours de préparation. Réessayez lorsqu’il est prêt.".into())
            }
            TranscriptionRuntimePhase::Failed => {
                Err("Le modèle est en erreur. Réessayez depuis Réglages.".into())
            }
        }
    })
    .await
    .map_err(|error| format!("Join ensure_model_ready task: {error}"))?
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
fn live_segment_channel(weak: slint::Weak<MainWindow>) -> ProgressChannel<TranscriptionSegment> {
    ProgressChannel::new(move |segment: TranscriptionSegment| {
        let weak = weak.clone();
        let _ = slint::invoke_from_event_loop(move || {
            let Some(window) = weak.upgrade() else {
                return;
            };
            if segment.is_final {
                let mut text = match segment.speaker {
                    Some(Speaker::Me) => window.get_live_me_text().to_string(),
                    Some(Speaker::Them) => window.get_live_them_text().to_string(),
                    None => window.get_live_text().to_string(),
                };
                let trimmed = segment.text.trim();
                if !trimmed.is_empty() {
                    if !text.is_empty() {
                        text.push(' ');
                    }
                    text.push_str(trimmed);
                }
                match segment.speaker {
                    Some(Speaker::Me) => window.set_live_me_text(text.into()),
                    Some(Speaker::Them) => window.set_live_them_text(text.into()),
                    None => window.set_live_text(text.into()),
                }
                window.set_live_tentative("".into());
                window.set_live_tentative_has_speaker(false);
            } else {
                window.set_live_tentative(segment.text.into());
                match segment.speaker {
                    Some(Speaker::Me) => {
                        window.set_live_tentative_speaker(SpeakerRole::Me);
                        window.set_live_tentative_has_speaker(true);
                    }
                    Some(Speaker::Them) => {
                        window.set_live_tentative_speaker(SpeakerRole::Them);
                        window.set_live_tentative_has_speaker(true);
                    }
                    None => window.set_live_tentative_has_speaker(false),
                }
            }
        });
    })
}

#[derive(Debug, PartialEq, Eq)]
struct DictationTextBuffers {
    live: String,
    tentative: String,
    me: String,
    them: String,
    recovery: String,
}

fn reset_live_buffers(mut buffers: DictationTextBuffers) -> DictationTextBuffers {
    buffers.live.clear();
    buffers.tentative.clear();
    buffers.me.clear();
    buffers.them.clear();
    buffers
}

fn merge_recovery_text(existing: &str, incoming: &str) -> String {
    match (existing.trim(), incoming.trim()) {
        ("", incoming) => incoming.to_string(),
        (existing, "") => existing.to_string(),
        (existing, incoming) => format!("{existing}\n\n——\n\n{incoming}"),
    }
}

fn clear_live_transcript(window: &MainWindow) {
    let buffers = reset_live_buffers(DictationTextBuffers {
        live: window.get_live_text().to_string(),
        tentative: window.get_live_tentative().to_string(),
        me: window.get_live_me_text().to_string(),
        them: window.get_live_them_text().to_string(),
        recovery: window.get_dictation_recovery_text().to_string(),
    });
    window.set_live_text(buffers.live.into());
    window.set_live_tentative(buffers.tentative.into());
    window.set_live_me_text(buffers.me.into());
    window.set_live_them_text(buffers.them.into());
    window.set_dictation_recovery_text(buffers.recovery.into());
    window.set_live_tentative_has_speaker(false);
}

/// Pushes each `OllamaPullProgress` update into the settings window's
/// pull-status properties - same `Channel` + `invoke_from_event_loop`
/// pattern as `live_segment_channel`, for the real download triggered by
/// "Download recommended model" in the IA tab.
fn ollama_pull_channel(
    weak: slint::Weak<MainWindow>,
) -> ProgressChannel<souffle_lib::summary::OllamaPullProgress> {
    ProgressChannel::new(move |progress: souffle_lib::summary::OllamaPullProgress| {
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
    // An explicit retry recovers the canonical machine, not a local busy flag.
    if let Ok(souffle_lib::state_machine::AppStateMachine::Error { .. }) =
        handle.current_machine_state()
        && let Err(error) = souffle_lib::commands::recover_state(Arc::clone(&handle))
    {
        if let Some(window) = weak.upgrade() {
            window.set_settings_model_error_message(error.into());
        }
        return;
    }
    let state = Arc::clone(&handle);
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
    refresh_model_runtime(weak.clone(), handle.clone());
    match status.phase {
        TranscriptionRuntimePhase::Ready => {}
        TranscriptionRuntimePhase::LoadRequired => {
            load_model_in_background(weak, handle, selection);
        }
        TranscriptionRuntimePhase::DownloadRequired => {
            window.set_settings_model_download_progress_label("".into());
            window.set_settings_model_download_progress_fraction(0.0);
            let state = Arc::clone(&handle);
            let channel = model_download_channel(weak.clone(), handle.clone(), selection.clone());
            if let Err(e) = souffle_lib::commands::download_model(state, selection, channel) {
                window.set_settings_model_error_message(e.into());
            }
        }
        // A startup load and a settings-open action can overlap. Observing an
        // in-flight operation never starts a second one.
        TranscriptionRuntimePhase::Downloading
        | TranscriptionRuntimePhase::Loading
        | TranscriptionRuntimePhase::Unloading
        | TranscriptionRuntimePhase::Failed => {}
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
        let result = souffle_lib::async_runtime::spawn_blocking(move || {
            let state = Arc::clone(&handle_for_load);
            souffle_lib::commands::load_model(state, selection_for_load)
        })
        .await
        .map_err(|e| format!("Join load_model task: {e}"))
        .and_then(|r| r);
        if let Some(window) = weak.upgrade() {
            refresh_model_runtime(weak.clone(), handle.clone());
            if let Err(e) = result {
                let phase = souffle_lib::commands::get_model_status(handle.clone(), selection)
                    .map(|status| status.phase);
                if !phase.is_ok_and(model_ui::load_is_already_in_progress_or_ready) {
                    window.set_settings_model_error_message(e.into());
                }
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
) -> ProgressChannel<souffle_lib::models::DownloadProgress> {
    let load_started = Arc::new(AtomicBool::new(false));
    ProgressChannel::new(move |progress: souffle_lib::models::DownloadProgress| {
        let weak = weak.clone();
        let handle = handle.clone();
        let selection = selection.clone();
        let load_started = Arc::clone(&load_started);
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
                    let globally_complete = model_ui::download_is_globally_complete(&progress);
                    if globally_complete && !load_started.swap(true, Ordering::AcqRel) {
                        load_model_in_background(weak.clone(), handle.clone(), selection.clone());
                    } else {
                        window.set_settings_model_download_progress_label(
                            format!(
                                "{} / {} fichiers",
                                progress.completed_files, progress.total_files
                            )
                            .into(),
                        );
                    }
                }
                souffle_lib::models::DownloadStatus::Error(e) => {
                    refresh_model_runtime(weak.clone(), handle.clone());
                    window.set_settings_model_error_message(e.as_str().into());
                }
            }
        });
    })
}

/// Runs `f` on the real Slint/OS main thread and returns its result.
///
/// Found the hard way (milestone 4): `start_transcription` internally calls
/// `dictation_cancel::sync`, which touches `app.global_shortcut()` to arm the
/// Escape-cancels-dictation binding. That registration needs a thread with a
/// live run loop; a background tokio worker thread (where a plain
/// `souffle_lib::async_runtime::spawn`ed task runs) has none, and the call hangs
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
        souffle_lib::async_runtime::block_on(async move {
            let state = Arc::clone(&handle);
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
        souffle_lib::async_runtime::block_on(async move {
            let state = Arc::clone(&handle);
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
    souffle_lib::commands::stop_transcription(handle).await
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DictationEndIntent {
    Finalize,
    Cancel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DictationTranscriptDisposition {
    Clear,
    RetainForRecovery,
}

fn dictation_transcript_disposition(
    intent: DictationEndIntent,
    succeeded: bool,
) -> DictationTranscriptDisposition {
    match intent {
        DictationEndIntent::Finalize if succeeded => DictationTranscriptDisposition::Clear,
        DictationEndIntent::Finalize => DictationTranscriptDisposition::RetainForRecovery,
        DictationEndIntent::Cancel => DictationTranscriptDisposition::Clear,
    }
}

async fn stop_meeting(handle: AppHandle) -> Result<String, String> {
    souffle_lib::commands::stop_meeting_recording(handle).await
}

async fn wait_for_meeting_finalization(handle: &AppHandle, meeting_id: &str) -> Result<(), String> {
    use souffle_lib::state_machine::{AppStateMachine, RecordingKind};

    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let machine = handle.current_machine_state()?;
        match machine {
            AppStateMachine::Stopping {
                was_recording: RecordingKind::Meeting { meeting_id: active },
                ..
            } if active == meeting_id => {}
            AppStateMachine::Ready { .. } => return Ok(()),
            AppStateMachine::Error { message, .. } => return Err(message),
            AppStateMachine::Idle
            | AppStateMachine::Downloading { .. }
            | AppStateMachine::Downloaded { .. }
            | AppStateMachine::Loading { .. }
            | AppStateMachine::RecordingDictation { .. }
            | AppStateMachine::RecordingMeeting { .. }
            | AppStateMachine::Stopping {
                was_recording: RecordingKind::Dictation,
                ..
            }
            | AppStateMachine::Stopping {
                was_recording: RecordingKind::Meeting { .. },
                ..
            }
            | AppStateMachine::Unloading { .. } => {
                return Err(
                    "La réunion a quitté son état de finalisation de façon inattendue.".into(),
                );
            }
        }
        if tokio::time::Instant::now() >= deadline {
            return Err("La finalisation de la réunion a dépassé 30 secondes.".into());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn read_live_dictation_text(weak: slint::Weak<MainWindow>) -> String {
    run_on_main_thread(move || {
        weak.upgrade()
            .map(|window| window.get_live_text().trim().to_string())
            .unwrap_or_default()
    })
    .await
}

async fn end_dictation(
    handle: AppHandle,
    weak: slint::Weak<MainWindow>,
    focused_app: Option<String>,
    intent: DictationEndIntent,
) -> Result<(), String> {
    stop_dictation(handle.clone()).await?;
    match intent {
        DictationEndIntent::Finalize => {
            let raw_text = read_live_dictation_text(weak).await;
            finalize_dictation(handle, raw_text, focused_app).await
        }
        DictationEndIntent::Cancel => Ok(()),
    }
}

/// Persist raw text before the optional network polish, then insert the final
/// text using the exact settings-owned paste contract. A polish failure never
/// loses the raw dictation; insertion failures remain visible to the caller.
async fn finalize_dictation(
    handle: AppHandle,
    raw_text: String,
    focused_app: Option<String>,
) -> Result<(), String> {
    let raw_text = raw_text.trim().to_string();
    if raw_text.is_empty() {
        return Ok(());
    }

    let entry_id =
        souffle_lib::commands::add_dictation_entry(Arc::clone(&handle), raw_text.clone())?;
    let settings = souffle_lib::commands::get_settings(Arc::clone(&handle))?;
    let polished = match tokio::time::timeout(
        Duration::from_secs(25),
        souffle_lib::commands::polish_dictation(Arc::clone(&handle), raw_text.clone(), focused_app),
    )
    .await
    {
        Ok(Ok(result)) => result.text.trim().to_string(),
        Ok(Err(error)) => {
            eprintln!("Dictation polish failed; using raw text: {error}");
            raw_text.clone()
        }
        Err(_) => {
            eprintln!("Dictation polish timed out; using raw text");
            raw_text.clone()
        }
    };
    let final_text = if polished.is_empty() {
        raw_text.clone()
    } else {
        polished
    };
    if final_text != raw_text {
        souffle_lib::commands::update_dictation_entry(
            Arc::clone(&handle),
            entry_id,
            final_text.clone(),
        )?;
    }

    if settings.auto_paste {
        if let Err(error) = souffle_lib::commands::paste_text(
            final_text.clone(),
            settings.paste_delay_ms,
            settings.paste_method,
        ) {
            return match souffle_lib::commands::copy_text(final_text) {
                Ok(()) => Err(format!(
                    "Collage impossible. Le texte a été copié dans le presse-papiers : {error}"
                )),
                Err(copy_error) => Err(format!(
                    "Collage et copie impossibles. Récupérez le texte ci-dessous : {error}; {copy_error}"
                )),
            };
        }
    } else {
        souffle_lib::commands::copy_text(final_text)?;
    }
    Ok(())
}

/// All mutable state the onboarding wizard needs across its 4 steps, in one
/// place rather than a dozen separate `Rc<RefCell<..>>`s - unlike
/// `wire_callbacks`'s per-field caches, this state is only ever touched
/// while the wizard is open, so bundling it doesn't create the same
/// cross-feature borrow-conflict risk.
#[derive(Default)]
struct OnboardingState {
    steps: Vec<&'static str>,
    step_index: usize,
    recovery_only: bool,
    devices: Vec<AudioInputDevice>,
    selected_device: String,
    model_options: Vec<model_ui::FlatModelOption>,
    selected_model_index: Option<usize>,
    toggle_shortcut: String,
    pending_modifier: Option<String>,
    auto_paste: bool,
}

fn project_startup_settings(
    window: &MainWindow,
    settings: &AppSettings,
    onboarding: &Rc<RefCell<OnboardingState>>,
) -> bool {
    let dark = settings_ui::resolve_dark(settings.theme);
    window.global::<Theme>().set_dark(dark);
    window.set_settings_calendar_enabled(settings.calendar_integration_enabled);
    window.set_onboarding_locale(settings.locale.as_str().into());
    let mut onboarding = onboarding.borrow_mut();
    onboarding.selected_device = settings.audio_device.clone().unwrap_or_default();
    onboarding.auto_paste = settings.auto_paste;
    dark
}

struct StartupRuntimeSnapshot {
    settings: AppSettings,
    catalog: souffle_lib::engine::TranscriptionCatalog,
    model_phase: TranscriptionRuntimePhase,
    setup_flags: onboarding_flags::SetupFlags,
    devices: Vec<AudioInputDevice>,
    shortcuts: ShortcutSettings,
}

fn load_startup_runtime_snapshot(
    handle: AppHandle,
    settings: AppSettings,
) -> Result<StartupRuntimeSnapshot, String> {
    let catalog = souffle_lib::commands::transcription_catalog_from_settings(&settings)?;
    let model_phase = souffle_lib::commands::get_model_status(
        handle.clone(),
        model_ui::selected_profile(&catalog),
    )
    .map(|status| status.phase)
    .unwrap_or(TranscriptionRuntimePhase::DownloadRequired);
    let setup_flags = onboarding_flags::read_setup_flags();
    let should_show = onboarding_flags::decide_show_setup_wizard(model_phase, setup_flags);
    let shortcuts = souffle_lib::commands::get_shortcuts(handle).unwrap_or_default();
    let devices = if should_show {
        souffle_lib::commands::list_audio_devices().unwrap_or_default()
    } else {
        Vec::new()
    };
    Ok(StartupRuntimeSnapshot {
        settings,
        catalog,
        model_phase,
        setup_flags,
        devices,
        shortcuts,
    })
}

fn initialize_onboarding(
    window: &MainWindow,
    startup: &StartupRuntimeSnapshot,
    handle: &AppHandle,
    ob: &Rc<RefCell<OnboardingState>>,
    permissions: &Rc<permissions_ui::PermissionController>,
) -> bool {
    let should_show =
        onboarding_flags::decide_show_setup_wizard(startup.model_phase, startup.setup_flags);
    if !should_show {
        return false;
    }

    let mut guard = ob.borrow_mut();
    guard.steps = onboarding_flags::wizard_steps(startup.setup_flags);
    guard.recovery_only = startup.setup_flags.setup_done;
    guard.devices = startup.devices.clone();
    guard.selected_device = startup.settings.audio_device.clone().unwrap_or_default();
    guard.auto_paste = startup.settings.auto_paste;
    guard.model_options = model_ui::list_available_model_options(&startup.catalog);
    guard.selected_model_index = guard.model_options.iter().position(|option| {
        option.engine_id == startup.catalog.selected_engine_id
            && option.model_id == startup.catalog.selected_model_id
    });
    guard.toggle_shortcut = startup.shortcuts.toggle.clone();
    drop(guard);

    window.set_onboarding_open(true);
    show_onboarding_step(window, handle, ob, permissions);
    permissions.sync_activity();
    true
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
    permissions: &Rc<permissions_ui::PermissionController>,
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
    // Otherwise an error from a previous step (e.g. a failed download)
    // stays pinned to the banner forever, since nothing else clears it.
    window.set_onboarding_status_message("".into());

    match step {
        "permissions" => {
            permissions.project();
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
                souffle_lib::commands::get_model_status(Arc::clone(handle), selection).ok()
            });
            // LoadRequired collapses to Pick here (not Loading): re-entering this
            // step only shows a spinner once the user has actually clicked
            // "Continuer" to kick off the load, matching prior behavior.
            let model_phase = match phase.map(|s| s.phase) {
                Some(TranscriptionRuntimePhase::Ready) => ModelPhase::Ready,
                Some(TranscriptionRuntimePhase::Downloading) => ModelPhase::Downloading,
                Some(TranscriptionRuntimePhase::Loading)
                | Some(TranscriptionRuntimePhase::Unloading) => ModelPhase::Loading,
                Some(TranscriptionRuntimePhase::DownloadRequired)
                | Some(TranscriptionRuntimePhase::LoadRequired)
                | Some(TranscriptionRuntimePhase::Failed)
                | None => ModelPhase::Pick,
            };
            window.set_onboarding_model_phase(model_phase);
            if model_phase == ModelPhase::Ready {
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
                permissions.status().accessibility == PermState::Granted,
            );
            window.set_onboarding_continue_label("Terminer".into());
        }
        _ => {}
    }
}

/// Sets up the whole onboarding overlay: decides whether to show it at
/// startup (`decide_show_setup_wizard`, never a hardcoded "first launch"
/// flag), then wires every step's callbacks. Model download/load reuse the
/// same commands `wire_callbacks`'s Settings-tab model selector calls
/// (`get_model_status`/`download_model`/`load_model`); shortcut capture
/// reuses `shortcut_capture.rs` the same way the Interface tab does.
fn wire_onboarding_callbacks(
    window: &MainWindow,
    tauri_handle: AppHandle,
    settings_io: Rc<settings_io::SettingsIoCoordinator>,
    permissions: Rc<permissions_ui::PermissionController>,
) -> Rc<RefCell<OnboardingState>> {
    let ob: Rc<RefCell<OnboardingState>> = Rc::new(RefCell::new(OnboardingState::default()));

    let weak = window.as_weak();
    let settings_io_for_locale = settings_io.clone();
    window.on_onboarding_locale_changed(move |locale| {
        let locale_value = locale.to_string();
        settings_io_for_locale.submit(
            souffle_lib::commands::SettingsSaveLane::General,
            move |settings| settings.locale = locale_value,
            |_, outcome| log_settings_save_outcome("onboarding locale", outcome),
        );
        if let Some(window) = weak.upgrade() {
            window.set_onboarding_locale(locale);
        }
    });

    let permissions_for_grant = permissions.clone();
    window.on_onboarding_grant_requested(move |slint_kind| {
        let kind = settings_ui::permission_kind_from_slint(slint_kind);
        permissions_for_grant.request(kind);
    });

    let permissions_for_hint = permissions.clone();
    window.on_onboarding_hint_action_requested(move |slint_kind| {
        permissions_for_hint.open_settings(settings_ui::permission_kind_from_slint(slint_kind));
    });

    let permissions_for_repair = permissions.clone();
    window.on_onboarding_repair_requested(move || {
        permissions_for_repair.repair_accessibility();
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
    let permissions_for_device_refresh = permissions.clone();
    window.on_onboarding_refresh_devices_requested(move || {
        let Some(window) = weak.upgrade() else {
            return;
        };
        ob_for_refresh.borrow_mut().devices =
            souffle_lib::commands::list_audio_devices().unwrap_or_default();
        show_onboarding_step(
            &window,
            &handle,
            &ob_for_refresh,
            &permissions_for_device_refresh,
        );
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

    let weak_for_auto_paste = window.as_weak();
    let ob_for_auto_paste = ob.clone();
    window.on_onboarding_auto_paste_changed(move |enabled| {
        ob_for_auto_paste.borrow_mut().auto_paste = enabled;
        if let Some(window) = weak_for_auto_paste.upgrade() {
            window.set_onboarding_auto_paste(enabled);
        }
    });

    let permissions_for_accessibility_review = permissions.clone();
    window.on_onboarding_review_accessibility_requested(move || {
        permissions_for_accessibility_review.open_settings(DomainPermissionKind::Accessibility);
    });

    let weak = window.as_weak();
    let handle = tauri_handle.clone();
    let ob_for_back = ob.clone();
    let permissions_for_back = permissions.clone();
    window.on_onboarding_back_requested(move || {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let mut guard = ob_for_back.borrow_mut();
        if guard.step_index > 0 {
            guard.step_index -= 1;
        }
        drop(guard);
        show_onboarding_step(&window, &handle, &ob_for_back, &permissions_for_back);
        permissions_for_back.sync_activity();
    });

    let weak = window.as_weak();
    let handle = tauri_handle.clone();
    let ob_for_continue = ob.clone();
    let permissions_for_continue = permissions.clone();
    let settings_io_for_continue = settings_io.clone();
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
                    &permissions_for_continue,
                );
            }
            "microphone" => {
                let uid = ob_for_continue.borrow().selected_device.clone();
                window.set_onboarding_busy(true);
                window.set_onboarding_continue_enabled(false);
                let uid_for_save = uid.clone();
                let completion_weak = weak.clone();
                let retained_weak = weak.clone();
                let completion_handle = handle.clone();
                let completion_ob = ob_for_continue.clone();
                let completion_permissions = permissions_for_continue.clone();
                settings_io_for_continue.submit(
                    souffle_lib::commands::SettingsSaveLane::General,
                    move |settings| {
                        settings.audio_device = if uid_for_save.is_empty() {
                            None
                        } else {
                            Some(uid_for_save)
                        };
                    },
                    move |_, outcome| {
                        log_settings_save_outcome("onboarding audio device", outcome);
                        settle_onboarding_completion(
                            outcome,
                            move || {
                                let Some(window) = completion_weak.upgrade() else {
                                    return;
                                };
                                match souffle_lib::commands::select_audio_device(
                                    completion_handle.clone(),
                                    uid,
                                ) {
                                    Ok(()) => advance_onboarding_step(
                                        &window,
                                        &completion_handle,
                                        &completion_ob,
                                        &completion_permissions,
                                    ),
                                    Err(error) => {
                                        window.set_onboarding_busy(false);
                                        window.set_onboarding_continue_enabled(true);
                                        window.set_onboarding_status_message(error.into());
                                    }
                                }
                            },
                            move |message| {
                                if let Some(window) = retained_weak.upgrade() {
                                    window.set_onboarding_busy(false);
                                    window.set_onboarding_continue_enabled(true);
                                    window.set_onboarding_status_message(message.into());
                                }
                            },
                        );
                    },
                );
            }
            "model" => {
                let phase = window.get_onboarding_model_phase();
                if phase == ModelPhase::Ready {
                    advance_onboarding_step(
                        &window,
                        &handle,
                        &ob_for_continue,
                        &permissions_for_continue,
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
                window.set_onboarding_busy(true);
                window.set_onboarding_continue_enabled(false);
                let completion_weak = weak.clone();
                let retained_weak = weak.clone();
                let completion_handle = handle.clone();
                settings_io_for_continue.submit(
                    souffle_lib::commands::SettingsSaveLane::General,
                    move |settings| {
                        settings.transcription_engine_id = engine_id;
                        settings.transcription_model_id = model_id;
                        settings.transcription_backend_id = backend_id;
                    },
                    move |_, outcome| {
                        log_settings_save_outcome("onboarding model", outcome);
                        settle_onboarding_completion(
                            outcome,
                            move || {
                                start_onboarding_model_transition(
                                    completion_weak,
                                    completion_handle,
                                    selection,
                                );
                            },
                            move |message| {
                                if let Some(window) = retained_weak.upgrade() {
                                    window.set_onboarding_busy(false);
                                    window.set_onboarding_continue_enabled(true);
                                    window.set_onboarding_status_message(message.into());
                                }
                            },
                        );
                    },
                );
            }
            "shortcut" => {
                let (auto_paste, recovery_only) = {
                    let guard = ob_for_continue.borrow();
                    (guard.auto_paste, guard.recovery_only)
                };
                window.set_onboarding_busy(true);
                window.set_onboarding_continue_enabled(false);
                let completion_weak = weak.clone();
                let completion_permissions = permissions_for_continue.clone();
                settings_io_for_continue.submit(
                    souffle_lib::commands::SettingsSaveLane::Autostart,
                    move |settings| {
                        settings.auto_paste = auto_paste;
                        settings.autostart_enabled = onboarding_flags::decide_autostart_on_finish(
                            recovery_only,
                            settings.autostart_enabled,
                        );
                    },
                    move |_, outcome| {
                        log_settings_save_outcome("onboarding completion", outcome);
                        let retained_weak = completion_weak.clone();
                        settle_onboarding_completion(
                            outcome,
                            move || {
                                onboarding_flags::mark_setup_complete();
                                if let Some(window) = completion_weak.upgrade() {
                                    window.set_onboarding_busy(false);
                                    window.set_onboarding_open(false);
                                }
                                completion_permissions.sync_activity();
                            },
                            move |message| {
                                if let Some(window) = retained_weak.upgrade() {
                                    window.set_onboarding_busy(false);
                                    window.set_onboarding_continue_enabled(true);
                                    window.set_onboarding_status_message(message.into());
                                }
                            },
                        );
                    },
                );
            }
            _ => {}
        }
    });

    ob
}

/// Advances to the next step, stopping/starting the permissions poll timer
/// as the wizard enters/leaves that step.
fn advance_onboarding_step(
    window: &MainWindow,
    handle: &AppHandle,
    ob: &Rc<RefCell<OnboardingState>>,
    permissions: &Rc<permissions_ui::PermissionController>,
) {
    {
        let mut guard = ob.borrow_mut();
        if guard.step_index + 1 < guard.steps.len() {
            guard.step_index += 1;
        }
    }
    show_onboarding_step(window, handle, ob, permissions);
    permissions.sync_activity();
}

fn apply_onboarding_shortcut(
    handle: &AppHandle,
    window: &MainWindow,
    ob: &Rc<RefCell<OnboardingState>>,
    value: String,
) {
    window.set_onboarding_shortcut_recording(false);
    ob.borrow_mut().toggle_shortcut = value.clone();
    let state = Arc::clone(handle);
    let mut shortcuts = souffle_lib::commands::get_shortcuts(state).unwrap_or_default();
    shortcuts.toggle = value;
    let state = Arc::clone(handle);
    match souffle_lib::commands::save_shortcuts(state, shortcuts) {
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
    if let Ok(souffle_lib::state_machine::AppStateMachine::Error { .. }) =
        handle.current_machine_state()
        && let Err(error) = souffle_lib::commands::recover_state(Arc::clone(&handle))
    {
        if let Some(window) = weak.upgrade() {
            window.set_onboarding_busy(false);
            window.set_onboarding_continue_enabled(true);
            window.set_onboarding_status_message(error.into());
        }
        return;
    }
    let state = Arc::clone(&handle);
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
    // Clear any error left over from a previous attempt in this same step
    // (e.g. a retried download) before reporting on the new one.
    window.set_onboarding_status_message("".into());
    match status.phase {
        TranscriptionRuntimePhase::Ready => {
            window.set_onboarding_model_phase(ModelPhase::Ready);
            window.set_onboarding_busy(false);
            window.set_onboarding_continue_enabled(true);
        }
        TranscriptionRuntimePhase::LoadRequired => {
            window.set_onboarding_model_phase(ModelPhase::Loading);
            let weak2 = weak.clone();
            let handle2 = handle.clone();
            let selection2 = selection.clone();
            slint::spawn_local(async move {
                let handle3 = handle2.clone();
                let result = souffle_lib::async_runtime::spawn_blocking(move || {
                    souffle_lib::commands::load_model(handle3, selection2)
                })
                .await
                .map_err(|e| format!("Join load_model task: {e}"))
                .and_then(|r| r);
                if let Some(window) = weak2.upgrade() {
                    window.set_onboarding_busy(false);
                    window.set_onboarding_continue_enabled(true);
                    match result {
                        Ok(()) => window.set_onboarding_model_phase(ModelPhase::Ready),
                        Err(e) => window.set_onboarding_status_message(e.into()),
                    }
                }
            })
            .expect("slint event loop not running");
        }
        TranscriptionRuntimePhase::DownloadRequired => {
            window.set_onboarding_model_phase(ModelPhase::Downloading);
            window.set_onboarding_download_progress_label("".into());
            window.set_onboarding_download_progress_fraction(0.0);
            let state = Arc::clone(&handle);
            let channel =
                onboarding_model_download_channel(weak.clone(), handle.clone(), selection.clone());
            if let Err(e) = souffle_lib::commands::download_model(state, selection, channel) {
                window.set_onboarding_busy(false);
                window.set_onboarding_continue_enabled(true);
                window.set_onboarding_status_message(e.into());
            }
        }
        TranscriptionRuntimePhase::Downloading => {
            window.set_onboarding_model_phase(ModelPhase::Downloading);
        }
        TranscriptionRuntimePhase::Loading | TranscriptionRuntimePhase::Unloading => {
            window.set_onboarding_model_phase(ModelPhase::Loading);
        }
        TranscriptionRuntimePhase::Failed => {
            window.set_onboarding_busy(false);
            window.set_onboarding_continue_enabled(true);
            window.set_onboarding_status_message("Le modèle est en erreur.".into());
        }
    }
}

fn onboarding_model_download_channel(
    weak: slint::Weak<MainWindow>,
    handle: AppHandle,
    selection: TranscriptionProfileSelection,
) -> ProgressChannel<souffle_lib::models::DownloadProgress> {
    let load_started = Arc::new(AtomicBool::new(false));
    ProgressChannel::new(move |progress: souffle_lib::models::DownloadProgress| {
        let weak = weak.clone();
        let handle = handle.clone();
        let selection = selection.clone();
        let load_started = Arc::clone(&load_started);
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
                    let globally_complete = model_ui::download_is_globally_complete(&progress);
                    if globally_complete && !load_started.swap(true, Ordering::AcqRel) {
                        window.set_onboarding_model_phase(ModelPhase::Loading);
                        start_onboarding_model_transition(
                            weak.clone(),
                            handle.clone(),
                            selection.clone(),
                        );
                    } else if !globally_complete {
                        window.set_onboarding_download_progress_label(
                            format!(
                                "{} / {} fichiers",
                                progress.completed_files, progress.total_files
                            )
                            .into(),
                        );
                    }
                }
                souffle_lib::models::DownloadStatus::Error(e) => {
                    window.set_onboarding_busy(false);
                    window.set_onboarding_continue_enabled(true);
                    window.set_onboarding_status_message(e.as_str().into());
                }
            }
        });
    })
}

/// "What's New" (a version bump since the last launch) + the automatic
/// "Update Available" startup check - port of `bootstrap.ts`'s
/// `whatsNew`/auto-check logic. Never stacks either dialog on top of the
/// onboarding wizard (`onboarding_open`), matching `bootstrap.ts`'s own
/// "never stacks on the wizard" comment.
fn wire_update_dialogs(
    window: &MainWindow,
    tauri_handle: AppHandle,
    settings_io: Rc<settings_io::SettingsIoCoordinator>,
) {
    let settings_io_for_dismiss = settings_io.clone();
    window.on_whats_new_dismissed(move || {
        let version = souffle_lib::commands::get_app_version().version;
        settings_io_for_dismiss.submit(
            souffle_lib::commands::SettingsSaveLane::General,
            move |settings| settings.last_seen_version = version,
            |_, outcome| log_settings_save_outcome("dismiss what's new", outcome),
        );
    });

    let weak = window.as_weak();
    window.on_update_download_requested(move || {
        let Some(window) = weak.upgrade() else {
            return;
        };
        window.set_update_phase("downloading".into());
        window.set_update_error_message("".into());
        let weak = weak.clone();
        slint::spawn_local(async move {
            // `download_update` awaits a `reqwest` fetch, which needs an
            // ambient Tokio reactor that `slint::spawn_local`'s own executor
            // doesn't provide - route it through the global Tokio runtime
            // (see `refresh_summary_providers`'s identical fix).
            let result =
                souffle_lib::async_runtime::spawn(souffle_lib::commands::download_update())
                    .await
                    .map_err(|e| format!("Join download_update task: {e}"))
                    .and_then(|r| r);
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
            let state = Arc::clone(&handle);
            if let Err(e) = souffle_lib::commands::install_update(state).await
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
}

fn apply_startup_update_dialogs(
    window: &MainWindow,
    tauri_handle: AppHandle,
    onboarding_open: bool,
    settings: AppSettings,
    settings_io: Rc<settings_io::SettingsIoCoordinator>,
) {
    if onboarding_open {
        return;
    }

    let handle = tauri_handle;
    let weak = window.as_weak();
    let app_version = souffle_lib::commands::get_app_version();
    let current_version = app_version.version.clone();
    let setup_done = onboarding_flags::read_setup_flags().setup_done;

    if app_version.is_local_build {
        // No release notes to show for a local checkout build.
    } else if !setup_done || settings.last_seen_version.is_empty() {
        // First launch / unfinished setup: stamp silently so the changelog
        // never stacks on the wizard and doesn't pop right after it either.
        if settings.last_seen_version != current_version {
            let version = current_version.clone();
            settings_io.submit(
                souffle_lib::commands::SettingsSaveLane::General,
                move |settings| settings.last_seen_version = version,
                |_, outcome| log_settings_save_outcome("startup version stamp", outcome),
            );
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
                let state = Arc::clone(&handle);
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
fn wire_callbacks(
    window: &MainWindow,
    tauri_handle: AppHandle,
    permissions: Rc<permissions_ui::PermissionController>,
    settings_state: SettingsCache,
    settings_io: Rc<settings_io::SettingsIoCoordinator>,
) {
    // Shared with load_meeting_audio/stop_audio_player/open_meeting_detail
    // and the play-pause/seek callbacks below - one loaded player at a
    // time, for whichever meeting is currently open in MeetingDetail.
    let player: Rc<RefCell<Option<audio_player::AudioPlayer>>> = Rc::new(RefCell::new(None));
    let progress_timer: Rc<RefCell<Option<slint::Timer>>> = Rc::new(RefCell::new(None));
    // Same sharing pattern, for the virtualized transcript list (AC15).
    let transcript_state: Rc<RefCell<Option<TranscriptState>>> = Rc::new(RefCell::new(None));
    let transcript_timer: Rc<RefCell<Option<slint::Timer>>> = Rc::new(RefCell::new(None));
    let upcoming_cache: Rc<RefCell<Vec<CalendarEvent>>> = Rc::new(RefCell::new(Vec::new()));
    let upcoming_timer: Rc<RefCell<Option<slint::Timer>>> = Rc::new(RefCell::new(None));
    let saved_live_notes: Rc<RefCell<Option<(String, String)>>> = Rc::new(RefCell::new(None));
    let live_notes_timer: Rc<RefCell<Option<slint::Timer>>> = Rc::new(RefCell::new(None));

    let timer = slint::Timer::default();
    let weak_for_notes = window.as_weak();
    let handle_for_notes = tauri_handle.clone();
    let saved_live_notes_for_timer = saved_live_notes.clone();
    timer.start(slint::TimerMode::Repeated, NOTES_DEBOUNCE, move || {
        let Some(window) = weak_for_notes.upgrade() else {
            return;
        };
        let meeting_id = match handle_for_notes.current_machine_state() {
            Ok(souffle_lib::state_machine::AppStateMachine::RecordingMeeting {
                meeting_id,
                ..
            }) => Some(meeting_id),
            Ok(souffle_lib::state_machine::AppStateMachine::Idle)
            | Ok(souffle_lib::state_machine::AppStateMachine::Downloading { .. })
            | Ok(souffle_lib::state_machine::AppStateMachine::Downloaded { .. })
            | Ok(souffle_lib::state_machine::AppStateMachine::Loading { .. })
            | Ok(souffle_lib::state_machine::AppStateMachine::Ready { .. })
            | Ok(souffle_lib::state_machine::AppStateMachine::RecordingDictation { .. })
            | Ok(souffle_lib::state_machine::AppStateMachine::Stopping { .. })
            | Ok(souffle_lib::state_machine::AppStateMachine::Unloading { .. })
            | Ok(souffle_lib::state_machine::AppStateMachine::Error { .. })
            | Err(_) => None,
        };
        let Some(meeting_id) = meeting_id else {
            *saved_live_notes_for_timer.borrow_mut() = None;
            return;
        };
        let notes = window.get_live_notes().to_string();
        if saved_live_notes_for_timer.borrow().as_ref()
            == Some(&(meeting_id.clone(), notes.clone()))
        {
            return;
        }
        match souffle_lib::commands::save_meeting_notes(
            Arc::clone(&handle_for_notes),
            meeting_id.clone(),
            Some(notes.clone()),
        ) {
            Ok(()) => *saved_live_notes_for_timer.borrow_mut() = Some((meeting_id, notes)),
            Err(error) => eprintln!("Failed to autosave live meeting notes: {error}"),
        }
    });
    *live_notes_timer.borrow_mut() = Some(timer);

    let weak = window.as_weak();
    let handle = tauri_handle.clone();
    window.on_dictate_requested(move || {
        let weak = weak.clone();
        let handle = handle.clone();
        souffle_lib::async_runtime::spawn(async move {
            let result = start_dictation(handle, weak.clone()).await;
            if let Err(e) = weak.upgrade_in_event_loop(move |window| match result {
                Ok(()) => {
                    clear_live_transcript(&window);
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
        souffle_lib::async_runtime::spawn(async move {
            let result = start_meeting(handle, weak.clone()).await;
            if let Err(e) = weak.upgrade_in_event_loop(move |window| match result {
                Ok(()) => {
                    clear_live_transcript(&window);
                    window.set_live_notes("".into());
                    window.set_recording_mode(RecordingMode::Meeting);
                }
                Err(e) => window.set_meeting_status_message(e.into()),
            }) {
                eprintln!("upgrade_in_event_loop failed (meeting): {e}");
            }
        });
    });

    let stop_in_flight = Arc::new(AtomicBool::new(false));
    let weak = window.as_weak();
    let handle = tauri_handle.clone();
    let stop_in_flight_for_request = Arc::clone(&stop_in_flight);
    let live_notes_timer_keepalive = live_notes_timer.clone();
    window.on_stop_requested(move || {
        let _ = &live_notes_timer_keepalive;
        if stop_in_flight_for_request.swap(true, Ordering::AcqRel) {
            return;
        }
        let Some(current) = weak.upgrade() else {
            stop_in_flight_for_request.store(false, Ordering::Release);
            return;
        };
        let mode = current.get_recording_mode();
        let live_notes = current.get_live_notes().to_string();
        let focused_app = souffle_lib::commands::frontmost_app_name().unwrap_or(None);
        let weak = weak.clone();
        let handle = handle.clone();
        let stop_in_flight = Arc::clone(&stop_in_flight_for_request);
        souffle_lib::async_runtime::spawn(async move {
            let handle_for_refresh = handle.clone();
            if mode == RecordingMode::Meeting {
                let result = match stop_meeting(handle.clone()).await {
                    Ok(meeting_id) => wait_for_meeting_finalization(&handle, &meeting_id)
                        .await
                        .map(|()| meeting_id),
                    Err(error) => Err(error),
                }
                .and_then(|meeting_id| {
                    souffle_lib::commands::save_meeting_notes(
                        Arc::clone(&handle),
                        meeting_id.clone(),
                        Some(live_notes),
                    )?;
                    Ok(meeting_id)
                });
                // Send-safe on purpose: this closure cannot capture the
                // Rc<RefCell<AudioPlayer>> player state (Rc isn't Send, and
                // upgrade_in_event_loop requires it) - re-invoking the
                // already-registered timeline-item-opened callback (which
                // does capture it, as a plain same-thread closure) reuses
                // the real open-meeting path instead of duplicating it.
                if let Err(e) = weak.upgrade_in_event_loop(move |window| {
                    clear_live_transcript(&window);
                    match result {
                        // A stopped meeting recording lands the user back on
                        // that meeting's detail, not the Timeline - they
                        // were just looking at it live.
                        Ok(meeting_id) => {
                            window.set_live_notes("".into());
                            window.set_recording_mode(RecordingMode::Idle);
                            window.invoke_timeline_item_opened(
                                TimelineKind::Meeting,
                                meeting_id.into(),
                            );
                        }
                        Err(e) => {
                            eprintln!("Failed to stop meeting: {e}");
                            window.set_recording_mode(RecordingMode::Idle);
                            window.set_meeting_status_message(e.into());
                            refresh_timeline(&window, &handle_for_refresh);
                        }
                    }
                }) {
                    eprintln!("upgrade_in_event_loop failed (stop meeting): {e}");
                }
            } else {
                let result = end_dictation(
                    handle,
                    weak.clone(),
                    focused_app,
                    DictationEndIntent::Finalize,
                )
                .await;
                if let Err(e) = weak.upgrade_in_event_loop(move |window| {
                    let disposition = dictation_transcript_disposition(
                        DictationEndIntent::Finalize,
                        result.is_ok(),
                    );
                    match result {
                        Ok(()) => {
                            window.set_transcription_status_message("".into());
                        }
                        Err(e) => {
                            eprintln!("Failed to finalize dictation: {e}");
                            window.set_transcription_status_message(e.into());
                            // Keep live-text intact: IdleView exposes it in an
                            // editable recovery card until copy/discard.
                        }
                    }
                    match disposition {
                        DictationTranscriptDisposition::Clear => clear_live_transcript(&window),
                        DictationTranscriptDisposition::RetainForRecovery => {
                            let recovery = merge_recovery_text(
                                &window.get_dictation_recovery_text(),
                                &window.get_live_text(),
                            );
                            window.set_dictation_recovery_text(recovery.into());
                            clear_live_transcript(&window);
                        }
                    }
                    window.set_recording_mode(RecordingMode::Idle);
                    refresh_timeline(&window, &handle_for_refresh);
                }) {
                    eprintln!("upgrade_in_event_loop failed (stop dictation): {e}");
                }
            }
            stop_in_flight.store(false, Ordering::Release);
        });
    });

    let weak = window.as_weak();
    let handle = tauri_handle.clone();
    let stop_in_flight_for_cancel = Arc::clone(&stop_in_flight);
    window.on_cancel_dictation_requested(move || {
        if stop_in_flight_for_cancel.swap(true, Ordering::AcqRel) {
            return;
        }
        let Some(current) = weak.upgrade() else {
            stop_in_flight_for_cancel.store(false, Ordering::Release);
            return;
        };
        if current.get_recording_mode() != RecordingMode::Dictation {
            stop_in_flight_for_cancel.store(false, Ordering::Release);
            return;
        }
        let weak = weak.clone();
        let handle = handle.clone();
        let stop_in_flight = Arc::clone(&stop_in_flight_for_cancel);
        souffle_lib::async_runtime::spawn(async move {
            let result =
                end_dictation(handle, weak.clone(), None, DictationEndIntent::Cancel).await;
            if let Err(error) = weak.upgrade_in_event_loop(move |window| {
                match dictation_transcript_disposition(DictationEndIntent::Cancel, result.is_ok()) {
                    DictationTranscriptDisposition::Clear => clear_live_transcript(&window),
                    DictationTranscriptDisposition::RetainForRecovery => {}
                }
                window.set_recording_mode(RecordingMode::Idle);
                match result {
                    Ok(()) => window.set_transcription_status_message("".into()),
                    Err(error) => {
                        eprintln!("Failed to cancel dictation cleanly: {error}");
                        window.set_transcription_status_message(error.into());
                    }
                }
            }) {
                eprintln!("upgrade_in_event_loop failed (cancel dictation): {error}");
            }
            stop_in_flight.store(false, Ordering::Release);
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
    let handle = tauri_handle.clone();
    let player_for_back = player.clone();
    let progress_timer_for_back = progress_timer.clone();
    let transcript_state_for_back = transcript_state.clone();
    let transcript_timer_for_back = transcript_timer.clone();
    window.on_meeting_detail_back(move || {
        if let Some(window) = weak.upgrade() {
            window.set_active_meeting_id("".into());
            // Port of HomeView.svelte's `$effect` that refreshes whenever
            // `showMeetingDetail` goes false - stopping a meeting lands the
            // user on its detail view without a refresh (deliberately, see
            // that stop-meeting handler's comment), so the Timeline is
            // stale until whatever closes the detail view refreshes it.
            // Without this, a just-finished meeting doesn't appear until
            // the next unrelated refresh (filter change, search, restart).
            refresh_timeline(&window, &handle);
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
        let state = Arc::clone(&handle);
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
        let state = Arc::clone(&handle);
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
            let state = Arc::clone(&handle);
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
        let state = Arc::clone(&handle);
        let result = if kind == TimelineKind::Dictation {
            souffle_lib::commands::delete_dictation_entry(state, id.to_string())
        } else {
            souffle_lib::commands::delete_meeting(state, id.to_string())
        };
        if let Err(e) = result {
            eprintln!("Failed to delete {kind:?} {id}: {e}");
        }
        if let Some(window) = weak.upgrade() {
            if kind == TimelineKind::Dictation && window.get_expanded_dictation_id() == id {
                window.set_expanded_dictation_id("".into());
            }
            refresh_timeline(&window, &handle);
        }
    });

    window.on_timeline_item_copy_requested(move |text| {
        if let Err(e) = souffle_lib::commands::copy_text(text.to_string()) {
            eprintln!("Failed to copy dictation: {e}");
        }
    });

    let weak = window.as_weak();
    let handle = tauri_handle.clone();
    let upcoming_for_start = upcoming_cache.clone();
    window.on_calendar_event_start_requested(move |occurrence_id| {
        let event = upcoming_for_start
            .borrow()
            .iter()
            .find(|event| timeline::occurrence_key(event) == occurrence_id.as_str())
            .cloned();
        let Some(event) = event else {
            eprintln!("calendar start: no cached event for {occurrence_id}");
            return;
        };
        let weak = weak.clone();
        let handle = handle.clone();
        souffle_lib::async_runtime::spawn(async move {
            let result = start_meeting_from_event(handle, weak.clone(), event).await;
            if let Err(e) = weak.upgrade_in_event_loop(move |window| match result {
                Ok(()) => {
                    clear_live_transcript(&window);
                    window.set_live_notes("".into());
                    window.set_recording_mode(RecordingMode::Meeting);
                }
                Err(e) => window.set_meeting_status_message(e.into()),
            }) {
                eprintln!("upgrade_in_event_loop failed (calendar start): {e}");
            }
        });
    });

    refresh_upcoming(window, &tauri_handle, upcoming_cache.clone());
    {
        let weak = window.as_weak();
        let cache = upcoming_cache.clone();
        let timer = slint::Timer::default();
        timer.start(
            slint::TimerMode::Repeated,
            Duration::from_secs(30),
            move || {
                let Some(window) = weak.upgrade() else {
                    return;
                };
                let events = cache.borrow().clone();
                if events.is_empty() {
                    return;
                }
                let rows = timeline::upcoming_rows(&events, chrono::Utc::now());
                window.set_upcoming_events(std::rc::Rc::new(slint::VecModel::from(rows)).into());
            },
        );
        *upcoming_timer.borrow_mut() = Some(timer);
    }

    // SOU-188: Settings shell. `settings_state` caches the full `AppSettings`
    // struct while the sheet is open, since `save_settings` always writes
    // the whole object back (matching `saveSettings()` in
    // controller.svelte.ts) - every field-level callback below mutates this
    // cache and re-saves it, it never redeclares state on the Slint side.
    let settings_log_timer: Rc<RefCell<Option<slint::Timer>>> = Rc::new(RefCell::new(None));
    let settings_drafts =
        settings_drafts::SettingsDraftController::new(window, settings_io.clone());
    let settings_drafts_for_quit = settings_drafts.clone();
    window.on_settings_quit_requested(move || settings_drafts_for_quit.request_quit());
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
    let audio_device_projection_state =
        Rc::new(RefCell::new(AudioDeviceProjectionState::Canonical {
            generation: 0,
        }));
    // Cached so the model picker/download button don't each re-query Ollama.
    let summary_status_state: Rc<RefCell<Option<souffle_lib::summary::SummaryProvidersStatus>>> =
        Rc::new(RefCell::new(None));
    let summary_refresh_state = Rc::new(SummaryRefreshState::new(summary_status_state.clone()));
    let settings_values = settings_values::SettingsValueController::new(
        window,
        settings_state.clone(),
        settings_io.clone(),
        audio_devices_state.clone(),
        summary_status_state.clone(),
    );
    settings_values::wire(window, settings_values.clone());

    // Which summary template's name/prompt are shown in the edit fields -
    // mirrors `editingTemplateId` in controller.svelte.ts (local UI state,
    // not part of `AppSettings`).
    let summary_template_editing: Rc<RefCell<String>> = Rc::new(RefCell::new(String::new()));
    let summary_template_add_revision = Rc::new(Cell::new(0_u64));
    // Cached so a model switch resolves the clicked label without re-querying
    // the catalog (a pure static list, but avoids an extra command round trip
    // on every pick).
    let model_options_state: Rc<RefCell<Vec<model_ui::FlatModelOption>>> =
        Rc::new(RefCell::new(Vec::new()));
    let settings_list_models = lists_ui::SettingsListModels::install(window);
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
    let audio_device_projection_for_open = audio_device_projection_state.clone();
    let summary_refresh_state_for_open = summary_refresh_state.clone();
    let summary_template_editing_for_open = summary_template_editing.clone();
    let model_options_state_for_open = model_options_state.clone();
    let settings_list_models_for_open = settings_list_models.clone();
    let snippets_list_state_for_open = snippets_list_state.clone();
    let snippet_editing_for_open = snippet_editing.clone();
    let settings_log_timer_for_open = settings_log_timer.clone();
    let settings_drafts_for_open = settings_drafts.clone();
    let settings_io_for_open = settings_io.clone();
    let settings_values_for_open = settings_values.clone();
    let permissions_for_open = permissions.clone();
    window.on_settings_requested(move || {
        let Some(window) = weak.upgrade() else {
            return;
        };
        summary_refresh_state_for_open.begin_session();
        let token = settings_io_for_open.begin_open();
        if let Some(settings) = settings_state_for_open.known_snapshot() {
            summary_refresh_state_for_open.project_url_if_pristine(&window, &settings.ollama_url);
            settings_ui::populate(&window, &settings);
            data_ui::populate(&window, &settings);
            settings_values_for_open.observe_loaded(&settings);
            prime_summary_template_editor(
                &window,
                &settings,
                &summary_template_editing_for_open,
                &settings_drafts_for_open,
            );
        }
        window.set_settings_open(true);
        permissions_for_open.sync_activity();
        refresh_settings_log_tail(&window, settings_io_for_open.clone());
        *settings_log_timer_for_open.borrow_mut() = Some(start_settings_log_timer(
            weak.clone(),
            settings_io_for_open.clone(),
        ));
        drop(window);

        let core_weak = weak.clone();
        let core_handle = handle.clone();
        let core_io = settings_io_for_open.clone();
        let core_state = settings_state_for_open.clone();
        let core_devices = audio_devices_state_for_open.clone();
        let core_device_projection = audio_device_projection_for_open.clone();
        let core_summary_refresh = summary_refresh_state_for_open.clone();
        let core_editing = summary_template_editing_for_open.clone();
        let core_models = model_options_state_for_open.clone();
        let core_drafts = settings_drafts_for_open.clone();
        let core_values = settings_values_for_open.clone();
        let finish_core_io = core_io.clone();
        let finish_core = Rc::new(move |settings: AppSettings| {
            if !finish_core_io.accepts_load(token) {
                return;
            }
            let Some(window) = core_weak.upgrade() else {
                return;
            };
            core_summary_refresh.project_url_if_pristine(&window, &settings.ollama_url);
            let current_editing_id = core_editing.borrow().clone();
            let editing_id = settings_drafts::summary_editing_id(&current_editing_id, &settings);
            *core_editing.borrow_mut() = editing_id;
            settings_ui::populate(&window, &settings);
            data_ui::populate(&window, &settings);
            core_values.observe_loaded(&settings);
            core_device_projection.borrow_mut().observe_canonical();
            load_calendars_if_enabled(&window, &core_state, finish_core_io.clone());
            load_audio_devices(
                &window,
                &core_state,
                &core_devices,
                &core_device_projection,
                finish_core_io.clone(),
            );
            refresh_summary_providers(
                core_weak.clone(),
                core_handle.clone(),
                core_state.clone(),
                core_summary_refresh.clone(),
                core_editing.clone(),
                core_drafts.clone(),
                finish_core_io.clone(),
            );
            load_transcription_model_state(
                &window,
                &core_handle,
                &core_state,
                &core_models,
                finish_core_io.clone(),
            );
        });
        core_io.load_effective_snapshot(token, move |settings| finish_core(settings));

        let stats_handle = handle.clone();
        spawn_settings_load_stage(
            weak.clone(),
            settings_io_for_open.clone(),
            token,
            "Settings data stats",
            souffle_lib::async_runtime::spawn_blocking(move || {
                souffle_lib::commands::get_data_stats(stats_handle)
            }),
            |window, stats| data_ui::populate_stats(window, &stats),
        );
        spawn_settings_load_stage(
            weak.clone(),
            settings_io_for_open.clone(),
            token,
            "Settings MCP setup",
            souffle_lib::async_runtime::spawn_blocking(souffle_lib::commands::get_mcp_setup_info),
            |window, info| data_ui::populate_mcp(window, &info),
        );
        spawn_settings_load_stage(
            weak.clone(),
            settings_io_for_open.clone(),
            token,
            "Settings platform capabilities",
            souffle_lib::async_runtime::spawn_blocking(|| {
                Ok::<_, String>((
                    souffle_lib::commands::is_laptop(),
                    souffle_lib::commands::get_system_audio_support(),
                ))
            }),
            |window, (is_laptop, system_audio_supported)| {
                settings_ui::populate_platform_capabilities(
                    window,
                    is_laptop,
                    system_audio_supported,
                );
            },
        );
        let shortcuts_handle = handle.clone();
        let shortcuts_state = shortcuts_state_for_open.clone();
        spawn_settings_load_stage(
            weak.clone(),
            settings_io_for_open.clone(),
            token,
            "Settings shortcuts",
            souffle_lib::async_runtime::spawn_blocking(move || {
                souffle_lib::commands::get_shortcuts(shortcuts_handle).map(|shortcuts| {
                    let natives = souffle_lib::commands::get_native_shortcuts();
                    let tap = souffle_lib::commands::get_modifier_tap_status()
                        .map(|status| status.installed);
                    (shortcuts, natives, tap)
                })
            }),
            move |window, (shortcuts, natives, tap)| {
                settings_ui::populate_shortcuts(window, &shortcuts, &natives, tap);
                *shortcuts_state.borrow_mut() = Some(shortcuts);
            },
        );
        let dictionary_handle = handle.clone();
        let dictionary_models = settings_list_models_for_open.clone();
        spawn_settings_load_stage(
            weak.clone(),
            settings_io_for_open.clone(),
            token,
            "Settings dictionary",
            souffle_lib::async_runtime::spawn_blocking(move || {
                souffle_lib::commands::list_dictionary(dictionary_handle)
            }),
            move |_window, entries| dictionary_models.populate_dictionary(&entries),
        );
        let snippets_handle = handle.clone();
        let snippets_state = snippets_list_state_for_open.clone();
        let snippet_editing = snippet_editing_for_open.clone();
        let snippet_models = settings_list_models_for_open.clone();
        spawn_settings_load_stage(
            weak.clone(),
            settings_io_for_open.clone(),
            token,
            "Settings snippets",
            souffle_lib::async_runtime::spawn_blocking(move || {
                souffle_lib::commands::list_snippets(snippets_handle)
            }),
            move |_window, entries| {
                let editing = *snippet_editing.borrow();
                snippet_models.populate_snippets(&entries, editing);
                *snippets_state.borrow_mut() = entries;
            },
        );
    });

    let weak = window.as_weak();
    let settings_log_timer_for_close = settings_log_timer.clone();
    let settings_drafts_for_close = settings_drafts.clone();
    let pending_modifier_for_close = pending_modifier.clone();
    let upcoming_for_close = upcoming_cache.clone();
    let handle_for_close = tauri_handle.clone();
    let settings_io_for_close = settings_io.clone();
    let permissions_for_close = permissions.clone();
    window.on_settings_closed(move || {
        permissions_for_close.sync_activity();
        let close_token = settings_io_for_close.close();
        let weak = weak.clone();
        let settings_log_timer = settings_log_timer_for_close.clone();
        let pending_modifier = pending_modifier_for_close.clone();
        let upcoming = upcoming_for_close.clone();
        let handle = handle_for_close.clone();
        let io = settings_io_for_close.clone();
        let permissions = permissions_for_close.clone();
        settings_drafts_for_close.flush_explicit(move |result| {
            if !io.accepts_close(close_token) {
                return;
            }
            match result {
                settings_drafts::DraftFlushResult::RetainedAfterFailure => {
                    io.begin_open();
                    if let Some(window) = weak.upgrade() {
                        window.set_settings_open(true);
                    }
                    permissions.sync_activity();
                }
                settings_drafts::DraftFlushResult::NothingPending
                | settings_drafts::DraftFlushResult::Committed => {
                    *settings_log_timer.borrow_mut() = None;
                    *pending_modifier.borrow_mut() = None;
                    if let Some(window) = weak.upgrade() {
                        window.set_settings_recording_field(ShortcutField::None);
                        window.set_settings_shortcut_error("".into());
                        refresh_upcoming(&window, &handle, upcoming);
                    }
                }
            }
        });
    });

    fn save_settings_field(
        io: &Rc<settings_io::SettingsIoCoordinator>,
        lane: souffle_lib::commands::SettingsSaveLane,
        mutate: impl FnOnce(&mut AppSettings) + Send + 'static,
    ) {
        save_settings_field_then(io, lane, mutate, |_, _| {});
    }

    fn save_settings_field_then(
        io: &Rc<settings_io::SettingsIoCoordinator>,
        lane: souffle_lib::commands::SettingsSaveLane,
        mutate: impl FnOnce(&mut AppSettings) + Send + 'static,
        completion: impl FnOnce(settings_io::SettingsResponseOrder, &SettingsSaveOutcome) + 'static,
    ) {
        io.submit(lane, mutate, move |order, outcome| {
            match outcome {
                SettingsSaveOutcome::Observed { result, .. } => {
                    if let Err(error) = result {
                        eprintln!("Failed to save settings: {error}");
                    }
                }
                SettingsSaveOutcome::Unavailable { result, read_error } => {
                    eprintln!("Settings state unavailable after save {result:?}: {read_error}");
                }
            }
            completion(order, outcome);
        });
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
        field: ShortcutField,
        value: String,
    ) {
        window.set_settings_recording_field(ShortcutField::None);
        let mut guard = shortcuts_state.borrow_mut();
        let Some(shortcuts) = guard.as_mut() else {
            return;
        };
        match field {
            ShortcutField::Toggle => shortcuts.toggle = value,
            ShortcutField::Ptt => shortcuts.push_to_talk = value,
            ShortcutField::None => return,
        }
        let state = Arc::clone(handle);
        match souffle_lib::commands::save_shortcuts(state, shortcuts.clone()) {
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

    let weak = window.as_weak();
    let settings_io_for_autostart = settings_io.clone();
    window.on_settings_autostart_changed(move |enabled| {
        save_settings_field(
            &settings_io_for_autostart,
            souffle_lib::commands::SettingsSaveLane::Autostart,
            move |settings| settings.autostart_enabled = enabled,
        );
        if let Some(window) = weak.upgrade() {
            window.set_settings_autostart_enabled(enabled);
        }
    });

    let weak = window.as_weak();
    let settings_io_for_debug = settings_io.clone();
    window.on_settings_debug_transcription_changed(move |enabled| {
        save_settings_field(
            &settings_io_for_debug,
            souffle_lib::commands::SettingsSaveLane::General,
            move |settings| settings.debug_transcription = enabled,
        );
        if let Some(window) = weak.upgrade() {
            window.set_settings_debug_transcription(enabled);
        }
    });

    let weak = window.as_weak();
    let settings_io_for_log_level = settings_io.clone();
    window.on_settings_log_level_changed(move |value| {
        let level = settings_ui::log_level_from_slint(value);
        save_settings_field(
            &settings_io_for_log_level,
            souffle_lib::commands::SettingsSaveLane::General,
            move |settings| settings.log_level = level,
        );
        if let Some(window) = weak.upgrade() {
            window.set_settings_log_level(settings_ui::log_level_to_slint(level));
        }
    });

    let weak = window.as_weak();
    let settings_io_for_auto_paste = settings_io.clone();
    window.on_settings_auto_paste_changed(move |enabled| {
        save_settings_field(
            &settings_io_for_auto_paste,
            souffle_lib::commands::SettingsSaveLane::General,
            move |settings| settings.auto_paste = enabled,
        );
        if let Some(window) = weak.upgrade() {
            window.set_settings_auto_paste(enabled);
        }
    });

    let weak = window.as_weak();
    let settings_io_for_pill_hidden = settings_io.clone();
    window.on_settings_pill_hidden_changed(move |hidden| {
        save_settings_field(
            &settings_io_for_pill_hidden,
            souffle_lib::commands::SettingsSaveLane::General,
            move |settings| settings.pill_hidden = hidden,
        );
        if let Some(window) = weak.upgrade() {
            window.set_settings_pill_hidden(hidden);
        }
    });

    let weak = window.as_weak();
    let settings_io_for_feedback_enabled = settings_io.clone();
    window.on_settings_feedback_sounds_enabled_changed(move |enabled| {
        save_settings_field(
            &settings_io_for_feedback_enabled,
            souffle_lib::commands::SettingsSaveLane::General,
            move |settings| settings.feedback_sounds_enabled = enabled,
        );
        if let Some(window) = weak.upgrade() {
            window.set_settings_feedback_sounds_enabled(enabled);
        }
    });

    let weak = window.as_weak();
    let handle = tauri_handle.clone();
    let settings_io_for_calendar_enabled = settings_io.clone();
    let settings_state_for_calendar_enabled = settings_state.clone();
    let upcoming_for_enable = upcoming_cache.clone();
    window.on_settings_calendar_enabled_changed(move |enabled| {
        if !enabled {
            let weak = weak.clone();
            let upcoming = upcoming_for_enable.clone();
            save_settings_field_then(
                &settings_io_for_calendar_enabled,
                souffle_lib::commands::SettingsSaveLane::General,
                |settings| settings.calendar_integration_enabled = false,
                move |order, outcome| {
                    if !order.publishes_to_open_window() {
                        return;
                    }
                    if let SettingsSaveOutcome::Observed { settings, .. } = outcome
                        && let Some(window) = weak.upgrade()
                    {
                        let enabled = settings.calendar_integration_enabled;
                        window.set_settings_calendar_enabled(enabled);
                        if !enabled {
                            settings_ui::populate_calendars(
                                &window,
                                &[],
                                &[],
                                souffle_lib::calendar::authorization_state(),
                            );
                            apply_upcoming(&window, &[], &upcoming);
                        }
                    }
                },
            );
            return;
        }
        let handle = handle.clone();
        let settings_io = settings_io_for_calendar_enabled.clone();
        let settings_state = settings_state_for_calendar_enabled.clone();
        let upcoming_for_enable = upcoming_for_enable.clone();
        let weak = weak.clone();
        // Not `souffle_lib::async_runtime::spawn`: this closes over an
        // `Rc<RefCell<..>>`, which is not `Send`. `request_permission`
        // blocks on the native TCC prompt internally (off its own thread via
        // `spawn_blocking`), so awaiting it on Slint's single-threaded local
        // executor is exactly what it's for.
        slint::spawn_local(async move {
            let permission =
                souffle_lib::commands::request_permission(DomainPermissionKind::Calendar)
                    .await
                    .unwrap_or(PermState::Denied);
            let Some(window) = weak.upgrade() else {
                return;
            };
            if permission != PermState::Granted {
                window.set_settings_calendar_enabled(false);
                settings_ui::populate_calendars(&window, &[], &[], permission);
                return;
            }
            let weak = window.as_weak();
            let publication_io = settings_io.clone();
            save_settings_field_then(
                &settings_io,
                souffle_lib::commands::SettingsSaveLane::General,
                |settings| settings.calendar_integration_enabled = true,
                move |order, outcome| {
                    if !order.publishes_to_open_window() {
                        return;
                    }
                    let SettingsSaveOutcome::Observed { settings, .. } = outcome else {
                        return;
                    };
                    let Some(window) = weak.upgrade() else {
                        return;
                    };
                    let enabled = settings.calendar_integration_enabled;
                    window.set_settings_calendar_enabled(enabled);
                    if !enabled {
                        settings_ui::populate_calendars(
                            &window,
                            &[],
                            &[],
                            souffle_lib::calendar::authorization_state(),
                        );
                        apply_upcoming(&window, &[], &upcoming_for_enable);
                        return;
                    }
                    load_calendars_if_enabled(&window, &settings_state, publication_io);
                    refresh_upcoming(&window, &handle, upcoming_for_enable);
                },
            );
        })
        .expect("slint event loop not running");
    });

    let weak = window.as_weak();
    let settings_io_for_calendar_autostart = settings_io.clone();
    window.on_settings_calendar_autostart_enabled_changed(move |enabled| {
        save_settings_field(
            &settings_io_for_calendar_autostart,
            souffle_lib::commands::SettingsSaveLane::General,
            move |settings| settings.calendar_autostart_enabled = enabled,
        );
        if let Some(window) = weak.upgrade() {
            window.set_settings_calendar_autostart_enabled(enabled);
        }
    });

    let weak = window.as_weak();
    let settings_io_for_calendar_toggle = settings_io.clone();
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
        let id_string = id.to_string();
        let weak = weak.clone();
        let settings_state = settings_state_for_calendar_toggle.clone();
        let publication_io = settings_io_for_calendar_toggle.clone();
        save_settings_field_then(
            &settings_io_for_calendar_toggle,
            souffle_lib::commands::SettingsSaveLane::General,
            move |settings| {
                let effective = if settings.calendar_selected_ids.is_empty() {
                    all_ids.clone()
                } else {
                    settings.calendar_selected_ids.clone()
                };
                let mut next = effective.clone();
                if let Some(pos) = next.iter().position(|existing| existing == &id_string) {
                    next.remove(pos);
                } else {
                    next.push(id_string.clone());
                }
                if next.is_empty() {
                    // Mirrors `toggleCalendarSelected()`: the last checked
                    // calendar cannot be unchecked.
                    return;
                }
                settings.calendar_selected_ids = if next.len() == all_ids.len() {
                    Vec::new()
                } else {
                    next
                };
            },
            move |order, outcome| {
                if !order.publishes_to_open_window() {
                    return;
                }
                let SettingsSaveOutcome::Observed { .. } = outcome else {
                    return;
                };
                let Some(window) = weak.upgrade() else {
                    return;
                };
                load_calendars_if_enabled(&window, &settings_state, publication_io);
            },
        );
    });

    window.on_settings_calendar_open_system_settings_requested(move || {
        souffle_lib::commands::open_calendar_settings();
    });

    let weak = window.as_weak();
    let handle = tauri_handle.clone();
    let settings_state_for_device = settings_state.clone();
    let settings_io_for_device = settings_io.clone();
    let audio_devices_state_for_device = audio_devices_state.clone();
    let audio_device_projection_for_device = audio_device_projection_state.clone();
    let device_selection_revision = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let device_effect_lock = Arc::new(Mutex::new(()));
    window.on_settings_device_changed(move |label| {
        let uid = audio_ui::resolve_device_uid(&audio_devices_state_for_device.borrow(), &label)
            .unwrap_or_default();
        let revision = device_selection_revision.fetch_add(1, Ordering::AcqRel) + 1;
        audio_device_projection_for_device
            .borrow_mut()
            .begin_pending(uid.clone());
        let uid_for_save = uid.clone();
        let weak = weak.clone();
        let settings_state = settings_state_for_device.clone();
        let devices = audio_devices_state_for_device.clone();
        let publication_io = settings_io_for_device.clone();
        let selection_handle = handle.clone();
        let projection_state = audio_device_projection_for_device.clone();
        let latest_revision = Arc::clone(&device_selection_revision);
        let effect_lock = Arc::clone(&device_effect_lock);
        save_settings_field_then(
            &settings_io_for_device,
            souffle_lib::commands::SettingsSaveLane::General,
            move |settings| {
                settings.audio_device = if uid_for_save.is_empty() {
                    None
                } else {
                    Some(uid_for_save)
                };
            },
            move |_order, outcome| {
                if revision != latest_revision.load(Ordering::Acquire) {
                    return;
                }
                let fallback = settings_state.borrow().clone();
                let settlement = settle_audio_device_save(outcome, &fallback, &uid);
                // An unavailable reread deliberately leaves the actor cache
                // marked unknown while preserving its last observed value.
                // Never reload the picker from that stale snapshot after the
                // native effect; the user's submitted label remains visible
                // until a later serialized refresh observes persistence.
                let reload_after_effect = settlement.has_observed_canonical();
                apply_audio_device_projection_settlement(
                    &settlement,
                    &mut projection_state.borrow_mut(),
                );
                let (target_uid, canonical) = match settlement {
                    AudioDeviceSaveSettlement::Committed { uid, canonical } => {
                        (Some(uid), canonical)
                    }
                    AudioDeviceSaveSettlement::Rejected { canonical } => {
                        let uid = canonical
                            .as_ref()
                            .map(|settings| settings.audio_device.clone().unwrap_or_default());
                        (uid, canonical)
                    }
                };
                if let Some(window) = weak.upgrade()
                    && window.get_settings_open()
                    && let Some(settings) = canonical.as_deref()
                {
                    audio_ui::populate_device_pickers(
                        &window,
                        &devices.borrow(),
                        settings.audio_device.as_deref().unwrap_or_default(),
                        settings.clamshell_audio_device.as_deref(),
                    );
                }
                let Some(target_uid) = target_uid else {
                    return;
                };
                let worker_revision = Arc::clone(&latest_revision);
                let worker = souffle_lib::async_runtime::spawn_blocking(move || {
                    let _guard = effect_lock
                        .lock()
                        .unwrap_or_else(|error| error.into_inner());
                    if revision != worker_revision.load(Ordering::Acquire) {
                        return Ok(());
                    }
                    souffle_lib::commands::select_audio_device(selection_handle, target_uid)
                });
                slint::spawn_local(async move {
                    match worker.await {
                        Ok(Ok(())) => {}
                        Ok(Err(error)) => eprintln!("Failed to select audio device: {error}"),
                        Err(error) => {
                            eprintln!("Failed to join audio device selection worker: {error}")
                        }
                    }
                    if reload_after_effect
                        && revision == latest_revision.load(Ordering::Acquire)
                        && let Some(window) = weak.upgrade()
                        && window.get_settings_open()
                    {
                        load_audio_devices(
                            &window,
                            &settings_state,
                            &devices,
                            &projection_state,
                            publication_io,
                        );
                    }
                })
                .expect("slint event loop not running");
            },
        );
    });

    let weak = window.as_weak();
    let settings_state_for_refresh = settings_state.clone();
    let audio_devices_state_for_refresh = audio_devices_state.clone();
    let audio_device_projection_for_refresh = audio_device_projection_state.clone();
    let settings_io_for_refresh = settings_io.clone();
    window.on_settings_refresh_devices_requested(move || {
        if let Some(window) = weak.upgrade() {
            load_audio_devices(
                &window,
                &settings_state_for_refresh,
                &audio_devices_state_for_refresh,
                &audio_device_projection_for_refresh,
                settings_io_for_refresh.clone(),
            );
        }
    });

    let weak = window.as_weak();
    let settings_io_for_bt = settings_io.clone();
    window.on_settings_allow_bluetooth_mic_changed(move |allowed| {
        save_settings_field(
            &settings_io_for_bt,
            souffle_lib::commands::SettingsSaveLane::General,
            move |settings| settings.allow_bluetooth_mic = allowed,
        );
        if let Some(window) = weak.upgrade() {
            window.set_settings_allow_bluetooth_mic(allowed);
        }
    });

    let weak = window.as_weak();
    let settings_io_for_system_audio = settings_io.clone();
    window.on_settings_capture_system_audio_changed(move |enabled| {
        save_settings_field(
            &settings_io_for_system_audio,
            souffle_lib::commands::SettingsSaveLane::General,
            move |settings| settings.capture_system_audio = enabled,
        );
        if let Some(window) = weak.upgrade() {
            window.set_settings_capture_system_audio(enabled);
        }
    });

    let weak = window.as_weak();
    let settings_io_for_autostop_enabled = settings_io.clone();
    window.on_settings_meeting_autostop_enabled_changed(move |enabled| {
        save_settings_field(
            &settings_io_for_autostop_enabled,
            souffle_lib::commands::SettingsSaveLane::General,
            move |settings| settings.meeting_autostop_enabled = enabled,
        );
        if let Some(window) = weak.upgrade() {
            window.set_settings_meeting_autostop_enabled(enabled);
        }
    });

    let weak = window.as_weak();
    let settings_io_for_vad = settings_io.clone();
    window.on_settings_vad_enabled_changed(move |enabled| {
        save_settings_field(
            &settings_io_for_vad,
            souffle_lib::commands::SettingsSaveLane::General,
            move |settings| settings.vad_enabled = enabled,
        );
        if let Some(window) = weak.upgrade() {
            window.set_settings_vad_enabled(enabled);
        }
    });

    let weak = window.as_weak();
    let settings_io_for_filler = settings_io.clone();
    window.on_settings_filler_removal_changed(move |enabled| {
        save_settings_field(
            &settings_io_for_filler,
            souffle_lib::commands::SettingsSaveLane::General,
            move |settings| settings.filler_removal = enabled,
        );
        if let Some(window) = weak.upgrade() {
            window.set_settings_filler_removal(enabled);
        }
    });

    let weak = window.as_weak();
    let settings_io_for_stutter = settings_io.clone();
    window.on_settings_stutter_collapse_changed(move |enabled| {
        save_settings_field(
            &settings_io_for_stutter,
            souffle_lib::commands::SettingsSaveLane::General,
            move |settings| settings.stutter_collapse = enabled,
        );
        if let Some(window) = weak.upgrade() {
            window.set_settings_stutter_collapse(enabled);
        }
    });

    let weak = window.as_weak();
    let settings_io_for_dictionary = settings_io.clone();
    window.on_settings_dictionary_correction_changed(move |enabled| {
        save_settings_field(
            &settings_io_for_dictionary,
            souffle_lib::commands::SettingsSaveLane::General,
            move |settings| settings.dictionary_correction = enabled,
        );
        if let Some(window) = weak.upgrade() {
            window.set_settings_dictionary_correction(enabled);
        }
    });

    let weak = window.as_weak();
    let settings_state_for_move = settings_state.clone();
    let audio_devices_state_for_move = audio_devices_state.clone();
    let audio_device_projection_for_move = audio_device_projection_state.clone();
    let settings_io_for_move = settings_io.clone();
    window.on_settings_move_device_requested(move |uid, direction| {
        let devices = audio_devices_state_for_move.borrow().clone();
        let uid = uid.to_string();
        let weak = weak.clone();
        let state = settings_state_for_move.clone();
        let device_state = audio_devices_state_for_move.clone();
        let projection_state = audio_device_projection_for_move.clone();
        let publication_io = settings_io_for_move.clone();
        save_settings_field_then(
            &settings_io_for_move,
            souffle_lib::commands::SettingsSaveLane::General,
            move |settings| {
                let list =
                    microphone_list::build_microphone_list(&devices, &settings.input_priority);
                if let Some(next) = microphone_list::reorder_microphone_list(&list, &uid, direction)
                {
                    settings.input_priority.priorities = next;
                }
            },
            move |order, _outcome| {
                if !order.publishes_to_open_window() {
                    return;
                }
                if let Some(window) = weak.upgrade() {
                    load_audio_devices(
                        &window,
                        &state,
                        &device_state,
                        &projection_state,
                        publication_io,
                    );
                }
            },
        );
    });

    let weak = window.as_weak();
    let settings_state_for_hide = settings_state.clone();
    let audio_devices_state_for_hide = audio_devices_state.clone();
    let audio_device_projection_for_hide = audio_device_projection_state.clone();
    let settings_io_for_hide = settings_io.clone();
    window.on_settings_toggle_hidden_requested(move |uid, hidden| {
        let uid = uid.to_string();
        let weak = weak.clone();
        let state = settings_state_for_hide.clone();
        let devices = audio_devices_state_for_hide.clone();
        let projection_state = audio_device_projection_for_hide.clone();
        let publication_io = settings_io_for_hide.clone();
        save_settings_field_then(
            &settings_io_for_hide,
            souffle_lib::commands::SettingsSaveLane::General,
            move |settings| {
                let hidden_set = &mut settings.input_priority.hidden;
                hidden_set.retain(|existing| existing != &uid);
                if hidden {
                    hidden_set.push(uid);
                }
            },
            move |order, _outcome| {
                if !order.publishes_to_open_window() {
                    return;
                }
                if let Some(window) = weak.upgrade() {
                    load_audio_devices(
                        &window,
                        &state,
                        &devices,
                        &projection_state,
                        publication_io,
                    );
                }
            },
        );
    });

    let weak = window.as_weak();
    let settings_state_for_remove = settings_state.clone();
    let audio_devices_state_for_remove = audio_devices_state.clone();
    let audio_device_projection_for_remove = audio_device_projection_state.clone();
    let settings_io_for_remove = settings_io.clone();
    window.on_settings_remove_device_requested(move |uid| {
        let uid = uid.to_string();
        let weak = weak.clone();
        let state = settings_state_for_remove.clone();
        let devices = audio_devices_state_for_remove.clone();
        let projection_state = audio_device_projection_for_remove.clone();
        let publication_io = settings_io_for_remove.clone();
        save_settings_field_then(
            &settings_io_for_remove,
            souffle_lib::commands::SettingsSaveLane::General,
            move |settings| {
                settings.input_priority =
                    microphone_list::remove_known_device(&settings.input_priority, &uid);
                if settings.audio_device.as_deref() == Some(uid.as_str()) {
                    settings.audio_device = None;
                }
                if settings.clamshell_audio_device.as_deref() == Some(uid.as_str()) {
                    settings.clamshell_audio_device = None;
                }
            },
            move |order, _outcome| {
                if !order.publishes_to_open_window() {
                    return;
                }
                if let Some(window) = weak.upgrade() {
                    load_audio_devices(
                        &window,
                        &state,
                        &devices,
                        &projection_state,
                        publication_io,
                    );
                }
            },
        );
    });

    let weak = window.as_weak();
    let settings_state_for_reset_devices = settings_state.clone();
    let audio_devices_state_for_reset_devices = audio_devices_state.clone();
    let audio_device_projection_for_reset_devices = audio_device_projection_state.clone();
    let settings_io_for_reset_devices = settings_io.clone();
    window.on_settings_reset_devices_requested(move || {
        let weak = weak.clone();
        let state = settings_state_for_reset_devices.clone();
        let devices = audio_devices_state_for_reset_devices.clone();
        let projection_state = audio_device_projection_for_reset_devices.clone();
        let io = settings_io_for_reset_devices.clone();
        let Some(token) = io.current_load_token() else {
            return;
        };
        let worker =
            souffle_lib::async_runtime::spawn_blocking(souffle_lib::commands::list_audio_devices);
        slint::spawn_local(async move {
            let connected = match worker.await {
                Ok(Ok(connected)) => connected,
                Ok(Err(error)) => {
                    eprintln!("Failed to list connected devices for reset: {error}");
                    return;
                }
                Err(error) => {
                    eprintln!("Failed to join connected device reset worker: {error}");
                    return;
                }
            };
            if !io.accepts_load(token) {
                return;
            }
            let connected_uids: Vec<String> =
                connected.iter().map(|device| device.uid.clone()).collect();
            let publication_io = io.clone();
            save_settings_field_then(
                &io,
                souffle_lib::commands::SettingsSaveLane::General,
                move |settings| {
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
                },
                move |order, _outcome| {
                    if !order.publishes_to_open_window() {
                        return;
                    }
                    if let Some(window) = weak.upgrade() {
                        load_audio_devices(
                            &window,
                            &state,
                            &devices,
                            &projection_state,
                            publication_io,
                        );
                    }
                },
            );
        })
        .expect("slint event loop not running");
    });

    let weak = window.as_weak();
    let settings_state_for_reset_rate = settings_state.clone();
    let audio_devices_state_for_reset_rate = audio_devices_state.clone();
    let audio_device_projection_for_reset_rate = audio_device_projection_state;
    window.on_settings_reset_sample_rate_requested(move || {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let Some((generation, selected, _, _)) = audio_device_load_configuration(
            &settings_state_for_reset_rate,
            &audio_device_projection_for_reset_rate,
        ) else {
            return;
        };
        let devices = audio_devices_state_for_reset_rate.borrow().clone();
        let Some(uid) =
            audio_ui::resolve_sample_rate_device_uid(&selected, &devices).map(String::from)
        else {
            return;
        };
        window.set_settings_resetting_sample_rate(true);
        let weak = weak.clone();
        let projection_state = audio_device_projection_for_reset_rate.clone();
        slint::spawn_local(async move {
            let result = souffle_lib::commands::reset_input_sample_rate(uid).await;
            if projection_state.borrow().generation() != generation {
                if let Some(window) = weak.upgrade() {
                    window.set_settings_resetting_sample_rate(false);
                }
                return;
            }
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

    let weak = window.as_weak();
    let handle = tauri_handle.clone();
    window.on_settings_data_export_requested(move || {
        let Some(window) = weak.upgrade() else {
            return;
        };
        window.set_settings_data_exporting(true);
        window.set_settings_data_export_status("".into());
        window.set_settings_data_export_status_is_error(false);
        let weak = weak.clone();
        let state = Arc::clone(&handle);
        let picker = souffle_lib::async_runtime::spawn_blocking(choose_archive_export_folder);
        slint::spawn_local(async move {
            let selection = picker
                .await
                .map_err(|error| format!("Sélecteur de dossier interrompu : {error}"))
                .and_then(|result| result);
            let Some(window) = weak.upgrade() else {
                return;
            };
            let directory = match selection {
                Ok(Some(directory)) => directory,
                Ok(None) => {
                    window.set_settings_data_exporting(false);
                    return;
                }
                Err(error) => {
                    window.set_settings_data_exporting(false);
                    window.set_settings_data_export_status(error.into());
                    window.set_settings_data_export_status_is_error(true);
                    return;
                }
            };

            match souffle_lib::commands::export_archive(state, directory.clone()) {
                Ok(()) => {
                    window.set_settings_data_export_status(
                        format!("Export en cours vers {directory}…").into(),
                    );
                    monitor_archive_export(weak, directory);
                }
                Err(error) => {
                    window.set_settings_data_exporting(false);
                    window.set_settings_data_export_status(error.into());
                    window.set_settings_data_export_status_is_error(true);
                }
            }
        })
        .expect("slint event loop not running");
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
        let weak = weak.clone();
        let worker =
            souffle_lib::async_runtime::spawn_blocking(souffle_lib::commands::test_mcp_connection);
        slint::spawn_local(async move {
            let result = worker
                .await
                .map_err(|error| format!("Test MCP interrompu : {error}"))
                .and_then(|result| result);
            let Some(window) = weak.upgrade() else {
                return;
            };
            match result {
                Ok(tools) => window
                    .set_settings_mcp_test_status(format!("Connexion réussie ({tools}).").into()),
                Err(error) => window.set_settings_mcp_test_status(error.into()),
            }
            window.set_settings_testing_mcp(false);
        })
        .expect("slint event loop not running");
    });

    let weak = window.as_weak();
    let settings_io_for_auto_update = settings_io.clone();
    window.on_settings_auto_update_check_changed(move |enabled| {
        save_settings_field(
            &settings_io_for_auto_update,
            souffle_lib::commands::SettingsSaveLane::General,
            move |settings| settings.auto_update_check_enabled = enabled,
        );
        if let Some(window) = weak.upgrade() {
            window.set_settings_auto_update_check(enabled);
        }
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
    let settings_io_for_provider = settings_io.clone();
    let settings_state_for_provider = settings_state.clone();
    let summary_refresh_state_for_provider = summary_refresh_state.clone();
    let summary_template_editing_for_provider = summary_template_editing.clone();
    let settings_drafts_for_provider = settings_drafts.clone();
    window.on_settings_summary_provider_changed(move |value| {
        let submitted_generation = summary_refresh_state_for_provider.invalidate();
        let provider = ia_ui::summary_provider_from_slint(value);
        let weak = weak.clone();
        let handle = handle.clone();
        let state = settings_state_for_provider.clone();
        let refresh = summary_refresh_state_for_provider.clone();
        let editing = summary_template_editing_for_provider.clone();
        let drafts = settings_drafts_for_provider.clone();
        let io = settings_io_for_provider.clone();
        save_settings_field_then(
            &settings_io_for_provider,
            souffle_lib::commands::SettingsSaveLane::General,
            move |settings| settings.summary_provider = provider,
            move |order, _outcome| {
                if !summary_refresh_after_settings_response(order)
                    || !refresh.accepts(submitted_generation)
                {
                    return;
                }
                refresh_summary_providers(weak, handle, state, refresh, editing, drafts, io);
            },
        );
    });

    let weak = window.as_weak();
    let handle = tauri_handle.clone();
    let settings_io_for_ollama_url = settings_io.clone();
    let settings_state_for_ollama_url = settings_state.clone();
    let summary_refresh_state_for_ollama_url = summary_refresh_state.clone();
    let summary_template_editing_for_ollama_url = summary_template_editing.clone();
    let settings_drafts_for_ollama_url = settings_drafts.clone();
    window.on_settings_ollama_url_changed(move |value| {
        let submitted_generation = summary_refresh_state_for_ollama_url.mark_url_edited();
        let value = value.to_string();
        let weak = weak.clone();
        let handle = handle.clone();
        let state = settings_state_for_ollama_url.clone();
        let refresh = summary_refresh_state_for_ollama_url.clone();
        let editing = summary_template_editing_for_ollama_url.clone();
        let drafts = settings_drafts_for_ollama_url.clone();
        let io = settings_io_for_ollama_url.clone();
        save_settings_field_then(
            &settings_io_for_ollama_url,
            souffle_lib::commands::SettingsSaveLane::General,
            move |settings| settings.ollama_url = value,
            move |order, _outcome| {
                if !summary_refresh_after_settings_response(order)
                    || !refresh.accepts(submitted_generation)
                {
                    return;
                }
                refresh_summary_providers(weak, handle, state, refresh, editing, drafts, io);
            },
        );
    });

    let weak = window.as_weak();
    let handle = tauri_handle.clone();
    let settings_state_for_retry = settings_state.clone();
    let summary_refresh_state_for_retry = summary_refresh_state.clone();
    let summary_template_editing_for_retry = summary_template_editing.clone();
    let settings_drafts_for_retry = settings_drafts.clone();
    let settings_io_for_retry = settings_io.clone();
    window.on_settings_summary_providers_retry_requested(move || {
        refresh_summary_providers(
            weak.clone(),
            handle.clone(),
            settings_state_for_retry.clone(),
            summary_refresh_state_for_retry.clone(),
            summary_template_editing_for_retry.clone(),
            settings_drafts_for_retry.clone(),
            settings_io_for_retry.clone(),
        );
    });

    let weak = window.as_weak();
    let handle = tauri_handle.clone();
    let settings_state_for_pull = settings_state.clone();
    let summary_refresh_state_for_pull = summary_refresh_state.clone();
    let summary_template_editing_for_pull = summary_template_editing.clone();
    let settings_drafts_for_pull = settings_drafts.clone();
    let settings_io_for_pull = settings_io.clone();
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
        let summary_refresh_state_for_pull = summary_refresh_state_for_pull.clone();
        let summary_template_editing_for_pull = summary_template_editing_for_pull.clone();
        let settings_drafts_for_pull = settings_drafts_for_pull.clone();
        let settings_io_for_pull = settings_io_for_pull.clone();
        slint::spawn_local(async move {
            let state = Arc::clone(&handle);
            // Same reactor requirement as `refresh_summary_providers`/
            // `download_update`: the pull streams over `reqwest`.
            let result = souffle_lib::async_runtime::spawn(
                souffle_lib::commands::pull_recommended_ollama_model(state, channel),
            )
            .await
            .map_err(|e| format!("Join pull_recommended_ollama_model task: {e}"))
            .and_then(|r| r);
            let Some(window) = weak.upgrade() else {
                return;
            };
            window.set_settings_ollama_pulling(false);
            match result {
                Ok(_) => refresh_summary_providers(
                    weak,
                    handle,
                    settings_state_for_pull,
                    summary_refresh_state_for_pull,
                    summary_template_editing_for_pull,
                    settings_drafts_for_pull,
                    settings_io_for_pull,
                ),
                Err(e) => window.set_settings_ollama_pull_error(e.into()),
            }
        })
        .expect("slint event loop not running");
    });

    window.on_settings_open_apple_intelligence_settings_requested(move || {
        souffle_lib::commands::open_apple_intelligence_settings();
    });

    let settings_io_for_polish_enabled = settings_io.clone();
    let weak = window.as_weak();
    let settings_state_for_polish_enabled = settings_state.clone();
    window.on_settings_dictation_polish_enabled_changed(move |enabled| {
        let weak = weak.clone();
        let state = settings_state_for_polish_enabled.clone();
        save_settings_field_then(
            &settings_io_for_polish_enabled,
            souffle_lib::commands::SettingsSaveLane::General,
            move |settings| settings.dictation_polish_enabled = enabled,
            move |order, _outcome| {
                if !order.publishes_to_open_window() {
                    return;
                }
                if let Some(window) = weak.upgrade() {
                    let guard = state.borrow();
                    if let Some(settings) = guard.as_ref() {
                        let provider_available =
                            window.get_settings_summary_unusable_message().is_empty();
                        ia_ui::populate_dictation_polish(&window, settings, provider_available);
                    }
                }
            },
        );
    });

    let weak = window.as_weak();
    let settings_io_for_polish_template = settings_io.clone();
    let settings_state_for_polish_template = settings_state.clone();
    let settings_drafts_for_polish_template = settings_drafts.clone();
    window.on_settings_dictation_polish_template_changed(move |template_id| {
        let template_id = template_id.to_string();
        let valid = settings_state_for_polish_template
            .borrow()
            .as_ref()
            .is_some_and(|settings| {
                ia_ui::contains_dictation_polish_id(
                    &settings.dictation_polish_templates,
                    &template_id,
                )
            });
        if !valid {
            return;
        }
        let weak = weak.clone();
        let state = settings_state_for_polish_template.clone();
        let drafts = settings_drafts_for_polish_template.clone();
        save_settings_field_then(
            &settings_io_for_polish_template,
            souffle_lib::commands::SettingsSaveLane::General,
            move |settings| settings.dictation_polish_template_id = template_id,
            move |order, _outcome| {
                if !order.publishes_to_open_window() {
                    return;
                }
                let Some(window) = weak.upgrade() else {
                    return;
                };
                let guard = state.borrow();
                if let Some(settings) = guard.as_ref() {
                    let provider_available =
                        window.get_settings_summary_unusable_message().is_empty();
                    ia_ui::populate_dictation_polish(&window, settings, provider_available);
                    drafts.reapply_polish_prompt(&window, &settings.dictation_polish_template_id);
                }
            },
        );
    });

    let settings_state_for_polish_prompt = settings_state.clone();
    let settings_drafts_for_polish_prompt = settings_drafts.clone();
    window.on_settings_dictation_polish_prompt_changed(move |text| {
        let active_id = settings_state_for_polish_prompt
            .borrow()
            .as_ref()
            .map(|settings| settings.dictation_polish_template_id.clone());
        if let Some(active_id) = active_id {
            settings_drafts_for_polish_prompt.edit_polish_prompt(active_id, text.to_string());
        }
    });

    let weak = window.as_weak();
    let settings_io_for_template_default = settings_io.clone();
    let settings_state_for_template_default = settings_state.clone();
    let summary_template_editing_for_default = summary_template_editing.clone();
    let settings_drafts_for_template_default = settings_drafts.clone();
    window.on_settings_summary_template_default_changed(move |template_id| {
        let template_id = template_id.to_string();
        let valid = settings_state_for_template_default
            .borrow()
            .as_ref()
            .is_some_and(|settings| {
                ia_ui::contains_summary_template_id(&settings.summary_templates, &template_id)
            });
        if !valid {
            return;
        }
        let weak = weak.clone();
        let state = settings_state_for_template_default.clone();
        let editing = summary_template_editing_for_default.clone();
        let drafts = settings_drafts_for_template_default.clone();
        save_settings_field_then(
            &settings_io_for_template_default,
            souffle_lib::commands::SettingsSaveLane::General,
            move |settings| settings.default_summary_template_id = template_id,
            move |order, _outcome| {
                if !order.publishes_to_open_window() {
                    return;
                }
                let Some(window) = weak.upgrade() else {
                    return;
                };
                let guard = state.borrow();
                if let Some(settings) = guard.as_ref() {
                    let editing_id = editing.borrow().clone();
                    ia_ui::populate_summary_templates(&window, settings, &editing_id);
                    let editing_id = if editing_id.is_empty() {
                        settings.default_summary_template_id.as_str()
                    } else {
                        editing_id.as_str()
                    };
                    drafts.reapply_summary_template(&window, editing_id);
                }
            },
        );
    });

    let weak = window.as_weak();
    let settings_state_for_edit_target = settings_state.clone();
    let summary_template_editing_for_edit_target = summary_template_editing.clone();
    let settings_drafts_for_edit_target = settings_drafts.clone();
    window.on_settings_summary_template_edit_target_changed(move |template_id| {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let template_id = template_id.to_string();
        let guard = settings_state_for_edit_target.borrow();
        let Some(settings) = guard.as_ref() else {
            return;
        };
        if !ia_ui::contains_summary_template_id(&settings.summary_templates, &template_id) {
            return;
        }
        *summary_template_editing_for_edit_target.borrow_mut() = template_id.clone();
        ia_ui::populate_summary_templates(&window, settings, &template_id);
        settings_drafts_for_edit_target.reapply_summary_template(&window, &template_id);
    });

    let weak = window.as_weak();
    let settings_io_for_template_delete = settings_io.clone();
    let settings_state_for_template_delete = settings_state.clone();
    let summary_template_editing_for_delete = summary_template_editing.clone();
    let settings_drafts_for_template_delete = settings_drafts.clone();
    window.on_settings_summary_template_delete_requested(move || {
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
        let weak = weak.clone();
        let io = settings_io_for_template_delete.clone();
        let state = settings_state_for_template_delete.clone();
        let editing = summary_template_editing_for_delete.clone();
        let drafts = settings_drafts_for_template_delete.clone();
        settings_drafts_for_template_delete.flush_explicit(move |flush| {
            if flush == settings_drafts::DraftFlushResult::RetainedAfterFailure {
                if let Some(window) = weak.upgrade() {
                    drafts.reapply_summary_template(&window, &editing_id);
                }
                return;
            }
            let editing_for_mutation = editing_id.clone();
            let editing_for_completion = editing_id.clone();
            save_settings_field_then(
                &io,
                souffle_lib::commands::SettingsSaveLane::General,
                move |settings| {
                    settings
                        .summary_templates
                        .retain(|template| template.id != editing_for_mutation);
                    if settings.default_summary_template_id == editing_for_mutation {
                        settings.default_summary_template_id = settings
                            .summary_templates
                            .first()
                            .map(|template| template.id.clone())
                            .unwrap_or_else(|| "default".to_string());
                    }
                },
                move |order, outcome| match settings_drafts::settle_settings_submission(
                    editing_for_completion,
                    outcome,
                ) {
                    settings_drafts::SettingsSubmission::Committed => {
                        let current = editing.borrow().clone();
                        drafts.discard_summary_template(&current);
                        *editing.borrow_mut() = String::new();
                        if order.publishes_to_open_window()
                            && let Some(window) = weak.upgrade()
                            && let Some(settings) = state.borrow().as_ref()
                        {
                            ia_ui::populate_summary_templates(&window, settings, "");
                        }
                    }
                    settings_drafts::SettingsSubmission::Retained(editing_id) => {
                        *editing.borrow_mut() = editing_id.clone();
                        if order.publishes_to_open_window()
                            && let Some(window) = weak.upgrade()
                        {
                            drafts.reapply_summary_template(&window, &editing_id);
                        }
                    }
                },
            );
        });
    });

    wire_summary_template_edit_callbacks(
        window,
        settings_state.clone(),
        summary_template_editing.clone(),
        settings_drafts.clone(),
    );

    let weak = window.as_weak();
    let settings_io_for_template_add = settings_io.clone();
    let settings_state_for_template_add = settings_state.clone();
    let summary_template_editing_for_add = summary_template_editing.clone();
    let summary_template_add_revision_for_add = summary_template_add_revision.clone();
    window.on_settings_summary_template_add_requested(move |name| {
        let draft = name.to_string();
        let persisted_name = draft.trim().to_string();
        if persisted_name.is_empty() {
            return;
        }
        let new_id = uuid::Uuid::new_v4().to_string();
        let new_id_for_editing = new_id.clone();
        let weak = weak.clone();
        let state = settings_state_for_template_add.clone();
        let editing = summary_template_editing_for_add.clone();
        let submitted_revision = summary_template_add_revision_for_add.get().wrapping_add(1);
        summary_template_add_revision_for_add.set(submitted_revision);
        let latest_revision = summary_template_add_revision_for_add.clone();
        let submitted_draft = draft.clone();
        save_settings_field_then(
            &settings_io_for_template_add,
            souffle_lib::commands::SettingsSaveLane::General,
            move |settings| {
                let base_prompt = settings
                    .summary_templates
                    .iter()
                    .find(|template| template.id == "default")
                    .map(|template| template.prompt.clone())
                    .unwrap_or_default();
                settings
                    .summary_templates
                    .push(souffle_lib::settings::SummaryTemplate {
                        id: new_id,
                        name: persisted_name,
                        prompt: base_prompt,
                    });
            },
            move |order, outcome| match settings_drafts::settle_settings_submission(draft, outcome)
            {
                settings_drafts::SettingsSubmission::Committed => {
                    *editing.borrow_mut() = new_id_for_editing.clone();
                    if let Some(window) = weak.upgrade() {
                        clear_committed_summary_add_draft(
                            &window,
                            submitted_revision,
                            latest_revision.get(),
                            &submitted_draft,
                        );
                        if !order.publishes_to_open_window() {
                            return;
                        }
                        let guard = state.borrow();
                        if let Some(settings) = guard.as_ref() {
                            ia_ui::populate_summary_templates(
                                &window,
                                settings,
                                &new_id_for_editing,
                            );
                        }
                    }
                }
                settings_drafts::SettingsSubmission::Retained(name) => {
                    if order.publishes_to_open_window()
                        && let Some(window) = weak.upgrade()
                    {
                        window.set_settings_new_summary_template_draft(name.into());
                    }
                }
            },
        );
    });

    let weak = window.as_weak();
    let handle = tauri_handle.clone();
    let settings_io_for_model = settings_io.clone();
    let model_options_state_for_model = model_options_state.clone();
    let settings_state_for_model = settings_state.clone();
    let model_selection_revision = Rc::new(Cell::new(0_u64));
    let revision_for_model = model_selection_revision.clone();
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
        let revision = revision_for_model.get().wrapping_add(1);
        revision_for_model.set(revision);
        let latest_revision = revision_for_model.clone();
        let weak_for_completion = weak.clone();
        let handle_for_completion = handle.clone();
        let options_for_completion = model_options_state_for_model.clone();
        let cache_for_completion = settings_state_for_model.clone();
        save_settings_field_then(
            &settings_io_for_model,
            souffle_lib::commands::SettingsSaveLane::General,
            move |settings| {
                settings.transcription_engine_id = engine_id;
                settings.transcription_model_id = model_id;
                settings.transcription_backend_id = backend_id;
            },
            move |_, outcome| {
                if revision != latest_revision.get() {
                    return;
                }
                let fallback = cache_for_completion.borrow().clone();
                let Some(window) = weak_for_completion.upgrade() else {
                    return;
                };
                if let Err(error) = settle_model_selection_save(
                    outcome,
                    &fallback,
                    selection,
                    |settings| {
                        project_model_selection_snapshot(&window, settings, &options_for_completion)
                    },
                    |selection| {
                        start_model_transition(
                            weak_for_completion.clone(),
                            handle_for_completion,
                            selection,
                        );
                    },
                ) {
                    window.set_settings_model_error_message(error.into());
                }
            },
        );
    });

    let weak = window.as_weak();
    let settings_io_for_learn = settings_io.clone();
    window.on_settings_dictation_learn_from_edit_changed(move |enabled| {
        save_settings_field(
            &settings_io_for_learn,
            souffle_lib::commands::SettingsSaveLane::General,
            move |settings| settings.dictation_learn_from_edit = enabled,
        );
        if let Some(window) = weak.upgrade() {
            window.set_settings_dictation_learn_from_edit(enabled);
        }
    });

    let weak = window.as_weak();
    let handle = tauri_handle.clone();
    let settings_list_models_for_dictionary_add = settings_list_models.clone();
    window.on_settings_dictionary_add_requested(move |term, pronunciation, category| {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let state = Arc::clone(&handle);
        let draft = lists_ui::DictionaryAddDraft {
            term: term.to_string(),
            pronunciation: pronunciation.to_string(),
            category: category.to_string(),
        };
        let result =
            lists_ui::submit_dictionary_add(draft, move |term, pronunciation, category| {
                souffle_lib::commands::add_dictionary_entry(state, term, pronunciation, category)
            });
        match result {
            lists_ui::ListMutation::Committed(_) => {
                window.set_settings_dictionary_add_error("".into());
                window.set_settings_new_dictionary_term_draft("".into());
                window.set_settings_new_dictionary_pronunciation_draft("".into());
                window.set_settings_new_dictionary_category_draft("".into());
                let state = Arc::clone(&handle);
                match souffle_lib::commands::list_dictionary(state) {
                    Ok(entries) => {
                        settings_list_models_for_dictionary_add.populate_dictionary(&entries)
                    }
                    Err(e) => eprintln!("Failed to reload dictionary: {e}"),
                }
            }
            lists_ui::ListMutation::Rejected { draft, error } => {
                window.set_settings_new_dictionary_term_draft(draft.term.into());
                window.set_settings_new_dictionary_pronunciation_draft(draft.pronunciation.into());
                window.set_settings_new_dictionary_category_draft(draft.category.into());
                window.set_settings_dictionary_add_error(error.into());
            }
        }
    });

    let weak = window.as_weak();
    let handle = tauri_handle.clone();
    let settings_list_models_for_dictionary_delete = settings_list_models.clone();
    window.on_settings_dictionary_delete_requested(move |id| {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let state = Arc::clone(&handle);
        let result = lists_ui::submit_dictionary_delete(id as i64, move |id| {
            souffle_lib::commands::delete_dictionary_entry(state, id)
        });
        match result {
            lists_ui::ListMutation::Committed(()) => {
                window.set_settings_dictionary_delete_error("".into());
                let state = Arc::clone(&handle);
                match souffle_lib::commands::list_dictionary(state) {
                    Ok(entries) => {
                        settings_list_models_for_dictionary_delete.populate_dictionary(&entries)
                    }
                    Err(e) => eprintln!("Failed to reload dictionary: {e}"),
                }
            }
            lists_ui::ListMutation::Rejected { draft: id, error } => {
                window.set_settings_dictionary_delete_error_id(id as i32);
                window.set_settings_dictionary_delete_error(error.into());
            }
        }
    });

    let weak = window.as_weak();
    let handle = tauri_handle.clone();
    let snippets_list_state_for_add = snippets_list_state.clone();
    let snippet_editing_for_add = snippet_editing.clone();
    let settings_list_models_for_snippet_add = settings_list_models.clone();
    window.on_settings_snippet_add_requested(move |trigger, expansion| {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let state = Arc::clone(&handle);
        let draft = lists_ui::SnippetDraft {
            trigger: trigger.to_string(),
            expansion: expansion.to_string(),
        };
        let result = lists_ui::submit_snippet_add(draft, move |trigger, expansion| {
            souffle_lib::commands::add_snippet(state, trigger, expansion)
        });
        match result {
            lists_ui::ListMutation::Committed(_) => {
                window.set_settings_snippet_add_error("".into());
                window.set_settings_new_snippet_trigger_draft("".into());
                window.set_settings_new_snippet_expansion_draft("".into());
                let state = Arc::clone(&handle);
                match souffle_lib::commands::list_snippets(state) {
                    Ok(entries) => {
                        let editing = *snippet_editing_for_add.borrow();
                        settings_list_models_for_snippet_add.populate_snippets(&entries, editing);
                        *snippets_list_state_for_add.borrow_mut() = entries;
                    }
                    Err(e) => eprintln!("Failed to reload snippets: {e}"),
                }
            }
            lists_ui::ListMutation::Rejected { draft, error } => {
                window.set_settings_new_snippet_trigger_draft(draft.trigger.into());
                window.set_settings_new_snippet_expansion_draft(draft.expansion.into());
                window.set_settings_snippet_add_error(error.into());
            }
        }
    });

    let weak = window.as_weak();
    let handle = tauri_handle.clone();
    let snippets_list_state_for_delete = snippets_list_state.clone();
    let snippet_editing_for_delete = snippet_editing.clone();
    let settings_list_models_for_snippet_delete = settings_list_models.clone();
    window.on_settings_snippet_delete_requested(move |id| {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let state = Arc::clone(&handle);
        let result = lists_ui::submit_snippet_delete(id as i64, move |id| {
            souffle_lib::commands::delete_snippet(state, id)
        });
        match result {
            lists_ui::ListMutation::Committed(()) => {
                window.set_settings_snippet_delete_error("".into());
                let state = Arc::clone(&handle);
                match souffle_lib::commands::list_snippets(state) {
                    Ok(entries) => {
                        let editing = *snippet_editing_for_delete.borrow();
                        settings_list_models_for_snippet_delete
                            .populate_snippets(&entries, editing);
                        *snippets_list_state_for_delete.borrow_mut() = entries;
                    }
                    Err(e) => eprintln!("Failed to reload snippets: {e}"),
                }
            }
            lists_ui::ListMutation::Rejected { draft: id, error } => {
                window.set_settings_snippet_delete_error_id(id as i32);
                window.set_settings_snippet_delete_error(error.into());
            }
        }
    });

    let weak = window.as_weak();
    let snippets_list_state_for_edit = snippets_list_state.clone();
    let snippet_editing_for_edit = snippet_editing.clone();
    let settings_list_models_for_snippet_edit = settings_list_models.clone();
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
        settings_list_models_for_snippet_edit.set_snippet_editing(Some(id));
    });

    let weak = window.as_weak();
    let snippet_editing_for_cancel = snippet_editing.clone();
    let settings_list_models_for_snippet_cancel = settings_list_models.clone();
    window.on_settings_snippet_cancel_edit_requested(move || {
        *snippet_editing_for_cancel.borrow_mut() = None;
        if weak.upgrade().is_some() {
            settings_list_models_for_snippet_cancel.set_snippet_editing(None);
        }
    });

    let weak = window.as_weak();
    let handle = tauri_handle.clone();
    let snippets_list_state_for_save = snippets_list_state.clone();
    let snippet_editing_for_save = snippet_editing.clone();
    let settings_list_models_for_snippet_save = settings_list_models;
    window.on_settings_snippet_save_edit_requested(move |id, trigger, expansion| {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let state = Arc::clone(&handle);
        let draft = lists_ui::SnippetDraft {
            trigger: trigger.to_string(),
            expansion: expansion.to_string(),
        };
        let result = lists_ui::submit_snippet_update(draft, move |trigger, expansion| {
            souffle_lib::commands::update_snippet(state, id as i64, trigger, expansion)
        });
        match result {
            lists_ui::ListMutation::Committed(()) => {
                window.set_settings_snippet_update_error("".into());
                window.set_settings_edit_snippet_trigger_draft("".into());
                window.set_settings_edit_snippet_expansion_draft("".into());
                *snippet_editing_for_save.borrow_mut() = None;
                let state = Arc::clone(&handle);
                match souffle_lib::commands::list_snippets(state) {
                    Ok(entries) => {
                        settings_list_models_for_snippet_save.populate_snippets(&entries, None);
                        *snippets_list_state_for_save.borrow_mut() = entries;
                    }
                    Err(e) => eprintln!("Failed to reload snippets: {e}"),
                }
            }
            lists_ui::ListMutation::Rejected { draft, error } => {
                window.set_settings_edit_snippet_trigger_draft(draft.trigger.into());
                window.set_settings_edit_snippet_expansion_draft(draft.expansion.into());
                window.set_settings_snippet_update_error(error.into());
            }
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
        apply_shortcut(&handle, &shortcuts_state_for_capture, &window, field, value);
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
            field,
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
            field,
            String::new(),
        );
    });

    let weak = window.as_weak();
    let pending_modifier_for_cancel = pending_modifier.clone();
    window.on_settings_shortcut_cancelled(move || {
        *pending_modifier_for_cancel.borrow_mut() = None;
        if let Some(window) = weak.upgrade() {
            window.set_settings_recording_field(ShortcutField::None);
            window.set_settings_shortcut_error("".into());
        }
    });

    let weak = window.as_weak();
    let permissions_for_review = permissions.clone();
    window.on_settings_permissions_review_requested(move || {
        let Some(window) = weak.upgrade() else {
            return;
        };
        window.set_settings_permissions_dialog_open(true);
        permissions_for_review.refresh();
    });

    let weak = window.as_weak();
    let handle = tauri_handle.clone();
    window.on_settings_copy_diagnostics_requested(move || {
        let Some(window) = weak.upgrade() else {
            return;
        };
        window.set_settings_copying_diagnostics(true);
        let state = Arc::clone(&handle);
        let weak = weak.clone();
        let worker = souffle_lib::async_runtime::spawn_blocking(move || {
            souffle_lib::commands::get_diagnostics_text(state)
        });
        slint::spawn_local(async move {
            let text = worker
                .await
                .map_err(|error| format!("Join diagnostics worker: {error}"))
                .and_then(|result| result);
            let Some(window) = weak.upgrade() else {
                return;
            };
            window.set_settings_copying_diagnostics(false);
            match text {
                Ok(text) => copy_to_clipboard(&text),
                Err(error) => eprintln!("Failed to build diagnostics text: {error}"),
            }
        })
        .expect("slint event loop not running");
    });

    let weak = window.as_weak();
    window.on_dismiss_transcription_status(move || {
        if let Some(window) = weak.upgrade() {
            window.set_transcription_status_message("".into());
        }
    });
    let weak = window.as_weak();
    window.on_copy_dictation_recovery(move || {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let text = window.get_dictation_recovery_text().to_string();
        if text.trim().is_empty() {
            return;
        }
        match souffle_lib::commands::copy_text(text) {
            Ok(()) => {
                window.set_dictation_recovery_text("".into());
                window.set_transcription_status_message(
                    "Texte de la dictée copié dans le presse-papiers.".into(),
                );
            }
            Err(error) => window.set_transcription_status_message(
                format!("Copie impossible; le texte reste disponible : {error}").into(),
            ),
        }
    });
    let weak = window.as_weak();
    window.on_discard_dictation_recovery(move || {
        if let Some(window) = weak.upgrade() {
            window.set_dictation_recovery_text("".into());
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StartupPresentationState {
    Loading { pending_navigation: Option<AppView> },
    Ready,
}

struct StartupPresentationGate {
    state: Mutex<StartupPresentationState>,
}

impl StartupPresentationGate {
    fn new() -> Self {
        Self {
            state: Mutex::new(StartupPresentationState::Loading {
                pending_navigation: None,
            }),
        }
    }

    /// Returns true once presentation is ready. While startup is loading,
    /// retain only the latest navigation request; the initial completion will
    /// show the window exactly once after projecting either observed settings
    /// or the explicit fallback.
    fn dispatch_or_defer(&self, navigation: Option<AppView>) -> bool {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        match *state {
            StartupPresentationState::Loading {
                ref mut pending_navigation,
            } => {
                if let Some(navigation) = navigation {
                    *pending_navigation = Some(navigation);
                }
                false
            }
            StartupPresentationState::Ready => true,
        }
    }

    fn finish(&self) -> Option<AppView> {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        match *state {
            StartupPresentationState::Loading { pending_navigation } => {
                *state = StartupPresentationState::Ready;
                pending_navigation
            }
            StartupPresentationState::Ready => None,
        }
    }
}

fn finish_startup_presentation(window: &MainWindow, gate: &StartupPresentationGate) {
    match gate.finish() {
        Some(AppView::Home) => {
            if window.get_settings_open() {
                window.set_settings_open(false);
                window.invoke_settings_closed();
            }
        }
        Some(AppView::Settings) if !window.get_settings_open() => {
            window.invoke_settings_requested();
        }
        Some(AppView::Settings) => {}
        None => {}
    }
    window.show().expect("failed to show Slint window");
}

/// Handles a [`souffle_lib::native::bridge::NativeAction`] on the Slint main
/// thread by invoking the same window callbacks a button click would (SOU-191).
fn dispatch_native_action(
    window: &MainWindow,
    handle: &AppHandle,
    startup_gate: &StartupPresentationGate,
    action: NativeAction,
) {
    match action {
        NativeAction::RefreshRuntime => refresh_model_runtime(window.as_weak(), handle.clone()),
        NativeAction::ToggleDictation => match window.get_recording_mode() {
            RecordingMode::Idle => window.invoke_dictate_requested(),
            RecordingMode::Dictation => window.invoke_stop_requested(),
            // A meeting owns the recording session (SOU-044): the toggle
            // shortcut must not start dictation on top of it.
            RecordingMode::Meeting => {}
        },
        NativeAction::PttStart => {
            if window.get_recording_mode() == RecordingMode::Idle {
                window.invoke_dictate_requested();
            }
        }
        NativeAction::PttStop => {
            if window.get_recording_mode() == RecordingMode::Dictation {
                window.invoke_stop_requested();
            }
        }
        NativeAction::CancelDictation => {
            if window.get_recording_mode() == RecordingMode::Dictation {
                window.invoke_cancel_dictation_requested();
            }
        }
        NativeAction::StopMeeting => {
            if window.get_recording_mode() == RecordingMode::Meeting {
                window.invoke_stop_requested();
            }
        }
        NativeAction::StopDictation => {
            if window.get_recording_mode() == RecordingMode::Dictation {
                window.invoke_stop_requested();
            }
        }
        NativeAction::Navigate(AppView::Home) => {
            if !startup_gate.dispatch_or_defer(Some(AppView::Home)) {
                return;
            }
            if window.get_settings_open() {
                window.set_settings_open(false);
                window.invoke_settings_closed();
            }
        }
        NativeAction::Navigate(AppView::Settings) => {
            if !startup_gate.dispatch_or_defer(Some(AppView::Settings)) {
                return;
            }
            if !window.get_settings_open() {
                window.invoke_settings_requested();
            }
        }
        NativeAction::ShowMainWindow => {
            if !startup_gate.dispatch_or_defer(None) {
                return;
            }
            let _ = window.show();
        }
        NativeAction::Quit => window.invoke_settings_quit_requested(),
        NativeAction::UpdateAvailable {
            latest_version,
            release_notes,
            release_url,
        } => {
            if window.get_whats_new_open() || window.get_update_available_open() {
                return;
            }
            window.set_update_latest_version(latest_version.into());
            let notes = release_notes
                .unwrap_or_else(|| "Voir les notes de version sur GitHub.".to_string());
            let blocks = markdown::render_blocks(&notes);
            window.set_update_release_notes_blocks(
                std::rc::Rc::new(slint::VecModel::from(blocks)).into(),
            );
            window.set_update_release_url(release_url.unwrap_or_default().into());
            window.set_update_phase("idle".into());
            let blocked = souffle_lib::commands::get_update_install_block(Arc::clone(handle))
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
    }
}

/// Receives actions from native OS surfaces (global shortcuts, tray menu,
/// pill HUD stop button, a second app launch, the background update
/// checker) that have no window of their own to act through, and forwards
/// each to the Slint main thread (SOU-191). Replaces the pre-191
/// `tauri_specta::Event::emit` calls those surfaces used to make, which had
/// no listener once the Svelte webview was removed.
fn spawn_native_action_receiver(
    weak: slint::Weak<MainWindow>,
    handle: AppHandle,
    startup_gate: Arc<StartupPresentationGate>,
) {
    let (tx, rx) = crossbeam_channel::unbounded();
    souffle_lib::native::bridge::set_sink(tx);
    std::thread::Builder::new()
        .name("native-action-receiver".into())
        .spawn(move || {
            for action in rx {
                let weak = weak.clone();
                let handle = handle.clone();
                let startup_gate = Arc::clone(&startup_gate);
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(window) = weak.upgrade() {
                        dispatch_native_action(&window, &handle, &startup_gate, action);
                    }
                });
            }
        })
        .expect("failed to spawn native action receiver thread");
}

fn main() {
    if let Some(code) = souffle_lib::cli::try_run_headless() {
        std::process::exit(code);
    }
    if !souffle_lib::bootstrap::acquire_single_instance() {
        return;
    }

    // Real bootstrap (audio thread, engine actor, DB). Must run before
    // Slint's own window/event loop; `handle` is the same `Arc<AppState>`
    // every command below takes.
    let handle: AppHandle = souffle_lib::bootstrap::bootstrap();

    // Native equivalent of Tauri's `titleBarStyle: "overlay"` + `hiddenTitle:
    // true`: keep the traffic lights but remove the native title bar strip
    // and let AppHeader (main_window.slint) draw one continuous dark header
    // with the lights inline, instead of stacking a second "Soufflé" title
    // bar on top of it. Must run before `MainWindow::new()` - it configures
    // the window attributes winit uses to create the NSWindow.
    #[cfg(target_os = "macos")]
    slint::BackendSelector::new()
        .renderer_name("skia".into())
        .with_winit_window_attributes_hook(|attrs| {
            use slint::winit_030::winit::platform::macos::WindowAttributesExtMacOS;
            attrs
                .with_titlebar_transparent(true)
                .with_title_hidden(true)
                .with_fullsize_content_view(true)
                .with_movable_by_window_background(true)
        })
        .select()
        .expect("failed to select winit backend");

    let window = MainWindow::new().expect("failed to create Slint window");

    // Close hides; it must not destroy. The pill + tray keep the process
    // alive, so a destroyed main window leaves Soufflé in the Dock and menu
    // bar with no window to bring back (mirrors the pre-191 Tauri
    // `CloseRequested` -> `prevent_close` + `hide` behavior).
    window
        .window()
        .on_close_requested(|| slint::CloseRequestResponse::HideWindow);

    let startup_gate = Arc::new(StartupPresentationGate::new());
    spawn_native_action_receiver(window.as_weak(), handle.clone(), Arc::clone(&startup_gate));

    window.set_settings_app_version(souffle_lib::commands::get_app_version().version.into());

    // Startup Settings are loaded through the same serialized worker as every
    // later save. Creating the Slint item tree never performs a database or
    // ServiceManagement read on the UI thread.
    let settings_state = SettingsCache::new(&window);
    let settings_io =
        settings_io::SettingsIoCoordinator::new(handle.clone(), settings_state.clone());
    let permissions = permissions_ui::PermissionController::new(&window);
    permissions.wire_foreground_refresh(&window);

    refresh_timeline(&window, &handle);
    let onboarding_state = wire_onboarding_callbacks(
        &window,
        handle.clone(),
        settings_io.clone(),
        permissions.clone(),
    );
    wire_update_dialogs(&window, handle.clone(), settings_io.clone());
    wire_callbacks(
        &window,
        handle.clone(),
        permissions.clone(),
        settings_state,
        settings_io.clone(),
    );
    let startup_app_handle = handle.clone();

    let weak = window.as_weak();
    let startup_io = settings_io.clone();
    let startup_gate_for_completion = Arc::clone(&startup_gate);
    settings_io.load_startup_snapshot(move |snapshot| match snapshot {
        settings_io::SettingsSnapshotResult::Observed(settings) => {
            let worker_handle = startup_app_handle.clone();
            let worker = souffle_lib::async_runtime::spawn_blocking(move || {
                load_startup_runtime_snapshot(worker_handle, *settings)
            });
            let weak = weak.clone();
            let startup_app_handle = startup_app_handle.clone();
            let onboarding_state = onboarding_state.clone();
            let permissions = permissions.clone();
            let startup_io = startup_io.clone();
            let startup_gate = Arc::clone(&startup_gate_for_completion);
            slint::spawn_local(async move {
                let result = worker
                    .await
                    .map_err(|error| format!("Failed to join startup worker: {error}"))
                    .and_then(|result| result);
                let Some(window) = weak.upgrade() else {
                    return;
                };
                match result {
                    Ok(startup) => {
                        let dark =
                            project_startup_settings(&window, &startup.settings, &onboarding_state);
                        souffle_lib::native::appearance::apply_resolved(dark);
                        window.set_dictation_shortcut(
                            format_shortcut_label(&startup.shortcuts.toggle).into(),
                        );
                        let unload_timeout_options =
                            souffle_lib::settings::SettingsOptions::current()
                                .model_unload_timeout_minutes;
                        model_ui::populate_options(
                            &window,
                            &startup.catalog,
                            startup.settings.model_unload_timeout_minutes,
                            &unload_timeout_options,
                        );
                        let onboarding_open = initialize_onboarding(
                            &window,
                            &startup,
                            &startup_app_handle,
                            &onboarding_state,
                            &permissions,
                        );
                        initialize_model_at_startup(
                            &window,
                            startup_app_handle.clone(),
                            onboarding_open,
                            &startup.catalog,
                            startup.model_phase,
                        );
                        apply_startup_update_dialogs(
                            &window,
                            startup_app_handle,
                            onboarding_open,
                            startup.settings,
                            startup_io,
                        );
                    }
                    Err(error) => {
                        eprintln!("Startup runtime snapshot is unavailable: {error}");
                        window.set_settings_model_error_message(error.into());
                    }
                }
                finish_startup_presentation(&window, &startup_gate);
            })
            .expect("slint event loop not running");
        }
        settings_io::SettingsSnapshotResult::Unavailable => {
            eprintln!("Startup Settings snapshot is unavailable; using UI defaults");
            let Some(window) = weak.upgrade() else {
                return;
            };
            finish_startup_presentation(&window, &startup_gate_for_completion);
        }
    });

    // Capture publishes its normalized RMS at ~15 Hz. Polling the lock-free
    // value on Slint's event loop keeps rendering single-threaded while the
    // native pill and the main window display the exact same signal.
    let audio_level_timer = slint::Timer::default();
    let weak = window.as_weak();
    audio_level_timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(66),
        move || {
            if let Some(window) = weak.upgrade() {
                window.set_audio_level(souffle_lib::pill::current_rms());
            }
        },
    );

    // The first window is deliberately shown only after the async startup
    // snapshot is projected, and closing it hides rather than destroys it.
    // Keep the native tray/shortcut process alive across both intervals.
    slint::run_event_loop_until_quit().expect("event loop failed");
}

#[cfg(test)]
mod tests {
    use super::{
        AudioDeviceProjectionState, AudioDeviceSaveSettlement, DictationEndIntent,
        DictationTextBuffers, DictationTranscriptDisposition, MainWindow, OnboardingState,
        SettingsCache, StartupPresentationGate, SummaryRefreshState, Theme,
        apply_audio_device_projection_settlement, audio_device_load_configuration,
        clear_committed_summary_add_draft, dictation_transcript_disposition,
        finish_startup_presentation, merge_recovery_text, prime_summary_template_editor,
        project_model_selection_snapshot, project_startup_settings, reset_live_buffers,
        settle_audio_device_save, settle_model_selection_save, settle_onboarding_completion,
        should_clear_committed_summary_add_draft, wire_summary_template_edit_callbacks,
    };
    use crate::model_ui;
    use crate::settings_drafts::SettingsDraftController;
    use crate::settings_io::SettingsIoCoordinator;
    use slint::ComponentHandle;
    use slint::platform::{Platform, WindowAdapter, software_renderer::MinimalSoftwareWindow};
    use souffle_lib::commands::{SettingsSaveError, SettingsSaveOutcome};
    use souffle_lib::settings::AppSettings;
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    struct TestPlatform;

    impl Platform for TestPlatform {
        fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
            Ok(MinimalSoftwareWindow::new(Default::default()))
        }
    }

    fn test_window() -> MainWindow {
        let _ = slint::platform::set_platform(Box::new(TestPlatform));
        MainWindow::new().unwrap()
    }

    #[test]
    fn summary_refresh_accepts_only_the_latest_generation() {
        let state = SummaryRefreshState::new(Rc::new(RefCell::new(None)));
        let url_a = state.begin();
        let url_b = state.begin();

        assert!(!state.accepts(url_a));
        assert!(state.accepts(url_b));

        let _ = state.invalidate();
        assert!(!state.accepts(url_b));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn final_executable_can_resolve_the_swift_concurrency_runtime() {
        // A linker argument emitted by souffle_lib's build script does not
        // reach this binary. Inspect the linked executable, not a source string:
        // losing this rpath made FoundationModels abort on TaskPriority metadata.
        let output = std::process::Command::new("/usr/bin/otool")
            .arg("-l")
            .arg(std::env::current_exe().expect("test executable"))
            .output()
            .expect("inspect Mach-O load commands");
        assert!(output.status.success());
        let commands = String::from_utf8(output.stdout).expect("Mach-O command text");
        assert!(commands.split("Load command ").any(|command| {
            command.contains("cmd LC_RPATH") && command.contains("path /usr/lib/swift (offset ")
        }));
    }

    #[test]
    fn failed_finalization_retains_text_but_cancel_always_discards_it() {
        assert_eq!(
            dictation_transcript_disposition(DictationEndIntent::Finalize, true),
            DictationTranscriptDisposition::Clear
        );
        assert_eq!(
            dictation_transcript_disposition(DictationEndIntent::Finalize, false),
            DictationTranscriptDisposition::RetainForRecovery
        );
        assert_eq!(
            dictation_transcript_disposition(DictationEndIntent::Cancel, true),
            DictationTranscriptDisposition::Clear
        );
        assert_eq!(
            dictation_transcript_disposition(DictationEndIntent::Cancel, false),
            DictationTranscriptDisposition::Clear
        );
    }

    #[test]
    fn every_live_reset_preserves_the_independent_recovery_buffer() {
        let reset = reset_live_buffers(DictationTextBuffers {
            live: "new live dictation".into(),
            tentative: "partial".into(),
            me: "speaker me".into(),
            them: "speaker them".into(),
            recovery: "older recoverable draft".into(),
        });
        assert!(reset.live.is_empty());
        assert!(reset.tentative.is_empty());
        assert!(reset.me.is_empty());
        assert!(reset.them.is_empty());
        assert_eq!(reset.recovery, "older recoverable draft");
        assert_eq!(
            merge_recovery_text(&reset.recovery, "second failed dictation"),
            "older recoverable draft\n\n——\n\nsecond failed dictation"
        );
    }

    #[test]
    fn onboarding_finishes_only_after_a_committed_settings_outcome() {
        let committed = Rc::new(Cell::new(false));
        let retained_message = Rc::new(RefCell::new(None));
        let committed_flag = committed.clone();
        let retained = retained_message.clone();
        settle_onboarding_completion(
            &SettingsSaveOutcome::Observed {
                settings: Box::new(AppSettings::default()),
                result: Err(SettingsSaveError::NotCommitted {
                    message: "database unavailable".into(),
                }),
            },
            move || committed_flag.set(true),
            move |message| *retained.borrow_mut() = Some(message),
        );
        assert!(!committed.get());
        assert_eq!(
            retained_message.borrow().as_deref(),
            Some("database unavailable")
        );

        let committed = Rc::new(Cell::new(false));
        let retained = Rc::new(Cell::new(false));
        let committed_flag = committed.clone();
        let retained_flag = retained.clone();
        settle_onboarding_completion(
            &SettingsSaveOutcome::Observed {
                settings: Box::new(AppSettings::default()),
                result: Ok(()),
            },
            move || committed_flag.set(true),
            move |_| retained_flag.set(true),
        );
        assert!(committed.get());
        assert!(!retained.get());
    }

    #[test]
    fn committed_summary_add_clears_only_the_matching_latest_draft() {
        assert!(should_clear_committed_summary_add_draft(
            7, 7, "Team", "Team"
        ));
        assert!(!should_clear_committed_summary_add_draft(
            7, 8, "Team", "Team"
        ));
        assert!(!should_clear_committed_summary_add_draft(
            7,
            7,
            "Team",
            "Newer draft"
        ));
    }

    #[test]
    fn startup_projection_uses_the_observed_snapshot_before_first_show() {
        let window = test_window();
        window.global::<Theme>().set_dark(true);
        let onboarding = Rc::new(RefCell::new(OnboardingState::default()));
        let settings = AppSettings {
            theme: souffle_lib::settings::Theme::Light,
            locale: "fr".into(),
            audio_device: Some("mic-1".into()),
            auto_paste: true,
            calendar_integration_enabled: true,
            ..AppSettings::default()
        };

        let dark = project_startup_settings(&window, &settings, &onboarding);

        assert!(!dark);
        assert!(!window.global::<Theme>().get_dark());
        assert!(window.get_settings_calendar_enabled());
        assert_eq!(window.get_onboarding_locale().as_str(), "fr");
        let onboarding = onboarding.borrow();
        assert_eq!(onboarding.selected_device, "mic-1");
        assert!(onboarding.auto_paste);
    }

    #[test]
    fn first_show_stays_gated_until_success_or_failure_fallback_is_ready() {
        for snapshot in [
            crate::settings_io::SettingsSnapshotResult::Observed(Box::default()),
            crate::settings_io::SettingsSnapshotResult::Unavailable,
        ] {
            let window = test_window();
            let gate = StartupPresentationGate::new();
            assert!(!gate.dispatch_or_defer(None));
            assert!(!window.window().is_visible());

            match snapshot {
                crate::settings_io::SettingsSnapshotResult::Observed(_) => {}
                crate::settings_io::SettingsSnapshotResult::Unavailable => {}
            }
            finish_startup_presentation(&window, &gate);

            assert!(window.window().is_visible());
            assert!(gate.dispatch_or_defer(None));
        }
    }

    #[test]
    fn rejected_model_save_restores_both_labels_without_starting_transition() {
        let window = test_window();
        let previous = AppSettings::default();
        let previous_catalog =
            souffle_lib::commands::transcription_catalog_from_settings(&previous).unwrap();
        let expected_header = model_ui::selected_model_short_label(&previous_catalog);
        let expected_picker = model_ui::list_available_model_options(&previous_catalog)
            .into_iter()
            .find(|option| {
                option.engine_id == previous_catalog.selected_engine_id
                    && option.model_id == previous_catalog.selected_model_id
            })
            .map(|option| option.label)
            .expect("default selection is in the catalogue");
        window.set_header_model_label("Optimistic".into());
        window.set_settings_selected_model_label("Optimistic".into());
        let options = Rc::new(RefCell::new(Vec::new()));
        let transition_started = Rc::new(Cell::new(false));
        let transition_for_callback = transition_started.clone();
        let outcome = SettingsSaveOutcome::Observed {
            settings: Box::new(previous),
            result: Err(SettingsSaveError::Rejected {
                message: "model rejected".into(),
            }),
        };

        settle_model_selection_save(
            &outcome,
            &None,
            souffle_lib::engine::TranscriptionProfileSelection {
                engine_id: "optimistic".into(),
                model_id: "optimistic".into(),
                backend_id: "optimistic".into(),
            },
            |settings| project_model_selection_snapshot(&window, settings, &options),
            move |_| transition_for_callback.set(true),
        )
        .unwrap();

        assert!(!transition_started.get());
        assert_eq!(window.get_header_model_label().as_str(), expected_header);
        assert_eq!(
            window.get_settings_selected_model_label().as_str(),
            expected_picker
        );
    }

    #[test]
    fn committed_unavailable_model_save_starts_the_submitted_not_cached_selection() {
        let old = AppSettings::default();
        let requested = souffle_lib::engine::TranscriptionProfileSelection {
            engine_id: "engine-b".into(),
            model_id: "model-b".into(),
            backend_id: "backend-b".into(),
        };
        let started = Rc::new(RefCell::new(None));
        let started_for_callback = started.clone();
        let projected = Rc::new(Cell::new(false));
        let projected_for_callback = projected.clone();
        let outcome = SettingsSaveOutcome::Unavailable {
            result: Ok(()),
            read_error: "controlled reread failure".into(),
        };

        settle_model_selection_save(
            &outcome,
            &Some(old),
            requested.clone(),
            move |_| {
                projected_for_callback.set(true);
                Ok(souffle_lib::engine::TranscriptionProfileSelection::default())
            },
            move |selection| *started_for_callback.borrow_mut() = Some(selection),
        )
        .unwrap();

        assert!(!projected.get());
        assert_eq!(started.borrow().as_ref(), Some(&requested));
    }

    #[test]
    fn device_effect_settlement_ignores_global_visibility_order_and_rolls_back_rejections() {
        let committed = AppSettings {
            audio_device: Some("mic-b".into()),
            ..AppSettings::default()
        };
        let committed_outcome = SettingsSaveOutcome::Observed {
            settings: Box::new(committed),
            result: Ok(()),
        };
        for order in [
            crate::settings_io::SettingsResponseOrder::Intermediate,
            crate::settings_io::SettingsResponseOrder::LatestHidden,
        ] {
            assert!(!order.publishes_to_open_window());
            match settle_audio_device_save(&committed_outcome, &None, "mic-b") {
                AudioDeviceSaveSettlement::Committed { uid, .. } => assert_eq!(uid, "mic-b"),
                AudioDeviceSaveSettlement::Rejected { .. } => {
                    panic!("committed device save was classified as rejected")
                }
            }
        }

        let previous = AppSettings {
            audio_device: Some("mic-a".into()),
            ..AppSettings::default()
        };
        let rejected = SettingsSaveOutcome::Observed {
            settings: Box::new(previous),
            result: Err(SettingsSaveError::Rejected {
                message: "device rejected".into(),
            }),
        };
        match settle_audio_device_save(&rejected, &None, "mic-b") {
            AudioDeviceSaveSettlement::Rejected { canonical } => assert_eq!(
                canonical
                    .expect("observed rejection has canonical settings")
                    .audio_device
                    .as_deref(),
                Some("mic-a")
            ),
            AudioDeviceSaveSettlement::Committed { .. } => {
                panic!("rejected device save was classified as committed")
            }
        }
    }

    #[test]
    fn blocked_device_load_cannot_publish_stale_uid_after_unavailable_commit() {
        let window = test_window();
        window.set_settings_selected_device_label("Microphone B".into());
        let fallback = AppSettings {
            audio_device: Some("mic-a".into()),
            ..AppSettings::default()
        };
        let outcome = SettingsSaveOutcome::Unavailable {
            result: Ok(()),
            read_error: "controlled reread failure".into(),
        };
        let cache = SettingsCache::with_observed(&window, fallback.clone());
        let projection = Rc::new(RefCell::new(AudioDeviceProjectionState::Canonical {
            generation: 0,
        }));
        let (load_started_tx, load_started_rx) = std::sync::mpsc::sync_channel(1);
        let (release_load_tx, release_load_rx) = std::sync::mpsc::sync_channel(1);
        let blocked_load = std::thread::spawn(move || {
            load_started_tx.send(()).unwrap();
            release_load_rx.recv().unwrap();
        });
        load_started_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("controlled device load started with cached mic-a");

        // While CoreAudio is blocked, B commits but the actor cannot reread
        // persistence. This is the ordering that previously let captured A
        // overwrite the picker when the old load completed.
        projection.borrow_mut().begin_pending("mic-b".into());
        cache.observe_save_outcome_silent(&outcome);
        let settlement = settle_audio_device_save(&outcome, &Some(fallback), "mic-b");
        assert!(!settlement.has_observed_canonical());
        apply_audio_device_projection_settlement(&settlement, &mut projection.borrow_mut());
        match &settlement {
            AudioDeviceSaveSettlement::Committed { uid, canonical } => {
                assert_eq!(uid, "mic-b");
                assert!(canonical.is_none());
            }
            AudioDeviceSaveSettlement::Rejected { .. } => {
                panic!("committed device save was classified as rejected")
            }
        }
        release_load_tx.send(()).unwrap();
        blocked_load.join().unwrap();
        let (_, selected, _, _) = audio_device_load_configuration(&cache, &projection)
            .expect("unknown cache still has a pending selection");

        // Both the post-effect path and a manual Refresh use the pending
        // unknown selection instead of the stale preserved cache snapshot.
        assert_eq!(selected, "mic-b");
        assert_eq!(
            window.get_settings_selected_device_label().as_str(),
            "Microphone B"
        );
    }

    #[test]
    fn later_observed_device_mutation_supersedes_committed_unknown_selection() {
        let window = test_window();
        let initial = AppSettings {
            audio_device: Some("mic-a".into()),
            ..AppSettings::default()
        };
        let cache = SettingsCache::with_observed(&window, initial.clone());
        let projection = Rc::new(RefCell::new(AudioDeviceProjectionState::Canonical {
            generation: 0,
        }));

        projection.borrow_mut().begin_pending("mic-b".into());
        let unavailable = SettingsSaveOutcome::Unavailable {
            result: Ok(()),
            read_error: "controlled reread failure".into(),
        };
        cache.observe_save_outcome_silent(&unavailable);
        let settlement = settle_audio_device_save(&unavailable, &Some(initial), "mic-b");
        apply_audio_device_projection_settlement(&settlement, &mut projection.borrow_mut());
        let (_, selected, _, _) = audio_device_load_configuration(&cache, &projection)
            .expect("unknown cache preserves the committed selection");
        assert_eq!(selected, "mic-b");

        let later = AppSettings {
            audio_device: None,
            ..AppSettings::default()
        };
        cache.observe_save_outcome_silent(&SettingsSaveOutcome::Observed {
            settings: Box::new(later),
            result: Ok(()),
        });
        let (_, selected, _, _) = audio_device_load_configuration(&cache, &projection)
            .expect("later observed settings remain available");

        assert!(selected.is_empty());
        assert!(matches!(
            *projection.borrow(),
            AudioDeviceProjectionState::Canonical { .. }
        ));
    }

    #[test]
    fn summary_editor_callbacks_accept_drafts_while_core_refresh_is_blocked() {
        let window = test_window();
        let initial = AppSettings::default();
        let template_id = initial.default_summary_template_id.clone();
        let durable = Arc::new(Mutex::new(initial.clone()));
        let cache = SettingsCache::with_observed(&window, initial.clone());
        let (refresh_started_tx, refresh_started_rx) = std::sync::mpsc::sync_channel(1);
        let (release_refresh_tx, release_refresh_rx) = std::sync::mpsc::sync_channel(1);
        let release_refresh_rx = Arc::new(Mutex::new(release_refresh_rx));
        let load_general_state = Arc::clone(&durable);
        let load_effective_state = Arc::clone(&durable);
        let save_state = Arc::clone(&durable);
        let io = SettingsIoCoordinator::with_functions(
            cache.clone(),
            move || Ok(load_general_state.lock().unwrap().clone()),
            move || {
                refresh_started_tx.send(()).unwrap();
                release_refresh_rx.lock().unwrap().recv().unwrap();
                Ok(load_effective_state.lock().unwrap().clone())
            },
            move |settings| {
                *save_state.lock().unwrap() = settings.clone();
                SettingsSaveOutcome::Observed {
                    settings: Box::new(settings),
                    result: Ok(()),
                }
            },
            |settings| SettingsSaveOutcome::Observed {
                settings: Box::new(settings),
                result: Ok(()),
            },
        );
        let drafts = SettingsDraftController::new(&window, io.clone());
        let editing = Rc::new(RefCell::new(String::new()));
        prime_summary_template_editor(&window, &initial, &editing, &drafts);
        wire_summary_template_edit_callbacks(&window, cache, editing.clone(), drafts);

        let token = io.begin_open();
        io.load_effective_snapshot(token, |_| {});
        refresh_started_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("controlled core refresh started");
        assert_eq!(editing.borrow().as_str(), template_id);

        window.invoke_settings_summary_template_prompt_changed("Prompt before core".into());
        window.invoke_settings_summary_template_name_changed("Name before core".into());
        release_refresh_tx.send(()).unwrap();

        let deadline = Instant::now() + Duration::from_secs(1);
        while !io.is_idle_for_test() {
            io.drain_for_test();
            assert!(Instant::now() < deadline, "draft save did not settle");
            std::thread::yield_now();
        }
        let durable = durable.lock().unwrap();
        let template = durable
            .summary_templates
            .iter()
            .find(|template| template.id == template_id)
            .expect("edited template remains durable");
        assert_eq!(template.name, "Name before core");
        assert_eq!(template.prompt, "Prompt before core");
    }

    fn assert_add_draft_settlement_after_close(newer_draft: Option<&str>) {
        let window = test_window();
        let initial = AppSettings::default();
        let cache = SettingsCache::with_observed(&window, initial.clone());
        let (save_started_tx, save_started_rx) = std::sync::mpsc::sync_channel(1);
        let (release_save_tx, release_save_rx) = std::sync::mpsc::sync_channel(1);
        let release_save_rx = Arc::new(Mutex::new(release_save_rx));
        let io = SettingsIoCoordinator::with_functions(
            cache,
            {
                let initial = initial.clone();
                move || Ok(initial.clone())
            },
            {
                let initial = initial.clone();
                move || Ok(initial.clone())
            },
            move |settings| {
                save_started_tx.send(()).unwrap();
                release_save_rx.lock().unwrap().recv().unwrap();
                SettingsSaveOutcome::Observed {
                    settings: Box::new(settings),
                    result: Ok(()),
                }
            },
            |settings| SettingsSaveOutcome::Observed {
                settings: Box::new(settings),
                result: Ok(()),
            },
        );
        io.begin_open();
        window.set_settings_new_summary_template_draft("Team".into());
        let latest_revision = Rc::new(Cell::new(1_u64));
        let weak = window.as_weak();
        let latest_for_completion = latest_revision.clone();
        io.submit(
            souffle_lib::commands::SettingsSaveLane::General,
            |_| {},
            move |_order, outcome| {
                if super::settings_values::save_outcome_commit_status(outcome)
                    == super::settings_values::SettingsCommitStatus::Committed
                    && let Some(window) = weak.upgrade()
                {
                    clear_committed_summary_add_draft(
                        &window,
                        1,
                        latest_for_completion.get(),
                        "Team",
                    );
                }
            },
        );
        save_started_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("controlled template add started");
        let close_token = io.close();
        io.barrier(|_, _| {});
        if let Some(newer) = newer_draft {
            latest_revision.set(2);
            window.set_settings_new_summary_template_draft(newer.into());
        }
        release_save_tx.send(()).unwrap();

        let deadline = Instant::now() + Duration::from_secs(1);
        while !io.is_idle_for_test() {
            io.drain_for_test();
            assert!(Instant::now() < deadline, "close barrier did not settle");
            std::thread::yield_now();
        }
        assert!(io.accepts_close(close_token));
        io.begin_open();
        assert_eq!(
            window.get_settings_new_summary_template_draft().as_str(),
            newer_draft.unwrap_or_default()
        );
    }

    #[test]
    fn committed_add_clears_after_close_but_preserves_a_newer_draft() {
        assert_add_draft_settlement_after_close(None);
        assert_add_draft_settlement_after_close(Some("Newer draft"));
    }
}
