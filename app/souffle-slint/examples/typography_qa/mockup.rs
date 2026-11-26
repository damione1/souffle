//! Data reproducing the states drawn on the design boards, applied on top of the
//! typography fixture when SOU276_MOCKUP=1. French only, no live services.
use super::{audio_ui, ia_ui, microphone_list, model, strings};
use slint::{ComponentHandle, SharedString};
use souffle_lib::audio::{AudioInputDevice, InputPriority, KnownDevice, TransportType};
use souffle_lib::settings::{AppSettings, SummaryTemplate};
use souffle_lib::summary::{
    SummaryModelDescriptor, SummaryProviderChoice, SummaryProviderKind, SummaryProvidersStatus,
};
use souffle_slint::*;

pub(super) fn apply(window: &MainWindow, surface: &str) {
    // The boards draw the meeting summaries, which sit behind an opt-in.
    window.global::<SummarizationUi>().set_enabled(true);
    window.set_dictation_shortcut("⌥ Espace".into());
    window.set_header_model_label("Kyutai STT 1B".into());
    if surface.starts_with("settings-") {
        settings(window);
        settings_state(window, surface);
    } else if surface.starts_with("onboarding-") {
        onboarding(window);
    } else if surface.starts_with("detail") {
        detail(window, surface);
    } else if surface == "meeting" || surface == "dictation" {
        recording(window, surface);
    } else {
        home(window, surface);
        dialogs(window, surface);
    }
}

/// Same path shape as `audio_player::waveform_commands`.
fn waveform(peaks: &[f32]) -> String {
    peaks
        .iter()
        .enumerate()
        .map(|(i, peak)| {
            let top = (1.0 - peak) / 2.0;
            format!("M{i} {top:.3}h0.7v{peak:.3}h-0.7z")
        })
        .collect()
}

fn entry(
    kind: TimelineKind,
    id: &str,
    title: &str,
    duration: &str,
    time: &str,
    has_summary: bool,
) -> HistoryEntry {
    HistoryEntry {
        kind,
        id: id.into(),
        title: title.into(),
        duration_label: duration.into(),
        time_label: time.into(),
        has_summary,
        ..Default::default()
    }
}

fn event(title: &str, range: &str, people: i32, phase: UpcomingPhase) -> UpcomingEvent {
    UpcomingEvent {
        id: title.into(),
        title: title.into(),
        time_range: range.into(),
        participant_count: people,
        phase,
    }
}

fn home(window: &MainWindow, surface: &str) {
    use TimelineKind::{Dictation, Meeting};
    // First launch keeps the calendar disconnected and the history empty.
    if surface == "empty" {
        return;
    }
    window.set_settings_calendar_enabled(true);
    window.set_settings_calendar_permission(AccessState::Granted);
    if surface == "search-empty" {
        window.set_search_query("budget 2027".into());
        window.set_kind_filter(TimelineFilter::Meeting);
        window.set_timeline_has_matches(false);
        window.set_timeline_groups(model(Vec::new()));
        return;
    }
    window.set_upcoming_events(model(vec![
        event(
            "Point produit hebdo",
            "09:00 – 09:30",
            6,
            UpcomingPhase::Past,
        ),
        event(
            "Revue design : écran réunions",
            "10:30 – 11:15",
            4,
            UpcomingPhase::Now,
        ),
        event("1:1 Camille", "14:00 – 14:30", 2, UpcomingPhase::Next),
        event(
            "Comité de pilotage T4",
            "16:00 – 17:00",
            9,
            UpcomingPhase::Later,
        ),
    ]));
    window.set_timeline_groups(model(vec![
        TimelineDayGroup {
            day: TimelineDay::Today,
            entries: model(vec![
                entry(
                    Meeting,
                    "a",
                    "Revue design : écran réunions",
                    "41:12",
                    "10:34",
                    true,
                ),
                entry(
                    Dictation,
                    "b",
                    "Penser à relancer le fournisseur pour la facture de mars",
                    "0:32",
                    "09:48",
                    false,
                ),
                entry(Meeting, "c", "Point produit hebdo", "28:05", "09:00", true),
                entry(
                    Dictation,
                    "d",
                    "Idée : afficher le résumé en premier dans la vue lecture",
                    "0:18",
                    "08:12",
                    false,
                ),
            ]),
            ..Default::default()
        },
        TimelineDayGroup {
            day: TimelineDay::Yesterday,
            entries: model(vec![
                entry(Meeting, "e", "Atelier roadmap T1", "1:12:44", "16:20", true),
                entry(
                    Dictation,
                    "f",
                    "Mail à Julien : confirmation du devis",
                    "0:44",
                    "11:05",
                    false,
                ),
                entry(
                    Meeting,
                    "g",
                    "Entretien candidat : développeuse Rust",
                    "52:31",
                    "10:00",
                    false,
                ),
            ]),
            ..Default::default()
        },
    ]));
    if surface == "alerts" {
        window.set_mic_stall_visible(true);
        window.set_mic_stall_kind(MicStall::TimedOut);
        window.set_mic_stall_device("MacBook Pro Microphone".into());
        window.set_dictation_recovery_text("Pensez à relancer le fournisseur pour la facture de mars et à confirmer la date du comité avec Camille avant vendredi.".into());
        window.set_meeting_status_message("MEETING_FINALIZATION_TIMEOUT".into());
    }
}

