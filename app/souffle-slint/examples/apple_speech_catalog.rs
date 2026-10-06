//! Native SOU-121 catalogue QA without opening a user profile or audio device.
//! SOU121_LOCALE=fr/en selects the actual bundled translations. Availability
//! comes from the real bridge; this example never installs or deletes assets.
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

use slint::ComponentHandle;
use souffle_lib::engine::{self, TranscriptionCatalog, TranscriptionRuntimePhase};

fn main() {
    let status = engine::apple_speech::status();
    assert!(status.is_available(), "{status:?}");
    let catalog = TranscriptionCatalog {
        engines: engine::transcription_engine_catalog(),
        selected_engine_id: engine::APPLE_SPEECH_ENGINE_ID.into(),
        selected_model_id: engine::APPLE_SPEECH_MODEL_ID.into(),
        selected_backend_id: engine::APPLE_SPEECH_BACKEND_ID.into(),
    };
    slint::BackendSelector::new()
        .renderer_name("skia".into())
        .select()
        .unwrap();
    let window = MainWindow::new().unwrap();
    slint::select_bundled_translation(
        &std::env::var("SOU121_LOCALE").unwrap_or_else(|_| "en".into()),
    )
    .unwrap();
    window
        .window()
        .set_size(slint::LogicalSize::new(1040.0, 820.0));
    window.set_onboarding_open(false);
    window.set_settings_open(true);
    window.set_settings_tab(SettingsTab::Transcription);
    model_ui::populate_options(&window, &catalog, 0, &[0]);
    let phase = if status
        .locale()
        .is_some_and(|locale| status.is_installed_for(locale))
    {
        TranscriptionRuntimePhase::LoadRequired
    } else {
        TranscriptionRuntimePhase::DownloadRequired
    };
    model_ui::populate_runtime(&window, phase);
    eprintln!(
        "{status:?}; phase={phase:?}; files_deletable={}",
        window.get_settings_model_files_deletable()
    );
    window.run().unwrap();
}
