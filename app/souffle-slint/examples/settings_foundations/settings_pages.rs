//! Two retained settings pages, driven with synthetic provider and peripheral snapshots.
use super::{audio_ui, click, ia_ui, microphone_list, save_frame, send_key, settle};
use crate::{MainWindow, SettingsTab};
use i_slint_backend_testing::{ElementHandle, ElementQuery};
use slint::{ComponentHandle, Model, platform::Key};
use souffle_lib::apple_intelligence::AppleIntelligenceUnavailableReason as Reason;
use souffle_lib::audio::{AudioInputDevice, InputPriority, TransportType};
use souffle_lib::settings::{AppSettings, SummaryTemplate};
use souffle_lib::summary::{
    SummaryModelDescriptor, SummaryProviderChoice, SummaryProviderKind, SummaryProvidersStatus,
};
use std::{cell::Cell, path::Path, rc::Rc};

fn id(w: &MainWindow, name: &str) -> ElementHandle {
    ElementHandle::find_by_element_id(w, name)
        .next()
        .unwrap_or_else(|| panic!("missing {name}"))
}

fn items(w: &MainWindow, kind: &str) -> Vec<ElementHandle> {
    ElementQuery::from_root(w).match_inherits(kind).find_all()
}

fn frame(w: &MainWindow, path: &Path, scene: &str) {
    let stem = path.file_stem().unwrap().to_str().unwrap();
    save_frame(w, &path.with_file_name(format!("{stem}-{scene}.ppm")));
}

fn alerts(w: &MainWindow) -> Vec<slint::SharedString> {
    items(w, "KitAlert")
        .iter()
        .filter_map(ElementHandle::accessible_label)
        .collect()
}

fn wheel(w: &MainWindow, page: &ElementHandle, dy: f32) {
    let p = page.absolute_position();
    let position = slint::LogicalPosition::new(p.x + 5., p.y + page.size().height / 2.);
    w.window()
        .dispatch_event(slint::platform::WindowEvent::PointerScrolled {
            position,
            delta_x: 0.,
            delta_y: dy,
        });
    settle(w);
}

fn reveal(w: &MainWindow, page: &ElementHandle, name: &str) -> ElementHandle {
    // Strict pass keeps the element clear of the bottom edge so that a pointer press
    // is not followed by a focus reveal that moves it. The loose pass covers elements
    // that sit at the very end of the page.
    for strict in [true, false] {
        wheel(w, page, 100000.);
        for _ in 0..400 {
            if let Some(el) = ElementHandle::find_by_element_id(w, name).next() {
                let y = el.absolute_position().y - page.absolute_position().y;
                let height = el.size().height;
                let visible = if strict && height < page.size().height - 72. {
                    y >= 0. && y + height <= page.size().height - 64.
                } else if height < page.size().height - 8. {
                    y >= 0. && y + height <= page.size().height
                } else {
                    y >= 0. && y + height / 2. < page.size().height - 2.
                };
                if visible {
                    return el;
                }
            }
            wheel(w, page, -30.);
        }
    }
    panic!("scroll never revealed {name}");
}

fn assert_widths(w: &MainWindow) {
    for kind in [
        "KitTextField",
        "KitTextArea",
        "KitSelectTrigger",
        "KitButton",
    ] {
        for el in items(w, kind) {
            assert!(
                el.size().width > 24.,
                "{kind} collapsed: {:?} {:?}",
                el.accessible_label(),
                el.size()
            );
        }
    }
}

fn shortcut(w: &MainWindow, key: &str, shift: bool) {
    use slint::platform::WindowEvent;
    let modifier = Key::Control;
    w.window().dispatch_event(WindowEvent::KeyPressed {
        text: modifier.into(),
    });
    if shift {
        w.window().dispatch_event(WindowEvent::KeyPressed {
            text: Key::Shift.into(),
        });
    }
    send_key(w, key.into());
    if shift {
        w.window().dispatch_event(WindowEvent::KeyReleased {
            text: Key::Shift.into(),
        });
    }
    w.window().dispatch_event(WindowEvent::KeyReleased {
        text: modifier.into(),
    });
    settle(w);
}