fn release_notes() -> Vec<MarkdownBlock> {
    use MarkdownBlockKind::{Bullet, Heading, Link};
    [
        (Heading, "Nouveau"),
        (Bullet, "Vue lecture des réunions avec résumé en premier"),
        (Bullet, "Lecteur audio persistant et voies Moi / Eux"),
        (Heading, "Corrigé"),
        (Bullet, "Reprise après mise en veille"),
        (Link, "Voir toutes les notes de version"),
    ]
    .into_iter()
    .map(|(kind, text)| MarkdownBlock {
        kind,
        text: text.into(),
        ..Default::default()
    })
    .collect()
}

fn permission_rows(states: [AccessState; 4]) -> Vec<PermissionRow> {
    [
        PermissionKind::Microphone,
        PermissionKind::SystemAudio,
        PermissionKind::Accessibility,
        PermissionKind::Calendar,
    ]
    .into_iter()
    .zip(states)
    .map(|(kind, state)| PermissionRow {
        kind,
        state,
        busy: false,
    })
    .collect()
}

fn dialogs(window: &MainWindow, surface: &str) {
    use AccessState::{Denied, Granted, Unknown};
    if surface == "permissions" {
        window.set_onboarding_permission_rows(model(permission_rows([
            Granted, Granted, Denied, Unknown,
        ])));
    }
    if surface == "whats-new" {
        window.set_whats_new_version("0.42.0".into());
        window.set_whats_new_content_blocks(model(release_notes()));
    }
    if surface.starts_with("update") {
        window.set_update_available_open(true);
        window.set_update_latest_version("0.43.0".into());
        window.set_update_release_url("https://example.invalid/release".into());
        window.set_update_release_notes_blocks(model(Vec::new()));
        match surface {
            "update-downloading" => window.set_update_phase(UpdatePhase::Downloading),
            "update-failed" => {
                window.set_update_phase(UpdatePhase::Failed);
                window.set_update_error_message(
                    "Échec du téléchargement : connexion interrompue.".into(),
                );
            }
            _ => {
                window.set_update_phase(UpdatePhase::Ready);
                window.set_update_release_notes_blocks(model(release_notes()));
                window.set_update_install_blocked(true);
                window.set_update_install_block_reason(InstallBlockReason::RecordingMeeting);
            }
        }
    }
}

