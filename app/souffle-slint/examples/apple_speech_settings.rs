//! Native Settings QA using the real catalogue projection and Slint components.
//! No database writes, model installation, audio, Siri or TCC changes.
//! SPEECH_SETTINGS_UNAVAILABLE=locale_unsupported (or another serialized
//! TranscriptionUnavailableReason) injects an explicitly synthetic failure.
//! SPEECH_SETTINGS_LOCALE=fr/en controls only the UI language.

slint::include_modules!();

#[allow(dead_code)]
#[path = "../src/audio_ui.rs"]
mod audio_ui;
#[allow(dead_code)]
#[path = "../src/microphone_list.rs"]
mod microphone_list;
#[allow(dead_code)]
#[path = "../src/model_ui.rs"]
mod model_ui;
#[path = "../src/typography.rs"]
mod typography;

use slint::ComponentHandle;
use souffle_lib::engine::{APPLE_SPEECH_ENGINE_ID, TranscriptionUnavailableReason as DomainReason};

fn main() {
    slint::BackendSelector::new()
        .renderer_name("skia".into())
        .select()
        .unwrap();
    let locale = std::env::var("SPEECH_SETTINGS_LOCALE").unwrap_or_else(|_| "fr".into());
    slint::select_bundled_translation(&locale).unwrap();
    let window = MainWindow::new().unwrap();
    typography::initialize(&window).expect("Inter renderer font resolution failed");
    window
        .window()
        .set_size(slint::LogicalSize::new(860., 780.));
    window.global::<Theme>().set_dark(true);
    window.set_settings_locale(match locale.as_str() {
        "fr" => AppLocale::Fr,
        "en" => AppLocale::En,
        other => panic!("Unsupported UI locale: {other}"),
    });
    window.set_onboarding_open(false);
    window.set_settings_open(true);
    window.set_settings_tab(SettingsTab::Transcription);
    let settings = souffle_lib::settings::AppSettings::default();
    let mut catalog =
        souffle_lib::commands::transcription_catalog_from_settings(&settings).unwrap();
    let speech = catalog
        .engines
        .iter_mut()
        .find(|e| e.id == APPLE_SPEECH_ENGINE_ID)
        .unwrap();
    let model = &mut speech.models[0];
    if let Ok(reason) = std::env::var("SPEECH_SETTINGS_UNAVAILABLE") {
        let reason: DomainReason = serde_json::from_value(reason.into()).unwrap();
        eprintln!("Synthetic Apple Speech unavailability: {reason:?}");
        model.available_in_app = false;
        model.unavailable_reason = Some(reason);
        model.backends.clear();
    } else {
        assert!(
            model.available_in_app,
            "Use a supported Mac or inject an unavailable fixture"
        );
        catalog.selected_engine_id = speech.id.clone();
        catalog.selected_model_id = model.id.clone();
        catalog.selected_backend_id = model.backends[0].id.clone();
    }
    model_ui::populate_options(
        &window,
        &catalog,
        15,
        &souffle_lib::settings::SettingsOptions::current().model_unload_timeout_minutes,
    );
    window.set_header_model_label(model_ui::selected_model_short_label(&catalog).into());
    // The harness renders a fixture, so never report a live engine as Ready.
    window.set_model_runtime_phase(TranscriptionPhase::LoadRequired);
    window.on_settings_open_system_settings_requested(|pane| {
        eprintln!("Settings destination: {pane:?}")
    });
    window.run().unwrap();
}
