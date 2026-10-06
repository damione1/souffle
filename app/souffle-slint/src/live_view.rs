//! Glue between the live transcript state machine (`live_transcript.rs`) and
//! `MainWindow`'s live-meeting properties: segment routing and model push.
//!
//! Kept separate from `main.rs` so `examples/live_transcript_mock.rs` drives
//! the exact same code path as the app (SOU-256) - a fake segment stream goes
//! through `apply_live_segment`.

use std::sync::{Arc, Mutex};

use souffle_lib::engine::TranscriptionSegment;

use crate::live_transcript::{LiveTranscript, provisional_words};
use crate::{MainWindow, RecordingMode, TranscriptBlock, TranscriptWord};
use slint::{Model, ModelRc, VecModel};

pub type LiveTranscriptState = Arc<Mutex<LiveTranscript>>;

/// Routes by session mode: diarization is segment data, not a session kind.
/// Must run on the Slint main thread.
pub fn apply_live_segment(
    window: &MainWindow,
    live_state: &LiveTranscriptState,
    segment: &TranscriptionSegment,
) {
    match window.get_recording_mode() {
        RecordingMode::Idle => return,
        RecordingMode::Meeting => {
            {
                let mut live = live_state.lock().unwrap();
                if segment.is_final {
                    live.push_final(segment);
                } else {
                    live.push_tentative(segment);
                }
            }
            push_live_blocks(window, live_state);
            return;
        }
        RecordingMode::Dictation => {}
    }
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
        {
            let mut live = live_state.lock().unwrap();
            live.push_final(segment);
        }
        window.set_live_tentative("".into());
    } else {
        window.set_live_tentative(segment.text.trim().into());
    }
    push_dictation_words(window);
}

/// Queued updates from a cleared/stopped session cannot reach its successor.
/// The generation check and application both run on the UI thread.
pub fn apply_live_segment_for_generation(
    window: &MainWindow,
    live_state: &LiveTranscriptState,
    generation: u64,
    segment: &TranscriptionSegment,
) {
    if live_state.lock().unwrap().generation() == generation {
        apply_live_segment(window, live_state, segment);
    }
}

/// The dictation text as words, the word the engine still holds last and
/// dimmed (it used to be drawn like confirmed text, then vanish on the next
/// final). None is clickable: a dictation has no alias popover.
pub fn push_dictation_words(window: &MainWindow) {
    let text = window.get_live_text();
    let tentative = window.get_live_tentative();
    let words: Vec<TranscriptWord> = provisional_words(&text, Some(&tentative))
        .into_iter()
        .map(|word| TranscriptWord {
            clickable: false,
            ..word
        })
        .collect();
    window.set_live_dictation_words(ModelRc::new(VecModel::from(words)));
}

/// Publishes the current paragraphs to `live-transcript-blocks`. The first
/// call installs a `VecModel`; later calls update that same model row by
/// row, so the `for` rows in `recording_view.slint` are only rebuilt where
/// the text actually changed - instead of every row being torn down and
/// re-created on each segment and poll tick.
pub fn push_live_blocks(window: &MainWindow, live_state: &LiveTranscriptState) {
    let live = live_state.lock().unwrap();
    let is_empty = live.is_empty();
    let blocks = live.build_blocks();
    drop(live);
    let current = window.get_live_transcript_blocks();
    match current.as_any().downcast_ref::<VecModel<TranscriptBlock>>() {
        Some(model) => sync_blocks(model, blocks),
        None => window.set_live_transcript_blocks(ModelRc::new(VecModel::from(blocks))),
    }
    window.set_live_transcript_empty(is_empty);
}

/// What a live row renders. `words` is compared by content, not by model
/// identity (a fresh model is built on every `build_blocks`): the same text
/// can flip from provisional to finalized, which only changes which words
/// are clickable.
fn same_rendering(a: &TranscriptBlock, b: &TranscriptBlock) -> bool {
    a.text == b.text
        && a.can_edit == b.can_edit
        && a.has_speaker == b.has_speaker
        && a.speaker == b.speaker
        && a.timestamp == b.timestamp
        && a.start_time == b.start_time
        && a.words.row_count() == b.words.row_count()
        && a.words.iter().zip(b.words.iter()).all(|(x, y)| x == y)
}