fn onboarding(window: &MainWindow) {
    use AccessState::{Denied, Granted, Unknown};
    window.set_onboarding_toggle_shortcut_label("⌥ Espace".into());
    window.set_onboarding_permission_rows(model(permission_rows([
        Granted, Unknown, Denied, Unknown,
    ])));
    window.set_onboarding_device_labels(strings(&["MacBook Pro Microphone"]));
    window.set_onboarding_device_ids(strings(&["builtin"]));
    window.set_onboarding_default_device_index(0);
    window.set_onboarding_audio_level(0.6);
    window.set_onboarding_audio_levels(model(
        (0..28)
            .map(|i| 0.25 + 0.6 * ((i as f32) * 0.7).sin().abs())
            .collect::<Vec<f32>>(),
    ));
    window.set_onboarding_model_labels(strings(&["Kyutai STT 1B · FR + EN · streaming"]));
    window.set_onboarding_model_ids(strings(&["kyutai"]));
    if window.get_onboarding_step() == OnboardingStep::Model {
        window.set_onboarding_model_phase(ModelPhase::Downloading);
        window.set_onboarding_download_progress_label("62 %".into());
        window.set_onboarding_download_progress_fraction(0.62);
    }
    window.set_onboarding_step_index(match window.get_onboarding_step() {
        OnboardingStep::Permissions => 0,
        OnboardingStep::Microphone => 1,
        OnboardingStep::Model => 2,
        OnboardingStep::Shortcut => 3,
    });
}

fn paragraph(speaker: SpeakerRole, timestamp: &str, text: &str, tail: &str) -> TranscriptBlock {
    let words = text
        .split_whitespace()
        .map(|word| (word, false))
        .chain(tail.split_whitespace().map(|word| (word, true)))
        .map(|(word, provisional)| TranscriptWord {
            text: word.into(),
            trailing_text: " ".into(),
            clickable: !provisional,
            provisional,
        })
        .collect::<Vec<_>>();
    TranscriptBlock {
        has_speaker: true,
        speaker,
        timestamp: timestamp.into(),
        start_label: timestamp.into(),
        text: text.into(),
        words: model(words),
        recording_session_index: -1,
        ..Default::default()
    }
}

fn recording(window: &MainWindow, surface: &str) {
    if surface == "dictation" {
        let block = paragraph(
            SpeakerRole::Me,
            "",
            "Bonjour Camille, je te confirme que le comité aura bien lieu jeudi à seize heures, et je t'envoie l'ordre du jour",
            "avant la fin de la",
        );
        window.set_live_dictation_words(block.words);
        window.set_live_text("Bonjour Camille, je te confirme que le comité aura bien lieu jeudi à seize heures, et je t'envoie l'ordre du jour".into());
        window.set_live_tentative("avant la fin de la".into());
        window.set_settings_auto_paste(true);
        window.set_mic_stall_visible(true);
        window.set_mic_stall_kind(MicStall::Parked);
        window.set_mic_stall_device("MacBook Pro Microphone".into());
        window.set_live_elapsed_offset_seconds(14);
    } else {
        window.set_live_transcript_blocks(model(vec![
            paragraph(SpeakerRole::Them, "23:12", "Du coup côté planning, on peut viser la bêta pour mi-décembre si l'équipe valide le périmètre cette semaine.", ""),
            paragraph(SpeakerRole::Me, "23:31", "Oui, je pense que c'est réaliste. Il faut juste figer la liste des réglages avant lundi, sinon on risque de glisser sur la documentation.", ""),
            paragraph(SpeakerRole::Them, "23:49", "D'accord. Je note la décision et je prépare le point pour le comité de jeudi.", "On pourra aussi"),
        ]));
        window.set_meeting_idle_banner(MeetingIdleBanner::Silence);
        window.set_meeting_idle_minutes(4);
        window.set_live_elapsed_offset_seconds(24 * 60 + 8);
    }
    window.set_audio_level(0.5);
}

