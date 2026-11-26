//! The completed meeting's transcript list (SOU-187 AC15), separate from
//! `main.rs` so the design fixture drives the same code.
//!
//! The list receives the whole transcript: the kit `ListView` it sits in
//! creates rows only while they are on screen, so nothing is windowed here.

use std::cell::RefCell;
use std::rc::Rc;

use crate::{MainWindow, TranscriptBlock, transcript};

/// The meeting's full block list, kept so a search can be narrowed and
/// cleared without rebuilding the paragraphs.
pub(crate) struct TranscriptState {
    pub(crate) source: Vec<TranscriptBlock>,
}

fn show_blocks(window: &MainWindow, blocks: Vec<TranscriptBlock>) {
    window.set_meeting_detail_transcript_blocks(Rc::new(slint::VecModel::from(blocks)).into());
}

/// Installs a meeting's transcript and shows it from the top.
pub(crate) fn mount_transcript(
    window: &MainWindow,
    blocks: Vec<TranscriptBlock>,
    transcript_state: &Rc<RefCell<Option<TranscriptState>>>,
) {
    window.set_meeting_detail_transcript_preview(transcript::preview_text(&blocks).into());
    window.invoke_reset_meeting_detail_transcript_scroll();
    show_blocks(window, blocks.clone());
    *transcript_state.borrow_mut() = Some(TranscriptState { source: blocks });
}

/// Narrows the list to the paragraphs containing `query` (all of them when it
/// is blank) and shows the result from the top.
pub(crate) fn filter_transcript(
    window: &MainWindow,
    transcript_state: &Rc<RefCell<Option<TranscriptState>>>,
    query: &str,
) {
    let guard = transcript_state.borrow();
    let Some(state) = guard.as_ref() else {
        return;
    };
    window.invoke_reset_meeting_detail_transcript_scroll_position();
    show_blocks(window, transcript::filter_blocks(&state.source, query));
}