fn editor(w: &MainWindow, name: &str, path: &Path, scene: &str) {
    let page = id(w, "SettingsView::ai-page");
    let editor = reveal(w, &page, name);
    click(w, &editor);
    frame(w, path, scene);
    let ring = editor
        .query_descendants()
        .match_inherits("FocusOutline")
        .find_first()
        .unwrap_or_else(|| {
            panic!(
                "no focus outline after clicking {name} at {:?}, size {:?}",
                editor.absolute_position(),
                editor.size()
            )
        });
    let (p, r) = (page.absolute_position(), ring.absolute_position());
    println!(
        "{scene}: editor {:?} at {:?}, ring {:?} {:?}, page {:?} {:?}",
        editor.size(),
        editor.absolute_position(),
        r,
        ring.size(),
        p,
        page.size()
    );
    let row = id(w, &name.replace("instruction-editor", "instruction-row"));
    println!(
        "{scene}: row {:?} at {:?}",
        row.size(),
        row.absolute_position()
    );
    let fits = |ring: &ElementHandle| {
        let r = ring.absolute_position();
        r.y >= p.y - 1. && r.y + ring.size().height <= p.y + page.size().height + 1.
    };
    if !fits(&ring) {
        println!(
            "{scene}: ring clipped by the viewport after focus (row {:?} vs page {:?}), scrolling",
            row.size(),
            page.size()
        );
        let mut delta = 40.;
        let mut last_down = true;
        for _ in 0..200 {
            if fits(&ring) {
                break;
            }
            let r = ring.absolute_position();
            let down = r.y + ring.size().height > p.y + page.size().height + 1.;
            if down != last_down {
                delta = f32::max(delta / 2., 1.);
            }
            last_down = down;
            wheel(w, &page, if down { -delta } else { delta });
        }
    }
    let r = ring.absolute_position();
    println!("{scene}: ring after scroll {:?} {:?}", r, ring.size());
    assert!(
        fits(&ring),
        "ring cannot be brought fully into the viewport"
    );
    assert!(r.x >= p.x - 1. && r.x + ring.size().width <= p.x + page.size().width + 1.);
    let input = editor
        .query_descendants()
        .match_type_name("TextInput")
        .find_first()
        .unwrap();
    editor.set_accessible_selection_offsets(1, 3);
    send_key(w, "X".into());
    let value = editor.accessible_value().unwrap();
    assert!(value.starts_with("aXd"), "{value}");
    // Replacing a selection records the deletion and the insertion as two native undo steps.
    let mut undone = 0;
    while !editor.accessible_value().unwrap().starts_with("abcd") {
        assert!(undone < 2, "undo never restored the selection");
        shortcut(w, "z", false);
        undone += 1;
    }
    println!("undo steps to restore the replaced selection: {undone}");
    for _ in 0..undone {
        shortcut(w, "z", true);
    }
    assert_eq!(editor.accessible_value().unwrap(), value);
    editor.set_accessible_selection_offsets(2, 2);
    send_key(w, Key::Return.into());
    let value = editor.accessible_value().unwrap();
    assert!(value.starts_with("aX\nd"), "{value}");
    w.set_settings_tab(SettingsTab::Audio);
    settle(w);
    w.set_settings_tab(SettingsTab::Ai);
    settle(w);
    assert!(input.is_valid());
    assert_eq!(editor.accessible_value().unwrap(), value);
    // The row's label returns focus without placing the insertion point by pointer.
    let row = reveal(
        w,
        &page,
        &name.replace("instruction-editor", "instruction-row"),
    );
    let label = row
        .query_descendants()
        .match_inherits("KitFieldLabel")
        .find_first()
        .unwrap();
    click(w, &label);
    send_key(w, "Y".into());
    assert!(editor.accessible_value().unwrap().starts_with("aX\nYd"));
}

fn sample_devices(n: usize) -> Vec<AudioInputDevice> {
    (0..n).map(|i| AudioInputDevice {
        uid: format!("fixture-{i}"),
        name: if i < 2 { "Studio · USB".into() } else { format!("Station {i} — microphone de conférence à très longue désignation pour les réunions enregistrées") },
        transport: TransportType::Usb,
        is_default: i == 2,
    }).collect()
}