fn detail(window: &MainWindow, surface: &str) {
    window.set_meeting_detail_title("Revue design : écran réunions".into());
    window.set_meeting_detail_meta(MeetingMeta {
        day: "22".into(),
        month: "11".into(),
        year: "2026".into(),
        hour: "10".into(),
        minute: "34".into(),
        duration: "41:12".into(),
        segments: 128,
        sessions: 2,
    });
    window.set_meeting_detail_model_label("Kyutai STT 1B".into());
    window.set_meeting_detail_participants(model(
        [
            ("Camille Roy", true, false),
            ("Julien Martin", false, false),
            ("Damien", false, true),
            ("Inès Bailly", false, false),
        ]
        .into_iter()
        .map(
            |(name, is_organizer, is_current_user)| MeetingParticipantChip {
                name: name.into(),
                is_organizer,
                is_current_user,
            },
        )
        .collect::<Vec<_>>(),
    ));
    window.set_meeting_detail_can_resume(true);
    window.set_meeting_detail_summary_available_models(strings(&["Compte rendu détaillé"]));
    window.set_meeting_detail_summary_available_model_ids(strings(&["detailed"]));
    window.set_meeting_detail_summary_selected_model_id("detailed".into());
    if surface == "detail-empty" {
        window.set_meeting_detail_participants(model(Vec::new()));
        return;
    }

    let peaks = (0..110)
        .map(|i| 0.3 + 0.65 * ((i as f32) * 0.53).sin().abs())
        .collect::<Vec<f32>>();
    window.set_meeting_detail_has_audio(surface != "detail-no-audio");
    window.set_meeting_detail_audio_loading(false);
    window.set_meeting_detail_audio_waveform_commands(waveform(&peaks).into());
    window.set_meeting_detail_audio_waveform_bars(peaks.len() as i32);
    window.set_meeting_detail_audio_progress(0.38);
    window.set_meeting_detail_audio_position_label("15:38".into());
    window.set_meeting_detail_audio_duration_label("41:12".into());

    let mut blocks = vec![
        paragraph(
            SpeakerRole::Them,
            "23:12",
            "Du coup côté planning, on peut viser la bêta pour mi-décembre si l'équipe valide le périmètre cette semaine.",
            "",
        ),
        paragraph(
            SpeakerRole::Me,
            "23:31",
            "Oui, je pense que c'est réaliste. Il faut juste figer la liste des réglages avant lundi.",
            "",
        ),
        TranscriptBlock {
            is_session_break: true,
            ..Default::default()
        },
        paragraph(
            SpeakerRole::Them,
            "24:02",
            "D'accord. Je note la décision et je prépare le point pour le comité de jeudi.",
            "",
        ),
        paragraph(
            SpeakerRole::Me,
            "24:20",
            "Parfait, je m'occupe de la documentation en parallèle.",
            "",
        ),
        paragraph(
            SpeakerRole::Them,
            "24:41",
            "Dernier sujet : l'export des sous-titres, on le garde pour la bêta ?",
            "",
        ),
    ];
    for block in &mut blocks {
        block.recording_session_index = 0;
    }
    window.set_meeting_detail_transcript_segment_count(128);
    window.set_meeting_detail_transcript_blocks(model(blocks));
    window.set_meeting_detail_transcript_preview("Du coup côté planning, on peut viser la bêta pour mi-décembre si l'équipe valide le périmètre cette semaine. Oui, je pense que c'est réaliste. Il faut juste figer la liste des réglages avant lundi, sinon on risque de glisser sur la documentation.".into());
    window.set_meeting_detail_notes("Penser à valider la palette ambre avec l'équipe produit. Camille veut une version compacte du lecteur.".into());
    window.set_meeting_detail_notes_save_state(NotesSaveState::Saved);

    window.set_meeting_detail_summary("".into());
    window.set_meeting_detail_summary_body("".into());
    window.set_meeting_detail_summary_key_points(strings(&[]));
    match surface {
        "detail-no-summary" | "detail-no-audio" => {}
        "detail-error" => {
            window.set_meeting_detail_summary_generation_error(
                "le modèle Ollama n'a pas répondu.".into(),
            );
            window.set_meeting_detail_export_error(
                "Impossible d'exporter l'audio : fichier introuvable.".into(),
            );
            window.set_wake_resume_notice(WakeResumeNotice::Interrupted);
        }
        "detail-loading" => {
            window.set_meeting_detail_summary_is_generating(true);
            window.set_meeting_detail_summary_generation_progress("Génération en cours…".into());
        }
        "detail-no-provider" => {
            window.set_meeting_detail_summary_available_models(strings(&[]));
            window.set_meeting_detail_summary_available_model_ids(strings(&[]));
            window.set_meeting_detail_summary_selected_model_id("".into());
        }
        _ => {
            window.set_meeting_detail_summary(" ".into());
            window.set_meeting_detail_summary_model_label("Apple Intelligence".into());
            window.set_meeting_detail_summary_is_stale(true);
            window.set_meeting_detail_summary_key_points(strings(&[
                "La bêta est visée pour mi-décembre, sous réserve de figer le périmètre des réglages avant lundi.",
                "La vue lecture présente le résumé en premier ; la transcription complète reste accessible en un clic.",
                "Le lecteur audio reste visible pendant le défilement pour naviguer par horodatage.",
            ]));
            window.set_meeting_detail_summary_decisions(strings(&[
                "Figer la liste des réglages lundi",
                "Retenir l'ambre comme teinte unique",
            ]));
            window.set_meeting_detail_summary_action_items(model(
                [
                    ("Préparer le point comité", "Julien"),
                    ("Mettre à jour la documentation", "Inès"),
                ]
                .into_iter()
                .map(|(text, owner)| StructuredActionItem {
                    text: text.into(),
                    owner: owner.into(),
                })
                .collect::<Vec<_>>(),
            ));
            window.set_meeting_detail_summary_open_questions(strings(&[
                "Faut-il un mode compact ?",
                "Export audio dans la bêta ?",
            ]));
        }
    }
    if surface == "detail-transcript" {
        window.set_meeting_detail_view(MeetingDetailView::Transcript);
    }
}

