//! Glue between the live transcript state machine (`live_transcript.rs`) and
//! `MainWindow`'s live-meeting properties: segment routing and model push.
//!
//! Kept separate from `main.rs` so `examples/live_transcript_mock.rs` drives
//! the exact same code path as the app (SOU-256) - a fake segment stream goes
//! through `apply_live_segment`.

use std::sync::{Arc, Mutex};

use souffle_lib::engine::TranscriptionSegment;

use crate::live_transcript::{LiveTranscript, provisional_words};
use crate::{MainWindow, TranscriptBlock, TranscriptWord};
use slint::{Model, ModelRc, VecModel};

pub type LiveTranscriptState = Arc<Mutex<LiveTranscript>>;

/// Routes one streamed segment into the view. Speaker-tagged segments
/// (meetings) go through the paragraph model; untagged ones (dictation) feed
/// the flat `live-text`/`live-tentative` buffers. Must run on the Slint
/// main thread.
pub fn apply_live_segment(
    window: &MainWindow,
    live_state: &LiveTranscriptState,
    segment: &TranscriptionSegment,
) {
    if segment.speaker.is_some() {
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