pub(super) async fn exercise(w: &MainWindow, path: &Path) {
    let mode = std::env::var("SOUFFLE_FIXTURE_PAGES").unwrap();
    if mode == "audio" {
        audio(w, path).await;
        return;
    }
    if mode == "ai-actions" {
        ai_actions(w, path).await;
        return;
    }
    if mode == "audio-actions" {
        audio_actions(w, path).await;
        return;
    }
    let en = w.get_settings_locale() == crate::AppLocale::En;
    let mut settings = AppSettings {
        dictation_polish_enabled: true,
        summary_provider: SummaryProviderChoice::AppleIntelligence,
        ..Default::default()
    };
    let mut status = SummaryProvidersStatus {
        ollama_url: "http://fixture.invalid".into(),
        ollama_available: false,
        apple_intelligence_available: false,
        apple_intelligence_is_stub: false,
        apple_intelligence_unavailable_reason: None,
        recommended_ollama_model: "fixture-long-model".into(),
        models: Vec::new(),
    };
    w.set_settings_tab(SettingsTab::Ai);
    for reason in [
        Reason::DeviceNotEligible,
        Reason::AppleIntelligenceNotEnabled,
        Reason::ModelNotReady,
        Reason::MacosTooOld,
        Reason::Stub,
        Reason::UnsupportedPlatform,
        Reason::Unknown,
    ] {
        status.apple_intelligence_unavailable_reason = Some(reason);
        ia_ui::populate_intelligence(w, &settings, &status);
        settle(w);
        let page = id(w, "SettingsView::ai-page");
        wheel(w, &page, 100000.);
        let labels = alerts(w);
        let expected = if en {
            "Apple Intelligence is unavailable on this device."
        } else {
            "Apple Intelligence n'est pas disponible sur cet appareil."
        };
        assert!(labels.iter().any(|s| s == expected), "{labels:?}");
        let notice = reveal(w, &page, "IntelligenceSection::apple-notice");
        assert!(!notice.accessible_label().unwrap().is_empty());
        wheel(w, &page, -80.);
        let system = items(w, "KitButton").into_iter().find(|e| {
            e.accessible_label().as_deref()
                == Some(if en {
                    "System Settings"
                } else {
                    "Réglages Système"
                })
        });
        assert_eq!(
            system.is_some(),
            reason == Reason::AppleIntelligenceNotEnabled
        );
        if let Some(button) = system {
            let opened = Rc::new(Cell::new(0));
            let observed = opened.clone();
            w.on_settings_open_apple_intelligence_settings_requested(move || {
                observed.set(observed.get() + 1)
            });
            button.invoke_accessible_default_action();
            assert_eq!(opened.get(), 1);
        }
        println!("Apple {reason:?}: {}", notice.accessible_label().unwrap());
    }
    wheel(w, &id(w, "SettingsView::ai-page"), 100000.);
    let service_error = "fixture · ".repeat(18);
    w.set_settings_summary_refresh_error(service_error.as_str().into());
    frame(w, path, "apple");
    for provider in [SummaryProviderChoice::Auto, SummaryProviderChoice::Ollama] {
        settings.summary_provider = provider;
        ia_ui::populate_intelligence(w, &settings, &status);
        settle(w);
        assert!(alerts(w).iter().any(|s| s
            == if en {
                "No compatible Ollama model is installed."
            } else {
                "Aucun modèle Ollama compatible n'est installé."
            }));
    }
    let button = reveal(
        w,
        &id(w, "SettingsView::ai-page"),
        "IntelligenceSection::connection-button",
    );
    let retries = Rc::new(Cell::new(0));
    let observed = retries.clone();
    w.on_settings_summary_providers_retry_requested(move || observed.set(observed.get() + 1));
    button.invoke_accessible_default_action();
    assert_eq!(retries.get(), 1);
    w.set_settings_ollama_checking(true);
    settle(w);
    assert_eq!(button.accessible_enabled(), Some(false));
    w.set_settings_ollama_checking(false);
    status.ollama_available = true;
    ia_ui::populate_intelligence(w, &settings, &status);
    assert!(w.get_settings_show_ollama_setup());
    let download_area = reveal(
        w,
        &id(w, "SettingsView::ai-page"),
        "IntelligenceSection::download-actions",
    );
    let download = download_area
        .query_descendants()
        .match_inherits("KitButton")
        .find_first()
        .unwrap();
    let requests = Rc::new(Cell::new(0));
    let observed = requests.clone();
    w.on_settings_download_recommended_ollama_model_requested(move || {
        observed.set(observed.get() + 1)
    });
    click(w, &download);
    assert_eq!(requests.get(), 1);
    w.set_settings_ollama_pulling(true);
    w.set_settings_ollama_pull_error(service_error.as_str().into());
    settle(w);
    assert_eq!(button.accessible_enabled(), Some(false));
    for progress in ["", "128 / 256 MB"] {
        w.set_settings_ollama_pull_status(progress.into());
        settle(w);
    }
    w.set_settings_ollama_pulling(false);
    status.apple_intelligence_available = true;
    status.models.push(SummaryModelDescriptor {
        id: "fixture".into(),
        label: "Ready".into(),
        provider: SummaryProviderKind::Ollama,
        can_summarize: true,
    });
    ia_ui::populate_intelligence(w, &settings, &status);
    assert!(!w.get_settings_summary_provider_unavailable());
    assert!(w.get_settings_summary_model_picker_visible());
    w.set_settings_summary_refresh_error("".into());
    let prompt = format!(
        "abcd\n{}",
        "Instructions détaillées pour la fixture. ".repeat(24)
    );
    settings.dictation_polish_templates[0]
        .prompt
        .clone_from(&prompt);
    settings.dictation_polish_template_id = settings.dictation_polish_templates[0].id.clone();
    ia_ui::populate_dictation_polish(w, &settings, true);
    settings.summary_templates.push(SummaryTemplate {
        id: "custom".into(),
        name: "Rapport de la réunion – version de travail".into(),
        prompt: prompt.clone(),
    });
    ia_ui::populate_summary_templates(w, &settings, "custom");
    let page = id(w, "SettingsView::ai-page");
    reveal(w, &page, "SummaryTemplatesSection::name-editor");
    assert_widths(w);
    let adds = Rc::new(Cell::new(0));
    let observed = adds.clone();
    w.on_settings_summary_template_add_requested(move |_| observed.set(observed.get() + 1));
    editor(
        w,
        "DictationPolishSection::instruction-editor",
        path,
        "polish",
    );
    editor(
        w,
        "SummaryTemplatesSection::instruction-editor",
        path,
        "template",
    );
    assert_eq!(adds.get(), 0);
    settings.summary_templates.retain(|t| t.id != "custom");
    ia_ui::populate_summary_templates(w, &settings, "custom");
    assert_eq!(
        w.get_settings_editing_summary_template_id(),
        settings.default_summary_template_id.as_str()
    );
}