fn settings(window: &MainWindow) {
    window.set_settings_app_version("0.42.0".into());
    window.set_settings_about_transcription_label("Kyutai STT 1B".into());
    window.set_settings_about_summary_label("Apple Intelligence".into());
    window.set_settings_auto_update_check(true);
    window.set_settings_update_row(UpdateRowStatus::UpToDate);

    // Transcription
    window.set_settings_model_labels(strings(&["Kyutai STT 1B"]));
    window.set_settings_model_ids(strings(&["kyutai"]));
    window.set_settings_selected_model_id("kyutai".into());
    window.set_settings_selected_model_label("Kyutai STT 1B".into());
    window.set_settings_model_files_deletable(true);
    window.set_settings_unload_timeout_labels(strings(&["__NEVER__", "15 min"]));
    window.set_settings_unload_timeout_label("15 min".into());
    window.set_settings_dictionary_suggestions(model(vec![DictionarySuggestionRow {
        id: 1,
        misspelling: "coubernétisse".into(),
        term: "Kubernetes".into(),
    }]));
    window.set_settings_dictionary_suggestion_count(1);
    window.set_settings_dictionary_suggestion_content_height(64.0);
    window.set_settings_dictionary_entries(model(
        [
            (1, "Kubernetes", "coubernétisse", "tech"),
            (2, "Soufflé", "soufflet", "produit"),
        ]
        .into_iter()
        .map(|(id, term, aliases, category)| DictionaryRow {
            id,
            term: term.into(),
            aliases_display: aliases.into(),
            category: category.into(),
        })
        .collect::<Vec<_>>(),
    ));
    window.set_settings_dictionary_count(2);
    window.set_settings_dictionary_content_height(104.0);
    window.set_settings_snippets(model(vec![SnippetRow {
        id: 1,
        trigger: "mon adresse".into(),
        expansion: "12 rue des Lilas, 75011 Paris".into(),
        editing: false,
    }]));
    window.set_settings_snippets_count(1);
    window.set_settings_snippets_content_height(52.0);

    // AI
    let app_settings = AppSettings {
        dictation_polish_enabled: true,
        summary_provider: SummaryProviderChoice::Auto,
        ollama_model: "qwen3:14b".into(),
        summary_templates: [
            ("detailed", "Compte rendu détaillé"),
            ("brief", "Aperçu bref"),
        ]
        .into_iter()
        .map(|(id, name)| SummaryTemplate {
            id: id.into(),
            name: name.into(),
            prompt: "Rédige un compte rendu.".into(),
        })
        .collect(),
        default_summary_template_id: "detailed".into(),
        ..Default::default()
    };
    let status = SummaryProvidersStatus {
        ollama_url: "http://localhost:11434".into(),
        ollama_available: true,
        apple_intelligence_available: false,
        apple_intelligence_is_stub: false,
        apple_intelligence_unavailable_reason: Some(
            souffle_lib::apple_intelligence::AppleIntelligenceUnavailableReason::AppleIntelligenceNotEnabled,
        ),
        recommended_ollama_model: "qwen3:14b".into(),
        models: vec![SummaryModelDescriptor {
            id: "qwen3:14b".into(),
            label: "qwen3:14b".into(),
            provider: SummaryProviderKind::Ollama,
            can_summarize: true,
        }],
    };
    window.set_settings_ollama_url("http://localhost:11434".into());
    ia_ui::populate_intelligence(window, &app_settings, &status);
    ia_ui::populate_dictation_polish(window, &app_settings, true);
    ia_ui::populate_summary_templates(window, &app_settings, "brief");

    // Audio
    let device = |uid: &str, name: &str, transport, is_default| AudioInputDevice {
        uid: uid.into(),
        name: name.into(),
        transport,
        is_default,
    };
    let devices = vec![
        device(
            "builtin",
            "MacBook Pro Microphone",
            TransportType::BuiltIn,
            true,
        ),
        device("airpods", "AirPods", TransportType::Bluetooth, false),
    ];
    audio_ui::populate_device_pickers(window, &devices, "builtin", None);
    window.set_settings_sample_rate_label("96 kHz".into());
    window.set_settings_sample_rate_high(true);
    let priority = InputPriority {
        priorities: vec!["builtin".into(), "airpods".into(), "usb".into()],
        known: vec![KnownDevice {
            uid: "usb".into(),
            name: "Shure MV7".into(),
            last_seen: chrono::Utc::now().timestamp() - 3 * 86_400,
        }],
        ..Default::default()
    };
    let list = microphone_list::build_microphone_list(&devices, &priority);
    audio_ui::populate_microphones(window, &list);
    window.set_settings_is_laptop(true);
    window.set_settings_system_audio_supported(true);
    window.set_settings_capture_system_audio(true);
    window.set_settings_meeting_autostop_enabled(true);
    window.set_settings_meeting_autostop_labels(strings(&["5 min", "15 min"]));
    window.set_settings_meeting_autostop_label("5 min".into());
    window.set_settings_meeting_max_duration_labels(strings(&["3 h", "4 h"]));
    window.set_settings_meeting_max_duration_label("3 h".into());
    window.set_settings_vad_enabled(true);
    window.set_settings_filler_removal(true);
    window.set_settings_dictionary_correction(true);

    // Interface
    window.set_settings_auto_paste(true);
    window.set_settings_paste_delay_ms(150);
    window.set_settings_paste_delay_min(0);
    window.set_settings_paste_delay_max(1000);
    window.set_settings_toggle_shortcut_label("⌥ Espace".into());
    window.set_settings_native_tap_warning_visible(true);
    window.set_settings_pill_hidden_in_captures(true);
    window.set_settings_feedback_sounds_enabled(true);
    window.set_settings_feedback_sounds_volume(60);
    window.set_settings_feedback_sounds_volume_min(0);
    window.set_settings_feedback_sounds_volume_max(100);

    // Meetings
    window.set_settings_calendar_enabled(true);
    window.set_settings_calendar_permission(
        if window.get_settings_tab() == SettingsTab::Meetings {
            AccessState::Granted
        } else {
            AccessState::Unknown
        },
    );
    window.set_settings_calendar_autostart_enabled(true);
    window.set_settings_calendar_reminder_minutes(5);
    window.set_settings_calendar_reminder_minutes_min(0);
    window.set_settings_calendar_reminder_minutes_max(30);
    window.set_settings_calendars(model(
        [("work", "Travail", true), ("home", "Personnel", false)]
            .into_iter()
            .map(|(id, title, show_header)| CalendarRow {
                id: id.into(),
                title: title.into(),
                source_title: "iCloud".into(),
                selected: true,
                show_header,
            })
            .collect::<Vec<_>>(),
    ));

    // System
    window.set_settings_autostart_enabled(true);
    window.set_settings_log_level(LogLevel::Info);
    let lines = [
        "22:41:08 INFO model ready kyutai-stt-1b",
        "22:41:11 INFO mic stream opened 48000 Hz",
        "22:42:30 WARN mic stall timeout, retry in 2s",
        "22:42:33 INFO mic recovered",
    ];
    window.set_settings_log_lines(strings(&lines));
    window.set_settings_log_line_count(lines.len() as i32);
    window.set_settings_meeting_audio_retention(AudioRetention::Keep30d);
    window.set_settings_mcp_exists(true);
    window
        .set_settings_mcp_binary_path(SharedString::from("/Applications/Souffle.app/souffle-mcp"));
    window.set_settings_mcp_claude_desktop_snippet(
        "{\n  \"souffle\": { \"command\": \"souffle-mcp\" }\n}".into(),
    );
    window.set_settings_mcp_claude_code_command("claude mcp add souffle -- souffle-mcp".into());
}