/// Drops the transcript when its meeting closes, so neither the list nor a
/// late search keeps a meeting that is no longer shown.
pub(crate) fn unmount_transcript(
    window: &MainWindow,
    transcript_state: &Rc<RefCell<Option<TranscriptState>>>,
) {
    *transcript_state.borrow_mut() = None;
    show_blocks(window, Vec::new());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MeetingDetailView, TranscriptLayout};
    use i_slint_backend_testing::ElementHandle;
    use slint::ComponentHandle;
    use slint::platform::{Platform, WindowAdapter, software_renderer::MinimalSoftwareWindow};
    use souffle_lib::engine::{Speaker, TranscriptionSegment};
    use souffle_lib::transcript::MeetingRecordingSession;

    const WIDTH: u32 = 1040;
    const HEIGHT: u32 = 820;

    thread_local! {
        static TEST_WINDOW: RefCell<Option<Rc<MinimalSoftwareWindow>>> =
            const { RefCell::new(None) };
    }

    struct TestPlatform;

    impl Platform for TestPlatform {
        fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
            let window = MinimalSoftwareWindow::new(Default::default());
            TEST_WINDOW.with(|slot| *slot.borrow_mut() = Some(window.clone()));
            Ok(window)
        }
    }

    fn settle() {
        // The list replaces its height estimate as rows are laid out, so a
        // far seek takes a few frames to come to rest.
        draw_frames(6);
    }

    fn draw_frames(count: usize) {
        let window = TEST_WINDOW
            .with(|slot| slot.borrow().clone())
            .expect("no test window");
        let size = window.size();
        let mut frame = vec![
            slint::platform::software_renderer::Rgb565Pixel::default();
            (size.width * size.height) as usize
        ];
        for _ in 0..count {
            slint::platform::update_timers_and_animations();
            window.request_redraw();
            window.draw_if_needed(|renderer| {
                renderer.render(&mut frame, size.width as usize);
            });
        }
    }

    const SENTENCES: [&str; 4] = [
        "D'accord, je note la décision.",
        "Du coup côté planning, on peut viser la bêta pour mi-décembre si l'équipe valide le périmètre cette semaine.",
        "Camille relit le guide jeudi matin, Julien prépare les notes de version, et on garde une demi-journée pour les retours de l'équipe support avant d'ouvrir la bêta aux premiers testeurs.",
        "Oui, tout à fait.",
    ];

    /// One paragraph per segment: the speakers alternate, in two sessions.
    fn meeting(paragraphs: usize) -> Vec<TranscriptBlock> {
        let segments: Vec<TranscriptionSegment> = (0..paragraphs)
            .map(|i| TranscriptionSegment {
                text: format!("Repère {i}. {}", SENTENCES[i % SENTENCES.len()]),
                start_time: i as f64 * 20.0,
                end_time: i as f64 * 20.0 + 10.0,
                is_final: true,
                language: None,
                confidence: None,
                speaker: Some(if i % 2 == 0 {
                    Speaker::Them
                } else {
                    Speaker::Me
                }),
            })
            .collect();
        let half = paragraphs as u64 / 2;
        let sessions: Vec<MeetingRecordingSession> = [(0, half), (half, paragraphs as u64)]
            .into_iter()
            .map(|(start, end)| MeetingRecordingSession {
                id: format!("session-{start}"),
                started_at: chrono::Utc::now(),
                ended_at: chrono::Utc::now(),
                duration_seconds: 600.0,
                start_segment_index: start,
                end_segment_index: end,
            })
            .collect();
        transcript::build_transcript_blocks(&segments, &sessions)
    }

    struct Fixture {
        window: MainWindow,
        state: Rc<RefCell<Option<TranscriptState>>>,
        blocks: usize,
    }

    fn transcript_view(paragraphs: usize) -> Fixture {
        let _ = slint::platform::set_platform(Box::new(TestPlatform));
        let window = crate::settings_kit::new_window().unwrap();
        crate::typography::initialize(&window).unwrap();
        window
            .window()
            .set_size(slint::PhysicalSize::new(WIDTH, HEIGHT));
        window.set_onboarding_open(false);
        window.set_active_meeting_id("meeting".into());
        window.set_meeting_detail_has_audio(true);
        let blocks = meeting(paragraphs);
        let count = blocks.len();
        let state = Rc::new(RefCell::new(None));
        mount_transcript(&window, blocks, &state);
        window.set_meeting_detail_transcript_segment_count(paragraphs as i32);
        window.set_meeting_detail_view(MeetingDetailView::Transcript);
        settle();
        Fixture {
            window,
            state,
            blocks: count,
        }
    }

    impl Fixture {
        /// The rows that exist, top to bottom.
        fn rows(&self) -> Vec<ElementHandle> {
            let mut rows: Vec<_> =
                ElementHandle::find_by_element_type_name(&self.window, "TranscriptRow").collect();
            rows.sort_by(|a, b| a.absolute_position().y.total_cmp(&b.absolute_position().y));
            rows
        }

        fn list(&self) -> ElementHandle {
            ElementHandle::find_by_element_id(&self.window, "TranscriptSection::list")
                .next()
                .expect("the transcript list")
        }

        fn scroll_to_share(&self, share: f32) {
            // The travel is an estimate until the rows near the target exist.
            for _ in 0..4 {
                let travel = self.window.get_meeting_detail_transcript_scroll_travel();
                self.window
                    .set_meeting_detail_transcript_scroll_top(-share * travel);
                settle();
            }
        }

        /// The seek button of the paragraph starting at `timestamp`, when
        /// that paragraph is on screen.
        fn timestamp(&self, timestamp: &str) -> Option<ElementHandle> {
            ElementHandle::find_by_element_type_name(&self.window, "TimestampButton").find(
                |button| {
                    button
                        .accessible_label()
                        .is_some_and(|label| label.ends_with(&format!(" {timestamp}")))
                },
            )
        }

        fn has_timestamp(&self, timestamp: &str) -> bool {
            self.timestamp(timestamp).is_some()
        }

        /// Rows stacked without a hole or an overlap, covering the viewport.
        fn assert_gapless(&self, context: &str) {
            let rows = self.rows();
            assert!(!rows.is_empty(), "{context}: no row is mounted");
            for pair in rows.windows(2) {
                let bottom = pair[0].absolute_position().y + pair[0].size().height;
                let top = pair[1].absolute_position().y;
                assert!(
                    (bottom - top).abs() < 0.6,
                    "{context}: a row ends at {bottom} and the next starts at {top}"
                );
            }
            let list = self.list();
            let (top, bottom) = (
                list.absolute_position().y,
                list.absolute_position().y + list.size().height,
            );
            let first = rows.first().unwrap().absolute_position().y;
            let last = rows.last().unwrap();
            let end = last.absolute_position().y + last.size().height;
            assert!(first <= top + 0.6, "{context}: blank above the first row");
            let at_end = self.has_timestamp(&crate::timeline::format_duration(
                (self.paragraphs() - 1) as f64 * 20.0,
            ));
            assert!(
                end >= bottom - 0.6 || at_end,
                "{context}: blank below the last mounted row ({end} < {bottom})"
            );
        }

        fn paragraphs(&self) -> usize {
            // Every block is a paragraph but the one session break.
            self.blocks - 1
        }
    }

    #[test]
    fn a_long_transcript_only_creates_the_rows_on_screen_at_any_position() {
        for paragraphs in [200, 1400] {
            let fixture = transcript_view(paragraphs);
            for share in [0.0, 0.5, 1.0, 0.25] {
                fixture.scroll_to_share(share);
                let mounted = fixture.rows().len();
                assert!(
                    (1..=20).contains(&mounted),
                    "{paragraphs} paragraphs at {share}: {mounted} rows exist"
                );
                fixture.assert_gapless(&format!("{paragraphs} paragraphs at {share}"));
            }
        }
    }

    /// Records which rows the list asked for: it reads a row only to create it.
    struct CountingModel {
        rows: slint::VecModel<TranscriptBlock>,
        read: RefCell<std::collections::BTreeSet<usize>>,
    }

    impl slint::Model for CountingModel {
        type Data = TranscriptBlock;

        fn row_count(&self) -> usize {
            self.rows.row_count()
        }

        fn row_data(&self, row: usize) -> Option<TranscriptBlock> {
            self.read.borrow_mut().insert(row);
            self.rows.row_data(row)
        }

        fn model_tracker(&self) -> &dyn slint::ModelTracker {
            self.rows.model_tracker()
        }
    }

    #[test]
    fn rows_far_from_the_viewport_are_never_created() {
        let fixture = transcript_view(1400);
        let model = Rc::new(CountingModel {
            rows: slint::VecModel::from(meeting(1400)),
            read: Default::default(),
        });
        fixture
            .window
            .set_meeting_detail_transcript_blocks(slint::ModelRc::from(model.clone()));
        settle();
        let at_top = model.read.borrow().len();
        assert!(
            (1..=12).contains(&at_top),
            "{at_top} rows created at the top"
        );
        fixture.scroll_to_share(0.5);
        fixture.scroll_to_share(1.0);
        let read = model.read.borrow();
        assert!(
            read.len() < 120,
            "{} of 1401 rows were created by two far seeks",
            read.len()
        );
        assert!(read.contains(&1400), "the last row was created at the end");
        assert!(!read.contains(&300) && !read.contains(&1000));
    }

    /// The list re-anchors its offset whenever a new row changes its height
    /// estimate. What is on screen must still move by exactly the distance
    /// scrolled, in both directions, across many row boundaries.
    #[test]
    fn scrolling_moves_the_text_by_the_distance_scrolled_without_a_jump() {
        let fixture = transcript_view(600);
        fixture.scroll_to_share(0.4);
        let positions = |fixture: &Fixture| -> std::collections::BTreeMap<String, f32> {
            ElementHandle::find_by_element_type_name(&fixture.window, "TimestampButton")
                .filter_map(|button| {
                    Some((
                        button.accessible_label()?.to_string(),
                        button.absolute_position().y,
                    ))
                })
                .collect()
        };
        let mut boundaries = std::collections::BTreeSet::new();
        for step in [12.0f32, -12.0, 67.0, -67.0] {
            for _ in 0..40 {
                let before = positions(&fixture);
                let top = fixture.window.get_meeting_detail_transcript_scroll_top();
                fixture
                    .window
                    .set_meeting_detail_transcript_scroll_top(top - step);
                draw_frames(2);
                let after = positions(&fixture);
                let mut tracked = 0;
                for (label, y) in &before {
                    if let Some(moved) = after.get(label) {
                        tracked += 1;
                        assert!(
                            (y - moved - step).abs() < 0.6,
                            "{label} moved {} for a scroll of {step}",
                            y - moved
                        );
                    }
                }
                assert!(tracked > 0, "a paragraph stays on screen across a step");
                boundaries.extend(after.into_keys());
                fixture.assert_gapless(&format!("after a scroll of {step}"));
            }
        }
        assert!(
            boundaries.len() > 20,
            "the scroll crossed many paragraphs ({})",
            boundaries.len()
        );
    }

    #[test]
    fn the_last_paragraph_is_reachable_and_ends_with_the_bottom_inset() {
        let fixture = transcript_view(600);
        fixture.scroll_to_share(1.0);
        let last = crate::timeline::format_duration(599.0 * 20.0);
        assert!(fixture.has_timestamp(&last), "the last paragraph is shown");
        let rows = fixture.rows();
        let row = rows.last().unwrap();
        let list = fixture.list();
        let row_bottom = row.absolute_position().y + row.size().height;
        let list_bottom = list.absolute_position().y + list.size().height;
        assert!(
            (row_bottom - list_bottom).abs() < 0.6,
            "the last row ends at {row_bottom}, the list at {list_bottom}"
        );
        fixture.scroll_to_share(0.0);
        assert!(fixture.has_timestamp("0:00"), "back at the first paragraph");
        assert_eq!(
            fixture.window.get_meeting_detail_transcript_scroll_top(),
            0.0
        );
    }

    #[test]
    fn a_resize_reflows_the_mounted_rows_without_a_gap() {
        let fixture = transcript_view(400);
        fixture.scroll_to_share(0.5);
        let wide = fixture.rows()[0].size();
        for (width, height) in [(700, 820), (1300, 600), (WIDTH, HEIGHT)] {
            fixture
                .window
                .window()
                .set_size(slint::PhysicalSize::new(width, height));
            settle();
            fixture.assert_gapless(&format!("at {width}x{height}"));
            let row = fixture.rows()[0].size();
            assert!(
                (row.width - width as f32).abs() < 0.6,
                "rows span the list at {width}: {}",
                row.width
            );
        }
        assert_eq!(fixture.rows()[0].size().width, wide.width);
    }

    #[test]
    fn both_layouts_show_the_same_paragraphs_and_text_rows_are_shorter() {
        let fixture = transcript_view(300);
        let chat = fixture.rows()[0].size().height;
        fixture
            .window
            .set_meeting_detail_transcript_layout(TranscriptLayout::Text);
        settle();
        fixture.assert_gapless("text layout");
        assert!(fixture.has_timestamp("0:00"));
        let text = fixture.rows()[0].size().height;
        assert!(
            text < chat,
            "a text row ({text}) drops the bubble padding ({chat})"
        );
        fixture.scroll_to_share(1.0);
        assert!(fixture.has_timestamp(&crate::timeline::format_duration(299.0 * 20.0)));
    }

    #[test]
    fn a_search_narrows_the_list_from_the_top_and_clearing_it_restores_everything() {
        let fixture = transcript_view(300);
        fixture.scroll_to_share(0.6);
        assert!(fixture.window.get_meeting_detail_transcript_scroll_top() < 0.0);

        fixture
            .window
            .set_meeting_detail_transcript_search("repère 297.".into());
        filter_transcript(&fixture.window, &fixture.state, "repère 297.");
        settle();
        assert_eq!(
            fixture.window.get_meeting_detail_transcript_scroll_top(),
            0.0
        );
        assert_eq!(fixture.rows().len(), 1);
        assert!(fixture.has_timestamp(&crate::timeline::format_duration(297.0 * 20.0)));

        fixture
            .window
            .set_meeting_detail_transcript_search("introuvable".into());
        filter_transcript(&fixture.window, &fixture.state, "introuvable");
        settle();
        assert!(fixture.rows().is_empty());
        assert_eq!(
            ElementHandle::find_by_element_id(&fixture.window, "TranscriptSection::empty-note")
                .count(),
            1
        );

        fixture
            .window
            .set_meeting_detail_transcript_search("".into());
        filter_transcript(&fixture.window, &fixture.state, "");
        settle();
        assert!(fixture.has_timestamp("0:00"));
        fixture.scroll_to_share(1.0);
        assert!(fixture.has_timestamp(&crate::timeline::format_duration(299.0 * 20.0)));
    }

    #[test]
    fn a_timestamp_seeks_its_paragraph_after_scrolling() {
        let fixture = transcript_view(300);
        let seeks = Rc::new(RefCell::new(Vec::new()));
        let sink = seeks.clone();
        fixture
            .window
            .on_meeting_detail_transcript_paragraph_clicked(move |session, time| {
                sink.borrow_mut().push((session, time));
            });
        fixture.scroll_to_share(1.0);
        let button = fixture
            .timestamp(&crate::timeline::format_duration(299.0 * 20.0))
            .unwrap();
        button.invoke_accessible_default_action();
        settle();
        assert_eq!(*seeks.borrow(), vec![(1, 299.0 * 20.0)]);
    }

    #[test]
    fn a_click_on_a_word_opens_the_alias_popover_after_scrolling() {
        let fixture = transcript_view(300);
        let popover = || {
            ElementHandle::find_by_element_id(&fixture.window, "DictionaryAliasPopover::save-btn")
                .count()
        };
        fixture.scroll_to_share(0.5);
        assert_eq!(popover(), 0);
        let word =
            ElementHandle::find_by_element_id(&fixture.window, "TranscriptWordFlow::word-area")
                .find(|area| area.size().width > 30.0)
                .expect("a clickable word on screen");
        let position = slint::LogicalPosition::new(
            word.absolute_position().x + word.size().width / 2.0,
            word.absolute_position().y + word.size().height / 2.0,
        );
        let button = slint::platform::PointerEventButton::Left;
        for event in [
            slint::platform::WindowEvent::PointerMoved { position },
            slint::platform::WindowEvent::PointerPressed { position, button },
            slint::platform::WindowEvent::PointerReleased { position, button },
        ] {
            fixture.window.window().dispatch_event(event);
        }
        settle();
        assert_eq!(popover(), 1, "the alias popover is open");
    }

    #[test]
    fn the_playing_paragraph_is_ringed_only_while_it_holds_the_audio_position() {
        let fixture = transcript_view(40);
        let ring = || {
            ElementHandle::find_by_element_id(&fixture.window, "TranscriptRow::playing-ring")
                .count()
        };
        assert_eq!(ring(), 0);
        fixture.window.set_meeting_detail_audio_session_index(0);
        fixture
            .window
            .set_meeting_detail_audio_position_seconds(25.0);
        settle();
        assert_eq!(ring(), 1, "the second paragraph (20 s to 40 s) is playing");
        fixture.window.set_meeting_detail_audio_session_index(1);
        settle();
        assert_eq!(ring(), 0, "another session's position rings nothing here");
    }

    #[test]
    fn closing_the_meeting_empties_the_list() {
        let fixture = transcript_view(50);
        assert!(!fixture.rows().is_empty());
        unmount_transcript(&fixture.window, &fixture.state);
        settle();
        assert!(fixture.rows().is_empty());
        assert!(fixture.state.borrow().is_none());
    }
}