/// Presses an option of an open picker, runs `held`, then releases. The
/// testing backend reports the items of a popup that opens upward relative to
/// that popup, so its origin is rebuilt from the trigger it sits above.
fn click_option(
    w: &MainWindow,
    trigger: &ElementHandle,
    option: &ElementHandle,
    held: impl FnOnce(),
) {
    use slint::platform::{PointerEventButton, WindowEvent};
    let surface = ElementQuery::from_root(w)
        .match_inherits("KitSelectSurface")
        .find_first()
        .expect("open picker surface");
    let origin = surface.absolute_position();
    let local = option.absolute_position();
    let size = option.size();
    let anchor = trigger.absolute_position();
    let popup_relative = origin.x == 0. && origin.y == 0.;
    let gap = 4.;
    let (x, y) = if popup_relative {
        (
            anchor.x + local.x,
            anchor.y - gap - surface.size().height + local.y,
        )
    } else {
        (local.x, local.y)
    };
    let position = slint::LogicalPosition::new(x + size.width / 2., y + size.height / 2.);
    let button = PointerEventButton::Left;
    w.window()
        .dispatch_event(WindowEvent::PointerMoved { position });
    w.window()
        .dispatch_event(WindowEvent::PointerPressed { position, button });
    settle(w);
    held();
    settle(w);
    w.window()
        .dispatch_event(WindowEvent::PointerReleased { position, button });
    settle(w);
}

fn clamshell_popup(w: &MainWindow, page: &ElementHandle) {
    let devices = sample_devices(3);
    audio_ui::populate_clamshell_picker(w, &devices, Some("fixture-0"));
    // The clamshell choice sits in the collapsed "Avancé" block.
    let advanced = reveal(w, page, "KitCollapse::trigger");
    assert_eq!(advanced.accessible_expanded(), Some(false));
    advanced.invoke_accessible_default_action();
    settle(w);
    assert_eq!(advanced.accessible_expanded(), Some(true));
    let id = "AudioSection::clamshell-picker";
    let picker = reveal(w, page, id);
    let trigger = picker
        .query_descendants()
        .match_inherits("KitSelectTrigger")
        .find_first()
        .unwrap();
    let choices = Rc::new(std::cell::RefCell::new(Vec::new()));
    let accepted = choices.clone();
    let weak = w.as_weak();
    w.on_settings_clamshell_device_changed(move |uid| {
        weak.unwrap().set_settings_clamshell_device_id(uid.clone());
        accepted.borrow_mut().push(uid);
    });
    let rows = || {
        ElementQuery::from_root(w)
            .match_inherits("KitOption")
            .match_accessible_label("Studio · USB")
            .find_all()
    };
    println!(
        "{id}: picker {:?} {:?} before click, trigger {:?} {:?}",
        picker.absolute_position(),
        picker.size(),
        trigger.absolute_position(),
        trigger.size()
    );
    click(w, &trigger);
    println!(
        "{id}: trigger {:?} {:?}, page {:?} {:?}, expanded {:?}, rows {}",
        trigger.absolute_position(),
        trigger.size(),
        page.absolute_position(),
        page.size(),
        trigger.accessible_expanded(),
        rows().len()
    );
    assert_eq!(rows().len(), 2);
    click_option(w, &trigger, &rows()[1], || {});
    assert_eq!(
        choices.borrow().as_slice(),
        &[slint::SharedString::from("fixture-1")]
    );
    click(w, &trigger);
    let mut refreshed = devices;
    refreshed.swap(0, 1);
    click_option(w, &trigger, &rows()[0], || {
        audio_ui::populate_clamshell_picker(w, &refreshed, Some("fixture-1"));
    });
    assert_eq!(choices.borrow().len(), 1, "held choice after reorder");
    if trigger.accessible_expanded() == Some(true) {
        send_key(w, Key::Escape.into());
    }
    println!("{id}: second homonym retained by UID, held replacement rejected");
}

async fn audio(w: &MainWindow, path: &Path) {
    w.set_settings_tab(SettingsTab::Audio);
    w.set_settings_is_laptop(true);
    w.set_settings_system_audio_supported(true);
    w.set_settings_meeting_autostop_enabled(true);
    w.set_settings_sample_rate_label("96 kHz".into());
    w.set_settings_sample_rate_high(true);
    settle(w);
    let page = id(w, "SettingsView::audio-page");
    clamshell_popup(w, &page);
    for n in [0, 1, 100, 1000] {
        let devices = sample_devices(n);
        audio_ui::populate_clamshell_picker(w, &devices, Some("fixture-1"));
        audio_ui::populate_microphones(
            w,
            &microphone_list::build_microphone_list(&devices, &InputPriority::default()),
        );
        settle(w);
        if n > 0 {
            reveal(
                w,
                &id(w, "SettingsView::audio-page"),
                "MicrophoneSection::priority-list",
            );
        }
        let rows = items(w, "MicrophonePriorityRow");
        println!("{n} microphones / {} visible", rows.len());
        if n > 0 {
            let list = id(w, "MicrophoneSection::priority-list");
            println!(
                "list {:?}, first row {:?}",
                list.size(),
                rows.first().map(ElementHandle::size)
            );
            assert!(!rows.is_empty() && rows.len() < 32);
        } else {
            assert!(rows.is_empty());
        }
        if n == 1 {
            frame(w, path, "audio");
        }
    }
    let list = id(w, "MicrophoneSection::priority-list");
    let page = id(w, "SettingsView::audio-page");
    for _ in 0..40 {
        if list.absolute_position().y + list.size().height / 2.
            < page.absolute_position().y + page.size().height - 10.
        {
            break;
        }
        page.scroll(0., -160.);
        settle(w);
    }
    frame(w, path, "priority");
    for step in 0..40 {
        list.scroll(0., -100000.);
        settle(w);
        if list.absolute_position().y + list.size().height
            > page.absolute_position().y + page.size().height
        {
            wheel(w, &page, -60.);
        }
        println!(
            "scroll {step}: {:?}",
            items(w, "MicrophonePriorityRow")
                .iter()
                .map(ElementHandle::accessible_item_index)
                .collect::<Vec<_>>()
        );
        if items(w, "MicrophonePriorityRow")
            .iter()
            .any(|e| e.accessible_item_index() == Some(999))
        {
            break;
        }
    }
    let mounted: Vec<_> = items(w, "MicrophonePriorityRow")
        .iter()
        .map(ElementHandle::accessible_item_index)
        .collect();
    println!("end of list {mounted:?}");
    assert!(mounted.contains(&Some(999)));
    frame(w, path, "last");
    advanced_block_keyboard(w, &page);
}