const LONG_LIST_ROWS: i32 = 30;
/// First row of the mounted slice in the scrolled long-list state.
const SCROLLED_FIRST_ROW: i32 = 10;

/// States without an artboard, selected by surface suffix.
fn settings_state(window: &MainWindow, surface: &str) {
    match surface {
        "settings-transcription-long" => long_lists(window, 0),
        "settings-transcription-long-scrolled" => long_lists(window, SCROLLED_FIRST_ROW),
        "settings-meetings-denied" => {
            window.set_settings_calendar_permission(AccessState::Denied);
        }
        "settings-ai-ollama" => {
            let settings = AppSettings {
                summary_provider: SummaryProviderChoice::Ollama,
                ..Default::default()
            };
            let status = SummaryProvidersStatus {
                ollama_url: "http://localhost:11434".into(),
                ollama_available: true,
                apple_intelligence_available: false,
                apple_intelligence_is_stub: false,
                apple_intelligence_unavailable_reason: None,
                recommended_ollama_model: "qwen3:14b".into(),
                models: Vec::new(),
            };
            ia_ui::populate_intelligence(window, &settings, &status);
        }
        _ => {}
    }
}

/// Mirrors the windowing of `lists_ui.rs`: a slice of the rows between two
/// spacers sized with its row height constants (51px and 58px).
fn long_lists(window: &MainWindow, first: i32) {
    window.set_settings_dictionary_entries(model(
        (first..LONG_LIST_ROWS)
            .map(|i| DictionaryRow {
                id: i + 1,
                term: format!("Terme {:02}", i + 1).into(),
                aliases_display: format!("prononciation {:02}", i + 1).into(),
                category: if i % 2 == 0 { "tech" } else { "" }.into(),
            })
            .collect::<Vec<_>>(),
    ));
    window.set_settings_dictionary_count(LONG_LIST_ROWS);
    window.set_settings_dictionary_content_height(LONG_LIST_ROWS as f32 * 51.0);
    window.set_settings_dictionary_spacer_before(first as f32 * 51.0);
    window.set_settings_dictionary_spacer_after(0.0);
    window.set_settings_snippets(model(
        (first..LONG_LIST_ROWS)
            .map(|i| SnippetRow {
                id: i + 1,
                trigger: format!("extrait {:02}", i + 1).into(),
                expansion: format!("Texte inséré par l'extrait numéro {:02}", i + 1).into(),
                editing: false,
            })
            .collect::<Vec<_>>(),
    ));
    window.set_settings_snippets_count(LONG_LIST_ROWS);
    window.set_settings_snippets_content_height(LONG_LIST_ROWS as f32 * 58.0);
    window.set_settings_snippets_spacer_before(first as f32 * 58.0);
    window.set_settings_snippets_spacer_after(0.0);
    if first > 0 {
        // A few pixels into the second mounted row.
        window.set_settings_dictionary_scroll_top(-((first + 1) as f32 * 51.0 + 20.0));
        window.set_settings_snippets_scroll_top(-((first + 1) as f32 * 58.0 + 20.0));
    }
}
