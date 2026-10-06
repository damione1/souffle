//! SOU-276: real UI / native HUD typography fixture. No bootstrap, DB,
//! recording, network, permissions or user preferences are accessed.
//! SOU276_SURFACE=home|settings-{transcription,ai,audio,interface,meetings,system}
//! |onboarding-{permissions,microphone,model,shortcut}|detail|meeting|dictation
//! |permissions|update|whats-new|specimen|hud-{dictation,meeting,polishing}[-compact]
//! |empty|error|loading|detail-{empty,error,loading}
//! |live-editor[-error|-loading]|settings-suggestions[-empty|-error|-loading]
//! SOU276_LANG=fr|en, SOU276_DARK=0|1, SOU276_WIDTH=680|1040.
slint::include_modules!();
#[path = "../src/typography.rs"]
mod typography;
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};
use std::ffi::CString;

unsafe extern "C" {
    fn pill_panel_create();
    fn pill_panel_set_visible(visible: i32);
    fn pill_panel_set_capture_excluded(excluded: i32);
    fn pill_panel_set_mode(
        mode: i32,
        title: *const std::ffi::c_char,
        stop: *const std::ffi::c_char,
        a11y: *const std::ffi::c_char,
    );
    fn pill_panel_set_live_text(text: *const std::ffi::c_char, provisional: i32);
    fn pill_typography_verify() -> i32;
    fn pill_typography_register(bytes: *const u8, count: usize, index: i32) -> i32;
}
fn strings(values: &[&str]) -> ModelRc<SharedString> {
    ModelRc::new(VecModel::from(
        values
            .iter()
            .map(|value| SharedString::from(*value))
            .collect::<Vec<_>>(),
    ))
}
fn model<T: Clone + 'static>(values: Vec<T>) -> ModelRc<T> {
    ModelRc::new(VecModel::from(values))
}
fn remove_fixture_suggestion(window: &MainWindow, id: i32) {
    let rows = window.get_settings_dictionary_suggestions();
    let remaining = (0..rows.row_count())
        .filter_map(|index| rows.row_data(index))
        .filter(|row| row.id != id)
        .collect::<Vec<_>>();
    window.set_settings_dictionary_suggestion_count(remaining.len() as i32);
    window.set_settings_dictionary_suggestion_content_height(remaining.len() as f32 * 64.0);
    window.set_settings_dictionary_suggestions(model(remaining));
}
fn main() {
    let lang = std::env::var("SOU276_LANG").unwrap_or_else(|_| "fr".into());
    let dark = std::env::var("SOU276_DARK").as_deref() != Ok("0");
    let surface = std::env::var("SOU276_SURFACE").unwrap_or_else(|_| "home".into());
    let width = std::env::var("SOU276_WIDTH")
        .ok()
        .and_then(|value| value.parse::<f32>().ok())
        .unwrap_or(1040.);
    slint::BackendSelector::new()
        .renderer_name("skia".into())
        .require_metal()
        .select()
        .unwrap();
    souffle_lib::native::appearance::apply_resolved(dark);
    let window = MainWindow::new().unwrap();
    slint::select_bundled_translation(&lang).unwrap();
    let proof = typography::initialize(&window).unwrap();
    for row in proof {
        println!("Skia {row}");
    }
    if std::env::var("SOU276_FAIL_REGISTRATION").as_deref() == Ok("1") {
        // Test-only failure injection at the FFI boundary; never a product env.
        assert_ne!(
            unsafe { pill_typography_register(b"broken".as_ptr(), 6, 0) },
            0
        );
        assert_ne!(unsafe { pill_typography_verify() }, 0);
        println!("invalid native registration rejected before layout");
        return;
    }
    souffle_lib::pill::register_fonts().unwrap();
    assert_eq!(unsafe { pill_typography_verify() }, 0);
    println!(
        "QA sha={} profile={} surface={surface} locale={lang} dark={dark} size={width}x820 renderer=Skia/Metal no-bootstrap no-DB no-network",
        env!("SOUFFLE_BUILD_GIT_SHA"),
        env!("SOUFFLE_BUILD_PROFILE")
    );
    window
        .window()
        .set_size(slint::LogicalSize::new(width, 820.));
    window.global::<Theme>().set_dark(dark);
    window.set_settings_theme(if dark {
        AppTheme::Dark
    } else {
        AppTheme::Light
    });
    window.set_settings_locale(if lang == "en" {
        AppLocale::En
    } else {
        AppLocale::Fr
    });
    window.set_dictation_shortcut("⌘ ⌥ ⇧ D".into());
    window.set_header_model_label("Apple Speech & Inter".into());
    window.set_model_runtime_phase(TranscriptionPhase::Ready);
    window.set_settings_selected_model_label("Apple Speech — Français & English".into());
    window.set_settings_model_labels(strings(&[
        "Apple Speech — Français & English",
        "Whisper — Large v3",
    ]));
    window.set_settings_summary_model_labels(strings(&[
        "Apple Intelligence",
        "Local — Inter & français",
    ]));
    window.set_settings_about_summary_label("Inter 4.1 — 100–900 & vrais italiques".into());
    window.set_settings_app_version("SOU-276 QA".into());
    window.set_settings_open(surface.starts_with("settings-"));
    window.set_settings_tab(match surface.as_str() {
        "settings-ai" => SettingsTab::Ai,
        "settings-audio" => SettingsTab::Audio,
        "settings-interface" => SettingsTab::Interface,
        "settings-meetings" => SettingsTab::Meetings,
        "settings-system" => SettingsTab::System,
        _ => SettingsTab::Transcription,
    });
    window.set_settings_dictionary_learning_options(model(
        souffle_lib::settings::DictionaryLearningMode::ALL
            .into_iter()
            .map(|mode| DictionaryLearningOption {
                mode: match mode {
                    souffle_lib::settings::DictionaryLearningMode::Disabled => {
                        DictionaryLearningMode::Disabled
                    }
                    souffle_lib::settings::DictionaryLearningMode::Suggestions => {
                        DictionaryLearningMode::Suggestions
                    }
                    souffle_lib::settings::DictionaryLearningMode::Automatic => {
                        DictionaryLearningMode::Automatic
                    }
                },
            })
            .collect(),
    ));
    window.set_settings_dictionary_learning_mode(DictionaryLearningMode::Suggestions);
    let weak = window.as_weak();
    window.on_settings_dictionary_learning_mode_changed(move |mode| {
        if let Some(window) = weak.upgrade() {
            window.set_settings_dictionary_learning_mode(mode);
            println!("dictionary learning action accepted: {mode:?}");
        }
    });
    if surface.starts_with("settings-suggestions") {
        let rows = if surface == "settings-suggestions-empty" {
            Vec::new()
        } else {
            vec![
                DictionarySuggestionRow {
                    id: 1,
                    misspelling: "oeufs & equipe".into(),
                    term: "œufs & Équipe".into(),
                },
                DictionarySuggestionRow {
                    id: 2,
                    misspelling: "Kubernetis".into(),
                    term: "Kubernetes — accès ⌘ ⌥ ⇧".into(),
                },
            ]
        };
        window.set_settings_dictionary_suggestion_count(rows.len() as i32);
        window.set_settings_dictionary_suggestion_content_height(rows.len() as f32 * 64.0);
        window.set_settings_dictionary_suggestions(model(rows));
        window.set_settings_dictionary_suggestion_busy(surface == "settings-suggestions-loading");
        if surface == "settings-suggestions-error" {
            window.set_settings_dictionary_suggestion_error(if lang == "en" {
                "Correction failed & unavailable — try again.".into()
            } else {
                "Échec de correction & accès refusé — réessayez.".into()
            });
        }
    }
    let weak = window.as_weak();
    window.on_settings_dictionary_suggestion_accept_requested(move |id| {
        if let Some(window) = weak.upgrade() {
            remove_fixture_suggestion(&window, id);
            println!("dictionary suggestion accept action: {id}");
        }
    });
    let weak = window.as_weak();
    window.on_settings_dictionary_suggestion_dismiss_requested(move |id| {
        if let Some(window) = weak.upgrade() {
            remove_fixture_suggestion(&window, id);
            println!("dictionary suggestion dismiss action: {id}");
        }
    });
    let weak = window.as_weak();
    window.on_settings_theme_changed(move |theme| {
        if let Some(window) = weak.upgrade() {
            window.global::<Theme>().set_dark(match theme {
                AppTheme::Dark => true,
                AppTheme::Light => false,
                AppTheme::System => false,
            });
            window.set_settings_theme(theme);
            souffle_lib::native::appearance::apply_resolved(window.global::<Theme>().get_dark());
            println!("theme action accepted: {theme:?}");
        }
    });
    window.on_stop_requested(|| println!("stop action accepted"));
    window.set_timeline_is_empty(false);
    window.set_timeline_groups(model(vec![TimelineDayGroup {
        day: TimelineDay::Today,
        entries: model(vec![TimelineEntry {
            id: "fixture".into(),
            kind: TimelineKind::Meeting,
            title: "Équipe & « œufs » — accès ⌘ ⌥ ⇧".into(),
            time_label: "09:10".into(),
            duration_label: "00:09".into(),
            has_summary: true,
            ..Default::default()
        }]),
        ..Default::default()
    }]));
    if surface == "empty" {
        window.set_timeline_is_empty(true);
        window.set_timeline_groups(model(Vec::new()));
    }
    if surface == "error" {
        window.set_model_runtime_phase(TranscriptionPhase::Failed);
        window.set_transcription_status_message("MODEL_ERROR".into());
    }
    if surface == "loading" {
        window.set_model_runtime_phase(TranscriptionPhase::Loading);
        window.set_transcription_status_message("MODEL_PREPARING".into());
    }
    window.set_onboarding_open(surface.starts_with("onboarding-"));
    window.set_onboarding_step(match surface.as_str() {
        "onboarding-microphone" => OnboardingStep::Microphone,
        "onboarding-model" => OnboardingStep::Model,
        "onboarding-shortcut" => OnboardingStep::Shortcut,
        _ => OnboardingStep::Permissions,
    });
    window.set_onboarding_step_count(4);
    window.set_onboarding_permission_rows(model(vec![
        PermissionRow {
            kind: PermissionKind::Microphone,
            state: AccessState::Granted,
            busy: false,
        },
        PermissionRow {
            kind: PermissionKind::SystemAudio,
            state: AccessState::Denied,
            busy: false,
        },
        PermissionRow {
            kind: PermissionKind::Accessibility,
            state: AccessState::Denied,
            busy: false,
        },
        PermissionRow {
            kind: PermissionKind::Calendar,
            state: AccessState::Unknown,
            busy: false,
        },
    ]));
    window.set_onboarding_device_labels(strings(&["Microphone — Équipe & USB"]));
    window.set_onboarding_selected_device_label("Microphone — Équipe & USB".into());
    window.set_onboarding_model_labels(strings(&[
        "Apple Speech — Français & English",
        "Whisper — Large v3",
    ]));
    window.set_onboarding_selected_model_label("Apple Speech — Français & English".into());
    window.set_onboarding_toggle_shortcut_label("⌘ ⌥ ⇧ D".into());
    let text = "Équipe & œufs : « Bonjour ! » é è ê ë à ç Œ É ’ … € 0123456789. La dernière ligne reste lisible & accessible.";
    let blocks = model(vec![TranscriptBlock {
        can_edit: surface.starts_with("live-editor") || surface == "meeting",
        has_speaker: true,
        speaker: SpeakerRole::Me,
        timestamp: "00:09".into(),
        text: text.into(),
        words: model(
            text.split_inclusive(' ')
                .map(|word| TranscriptWord {
                    text: word.trim_end().into(),
                    trailing_text: if word.ends_with(' ') {
                        " ".into()
                    } else {
                        "".into()
                    },
                    clickable: true,
                    provisional: false,
                })
                .collect(),
        ),
        ..Default::default()
    }]);
    if surface.starts_with("detail") {
        window.set_active_meeting_id("fixture".into());
        window.set_meeting_detail_title("Équipe & français — œ Œ €".into());
        window.set_meeting_detail_meta(MeetingMeta {
            day: "5".into(),
            month: "10".into(),
            year: "2026".into(),
            hour: "09".into(),
            minute: "10".into(),
            duration: "00:09".into(),
            segments: 2,
            sessions: 1,
        });
        window.set_meeting_detail_transcript_blocks(blocks.clone());
        window.set_meeting_detail_transcript_segment_count(1);
        window.set_meeting_detail_summary(
            format!("# Résumé & décisions\n\n{text}\n\n- Dernière ligne : accès clavier & ⌘ ⌥ ⇧")
                .into(),
        );
        window.set_meeting_detail_summary_key_points(strings(&[text]));
        window.set_meeting_detail_notes(text.into());
        if surface == "detail-empty" {
            window.set_meeting_detail_transcript_blocks(model(Vec::new()));
            window.set_meeting_detail_transcript_segment_count(0);
            window.set_meeting_detail_summary("".into());
            window.set_meeting_detail_summary_key_points(strings(&[]));
            window.set_meeting_detail_notes("".into());
        }
        if surface == "detail-error" {
            window.set_meeting_detail_summary("".into());
            window.set_meeting_detail_summary_generation_error(
                "Échec & accès refusé : réessayez.".into(),
            );
            window.set_meeting_detail_export_error("Échec & accès refusé : réessayez.".into());
        }
        if surface == "detail-loading" {
            window.set_meeting_detail_summary("".into());
            window.set_meeting_detail_summary_is_generating(true);
            window.set_meeting_detail_summary_generation_progress(
                "Préparation & dernière ligne…".into(),
            );
            window.set_meeting_detail_has_audio(true);
            window.set_meeting_detail_audio_loading(true);
        }
    }
    if surface == "meeting" || surface == "dictation" || surface.starts_with("live-editor") {
        window.set_recording_mode(if surface != "dictation" {
            RecordingMode::Meeting
        } else {
            RecordingMode::Dictation
        });
        window.set_live_system_audio(LiveSystemAudio::Active);
        window.set_live_transcript_empty(false);
        window.set_live_transcript_blocks(blocks);
        window.set_live_text(text.into());
        window.set_live_dictation_words(model(
            text.split_whitespace()
                .map(|word| TranscriptWord {
                    text: word.into(),
                    trailing_text: " ".into(),
                    clickable: true,
                    provisional: false,
                })
                .collect(),
        ));
        window.set_live_elapsed_offset_seconds(9);
    }
    if surface.starts_with("live-editor") {
        window.set_live_edit_text(text.into());
        window.set_live_edit_open(true);
        window.set_live_edit_busy(surface == "live-editor-loading");
        window.set_live_edit_failed(surface == "live-editor-error");
    }
    let weak = window.as_weak();
    window.on_live_paragraph_edit_requested(move |row| {
        if let Some(window) = weak.upgrade() {
            window.set_live_edit_text(window.get_live_text());
            window.set_live_edit_open(true);
            println!("live paragraph edit requested: {row}");
        }
    });
    let weak = window.as_weak();
    window.on_live_paragraph_edit_cancelled(move || {
        if let Some(window) = weak.upgrade() {
            window.set_live_edit_open(false);
            println!("live paragraph edit cancelled");
        }
    });
    let weak = window.as_weak();
    window.on_live_paragraph_edit_saved(move |text| {
        if let Some(window) = weak.upgrade() {
            window.set_live_edit_text(text.clone());
            window.set_live_edit_open(false);
            println!("live paragraph edit save action: {text}");
        }
    });
    if surface == "permissions" {
        window.set_settings_permissions_dialog_open(true);
    }
    if surface == "update" {
        window.set_update_available_open(true);
        window.set_update_latest_version("4.1 & Inter".into());
        window.set_update_release_notes_blocks(model(vec![MarkdownBlock {
            kind: MarkdownBlockKind::Paragraph,
            text: text.into(),
            ..Default::default()
        }]));
    }
    if surface == "whats-new" {
        window.set_whats_new_open(true);
        window.set_whats_new_version("4.1".into());
        window.set_whats_new_content_blocks(model(vec![MarkdownBlock {
            kind: MarkdownBlockKind::Paragraph,
            text: text.into(),
            ..Default::default()
        }]));
    }
    if surface == "specimen" {
        let specimen = TypographySpecimen::new().unwrap();
        specimen.set_foreground(window.global::<Theme>().get_text_primary().into());
        specimen
            .window()
            .set_size(slint::LogicalSize::new(1040., 780.));
        specimen.set_canvas(window.global::<Theme>().get_canvas().into());
        specimen.run().unwrap();
    } else {
        if surface.starts_with("hud-") {
            use souffle_lib::pill::PillPanelMode;
            let mode = if surface.starts_with("hud-meeting") {
                PillPanelMode::Meeting
            } else if surface.starts_with("hud-polishing") {
                PillPanelMode::Polishing
            } else {
                PillPanelMode::Dictation
            };
            let title = CString::new(match (mode, lang == "fr") {
                (PillPanelMode::Dictation, true) => "Dictée",
                (PillPanelMode::Dictation, false) => "Dictating",
                (PillPanelMode::Meeting, _) => "",
                (PillPanelMode::Polishing, true) => "Reformulation…",
                (PillPanelMode::Polishing, false) => "Reformulating…",
            })
            .unwrap();
            let a11y = match mode {
                PillPanelMode::Meeting => {
                    CString::new(if lang == "fr" { "Réunion" } else { "Meeting" }).unwrap()
                }
                PillPanelMode::Dictation | PillPanelMode::Polishing => title.clone(),
            };
            let stop = CString::new(if lang == "en" {
                "Stop recording"
            } else {
                "Arrêter l'enregistrement"
            })
            .unwrap();
            let live = CString::new(match mode {
                PillPanelMode::Dictation if !surface.ends_with("-compact") => {
                    format!("{text} {text} {text} Dernière ligne récente & conservée.")
                }
                PillPanelMode::Dictation | PillPanelMode::Meeting | PillPanelMode::Polishing => {
                    String::new()
                }
            })
            .unwrap();
            unsafe {
                pill_panel_create();
                pill_panel_set_capture_excluded(0);
                pill_panel_set_mode(mode as i32, title.as_ptr(), stop.as_ptr(), a11y.as_ptr());
                pill_panel_set_live_text(live.as_ptr(), 0);
                pill_panel_set_visible(1);
            }
        }
        window.run().unwrap();
    }
}