/// Runs last: once the block is closed, Space lands on whatever Tab reached.
fn advanced_block_keyboard(w: &MainWindow, page: &ElementHandle) {
    let advanced = reveal(w, page, "KitCollapse::trigger");
    assert_eq!(advanced.accessible_expanded(), Some(true));
    let trigger = reveal(w, page, "AudioSection::clamshell-picker")
        .query_descendants()
        .match_inherits("KitSelectTrigger")
        .find_first()
        .unwrap();
    click(w, &trigger);
    send_key(w, Key::Escape.into());
    send_key(w, " ".into());
    assert_eq!(
        trigger.accessible_expanded(),
        Some(true),
        "Space opens the focused picker"
    );
    send_key(w, Key::Escape.into());
    send_key(w, Key::Backtab.into());
    send_key(w, " ".into());
    assert_eq!(
        advanced.accessible_expanded(),
        Some(false),
        "Shift+Tab reaches the block header"
    );
    send_key(w, Key::Tab.into());
    send_key(w, " ".into());
    assert_ne!(
        trigger.accessible_expanded(),
        Some(true),
        "a closed block keeps its picker out of the focus order"
    );
    println!("advanced block: closed, its picker is out of the focus order");
}

fn press_at(w: &MainWindow, item: &ElementHandle) -> slint::LogicalPosition {
    use slint::platform::{PointerEventButton, WindowEvent};
    let p = item.absolute_position();
    let size = item.size();
    let position = slint::LogicalPosition::new(p.x + size.width / 2., p.y + size.height / 2.);
    w.window()
        .dispatch_event(WindowEvent::PointerMoved { position });
    w.window().dispatch_event(WindowEvent::PointerPressed {
        position,
        button: PointerEventButton::Left,
    });
    settle(w);
    position
}

fn release_at(w: &MainWindow, position: slint::LogicalPosition) {
    use slint::platform::{PointerEventButton, WindowEvent};
    w.window().dispatch_event(WindowEvent::PointerReleased {
        position,
        button: PointerEventButton::Left,
    });
    settle(w);
}

fn microphone(uid: &str, connected: bool, first: bool, last: bool) -> crate::MicrophoneRow {
    crate::MicrophoneRow {
        uid: uid.into(),
        name: format!("Micro {uid} à très longue désignation pour vérifier le retour à la ligne")
            .into(),
        transport_label: microphone_list::transport_label(TransportType::Usb).into(),
        is_default: false,
        connected,
        last_seen_age_seconds: if connected { -1 } else { 7200 },
        is_first: first,
        is_last: last,
    }
}

type Log = Rc<std::cell::RefCell<Vec<String>>>;