fn sync_blocks(model: &VecModel<TranscriptBlock>, blocks: Vec<TranscriptBlock>) {
    let target = blocks.len();
    for (i, block) in blocks.into_iter().enumerate() {
        match model.row_data(i) {
            Some(existing) if same_rendering(&existing, &block) => {}
            Some(_) => model.set_row_data(i, block),
            None => model.push(block),
        }
    }
    while model.row_count() > target {
        model.remove(model.row_count() - 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RecordingMode;
    use crate::SpeakerRole;
    use i_slint_backend_testing::ElementHandle;
    use slint::ComponentHandle;
    use slint::platform::{Platform, WindowAdapter, software_renderer::MinimalSoftwareWindow};
    use souffle_lib::engine::Speaker;
    use std::rc::Rc;

    const WIDTH: u32 = 1040;
    const HEIGHT: u32 = 820;

    thread_local! {
        // The window the test platform handed out, so `settle` can render
        // frames into it. Slint's platform is per thread, as is each test.
        static TEST_WINDOW: std::cell::RefCell<Option<Rc<MinimalSoftwareWindow>>> =
            const { std::cell::RefCell::new(None) };
    }

    struct TestPlatform;

    impl Platform for TestPlatform {
        fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
            let window = MinimalSoftwareWindow::new(Default::default());
            TEST_WINDOW.with(|w| *w.borrow_mut() = Some(window.clone()));
            Ok(window)
        }
    }

    fn meeting_window() -> MainWindow {
        let _ = slint::platform::set_platform(Box::new(TestPlatform));
        let window = MainWindow::new().unwrap();
        window
            .window()
            .set_size(slint::PhysicalSize::new(WIDTH, HEIGHT));
        window.set_onboarding_open(false);
        window.set_recording_mode(RecordingMode::Meeting);
        window
    }

    #[test]
    fn batch_sources_render_live_then_round_trip_callback_db_detail_and_export() {
        use souffle_lib::{
            db::Database, engine::TranscriptionEngine, engine::mock::BatchSourceDecoder,
            progress::ProgressChannel, state::MeetingAccumulator,
        };
        let window = meeting_window();
        let live = Arc::new(Mutex::new(LiveTranscript::new()));
        let directory = tempfile::tempdir().unwrap();
        let db = Arc::new(Database::open(&directory.path().join("meeting.db")).unwrap());
        let started = chrono::Utc::now();
        let acc = Arc::new(Mutex::new(Some(MeetingAccumulator {
            id: "two-sources".into(),
            title: "Two sources".into(),
            existing_segments: vec![],
            new_segments: vec![],
            recording_sessions: vec![],
            session_started_at: started,
            transcription_profile: souffle_lib::engine::default_transcription_profile(),
            summary: None,
            summary_is_stale: false,
            summary_model: None,
            summary_generated_at: None,
            structured_summary: None,
            notes: None,
            calendar_event_id: None,
            participants: vec![],
            persisted_new_count: 0,
        })));
        let publications = Arc::new(Mutex::new(Vec::new()));
        let sink = publications.clone();
        let callback = souffle_lib::commands::meeting_callback_for_test(
            ProgressChannel::new(move |s| sink.lock().unwrap().push(s)),
            acc.clone(),
            db.clone(),
        );
        let clock = std::time::Instant::now();
        let mut decoder = BatchSourceDecoder::new(clock);
        for hop in 1..=16 {
            decoder.set_clock(clock + std::time::Duration::from_millis(hop * 100));
            for s in decoder.transcribe_dual(&[0.1; 1600], &[0.2; 1600]).unwrap() {
                callback(s);
            }
        }
        for s in publications.lock().unwrap().drain(..) {
            apply_live_segment(&window, &live, &s);
        }
        settle();
        text_element(&window, "Micro");
        text_element(&window, "Système");
        assert!(live.lock().unwrap().finals.is_empty());
        let blocks = window.get_live_transcript_blocks();
        assert_eq!(blocks.row_count(), 2);
        assert!(
            blocks
                .iter()
                .all(|b| b.has_speaker && b.words.iter().all(|w| w.provisional && !w.clickable))
        );
        // Withdrawal on Me must preserve the simultaneous system hypothesis.
        let mut withdrawal = final_seg("", 0.0, Speaker::Me);
        withdrawal.is_final = false;
        callback(withdrawal.clone());
        apply_live_segment(&window, &live, &withdrawal);
        assert!(
            window.get_live_transcript_blocks().iter().any(|b| b.speaker
                == crate::SpeakerRole::Them
                && b.text.as_str() == "Système 25600")
        );
        // Both short remainders become finals; repeated Stop/flush adds nothing.
        for s in decoder.flush().unwrap() {
            callback(s);
        }
        assert!(decoder.flush().unwrap().is_empty());
        for s in publications.lock().unwrap().drain(..) {
            apply_live_segment(&window, &live, &s);
        }
        let saved = acc
            .lock()
            .unwrap()
            .take()
            .unwrap()
            .into_transcript(started + chrono::Duration::seconds(2));
        assert_eq!(saved.segments.len(), 2);
        assert!(saved.segments.iter().all(|s| s.is_final));
        db.save_meeting(&saved).unwrap();
        let restored = db.load_meeting("two-sources").unwrap();
        assert_eq!(
            serde_json::to_value(&restored.segments).unwrap(),
            serde_json::to_value(&saved.segments).unwrap()
        );
        let details = crate::transcript::build_transcript_blocks(
            &restored.segments,
            &restored.recording_sessions,
        );
        assert_eq!(details.len(), 2);
        assert_eq!(details[0].speaker, crate::SpeakerRole::Me);
        assert_eq!(details[1].speaker, crate::SpeakerRole::Them);
        assert!(details.iter().all(|b| b.has_speaker && b.start_time == 0.0));
        let export = souffle_lib::export::render_meeting(
            &restored,
            souffle_lib::export::ExportFormat::Markdown,
        )
        .unwrap();
        assert!(export.contains("**Me** [0:00] Micro"));
        assert!(export.contains("**Them** [0:00] Système"));
        // Short Them window arrives first, then an earlier-start long Me
        // window. Both live and saved views must still sort by session time.
        live.lock().unwrap().clear();
        let later = final_seg("Système court", 4.0, Speaker::Them);
        let earlier = final_seg("Micro long", 1.0, Speaker::Me);
        apply_live_segment(&window, &live, &later);
        apply_live_segment(&window, &live, &earlier);
        let blocks = window.get_live_transcript_blocks();
        assert_eq!(blocks.row_data(0).unwrap().text.as_str(), "Micro long");
        assert_eq!(blocks.row_data(1).unwrap().text.as_str(), "Système court");
        let mut out_of_order = restored;
        out_of_order.recording_sessions.clear();
        out_of_order.segments = vec![later, earlier];
        let details = crate::transcript::build_transcript_blocks(&out_of_order.segments, &[]);
        assert_eq!(details[0].speaker, crate::SpeakerRole::Me);
        let export = souffle_lib::export::render_meeting(
            &out_of_order,
            souffle_lib::export::ExportFormat::Markdown,
        )
        .unwrap();
        assert!(export.find("Micro long").unwrap() < export.find("Système court").unwrap());
    }

    #[test]
    fn word_and_chunk_streams_share_dialogue_in_live_database_detail_and_export() {
        use souffle_lib::{db::Database, transcript::MeetingTranscript};
        let window = meeting_window();
        let live = Arc::new(Mutex::new(LiveTranscript::new()));
        let directory = tempfile::tempdir().unwrap();
        let db = Database::open(&directory.path().join("dialogue.db")).unwrap();
        // The consumer receives only the segment contract, with no model id
        // or decoding strategy. Both granularities represent the same turns.
        for word_stream in [true, false] {
            live.lock().unwrap().clear();
            let mut finals = Vec::new();
            for (text, start, speaker) in [
                ("First question.", 10.0, Speaker::Me),
                ("The answer.", 11.1, Speaker::Them),
                ("Next question.", 12.2, Speaker::Me),
                ("Another answer.", 13.3, Speaker::Them),
            ] {
                let parts: Vec<_> = if word_stream {
                    text.split_whitespace().collect()
                } else {
                    vec![text]
                };
                for (i, text) in parts.iter().enumerate() {
                    let offset = i as f64 / parts.len() as f64;
                    let mut segment = final_seg(text, start + offset, speaker);
                    segment.end_time = start + (i + 1) as f64 / parts.len() as f64;
                    segment.is_final = false;
                    apply_live_segment(&window, &live, &segment);
                    let provisional = window.get_live_transcript_blocks();
                    let last = provisional.row_data(provisional.row_count() - 1).unwrap();
                    assert_eq!(
                        last.speaker,
                        crate::transcript::speaker_fields(Some(speaker)).1
                    );
                    assert_eq!(
                        last.timestamp.as_str(),
                        crate::timeline::format_duration(start)
                    );
                    segment.is_final = true;
                    apply_live_segment(&window, &live, &segment);
                    finals.push(segment);
                }
            }
            let blocks = window.get_live_transcript_blocks();
            assert_eq!(blocks.row_count(), 4);
            let meeting: MeetingTranscript = serde_json::from_value(serde_json::json!({
                "id": "shared-stream", "title": "Dialogue", "started_at": "2026-10-04T12:00:00Z",
                "ended_at": "2026-10-04T12:00:15Z", "duration_seconds": 15.0,
                "segments": finals,
            }))
            .unwrap();
            db.save_meeting(&meeting).unwrap();
            let restored = db.load_meeting("shared-stream").unwrap();
            assert_eq!(
                serde_json::to_value(&restored.segments).unwrap(),
                serde_json::to_value(&meeting.segments).unwrap()
            );
            let detail = crate::transcript::build_transcript_blocks(&restored.segments, &[]);
            assert_eq!(detail.len(), 4);
            let exported = souffle_lib::export::render_meeting(
                &restored,
                souffle_lib::export::ExportFormat::Markdown,
            )
            .unwrap();
            let mut previous = 0;
            for (i, saved) in detail.iter().enumerate() {
                let shown = blocks.row_data(i).unwrap();
                assert_eq!(shown.text, saved.text);
                assert_eq!(shown.timestamp, saved.timestamp);
                assert_eq!(shown.speaker, saved.speaker);
                assert!(shown.words.iter().all(|w| !w.provisional));
                let position = exported.find(shown.text.as_str()).unwrap();
                assert!(position >= previous);
                previous = position;
            }
        }
    }

    #[test]
    fn mono_meeting_final_is_visible_without_a_speaker_label() {
        let window = meeting_window();
        let live_state = Arc::new(Mutex::new(LiveTranscript::new()));
        let mut segment = final_seg("Bonjour monde", 0.0, Speaker::Me);
        segment.speaker = None;
        apply_live_segment(&window, &live_state, &segment);
        settle();
        text_element(&window, "Bonjour");
        text_element(&window, "monde");
        assert!(!window.get_live_transcript_empty());
        assert!(
            window
                .get_live_transcript_blocks()
                .iter()
                .all(|b| !b.has_speaker)
        );
        assert!(window.get_live_dictation_words().row_count() == 0);
    }

    #[test]
    fn stopped_session_publications_cannot_pollute_a_new_session() {
        let window = meeting_window();
        let live_state = Arc::new(Mutex::new(LiveTranscript::new()));
        let previous = live_state.lock().unwrap().generation();
        live_state.lock().unwrap().clear();
        let mut segment = final_seg("Ancien aperçu", 0.0, Speaker::Me);
        segment.speaker = None;
        for is_final in [false, true] {
            segment.is_final = is_final;
            apply_live_segment_for_generation(&window, &live_state, previous, &segment);
            assert!(live_state.lock().unwrap().is_empty());
        }
        let current = live_state.lock().unwrap().generation();
        apply_live_segment_for_generation(&window, &live_state, current, &segment);
        assert!(!live_state.lock().unwrap().is_empty());
        live_state.lock().unwrap().clear();
        window.set_recording_mode(RecordingMode::Idle);
        apply_live_segment(&window, &live_state, &segment);
        assert!(live_state.lock().unwrap().is_empty());
    }

    #[test]
    fn dictation_snapshot_revisions_and_withdrawal_do_not_append_finals() {
        let window = meeting_window();
        window.set_recording_mode(RecordingMode::Dictation);
        let live_state = Arc::new(Mutex::new(LiveTranscript::new()));
        let mut segment = final_seg("Premier texte. Deuxième phrase.", 0.0, Speaker::Me);
        segment.speaker = None;
        segment.is_final = false;
        apply_live_segment(&window, &live_state, &segment);
        segment.text = "Nouvelle hypothèse complète.".into();
        apply_live_segment(&window, &live_state, &segment);
        assert!(window.get_live_text().is_empty());
        assert_eq!(
            window.get_live_tentative().as_str(),
            "Nouvelle hypothèse complète."
        );
        assert!(
            window
                .get_live_dictation_words()
                .iter()
                .all(|w| w.provisional && !w.clickable)
        );
        segment.text.clear();
        apply_live_segment(&window, &live_state, &segment);
        assert_eq!(window.get_live_dictation_words().row_count(), 0);
        segment.text = "Texte définitif".into();
        segment.is_final = true;
        apply_live_segment(&window, &live_state, &segment);
        assert_eq!(window.get_live_text().as_str(), "Texte définitif");
        assert!(window.get_live_tentative().is_empty());
    }

    #[test]
    fn controlled_batch_decoder_renders_before_1600ms_in_both_session_modes() {
        use souffle_lib::engine::{TranscriptionEngine, mock::BatchPreviewDecoder};
        use std::time::{Duration, Instant};
        for mode in [RecordingMode::Dictation, RecordingMode::Meeting] {
            let window = meeting_window();
            window.set_recording_mode(mode);
            let live = Arc::new(Mutex::new(LiveTranscript::new()));
            let clock = Instant::now();
            let mut decoder = BatchPreviewDecoder::new(clock);
            let hop = decoder.audio_requirements().chunk_size_samples as usize;
            for tick in 1..=30 {
                decoder.set_clock(clock + Duration::from_millis(tick * 100));
                let segments = decoder.transcribe(&vec![0.1; hop], None).unwrap();
                if tick < 15 {
                    assert!(segments.is_empty());
                }
                for segment in segments {
                    assert!(!segment.is_final);
                    apply_live_segment(&window, &live, &segment);
                }
                if tick == 15 {
                    settle();
                    text_element(&window, "Hypothèse");
                    text_element(&window, "24000");
                }
            }
            settle();
            text_element(&window, "48000");
            assert!(
                live.lock().unwrap().finals.is_empty(),
                "revisions do not grow finals"
            );
            assert!(window.get_live_text().is_empty());
            match mode {
                RecordingMode::Meeting => {
                    let blocks = window.get_live_transcript_blocks();
                    assert_eq!(blocks.row_count(), 1);
                    assert!(!blocks.row_data(0).unwrap().has_speaker);
                }
                RecordingMode::Dictation => {
                    assert_eq!(window.get_live_tentative().as_str(), "Hypothèse 48000")
                }
                RecordingMode::Idle => unreachable!("only active session modes tested"),
            }
        }
    }

    #[test]
    fn mono_meeting_first_preview_and_revision_replace_one_another() {
        let window = meeting_window();
        let live_state = Arc::new(Mutex::new(LiveTranscript::new()));
        let mut segment = final_seg("Provisoire", 0.0, Speaker::Me);
        segment.speaker = None;
        segment.is_final = false;
        apply_live_segment(&window, &live_state, &segment);
        settle();
        text_element(&window, "Provisoire");
        segment.text = "Révision complète".into();
        apply_live_segment(&window, &live_state, &segment);
        settle();
        let blocks = window.get_live_transcript_blocks();
        assert_eq!(blocks.row_count(), 1);
        let block = blocks.row_data(0).unwrap();
        assert!(!block.has_speaker);
        assert_eq!(block.text.as_str(), "Révision complète");
        assert!(block.words.iter().all(|w| w.provisional && !w.clickable));
        assert!(live_state.lock().unwrap().finals.is_empty());
        segment.is_final = true;
        apply_live_segment(&window, &live_state, &segment);
        assert!(
            window
                .get_live_transcript_blocks()
                .row_data(0)
                .unwrap()
                .words
                .iter()
                .all(|w| !w.provisional)
        );
    }

    fn final_seg(text: &str, start: f64, speaker: Speaker) -> TranscriptionSegment {
        TranscriptionSegment {
            text: text.into(),
            start_time: start,
            end_time: start + 1.0,
            is_final: true,
            language: None,
            confidence: None,
            speaker: Some(speaker),
        }
    }

    /// A few event-loop turns: timers and `changed` handlers, then a real
    /// software-rendered frame. The frame matters - the recording view and
    /// its rows are only instantiated (and their `changed` handlers only
    /// armed) once something walks or draws the tree, as the app's
    /// renderer does every frame.
    fn settle() {
        let window = TEST_WINDOW
            .with(|w| w.borrow().clone())
            .expect("no test window");
        let mut frame = vec![
            slint::platform::software_renderer::Rgb565Pixel::default();
            (WIDTH * HEIGHT) as usize
        ];
        for _ in 0..3 {
            slint::platform::update_timers_and_animations();
            window.request_redraw();
            window.draw_if_needed(|renderer| {
                renderer.render(&mut frame, WIDTH as usize);
            });
        }
    }

    // The element search skips rows clipped out of the ScrollView, so a
    // row that is found is one the user can actually see.
    fn element(window: &MainWindow, id: &str) -> ElementHandle {
        ElementHandle::find_by_element_id(window, id)
            .next()
            .unwrap_or_else(|| panic!("no element {id}"))
    }

    fn text_element(window: &MainWindow, text: &str) -> ElementHandle {
        ElementHandle::find_by_accessible_label(window, text)
            .next()
            .unwrap_or_else(|| panic!("no element labelled {text:?}"))
    }

    // SOU-256 AC1/AC2: the live transcript's ScrollView must take the
    // card's whole height (not ~3 lines, not 0) and a streamed segment must
    // land inside it. A non-stretch `alignment` on the card's layout gave
    // it a preferred height of 0, hiding every segment while recording.
    #[test]
    fn live_meeting_transcript_is_laid_out_and_shows_segments() {
        let window = meeting_window();
        let live_state: LiveTranscriptState = Arc::new(Mutex::new(LiveTranscript::new()));
        apply_live_segment(
            &window,
            &live_state,
            &final_seg("Bonjour tout le monde", 0.0, Speaker::Me),
        );
        settle();

        let scroll = element(&window, "RecordingView::live-scroll");
        assert!(
            scroll.size().height > 300.0,
            "live transcript collapsed to {}px",
            scroll.size().height
        );

        // Words render as separate items (click-to-alias), so look one up.
        let text = text_element(&window, "monde");
        assert!(text.size().height > 0.0);
        let (top, bottom) = (
            scroll.absolute_position().y,
            scroll.absolute_position().y + scroll.size().height,
        );
        let y = text.absolute_position().y;
        assert!(
            y >= top && y < bottom,
            "segment at y={y} outside the transcript viewport {top}..{bottom}"
        );
    }

    // SOU-256 AC3: once the content overflows, the newest paragraph stays
    // in view without Rust driving the scroll position.
    #[test]
    fn live_meeting_transcript_follows_the_newest_paragraph() {
        let window = meeting_window();
        let live_state: LiveTranscriptState = Arc::new(Mutex::new(LiveTranscript::new()));
        let long = "Une phrase assez longue pour occuper plusieurs lignes dans la zone de transcription en direct pendant la réunion.";
        for i in 0..24 {
            let speaker = if i % 2 == 0 {
                Speaker::Me
            } else {
                Speaker::Them
            };
            apply_live_segment(
                &window,
                &live_state,
                &final_seg(&format!("{long} n{i}"), f64::from(i) * 10.0, speaker),
            );
            settle();
        }

        let scroll = element(&window, "RecordingView::live-scroll");
        let newest = text_element(&window, "n23");
        let bottom = scroll.absolute_position().y + scroll.size().height;
        let newest_bottom = newest.absolute_position().y + newest.size().height;
        assert!(
            newest_bottom <= bottom + 1.0
                && newest.absolute_position().y >= scroll.absolute_position().y,
            "newest paragraph {}..{newest_bottom} not visible in {}..{bottom}",
            newest.absolute_position().y,
            scroll.absolute_position().y
        );
    }

    // SOU-256 AC4: dictation shares the view; its single text surface must
    // still fill the card and show the streamed text.
    #[test]
    fn dictation_text_fills_the_card() {
        let window = meeting_window();
        window.set_recording_mode(RecordingMode::Dictation);
        let live_state: LiveTranscriptState = Arc::new(Mutex::new(LiveTranscript::new()));
        let mut seg = final_seg("Bonjour, ceci est une dictée.", 0.0, Speaker::Me);
        seg.speaker = None;
        apply_live_segment(&window, &live_state, &seg);
        settle();

        let scroll = element(&window, "RecordingView::dictation-scroll");
        assert!(
            scroll.size().height > 300.0,
            "dictation surface collapsed to {}px",
            scroll.size().height
        );
        // Drawn word by word now, so the pending word can be dimmed.
        let text = text_element(&window, "Bonjour");
        assert!(text.size().height > 0.0);
        text_element(&window, "dictée");
    }

    // Damien, 2026-09-24: the word the engine still holds is on screen,
    // after the confirmed text and marked provisional (drawn dimmed), and a
    // final for it makes it plain text.
    #[test]
    fn a_dictation_shows_its_pending_word_dimmed_until_confirmed() {
        let window = meeting_window();
        window.set_recording_mode(RecordingMode::Dictation);
        let live_state: LiveTranscriptState = Arc::new(Mutex::new(LiveTranscript::new()));
        let mut confirmed = final_seg("Purple elephants", 0.0, Speaker::Me);
        confirmed.speaker = None;
        apply_live_segment(&window, &live_state, &confirmed);
        let mut pending = final_seg("dance", 1.0, Speaker::Me);
        pending.speaker = None;
        pending.is_final = false;
        apply_live_segment(&window, &live_state, &pending);

        let words: Vec<(String, bool)> = window
            .get_live_dictation_words()
            .iter()
            .map(|w| (w.text.to_string(), w.provisional))
            .collect();
        assert_eq!(words.last(), Some(&("dance".to_string(), true)));
        assert!(words.iter().any(|(t, p)| t == "Purple" && !p));
        settle();
        text_element(&window, "dance");

        pending.is_final = true;
        apply_live_segment(&window, &live_state, &pending);
        assert!(
            window
                .get_live_dictation_words()
                .iter()
                .all(|w| !w.provisional && !w.clickable)
        );
    }

    fn visible_rows(window: &MainWindow, prefix: &'static str) -> Vec<(String, f32)> {
        use i_slint_backend_testing::ElementRoot;
        window
            .root_element()
            .query_descendants()
            .match_predicate(move |e| e.accessible_label().is_some_and(|l| l.starts_with(prefix)))
            .find_all()
            .iter()
            .map(|e| {
                (
                    e.accessible_label().unwrap_or_default().to_string(),
                    e.absolute_position().y,
                )
            })
            .collect()
    }

    // SOU-256 AC3: a reader who scrolled up is left where they are while
    // new paragraphs keep arriving - no jump back to the bottom.
    #[test]
    fn live_meeting_transcript_does_not_jump_when_scrolled_up() {
        let window = meeting_window();
        let live_state: LiveTranscriptState = Arc::new(Mutex::new(LiveTranscript::new()));
        let push = |i: i32| {
            let speaker = if i % 2 == 0 {
                Speaker::Me
            } else {
                Speaker::Them
            };
            apply_live_segment(
                &window,
                &live_state,
                &final_seg(
                    &format!(
                        "Marque{i} assez long pour prendre de la place à l'écran pendant la réunion."
                    ),
                    f64::from(i) * 10.0,
                    speaker,
                ),
            );
            settle();
        };
        for i in 0..20 {
            push(i);
        }
        assert!(
            visible_rows(&window, "Marque")
                .iter()
                .any(|(label, _)| label == "Marque19"),
            "not following the bottom before the scroll up"
        );

        let scroll = element(&window, "RecordingView::live-scroll");
        let centre = slint::LogicalPosition::new(
            scroll.absolute_position().x + scroll.size().width / 2.0,
            scroll.absolute_position().y + scroll.size().height / 2.0,
        );
        window
            .window()
            .dispatch_event(slint::platform::WindowEvent::PointerMoved { position: centre });
        window
            .window()
            .dispatch_event(slint::platform::WindowEvent::PointerScrolled {
                position: centre,
                delta_x: 0.0,
                delta_y: 400.0,
            });
        settle();
        let before = visible_rows(&window, "Marque");
        assert!(
            !before.iter().any(|(label, _)| label == "Marque19"),
            "the scroll up did not move away from the bottom: {before:?}"
        );

        for i in 20..24 {
            push(i);
        }
        let after = visible_rows(&window, "Marque");
        assert_eq!(
            before, after,
            "the view moved while the reader was scrolled up"
        );
    }

    fn click(window: &MainWindow, element: &ElementHandle) {
        let position = slint::LogicalPosition::new(
            element.absolute_position().x + element.size().width / 2.0,
            element.absolute_position().y + element.size().height / 2.0,
        );
        let button = slint::platform::PointerEventButton::Left;
        window
            .window()
            .dispatch_event(slint::platform::WindowEvent::PointerMoved { position });
        window
            .window()
            .dispatch_event(slint::platform::WindowEvent::PointerPressed { position, button });
        window
            .window()
            .dispatch_event(slint::platform::WindowEvent::PointerReleased { position, button });
        settle();
    }

    /// The popover's text inputs whose current value is `value`.
    fn inputs_with_value(window: &MainWindow, value: &'static str) -> Vec<ElementHandle> {
        use i_slint_backend_testing::ElementRoot;
        window
            .root_element()
            .query_descendants()
            .match_predicate(move |e| e.accessible_value().is_some_and(|v| v == value))
            .find_all()
    }

    // SOU-256 AC5: clicking a finalized live word opens the same dictionary
    // popover as the post-meeting transcript, prefilled with the heard word;
    // it survives more text streaming in underneath, and Save reaches the
    // live callback with (term, heard word).
    #[test]
    fn clicking_a_final_live_word_saves_a_dictionary_alias() {
        let window = meeting_window();
        let saved: Rc<std::cell::RefCell<Vec<(String, String)>>> = Rc::default();
        let sink = saved.clone();
        window.on_live_transcript_alias_save_requested(move |term, heard| {
            sink.borrow_mut().push((term.into(), heard.into()));
        });
        let live_state: LiveTranscriptState = Arc::new(Mutex::new(LiveTranscript::new()));
        apply_live_segment(
            &window,
            &live_state,
            &final_seg("On migre vers Cubernetis demain", 0.0, Speaker::Me),
        );
        settle();

        click(&window, &text_element(&window, "Cubernetis"));
        assert_eq!(
            inputs_with_value(&window, "Cubernetis").len(),
            1,
            "the popover did not open prefilled with the clicked word"
        );

        // Text keeps arriving (a partial, then finals) under the open popover.
        let mut partial = final_seg("et ensuite", 2.0, Speaker::Them);
        partial.is_final = false;
        apply_live_segment(&window, &live_state, &partial);
        apply_live_segment(
            &window,
            &live_state,
            &final_seg("et ensuite on verra", 2.0, Speaker::Them),
        );
        apply_live_segment(&window, &live_state, &final_seg("oui", 2.5, Speaker::Me));
        settle();

        let term = inputs_with_value(&window, "");
        assert!(
            !term.is_empty(),
            "the popover closed while text streamed in"
        );
        term[0].set_accessible_value("Kubernetes");
        settle();
        use i_slint_backend_testing::ElementRoot;
        let save = window
            .root_element()
            .query_descendants()
            .match_predicate(|e| {
                e.accessible_role() == Some(i_slint_backend_testing::AccessibleRole::Button)
                    && e.accessible_label()
                        .is_some_and(|l| l.to_lowercase().contains("alias"))
            })
            .find_first()
            .expect("no save button in the popover");
        save.invoke_accessible_default_action();
        settle();

        assert_eq!(
            saved.borrow().as_slice(),
            &[("Kubernetes".to_string(), "Cubernetis".to_string())]
        );
    }

    // SOU-257 AC1: a pointer-opened popup must own keyboard focus so its
    // existing Escape handler actually receives the key.
    #[test]
    fn escape_closes_a_dictionary_popup_opened_with_the_mouse() {
        let window = meeting_window();
        let live_state: LiveTranscriptState = Arc::new(Mutex::new(LiveTranscript::new()));
        apply_live_segment(
            &window,
            &live_state,
            &final_seg("On migre vers Cubernetis demain", 0.0, Speaker::Me),
        );
        settle();

        click(&window, &text_element(&window, "Cubernetis"));
        assert_eq!(inputs_with_value(&window, "Cubernetis").len(), 1);

        window
            .window()
            .dispatch_event(slint::platform::WindowEvent::KeyPressed {
                text: slint::platform::Key::Escape.into(),
            });
        settle();

        assert!(
            inputs_with_value(&window, "Cubernetis").is_empty(),
            "Escape did not close the pointer-opened dictionary popup"
        );
    }

    // SOU-256 AC5: the provisional tail is still being rewritten by the
    // engine - its words do not open the popover.
    #[test]
    fn provisional_live_words_are_not_clickable() {
        let window = meeting_window();
        let live_state: LiveTranscriptState = Arc::new(Mutex::new(LiveTranscript::new()));
        let mut partial = final_seg("Provisoire", 0.0, Speaker::Me);
        partial.is_final = false;
        apply_live_segment(&window, &live_state, &partial);
        settle();
        settle();

        click(&window, &text_element(&window, "Provisoire"));
        assert!(
            inputs_with_value(&window, "Provisoire").is_empty(),
            "a provisional word opened the dictionary popover"
        );
    }

    fn block(text: &str, speaker: SpeakerRole) -> TranscriptBlock {
        TranscriptBlock {
            has_speaker: true,
            speaker,
            text: text.into(),
            ..Default::default()
        }
    }

    #[test]
    fn sync_blocks_updates_in_place_appends_and_truncates() {
        let model = VecModel::from(vec![block("Bonjour", SpeakerRole::Me)]);
        sync_blocks(
            &model,
            vec![
                block("Bonjour tout", SpeakerRole::Me),
                block("Salut", SpeakerRole::Them),
            ],
        );
        assert_eq!(model.row_count(), 2);
        assert_eq!(model.row_data(0).unwrap().text.as_str(), "Bonjour tout");
        assert_eq!(model.row_data(1).unwrap().text.as_str(), "Salut");

        sync_blocks(&model, vec![block("Bonjour tout", SpeakerRole::Me)]);
        assert_eq!(model.row_count(), 1);

        sync_blocks(&model, Vec::new());
        assert_eq!(model.row_count(), 0);
    }
}
