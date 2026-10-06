//! Native live paragraph edits: freeze a session-bound snapshot on opening,
//! commit on a worker, then project only the committed texts on the UI thread.

use std::{cell::RefCell, rc::Rc, sync::Arc};

use slint::ComponentHandle;
use souffle_lib::{commands::LiveParagraphEditContext, state::AppState};

use crate::{MainWindow, RecordingMode, live_view::LiveTranscriptState};

struct EditDraft {
    generation: u64,
    meeting_id: String,
    indices: Vec<u32>,
    context: LiveParagraphEditContext,
}

pub fn wire_callbacks(window: &MainWindow, state: Arc<AppState>, live: LiveTranscriptState) {
    let draft = Rc::new(RefCell::new(None::<EditDraft>));
    let weak = window.as_weak();
    let open_draft = draft.clone();
    let open_live = live.clone();
    let open_state = state.clone();
    window.on_live_paragraph_edit_requested(move |row| {
        let Some(window) = weak.upgrade() else { return };
        if window.get_recording_mode() != RecordingMode::Meeting
            || window.get_recording_starting()
            || window.get_recording_stop_pending()
            || window.get_live_edit_busy()
        {
            return;
        }
        let Ok(row) = usize::try_from(row) else {
            return;
        };
        let (generation, target) = {
            let live = open_live.lock().unwrap();
            (live.generation(), live.edit_target(row))
        };
        let Some((indices, original_text)) = target else {
            return;
        };
        // Only a short in-memory snapshot under this mutex; no window calls
        // or database work until the guard has been dropped.
        let snapshot = {
            let Ok(acc) = open_state.meeting_accumulator.lock() else {
                return;
            };
            acc.as_ref()
                .map(|meeting| (meeting.id.clone(), meeting.session_started_at))
        };
        let Some((meeting_id, session_started_at)) = snapshot else {
            return;
        };
        window.set_live_edit_text(original_text.clone().into());
        window.set_live_edit_failed(false);
        *open_draft.borrow_mut() = Some(EditDraft {
            generation,
            meeting_id,
            indices,
            context: LiveParagraphEditContext {
                session_started_at,
                original_text,
            },
        });
        window.set_live_edit_open(true);
    });

    let weak = window.as_weak();
    let cancel_draft = draft.clone();
    window.on_live_paragraph_edit_cancelled(move || {
        if let Some(window) = weak.upgrade()
            && !window.get_live_edit_busy()
        {
            cancel_draft.borrow_mut().take();
            window.set_live_edit_open(false);
        }
    });

    let weak = window.as_weak();
    window.on_live_paragraph_edit_saved(move |text| {
        let Some(window) = weak.upgrade() else { return };
        if !window.get_live_edit_open() || window.get_live_edit_busy() || text.trim().is_empty() {
            return;
        }
        let Some(snapshot) = draft.borrow_mut().take() else {
            return;
        };
        if snapshot.generation != live.lock().unwrap().generation()
            || window.get_recording_mode() != RecordingMode::Meeting
            || window.get_recording_stop_pending()
        {
            window.set_live_edit_open(false);
            return;
        }
        window.set_live_edit_busy(true);
        let state = state.clone();
        let generation = snapshot.generation;
        let worker = souffle_lib::async_runtime::spawn_blocking(move || {
            souffle_lib::commands::apply_live_paragraph_edit_checked(
                state,
                snapshot.meeting_id,
                snapshot.indices,
                text.to_string(),
                snapshot.context,
            )
        });
        let weak = weak.clone();
        let live = live.clone();
        slint::spawn_local(async move {
            let result = worker.await;
            let Some(window) = weak.upgrade() else { return };
            if live.lock().unwrap().generation() != generation {
                return;
            }
            window.set_live_edit_busy(false);
            match result {
                Ok(Ok(updates)) => {
                    live.lock().unwrap().apply_edit(generation, &updates);
                    crate::live_view::push_live_blocks(&window, &live);
                    window.set_live_edit_open(false);
                }
                Ok(Err(error)) => {
                    eprintln!("Live paragraph edit failed: {error}");
                    window.set_live_edit_failed(true);
                }
                Err(error) => {
                    eprintln!("Live paragraph edit worker failed: {error}");
                    window.set_live_edit_failed(true);
                }
            }
        })
        .expect("live paragraph editor on UI thread");
    });
}