async fn audio_actions(w: &MainWindow, path: &Path) {
    let en = w.get_settings_locale() == crate::AppLocale::En;
    w.set_settings_tab(SettingsTab::Audio);
    w.set_settings_is_laptop(true);
    w.set_settings_system_audio_supported(true);
    w.set_settings_meeting_autostop_enabled(true);
    w.set_settings_sample_rate_label("96 kHz".into());
    w.set_settings_sample_rate_high(true);
    let log: Log = Default::default();
    macro_rules! record {
        ($setter:ident, $name:literal) => {{
            let log = log.clone();
            w.$setter(move |value| log.borrow_mut().push(format!("{} {value:?}", $name)));
        }};
    }
    {
        let l1 = log.clone();
        w.on_settings_move_device_requested(move |uid, d| {
            l1.borrow_mut().push(format!("move {uid} {d}"))
        });
        let l3 = log.clone();
        w.on_settings_remove_device_requested(move |uid| {
            l3.borrow_mut().push(format!("remove {uid}"))
        });
        let l5 = log.clone();
        w.on_settings_reset_sample_rate_requested(move || {
            l5.borrow_mut().push("reset-rate".into())
        });
    }
    record!(on_settings_capture_system_audio_changed, "system-audio");
    record!(on_settings_meeting_autostop_enabled_changed, "autostop");
    record!(on_settings_vad_enabled_changed, "vad");
    record!(on_settings_filler_removal_changed, "filler");
    record!(on_settings_stutter_collapse_changed, "stutter");
    record!(on_settings_dictionary_correction_changed, "dictionary");

    let model = Rc::new(slint::VecModel::from(vec![
        microphone("a", true, true, false),
        microphone("b", true, false, false),
        microphone("c", false, false, true),
    ]));
    w.set_settings_microphones(model.clone().into());
    settle(w);
    let page = id(w, "SettingsView::audio-page");
    reveal(w, &page, "MicrophoneSection::priority-list");
    // The list is taller than the window at large text scales, and the framework only
    // keeps the visible part of it instantiated, so a row is scrolled into the page and
    // list viewports before it is queried.
    let list = id(w, "MicrophoneSection::priority-list");
    let row = |index: i32| {
        for _ in 0..120 {
            let found = items(w, "MicrophonePriorityRow")
                .into_iter()
                .find(|e| e.accessible_item_index() == Some(index as usize));
            let Some(found) = found else {
                let list_top = list.absolute_position().y;
                let page_top = page.absolute_position().y;
                if list_top + list.size().height < page_top + 40. {
                    wheel(w, &page, 30.);
                } else if list_top > page_top + page.size().height - 40. {
                    wheel(w, &page, -30.);
                } else {
                    list.scroll(0., if index == 0 { 40. } else { -40. });
                    settle(w);
                }
                continue;
            };
            let (top, height) = (found.absolute_position().y, found.size().height);
            let page_top = page.absolute_position().y;
            let page_bottom = page_top + page.size().height - 8.;
            let list_top = list.absolute_position().y;
            let list_bottom = list_top + list.size().height;
            let inside = |from: f32, to: f32| {
                if height < to - from {
                    top >= from && top + height <= to
                } else {
                    top + height / 2. > from && top + height / 2. < to
                }
            };
            if !inside(page_top, page_bottom) {
                wheel(w, &page, if top < page_top { 30. } else { -30. });
            } else if !inside(list_top, list_bottom) {
                list.scroll(0., if top < list_top { 40. } else { -40. });
                settle(w);
            } else {
                return found;
            }
        }
        panic!(
            "row {index} cannot be brought into view: mounted {:?}, list {:?} at {:?}, page {:?} at {:?}",
            items(w, "MicrophonePriorityRow")
                .iter()
                .map(|e| (e.accessible_item_index(), e.size(), e.absolute_position()))
                .collect::<Vec<_>>(),
            list.size(),
            list.absolute_position(),
            page.size(),
            page.absolute_position()
        )
    };
    let buttons = |r: &ElementHandle| {
        r.query_descendants()
            .match_inherits("MicrophoneActionButton")
            .find_all()
    };
    frame(w, path, "actions");
    assert_eq!(buttons(&row(0)).len(), 2);
    assert_eq!(
        buttons(&row(2)).len(),
        3,
        "a disconnected device can be forgotten"
    );
    assert_eq!(
        buttons(&row(0))[0].accessible_enabled(),
        Some(false),
        "first row cannot move up"
    );
    assert_eq!(
        buttons(&row(2))[1].accessible_enabled(),
        Some(false),
        "last row cannot move down"
    );
    assert_eq!(buttons(&row(1))[0].accessible_enabled(), Some(true));
    assert_eq!(buttons(&row(1))[1].accessible_enabled(), Some(true));
    buttons(&row(1))[0].invoke_accessible_default_action();
    buttons(&row(1))[1].invoke_accessible_default_action();
    buttons(&row(2))[2].invoke_accessible_default_action();
    settle(w);
    assert_eq!(
        log.borrow().as_slice(),
        ["move b -1", "move b 1", "remove c"]
    );
    log.borrow_mut().clear();

    let target = buttons(&row(1))[0].clone();
    let at = press_at(w, &target);
    let mut replaced = microphone("a", true, false, false);
    replaced.name = "Remplaçant".into();
    model.set_row_data(1, replaced);
    settle(w);
    release_at(w, at);
    assert!(
        log.borrow().is_empty(),
        "a rebound delegate must not act on the new device: {:?}",
        log.borrow()
    );
    model.set_row_data(1, microphone("b", true, false, false));
    settle(w);
    println!("rebound delegates: no gesture reaches the replacement device");

    reveal(w, &page, "MicrophoneSection::rate-alert");
    let rate = items(w, "KitButton").into_iter().find(|e| {
        e.accessible_label().as_deref()
            == Some(if en {
                "Reset to 48 kHz"
            } else {
                "Réinitialiser à 48 kHz"
            })
    });
    let rate = rate.expect("rate correction offered");
    rate.invoke_accessible_default_action();
    assert_eq!(log.borrow().as_slice(), ["reset-rate"]);
    log.borrow_mut().clear();
    w.set_settings_resetting_sample_rate(true);
    settle(w);
    assert_eq!(
        rate.accessible_enabled(),
        Some(false),
        "no second request while resetting"
    );
    rate.invoke_accessible_default_action();
    assert!(log.borrow().is_empty());
    w.set_settings_resetting_sample_rate(false);
    w.set_settings_sample_rate_high(false);
    settle(w);

    for (name, setter) in [
        ("system-audio", "AudioSection::system-audio-switch"),
        ("autostop", "AudioSection::autostop-switch"),
        ("vad", "AudioSection::vad-switch"),
        ("filler", "AudioSection::filler-switch"),
        ("stutter", "AudioSection::stutter-switch"),
        ("dictionary", "AudioSection::dictionary-switch"),
    ] {
        let control = reveal(w, &page, setter);
        let before = control.accessible_checked();
        control.invoke_accessible_default_action();
        let expected = format!("{name} {:?}", before != Some(true));
        assert_eq!(log.borrow().as_slice(), [expected.as_str()], "{name}");
        assert_eq!(
            control.accessible_checked(),
            before,
            "{name} stays controlled"
        );
        log.borrow_mut().clear();
    }
    let present = |name: &str| ElementHandle::find_by_element_id(w, name).next().is_some();
    for enabled in [true, false, true] {
        w.set_settings_meeting_autostop_enabled(enabled);
        settle(w);
        // Only the part of the page inside the viewport is instantiated, so the rows
        // are scrolled into view before their presence is read. The absence case is
        // read with the neighbouring switches mounted.
        if enabled {
            reveal(w, &page, "AudioSection::delay-picker");
            reveal(w, &page, "AudioSection::duration-picker");
        } else {
            reveal(w, &page, "AudioSection::vad-switch");
            assert!(
                present("AudioSection::autostop-switch"),
                "the neighbourhood of the hidden rows is mounted"
            );
        }
        assert_eq!(
            (
                present("AudioSection::delay-picker"),
                present("AudioSection::duration-picker")
            ),
            (enabled, enabled),
            "delay and maximum duration follow auto-stop"
        );
    }
    frame(w, path, "controls");
    println!(
        "audio actions: {} gestures observed with exact UID, bounds, ownership and conditional rows",
        17
    );
}

