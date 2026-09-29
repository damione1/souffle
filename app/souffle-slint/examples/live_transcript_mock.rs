//! Standalone live-transcript harness (SOU-256).
//!
//! Opens the real `MainWindow` (same `.slint` sources, same generated code
//! as the app) in a live recording state and streams a scripted fake
//! Me/Them conversation through the app's own code path:
//! `live_view::apply_live_segment` -> `LiveTranscript` grouping, partials,
//! finals -> `live-transcript-blocks`, plus the same tentative-expiry poll
//! timer. Autoscroll lives in `recording_view.slint` itself, so it is
//! exercised as-is.
//! No audio, model, database or permissions involved.
//!
//! ```sh
//! cargo run --manifest-path app/Cargo.toml -p souffle-slint --example live_transcript_mock
//! MOCK_MODE=dictation cargo run ... --example live_transcript_mock   # AC4
//! ```
//!
//! `MOCK_SPEED` (default 1.0) scales the script's pace, `MOCK_WIDTH` /
//! `MOCK_HEIGHT` the window's logical size.

slint::include_modules!();

// The app is a binary crate, so the modules the live path needs are pulled
// in by path. Only a fraction of timeline/transcript is used here.
#[allow(dead_code)]
#[path = "../src/live_transcript.rs"]
mod live_transcript;
#[allow(dead_code)]
#[path = "../src/live_view.rs"]
mod live_view;
#[allow(dead_code)]
#[path = "../src/timeline.rs"]
mod timeline;
#[allow(dead_code)]
#[path = "../src/transcript.rs"]
mod transcript;

use std::cell::Cell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use souffle_lib::engine::{Speaker, TranscriptionSegment};

use live_transcript::LiveTranscript;
use live_view::LiveTranscriptState;

/// (lane, sentence). Each sentence is streamed word by word as growing
/// partials, then committed as one final.
const SCRIPT: &[(Option<Speaker>, &str)] = &[
    (
        Some(Speaker::Me),
        "Bonjour tout le monde, merci d'être là pour ce point hebdomadaire.",
    ),
    (
        Some(Speaker::Me),
        "On commence par le suivi du lancement de la version bêta.",
    ),
    (
        Some(Speaker::Them),
        "Oui, alors de notre côté les retours sont plutôt bons, mais on a remarqué quelques soucis sur la transcription en direct pendant les réunions longues.",
    ),
    (
        Some(Speaker::Them),
        "En particulier la zone de texte reste vide tant que la réunion n'est pas terminée, ce qui déroute pas mal les utilisateurs qui s'attendent à voir le texte défiler au fur et à mesure qu'ils parlent.",
    ),
    (Some(Speaker::Me), "D'accord, c'est noté."),
    (
        Some(Speaker::Me),
        "Est-ce que vous pouvez m'envoyer les identifiants des réunions concernées pour qu'on puisse regarder les journaux ?",
    ),
    (
        Some(Speaker::Them),
        "Bien sûr, je te les fais passer après l'appel.",
    ),
    (
        Some(Speaker::Them),
        "Autre point : l'export Markdown fonctionne bien, mais le PDF coupe parfois les longs paragraphes au milieu d'une phrase, ce qui rend la lecture un peu pénible quand on imprime le compte rendu pour le comité.",
    ),
    (
        Some(Speaker::Me),
        "Ok. On a une correction en cours pour la pagination, elle devrait arriver dans la prochaine nightly.",
    ),
    (
        Some(Speaker::Me),
        "Pour la suite, je propose qu'on fixe un nouveau point dans deux semaines pour valider tout ça ensemble avec l'équipe produit.",
    ),
    (Some(Speaker::Them), "Parfait, ça me va."),
];

fn main() {
    let speed: f64 = std::env::var("MOCK_SPEED")
        .ok()
        .and_then(|s| s.parse().ok())
        .filter(|s: &f64| *s > 0.0)
        .unwrap_or(1.0);
    let dictation = std::env::var("MOCK_MODE").as_deref() == Ok("dictation");
    let width: f32 = env_f32("MOCK_WIDTH", 1040.0);
    let height: f32 = env_f32("MOCK_HEIGHT", 820.0);

    slint::BackendSelector::new()
        .renderer_name("skia".into())
        .select()
        .expect("failed to select the skia renderer");

    let window = MainWindow::new().expect("failed to create the mock window");
    window
        .window()
        .set_size(slint::LogicalSize::new(width, height));
    window.set_onboarding_open(false);
    window.set_recording_mode(if dictation {
        RecordingMode::Dictation
    } else {
        RecordingMode::Meeting
    });
    window.set_live_system_audio(LiveSystemAudio::Active);
    // The app saves to the dictionary here; the mock just reports it.
    window.on_live_transcript_alias_save_requested(|term, pronunciation| {
        eprintln!("alias saved: {term:?} <- {pronunciation:?}");
    });

    let live_state: LiveTranscriptState = Arc::new(Mutex::new(LiveTranscript::new()));

    // Streaming cursor: (sentence index, words already emitted, clock).
    let sentence = Rc::new(Cell::new(0usize));
    let word = Rc::new(Cell::new(0usize));
    let clock = Rc::new(Cell::new(0.0f64));
    let seg_start = Rc::new(Cell::new(0.0f64));
    let feeder = slint::Timer::default();
    let weak = window.as_weak();
    let step = Duration::from_secs_f64(0.18 / speed);
    feeder.start(slint::TimerMode::Repeated, step, move || {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let (lane, text) = SCRIPT[sentence.get() % SCRIPT.len()];
        let lane = if dictation { None } else { lane };
        let words: Vec<&str> = text.split_whitespace().collect();
        let emitted = word.get() + 1;
        clock.set(clock.get() + 0.3);
        if word.get() == 0 {
            seg_start.set(clock.get());
        }
        let is_final = emitted >= words.len();
        let seg = TranscriptionSegment {
            text: words[..emitted.min(words.len())].join(" "),
            start_time: seg_start.get(),
            end_time: clock.get(),
            is_final,
            language: Some("fr".into()),
            confidence: None,
            speaker: lane,
        };
        live_view::apply_live_segment(&window, &live_state, &seg);
        if is_final {
            word.set(0);
            sentence.set(sentence.get() + 1);
            // A short pause between sentences: same-speaker sentences
            // merge into one paragraph, a speaker switch opens a new one.
            clock.set(clock.get() + 0.6);
        } else {
            word.set(emitted);
        }
    });

    window.run().expect("mock event loop failed");
}

fn env_f32(key: &str, default: f32) -> f32 {
    std::env::var(key)
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(default)
}