async fn pick(w: &MainWindow, page: &ElementHandle, picker: &str, index: usize) {
    use slint::platform::PointerEventButton;
    let picker = reveal(w, page, picker);
    let trigger = picker
        .query_descendants()
        .match_inherits("KitSelectTrigger")
        .find_first()
        .unwrap();
    click(w, &trigger);
    assert_eq!(trigger.accessible_expanded(), Some(true), "popup opened");
    let options: Vec<_> = items(w, "KitOption").into_iter().skip(6).collect();
    if options.len() > index {
        options[index].single_click(PointerEventButton::Left).await;
    } else {
        for _ in 0..=index {
            send_key(w, Key::DownArrow.into());
        }
        send_key(w, Key::Return.into());
    }
    settle(w);
}

fn type_text(w: &MainWindow, editor: &ElementHandle, text: &str) {
    click(w, editor);
    let length = editor
        .accessible_value()
        .map(|v| v.len() as i32)
        .unwrap_or(0);
    editor.set_accessible_selection_offsets(0, length);
    if text.is_empty() {
        send_key(w, Key::Backspace.into());
    }
    for c in text.chars() {
        send_key(w, c.to_string().into());
    }
}

async fn ai_actions(w: &MainWindow, path: &Path) {
    let en = w.get_settings_locale() == crate::AppLocale::En;
    w.set_settings_tab(SettingsTab::Ai);
    let log: Log = Default::default();
    {
        let l = log.clone();
        w.on_settings_ollama_url_changed(move |v| l.borrow_mut().push(format!("url {v:?}")));
        let l = log.clone();
        w.on_settings_ollama_model_changed(move |v| l.borrow_mut().push(format!("model {v}")));
        let l = log.clone();
        w.on_settings_summary_template_default_changed(move |v| {
            l.borrow_mut().push(format!("default {v}"))
        });
        let l = log.clone();
        w.on_settings_summary_template_edit_target_changed(move |v| {
            l.borrow_mut().push(format!("target {v}"))
        });
        let l = log.clone();
        w.on_settings_summary_template_delete_requested(move || {
            l.borrow_mut().push("delete".into())
        });
        let l = log.clone();
        w.on_settings_summary_template_name_changed(move |v| {
            l.borrow_mut().push(format!("name {v:?}"))
        });
        let l = log.clone();
        w.on_settings_summary_template_add_requested(move |v| {
            l.borrow_mut().push(format!("add {v:?}"))
        });
        let l = log.clone();
        w.on_settings_dictation_polish_enabled_changed(move |v| {
            l.borrow_mut().push(format!("polish {v}"))
        });
        let l = log.clone();
        w.on_settings_dictation_polish_template_changed(move |v| {
            l.borrow_mut().push(format!("polish-template {v}"))
        });
    }
    let mut settings = AppSettings {
        dictation_polish_enabled: true,
        summary_provider: SummaryProviderChoice::Ollama,
        ..Default::default()
    };
    let model = |id: &str| SummaryModelDescriptor {
        id: id.into(),
        label: "Modèle homonyme".into(),
        provider: SummaryProviderKind::Ollama,
        can_summarize: true,
    };
    let status = SummaryProvidersStatus {
        ollama_url: "http://fixture.invalid".into(),
        ollama_available: true,
        apple_intelligence_available: false,
        apple_intelligence_is_stub: false,
        apple_intelligence_unavailable_reason: None,
        recommended_ollama_model: "fixture".into(),
        models: vec![model("m1"), model("m2")],
    };
    ia_ui::populate_intelligence(w, &settings, &status);
    ia_ui::populate_dictation_polish(w, &settings, true);
    settings.summary_templates.push(SummaryTemplate {
        id: "custom".into(),
        name: "Compte rendu".into(),
        prompt: "Résume.".into(),
    });
    ia_ui::populate_summary_templates(w, &settings, "custom");
    settle(w);
    let page = id(w, "SettingsView::ai-page");

    for draft in [
        "http://localhost:11434",
        "https://ollama.example.com:8443/",
        "ollama",
        "  http://x  ",
        "",
    ] {
        let editor = reveal(w, &page, "IntelligenceSection::url-editor");
        log.borrow_mut().clear();
        type_text(w, &editor, draft);
        let last = log.borrow().last().cloned().unwrap_or_default();
        assert_eq!(
            last,
            format!("url {draft:?}"),
            "the host receives exactly what was typed"
        );
        println!(
            "url draft {draft:?}: {} edits forwarded",
            log.borrow().len()
        );
    }
    log.borrow_mut().clear();

    pick(w, &page, "IntelligenceSection::model-picker", 1).await;
    assert_eq!(
        log.borrow().as_slice(),
        ["model m2"],
        "homonymous labels keep distinct ids"
    );
    log.borrow_mut().clear();

    let switch = reveal(w, &page, "DictationPolishSection::polish-switch");
    switch.invoke_accessible_default_action();
    assert_eq!(log.borrow().as_slice(), ["polish false"]);
    assert_eq!(
        switch.accessible_checked(),
        Some(true),
        "the host owns the value"
    );
    log.borrow_mut().clear();

    pick(w, &page, "SummaryTemplatesSection::default-picker", 0).await;
    assert_eq!(log.borrow().len(), 1);
    assert!(log.borrow()[0].starts_with("default "));
    log.borrow_mut().clear();
    pick(w, &page, "SummaryTemplatesSection::template-picker", 0).await;
    assert!(log.borrow()[0].starts_with("target "));
    log.borrow_mut().clear();

    let delete = items(w, "KitButton")
        .into_iter()
        .find(|e| e.accessible_label().as_deref() == Some(if en { "Delete" } else { "Supprimer" }));
    reveal(w, &page, "SummaryTemplatesSection::template-picker");
    let delete = delete.expect("a custom template can be deleted");
    delete.invoke_accessible_default_action();
    assert_eq!(log.borrow().as_slice(), ["delete"]);
    log.borrow_mut().clear();
    w.set_settings_editing_summary_template_is_builtin(true);
    settle(w);
    assert!(
        items(w, "KitButton")
            .iter()
            .all(|e| e.accessible_label().as_deref()
                != Some(if en { "Delete" } else { "Supprimer" })),
        "built-in templates have no delete action"
    );
    assert!(
        ElementHandle::find_by_element_id(w, "SummaryTemplatesSection::name-editor")
            .next()
            .is_none(),
        "built-in names are not editable"
    );
    w.set_settings_editing_summary_template_is_builtin(false);
    settle(w);

    let name = reveal(w, &page, "SummaryTemplatesSection::name-editor");
    log.borrow_mut().clear();
    type_text(w, &name, "Nouveau nom");
    for field in items(w, "KitTextField") {
        assert!(
            field.size().width > 40.,
            "text field collapsed: {:?} {:?}",
            field.accessible_label(),
            field.size()
        );
    }
    println!(
        "name editor at {:?} {:?}, page {:?} {:?}",
        name.absolute_position(),
        name.size(),
        page.absolute_position(),
        page.size()
    );
    println!(
        "name editor value {:?} log {:?} focus {:?}",
        name.accessible_value(),
        log.borrow(),
        name.accessible_label()
    );
    assert_eq!(log.borrow().last().unwrap(), "name \"Nouveau nom\"");
    log.borrow_mut().clear();

    let add = reveal(w, &page, "SummaryTemplatesSection::add-button");
    assert_eq!(
        add.accessible_enabled(),
        Some(false),
        "an empty draft cannot be added"
    );
    add.invoke_accessible_default_action();
    assert!(log.borrow().is_empty());
    let field = items(w, "KitTextField").into_iter().last().unwrap();
    type_text(w, &field, "  Réunion produit  ");
    send_key(w, Key::Return.into());
    assert!(
        log.borrow().is_empty(),
        "Enter in the name field does not submit implicitly"
    );
    assert_eq!(add.accessible_enabled(), Some(true));
    add.invoke_accessible_default_action();
    assert_eq!(
        log.borrow().as_slice(),
        ["add \"  Réunion produit  \""],
        "the host receives the original text"
    );
    frame(w, path, "ai-actions");
    println!(
        "ai actions: URL drafts, homonymous model ids, polish ownership, template target/default/delete/add observed"
    );
}
