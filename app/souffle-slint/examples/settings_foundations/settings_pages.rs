//! Two retained settings pages, driven with synthetic provider and peripheral snapshots.
use super::{
    audio_ui, calendar_selection, click, ia_ui, microphone_list, save_frame, send_key, settle,
    shortcut_capture,
};
use crate::{MainWindow, SettingsTab, ShortcutField, ShortcutRejection};
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
    let found = ElementHandle::find_by_element_id(w, name).next();
    panic!(
        "scroll never revealed {name}: element {:?} {:?}, page {:?} {:?}",
        found.as_ref().map(|e| e.absolute_position()),
        found.as_ref().map(|e| e.size()),
        page.absolute_position(),
        page.size()
    );
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
        name: if i < 2 { "Studio · USB".into() } else { format!("Station {i} - microphone de conférence à très longue désignation pour les réunions enregistrées") },
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
    if mode == "interface" {
        interface(w, path).await;
        return;
    }
    if mode == "meetings" {
        meetings(w, path).await;
        return;
    }
    if mode == "meetings-scale" {
        meetings_scale(w, path).await;
        return;
    }
    if mode == "system" {
        super::system_page::system(w, path).await;
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

async fn device_popup(w: &MainWindow, page: &ElementHandle, clamshell: bool) {
    use slint::platform::PointerEventButton;
    let devices = sample_devices(3);
    audio_ui::populate_device_pickers(w, &devices, "fixture-0", Some("fixture-0"));
    let id = if clamshell {
        "AudioSection::clamshell-picker"
    } else {
        "MicrophoneSection::device-picker"
    };
    let picker = reveal(w, page, id);
    let trigger = picker
        .query_descendants()
        .match_inherits("KitSelectTrigger")
        .find_first()
        .unwrap();
    let choices = Rc::new(std::cell::RefCell::new(Vec::new()));
    let accepted = choices.clone();
    let weak = w.as_weak();
    let choose = move |uid: slint::SharedString| {
        let window = weak.unwrap();
        if clamshell {
            window.set_settings_clamshell_device_id(uid.clone());
        } else {
            window.set_settings_selected_device_id(uid.clone());
        }
        accepted.borrow_mut().push(uid);
    };
    if clamshell {
        w.on_settings_clamshell_device_changed(choose);
    } else {
        w.on_settings_device_changed(choose);
    }
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
    rows()[1].single_click(PointerEventButton::Left).await;
    settle(w);
    assert_eq!(
        choices.borrow().as_slice(),
        &[slint::SharedString::from("fixture-1")]
    );
    click(w, &trigger);
    let weak = w.as_weak();
    let mut refreshed = devices;
    refreshed.swap(0, 1);
    slint::Timer::single_shot(std::time::Duration::from_millis(10), move || {
        audio_ui::populate_device_pickers(
            &weak.unwrap(),
            &refreshed,
            "fixture-1",
            Some("fixture-1"),
        );
    });
    rows()[0].single_click(PointerEventButton::Left).await;
    settle(w);
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
    device_popup(w, &page, false).await;
    device_popup(w, &page, true).await;
    for n in [0, 1, 100, 1000] {
        let devices = sample_devices(n);
        audio_ui::populate_device_pickers(w, &devices, "fixture-1", Some("fixture-1"));
        assert_eq!(w.get_settings_pin_unavailable(), n < 2);
        audio_ui::populate_microphones(
            w,
            &microphone_list::build_microphone_list(&devices, &InputPriority::default()),
        );
        settle(w);
        if n == 1 {
            frame(w, path, "fallback");
        }
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
        hidden: false,
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
    w.set_settings_has_disconnected_devices(true);
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
        let l2 = log.clone();
        w.on_settings_toggle_hidden_requested(move |uid, hidden| {
            l2.borrow_mut().push(format!("hide {uid} {hidden}"))
        });
        let l3 = log.clone();
        w.on_settings_remove_device_requested(move |uid| {
            l3.borrow_mut().push(format!("remove {uid}"))
        });
        let l4 = log.clone();
        w.on_settings_reset_devices_requested(move || l4.borrow_mut().push("reset-list".into()));
        let l5 = log.clone();
        w.on_settings_reset_sample_rate_requested(move || {
            l5.borrow_mut().push("reset-rate".into())
        });
        let l6 = log.clone();
        w.on_settings_refresh_devices_requested(move || l6.borrow_mut().push("refresh".into()));
    }
    record!(on_settings_allow_bluetooth_mic_changed, "bluetooth");
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
    let switch = |r: &ElementHandle| {
        r.query_descendants()
            .match_inherits("KitSwitch")
            .find_first()
            .unwrap()
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
    switch(&row(1)).invoke_accessible_default_action();
    buttons(&row(2))[2].invoke_accessible_default_action();
    settle(w);
    assert_eq!(
        log.borrow().as_slice(),
        ["move b -1", "move b 1", "hide b true", "remove c"]
    );
    log.borrow_mut().clear();

    for hold_switch in [false, true] {
        let target = if hold_switch {
            switch(&row(1))
        } else {
            buttons(&row(1))[0].clone()
        };
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
    }
    println!("rebound delegates: no gesture reaches the replacement device");

    reveal(w, &page, "MicrophoneSection::device-picker");
    let reset = items(w, "KitButton").into_iter().find(|e| {
        e.accessible_label().as_deref()
            == Some(if en {
                "Reset list"
            } else {
                "Réinitialiser la liste"
            })
    });
    let reset = reset.expect("reset list is offered while a device is disconnected");
    reset.invoke_accessible_default_action();
    let refresh = items(w, "KitIconButton").into_iter().find(|e| {
        e.accessible_label().as_deref()
            == Some(if en {
                "Refresh microphones"
            } else {
                "Actualiser les microphones"
            })
    });
    refresh
        .expect("refresh button")
        .invoke_accessible_default_action();
    assert_eq!(log.borrow().as_slice(), ["reset-list", "refresh"]);
    log.borrow_mut().clear();
    w.set_settings_has_disconnected_devices(false);
    settle(w);
    assert!(
        !reset.is_valid()
            || items(w, "KitButton")
                .iter()
                .all(|e| e.accessible_label().as_deref()
                    != Some(if en {
                        "Reset list"
                    } else {
                        "Réinitialiser la liste"
                    }))
    );

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

    let bluetooth = reveal(w, &page, "MicrophoneSection::bluetooth-switch");
    assert_eq!(bluetooth.accessible_checked(), Some(false));
    bluetooth.invoke_accessible_default_action();
    assert_eq!(log.borrow().as_slice(), ["bluetooth true"]);
    assert_eq!(
        bluetooth.accessible_checked(),
        Some(false),
        "the host owns the value"
    );
    log.borrow_mut().clear();
    w.set_settings_allow_bluetooth_mic(true);
    settle(w);
    assert_eq!(bluetooth.accessible_checked(), Some(true));

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

fn shortcut_label(value: &str) -> String {
    value
        .replace("CommandOrControl", "\u{2318}")
        .replace("Shift", "\u{21e7}")
        .replace("Alt", "\u{2325}")
        .replace('+', " ")
}

/// The host side of Interface and sounds: the Rust decision comes from the real
/// classifier, persistence is a pair held in memory.
struct Host {
    pair: Rc<std::cell::RefCell<souffle_lib::settings::ShortcutSettings>>,
    log: Log,
}

fn wire_interface_host(w: &MainWindow) -> Host {
    use shortcut_capture::{Modifiers, Target, decide};
    let pair = Rc::new(std::cell::RefCell::new(
        souffle_lib::settings::ShortcutSettings {
            toggle: "CommandOrControl+Shift+Space".into(),
            push_to_talk: "CommandOrControl+Alt+Shift+ArrowLeft".into(),
        },
    ));
    let log: Log = Default::default();
    let target = |field| match field {
        ShortcutField::Toggle => Some(Target::Toggle),
        ShortcutField::Ptt => Some(Target::PushToTalk),
        ShortcutField::None => None,
    };
    let decision = {
        let pair = pair.clone();
        move |field, text: &str, c, s, a, m| {
            let target = target(field);
            let current = pair.borrow().clone();
            target.map_or(Err(shortcut_capture::Rejection::Unavailable), |target| {
                decide(
                    Some(&current),
                    target,
                    text,
                    Modifiers {
                        control: c,
                        shift: s,
                        alt: a,
                        meta: m,
                    },
                )
            })
        }
    };
    let rejection = |reason: Option<shortcut_capture::Rejection>| {
        use shortcut_capture::Rejection;
        match reason {
            None => ShortcutRejection::None,
            Some(Rejection::MissingModifier) => ShortcutRejection::MissingModifier,
            Some(Rejection::Unmappable) => ShortcutRejection::Unmappable,
            Some(Rejection::Unavailable) => ShortcutRejection::Unavailable,
            Some(Rejection::Conflict) => ShortcutRejection::Conflict,
        }
    };
    let project = |w: &MainWindow, pair: &souffle_lib::settings::ShortcutSettings| {
        w.set_settings_toggle_shortcut_label(shortcut_label(&pair.toggle).into());
        w.set_settings_ptt_shortcut_label(shortcut_label(&pair.push_to_talk).into());
    };
    project(w, &pair.borrow());
    let weak = w.as_weak();
    w.on_settings_shortcut_record_requested(move |field| {
        let w = weak.unwrap();
        w.set_settings_recording_field(field);
        w.set_settings_shortcut_error("".into());
        w.set_settings_shortcut_rejection(ShortcutRejection::None);
    });
    let d = decision.clone();
    w.on_settings_shortcut_key_allowed(move |field, text, c, s, a, m| {
        d(field, &text, c, s, a, m).is_ok()
    });
    let (d, weak) = (decision.clone(), w.as_weak());
    w.on_settings_shortcut_rejected(move |field, text, c, s, a, m| {
        let w = weak.unwrap();
        w.set_settings_recording_field(field);
        w.set_settings_shortcut_rejection(rejection(d(field, &text, c, s, a, m).err()));
    });
    for released in [false, true] {
        let (d, weak, pair, log) = (decision.clone(), w.as_weak(), pair.clone(), log.clone());
        let apply = move |field, text: slint::SharedString, c, s, a, m| {
            let w = weak.unwrap();
            if let Ok(value) = d(field, &text, c, s, a, m) {
                match field {
                    ShortcutField::Toggle => pair.borrow_mut().toggle = value.clone(),
                    ShortcutField::Ptt => pair.borrow_mut().push_to_talk = value.clone(),
                    ShortcutField::None => {}
                }
                log.borrow_mut().push(format!("save {field:?} {value:?}"));
                w.set_settings_recording_field(ShortcutField::None);
                w.set_settings_shortcut_rejection(ShortcutRejection::None);
                project(&w, &pair.borrow());
            }
        };
        if released {
            w.on_settings_shortcut_released(apply);
        } else {
            w.on_settings_shortcut_captured(apply);
        }
    }
    let (weak, pair2, log2) = (w.as_weak(), pair.clone(), log.clone());
    w.on_settings_shortcut_cleared(move |field| {
        let w = weak.unwrap();
        match field {
            ShortcutField::Toggle => pair2.borrow_mut().toggle.clear(),
            ShortcutField::Ptt => pair2.borrow_mut().push_to_talk.clear(),
            ShortcutField::None => {}
        }
        log2.borrow_mut().push(format!("clear {field:?}"));
        w.set_settings_recording_field(ShortcutField::None);
        project(&w, &pair2.borrow());
    });
    let weak = w.as_weak();
    w.on_settings_shortcut_cancelled(move || {
        let w = weak.unwrap();
        w.set_settings_recording_field(ShortcutField::None);
        w.set_settings_shortcut_rejection(ShortcutRejection::None);
    });
    let (weak, l) = (w.as_weak(), log.clone());
    w.on_settings_paste_delay_changed(move |v| {
        l.borrow_mut().push(format!("delay {v}"));
        weak.unwrap().set_settings_paste_delay_ms(v);
    });
    let (weak, l) = (w.as_weak(), log.clone());
    w.on_settings_feedback_sounds_volume_changed(move |v| {
        l.borrow_mut().push(format!("volume {v}"));
        weak.unwrap().set_settings_feedback_sounds_volume(v);
    });
    let (weak, l) = (w.as_weak(), log.clone());
    w.on_settings_feedback_sounds_enabled_changed(move |v| {
        l.borrow_mut().push(format!("sounds {v}"));
        weak.unwrap().set_settings_feedback_sounds_enabled(v);
    });
    let l = log.clone();
    w.on_settings_permissions_review_requested(move || l.borrow_mut().push("permissions".into()));
    Host { pair, log }
}

fn focus_ring_fits(w: &MainWindow, page: &ElementHandle, owner: &str, what: &str) {
    let root = id(w, owner);
    let ring = root
        .query_descendants()
        .match_inherits("FocusOutline")
        .find_first()
        .unwrap_or_else(|| panic!("{what}: no focus ring"));
    let (p, r) = (page.absolute_position(), ring.absolute_position());
    let (ps, rs) = (page.size(), ring.size());
    println!("{what}: ring {:?} {:?} in page {:?} {:?}", r, rs, p, ps);
    assert!(
        r.y >= p.y - 1. && r.y + rs.height <= p.y + ps.height + 1.,
        "{what}: ring clipped vertically"
    );
    assert!(
        r.x >= p.x - 1. && r.x + rs.width <= p.x + ps.width + 1.,
        "{what}: ring clipped horizontally"
    );
}

/// Reaches `owner` with Tab from the top of the page, so the framework's own
/// reveal is what keeps the ring inside the scroll, then measures it.
fn tab_to_and_measure(w: &MainWindow, page: &ElementHandle, owner: &str, what: &str) {
    wheel(w, page, 100000.);
    for _ in 0..80 {
        let focused = ElementHandle::find_by_element_id(w, owner)
            .next()
            .is_some_and(|e| {
                e.query_descendants()
                    .match_inherits("FocusOutline")
                    .find_first()
                    .is_some()
            });
        if focused {
            focus_ring_fits(w, page, owner, what);
            return;
        }
        send_key(w, Key::Tab.into());
    }
    panic!("{what}: Tab never reached {owner}");
}

async fn interface(w: &MainWindow, path: &Path) {
    use slint::platform::WindowEvent;
    let en = w.get_settings_locale() == crate::AppLocale::En;
    w.set_settings_tab(SettingsTab::Interface);
    w.set_settings_auto_paste(true);
    w.set_settings_feedback_sounds_enabled(true);
    w.set_settings_feedback_sounds_volume(70);
    w.set_settings_paste_delay_ms(150);
    let options = souffle_lib::settings::SettingsOptions::current();
    w.set_settings_paste_delay_min(options.paste_delay_ms_min as i32);
    w.set_settings_paste_delay_max(options.paste_delay_ms_max as i32);
    w.set_settings_paste_delay_step(options.paste_delay_ms_step as i32);
    w.set_settings_feedback_sounds_volume_min(options.feedback_sounds_volume_min as i32);
    w.set_settings_feedback_sounds_volume_max(options.feedback_sounds_volume_max as i32);
    w.set_settings_feedback_sounds_volume_step(options.feedback_sounds_volume_step as i32);
    w.set_settings_native_tap_warning_visible(true);
    let host = wire_interface_host(w);
    settle(w);
    let page = id(w, "SettingsView::interface-page");
    assert_widths(w);
    frame(w, path, "interface-top");

    // Every focus stop is reached with Tab and its whole ring stays in the scroll.
    for (owner, what) in [
        ("InterfaceSection::toggle-recorder", "toggle recorder"),
        ("InterfaceSection::ptt-recorder", "push-to-talk recorder"),
        ("InterfaceSection::delay-stepper", "delay stepper"),
        ("FeedbackSoundsSection::sounds-switch", "sounds switch"),
        ("FeedbackSoundsSection::volume-slider", "volume slider"),
    ] {
        tab_to_and_measure(w, &page, owner, what);
    }
    // The clear icon sits after the recorder and has its own ring, which the
    // recorder-focused output does not cover; Shift+Tab walks back.
    tab_to_and_measure(
        w,
        &page,
        "InterfaceSection::toggle-recorder",
        "toggle recorder",
    );
    send_key(w, Key::Tab.into());
    focus_ring_fits(
        w,
        &page,
        "InterfaceSection::toggle-recorder",
        "toggle clear icon",
    );
    tab_to_and_measure(
        w,
        &page,
        "FeedbackSoundsSection::volume-slider",
        "volume slider",
    );
    send_key(w, Key::Backtab.into());
    focus_ring_fits(
        w,
        &page,
        "FeedbackSoundsSection::sounds-switch",
        "sounds switch backwards",
    );
    let clear = |field: &str| {
        if en {
            format!("Clear shortcut \u{201c}{field}\u{201d}")
        } else {
            format!("Effacer le raccourci \u{ab} {field} \u{bb}")
        }
    };
    let (toggle_name, ptt_name) = if en {
        ("Toggle recording", "Push to talk")
    } else {
        ("Basculer l'enregistrement", "Appuyer pour parler")
    };
    let button = |name: &str| {
        items(w, "KitButton")
            .into_iter()
            .chain(items(w, "KitIconButton"))
            .find(|e| e.accessible_label().as_deref() == Some(name))
            .unwrap_or_else(|| panic!("no button {name:?}"))
    };
    // The clear icon is a real, translated action reachable by Tab.
    reveal(w, &page, "InterfaceSection::ptt-recorder");
    assert_eq!(
        button(&clear(ptt_name)).accessible_role(),
        Some(i_slint_backend_testing::AccessibleRole::Button)
    );

    // Capture, refusal and retry without a second click.
    reveal(w, &page, "InterfaceSection::toggle-recorder");
    click(w, &button(toggle_name));
    assert_eq!(w.get_settings_recording_field(), ShortcutField::Toggle);
    send_key(w, "d".into());
    assert_eq!(
        w.get_settings_shortcut_rejection(),
        ShortcutRejection::MissingModifier
    );
    assert_eq!(w.get_settings_recording_field(), ShortcutField::Toggle);
    let message = if en {
        "The shortcut must include a modifier key (Cmd, Ctrl, Shift, Alt) or be a function key."
    } else {
        "Le raccourci doit inclure une touche de modification (Cmd, Ctrl, Maj, Alt) ou \u{ea}tre une touche de fonction."
    };
    assert_eq!(
        button(toggle_name).accessible_description().as_deref(),
        Some(message)
    );
    frame(w, path, "interface-rejection");
    shortcut(w, "j", false);
    assert_eq!(host.pair.borrow().toggle, "CommandOrControl+J");
    assert_eq!(w.get_settings_recording_field(), ShortcutField::None);

    // Conflict, then the other binding is untouched.
    reveal(w, &page, "InterfaceSection::ptt-recorder");
    click(w, &button(ptt_name));
    assert_eq!(w.get_settings_recording_field(), ShortcutField::Ptt);
    shortcut(w, "j", false);
    assert_eq!(
        w.get_settings_shortcut_rejection(),
        ShortcutRejection::Conflict
    );
    assert_eq!(w.get_settings_recording_field(), ShortcutField::Ptt);
    send_key(w, Key::Escape.into());
    assert_eq!(w.get_settings_recording_field(), ShortcutField::None);
    assert_eq!(
        host.pair.borrow().push_to_talk,
        "CommandOrControl+Alt+Shift+ArrowLeft"
    );

    // A modifier alone on its release, and the icon clears.
    reveal(w, &page, "InterfaceSection::ptt-recorder");
    click(w, &button(ptt_name));
    assert_eq!(w.get_settings_recording_field(), ShortcutField::Ptt);
    w.window().dispatch_event(WindowEvent::KeyPressed {
        text: Key::Shift.into(),
    });
    settle(w);
    assert!(!host.log.borrow().iter().any(|l| l.contains("ShiftLeft")));
    w.window().dispatch_event(WindowEvent::KeyReleased {
        text: Key::Shift.into(),
    });
    settle(w);
    assert_eq!(host.pair.borrow().push_to_talk, "ShiftLeft");
    click(w, &button(&clear(ptt_name)));
    assert_eq!(host.pair.borrow().push_to_talk, "");
    assert_eq!(w.get_settings_recording_field(), ShortcutField::None);

    // The permission action of the alert.
    reveal(w, &page, "InterfaceSection::permission-alert");
    let review = items(w, "KitButton")
        .into_iter()
        .find(|e| {
            e.accessible_label().as_deref()
                == Some(if en {
                    "Check permissions"
                } else {
                    "V\u{e9}rifier les permissions"
                })
        })
        .expect("permission action");
    review.invoke_accessible_default_action();
    assert!(host.log.borrow().iter().any(|l| l == "permissions"));
    frame(w, path, "interface-alert");

    // Delay: host step, bounds and a value between two steps.
    let more = || {
        items(w, "KitPressable")
            .into_iter()
            .find(|e| {
                e.accessible_label().as_deref() == Some(if en { "Increase" } else { "Augmenter" })
            })
            .expect("increase")
    };
    reveal(w, &page, "InterfaceSection::delay-stepper");
    more().invoke_accessible_default_action();
    settle(w);
    assert_eq!(w.get_settings_paste_delay_ms(), 200);
    w.set_settings_paste_delay_ms(1000);
    settle(w);
    more().invoke_accessible_default_action();
    settle(w);
    assert_eq!(w.get_settings_paste_delay_ms(), 1000);
    w.set_settings_paste_delay_ms(125);
    settle(w);
    more().invoke_accessible_default_action();
    settle(w);
    println!("125 + one step = {}", w.get_settings_paste_delay_ms());
    assert!((126..=1000).contains(&w.get_settings_paste_delay_ms()));

    // Sounds: keyboard steps, extremes, percentage and switch.
    let slider = reveal(w, &page, "FeedbackSoundsSection::volume-slider");
    let row = id(w, "FeedbackSoundsSection::volume-row");
    let label = row
        .query_descendants()
        .match_inherits("KitFieldLabel")
        .find_first()
        .unwrap();
    click(w, &label);
    send_key(w, Key::RightArrow.into());
    assert_eq!(w.get_settings_feedback_sounds_volume(), 71);
    send_key(w, Key::End.into());
    assert_eq!(w.get_settings_feedback_sounds_volume(), 100);
    send_key(w, Key::RightArrow.into());
    assert_eq!(w.get_settings_feedback_sounds_volume(), 100);
    frame(w, path, "interface-volume-max");
    send_key(w, Key::Home.into());
    assert_eq!(w.get_settings_feedback_sounds_volume(), 0);
    let percent = if en { "0 %" } else { "0%" };
    assert!(
        items(w, "KitText")
            .iter()
            .any(|e| e.accessible_label().as_deref() == Some(percent)),
        "visible percentage {percent}"
    );
    assert_eq!(slider.accessible_value().as_deref(), Some("0"));
    let slider = reveal(w, &page, "FeedbackSoundsSection::volume-slider");
    let at = |fraction: f32| {
        slint::LogicalPosition::new(
            slider.absolute_position().x + slider.size().width * fraction,
            slider.absolute_position().y + slider.size().height / 2.,
        )
    };
    let pointer = slint::platform::PointerEventButton::Left;
    let before = host.log.borrow().len();
    w.window()
        .dispatch_event(WindowEvent::PointerMoved { position: at(0.2) });
    w.window().dispatch_event(WindowEvent::PointerPressed {
        position: at(0.2),
        button: pointer,
    });
    w.window()
        .dispatch_event(WindowEvent::PointerMoved { position: at(0.6) });
    send_key(w, Key::Escape.into());
    w.window().dispatch_event(WindowEvent::PointerReleased {
        position: at(0.6),
        button: pointer,
    });
    settle(w);
    assert_eq!(
        host.log.borrow().len(),
        before,
        "a cancelled gesture proposes nothing"
    );
    w.window()
        .dispatch_event(WindowEvent::PointerMoved { position: at(0.6) });
    w.window().dispatch_event(WindowEvent::PointerPressed {
        position: at(0.6),
        button: pointer,
    });
    w.window().dispatch_event(WindowEvent::PointerReleased {
        position: at(0.6),
        button: pointer,
    });
    settle(w);
    let chosen = w.get_settings_feedback_sounds_volume();
    println!("pointer volume {chosen}");
    assert!((50..=70).contains(&chosen));
    let sounds = items(w, "KitSwitch")
        .into_iter()
        .find(|e| {
            e.accessible_label().as_deref()
                == Some(if en {
                    "Enable sounds"
                } else {
                    "Activer les sons"
                })
        })
        .expect("sounds switch");
    sounds.invoke_accessible_default_action();
    assert!(!w.get_settings_feedback_sounds_enabled());
    assert!(
        sounds.is_valid(),
        "the switch stays after the volume row is gone"
    );
    sounds.invoke_accessible_default_action();
    assert!(w.get_settings_feedback_sounds_enabled());
    frame(w, path, "interface-sounds");
    println!(
        "interface and sounds interactions verified: {:?}",
        host.log.borrow()
    );
}

fn calendar(id: &str, title: &str, source: Option<&str>) -> souffle_lib::calendar::CalendarInfo {
    souffle_lib::calendar::CalendarInfo {
        id: id.into(),
        title: title.into(),
        source_title: source.map(str::to_string),
    }
}

/// Three accounts, two homonymous calendars in one of them and a third elsewhere.
fn meeting_calendars() -> Vec<souffle_lib::calendar::CalendarInfo> {
    vec![
        calendar("a1", "Travail", Some("iCloud")),
        calendar("a2", "Travail", Some("iCloud")),
        calendar("g1", "Travail", Some("Google")),
        calendar(
            "n1",
            "Calendrier personnel à très longue désignation pour vérifier le retour à la ligne",
            None,
        ),
    ]
}

/// The host side of Meetings: the real selection rules over an in-memory catalogue.
struct MeetingsHost {
    stored: Rc<std::cell::RefCell<Vec<String>>>,
    log: Log,
}

fn wire_meetings_host(
    w: &MainWindow,
    catalogue: Vec<souffle_lib::calendar::CalendarInfo>,
    granted_by_system_settings: bool,
) -> MeetingsHost {
    use crate::AccessState;
    use calendar_selection::{SelectionChange, change_selection, publish_rows, rows};
    let options = souffle_lib::settings::SettingsOptions::current();
    w.set_settings_calendar_reminder_minutes_min(options.calendar_reminder_minutes_min as i32);
    w.set_settings_calendar_reminder_minutes_max(options.calendar_reminder_minutes_max as i32);
    w.set_settings_calendar_reminder_minutes(2);
    w.set_settings_calendar_enabled(false);
    w.set_settings_calendar_autostart_enabled(false);
    w.set_settings_calendar_permission(AccessState::Denied);
    let stored: Rc<std::cell::RefCell<Vec<String>>> = Default::default();
    let log: Log = Default::default();
    let granted = Rc::new(Cell::new(false));
    let catalogue = Rc::new(catalogue);

    let (weak, granted_for_open, l) = (w.as_weak(), granted.clone(), log.clone());
    w.on_settings_calendar_open_system_settings_requested(move || {
        l.borrow_mut().push("system settings".into());
        granted_for_open.set(granted_by_system_settings);
        let _ = weak.upgrade();
    });
    let (weak, granted_for_enable, catalogue_for_enable, stored_for_enable, l) = (
        w.as_weak(),
        granted.clone(),
        catalogue.clone(),
        stored.clone(),
        log.clone(),
    );
    w.on_settings_calendar_enabled_changed(move |value| {
        let w = weak.unwrap();
        l.borrow_mut().push(format!("enabled {value}"));
        if value && !granted_for_enable.get() {
            w.set_settings_calendar_permission(AccessState::Denied);
            return;
        }
        w.set_settings_calendar_enabled(value);
        if value {
            w.set_settings_calendar_permission(AccessState::Granted);
            publish_rows(&w, rows(&catalogue_for_enable, &stored_for_enable.borrow()));
        } else {
            publish_rows(&w, Vec::new());
        }
    });
    let (weak, catalogue_for_toggle, stored_for_toggle, l) =
        (w.as_weak(), catalogue.clone(), stored.clone(), log.clone());
    w.on_settings_calendar_toggled(move |id| {
        let w = weak.unwrap();
        let Some(shown) = w.get_settings_calendars().iter().find(|row| row.id == id) else {
            l.borrow_mut().push(format!("ignored {id}"));
            return;
        };
        let change = change_selection(
            &stored_for_toggle.borrow(),
            &catalogue_for_toggle,
            &id,
            !shown.selected,
        );
        match change {
            SelectionChange::Write(ids) => {
                l.borrow_mut().push(format!("write {id} {ids:?}"));
                *stored_for_toggle.borrow_mut() = ids;
            }
            SelectionChange::Unchanged(refusal) => {
                l.borrow_mut().push(format!("kept {id} {refusal:?}"));
            }
        }
        publish_rows(&w, rows(&catalogue_for_toggle, &stored_for_toggle.borrow()));
    });
    let (weak, l) = (w.as_weak(), log.clone());
    w.on_settings_calendar_autostart_enabled_changed(move |value| {
        l.borrow_mut().push(format!("autostart {value}"));
        weak.unwrap().set_settings_calendar_autostart_enabled(value);
    });
    let (weak, l) = (w.as_weak(), log.clone());
    let (min, max) = (
        options.calendar_reminder_minutes_min as i32,
        options.calendar_reminder_minutes_max as i32,
    );
    w.on_settings_calendar_reminder_minutes_changed(move |value| {
        let value = value.clamp(min, max);
        l.borrow_mut().push(format!("reminder {value}"));
        weak.unwrap().set_settings_calendar_reminder_minutes(value);
    });
    MeetingsHost { stored, log }
}

/// Every focus ring on screen must sit inside the page viewport.
fn rings_fit(w: &MainWindow, page: &ElementHandle, what: &str) -> usize {
    let rings = items(w, "FocusOutline");
    let (p, ps) = (page.absolute_position(), page.size());
    for ring in &rings {
        let (r, rs) = (ring.absolute_position(), ring.size());
        assert!(
            r.y >= p.y - 1. && r.y + rs.height <= p.y + ps.height + 1.,
            "{what}: ring clipped vertically {r:?} {rs:?} in {p:?} {ps:?}"
        );
        assert!(
            r.x >= p.x - 1. && r.x + rs.width <= p.x + ps.width + 1.,
            "{what}: ring clipped horizontally {r:?} {rs:?} in {p:?} {ps:?}"
        );
    }
    rings.len()
}

/// The accessible name of the control that currently shows a focus ring.
fn focused_label(w: &MainWindow) -> Option<String> {
    [
        "KitSwitch",
        "KitDecimalField",
        "KitTextField",
        "KitCheckbox",
        "KitButton",
    ]
    .into_iter()
    .flat_map(|kind| items(w, kind))
    .find(|e| {
        e.query_descendants()
            .match_inherits("FocusOutline")
            .find_first()
            .is_some()
    })
    .and_then(|e| e.accessible_label())
    .map(|label| label.to_string())
}

async fn meetings(w: &MainWindow, path: &Path) {
    use crate::AccessState;
    let en = w.get_settings_locale() == crate::AppLocale::En;
    w.set_settings_tab(SettingsTab::Meetings);
    let host = wire_meetings_host(w, meeting_calendars(), true);
    settle(w);
    let page = id(w, "SettingsView::meetings-page");
    assert_widths(w);

    // Refusal: the alert and its action are there even though the integration is off.
    let denied = if en {
        "Calendar access was denied."
    } else {
        "L'acc\u{e8}s au calendrier a \u{e9}t\u{e9} refus\u{e9}."
    };
    assert_eq!(alerts(w), [denied]);
    assert!(items(w, "KitCheckbox").is_empty());
    frame(w, path, "meetings-denied");
    tab_to_and_measure(
        w,
        &page,
        "CalendarSection::calendar-switch",
        "integration switch",
    );
    send_key(w, Key::Tab.into());
    assert_eq!(rings_fit(w, &page, "alert action"), 1);
    let open = items(w, "KitButton")
        .into_iter()
        .find(|e| {
            e.accessible_label().as_deref()
                == Some(if en {
                    "Open System Settings"
                } else {
                    "Ouvrir les R\u{e9}glages Syst\u{e8}me"
                })
        })
        .expect("the recovery action");
    open.invoke_accessible_default_action();
    settle(w);
    assert!(host.log.borrow().iter().any(|l| l == "system settings"));

    // Authorization: the switch now succeeds, the alert disappears and the list shows.
    let integration = id(w, "CalendarSection::calendar-switch");
    integration.invoke_accessible_default_action();
    settle(w);
    assert!(w.get_settings_calendar_enabled());
    assert_eq!(w.get_settings_calendar_permission(), AccessState::Granted);
    assert!(alerts(w).is_empty());
    assert_eq!(w.get_settings_calendars().row_count(), 4);
    assert_widths(w);
    frame(w, path, "meetings-list");

    // Walk every stop with Tab: the switches, the stepper, then the calendars in
    // display order. Each ring stays inside the page whatever the scroll.
    tab_to_and_measure(
        w,
        &page,
        "CalendarSection::calendar-switch",
        "integration switch",
    );
    let expected: Vec<String> = [
        if en {
            "Calendar integration"
        } else {
            "Int\u{e9}gration du calendrier"
        },
        if en {
            "Prompt at start of meeting"
        } else {
            "Proposer au d\u{e9}but de la r\u{e9}union"
        },
        if en {
            "Reminder delay in minutes"
        } else {
            "D\u{e9}lai du rappel en minutes"
        },
    ]
    .into_iter()
    .map(str::to_string)
    .chain(
        w.get_settings_calendars()
            .iter()
            .map(|row| row.title.to_string()),
    )
    .collect();
    let mut reached = vec![focused_label(w).expect("the integration switch is focused")];
    for _ in 1..expected.len() {
        send_key(w, Key::Tab.into());
        assert!(rings_fit(w, &page, "tab walk") > 0);
        let label = focused_label(w);
        reached.push(label.expect("a focused control"));
    }
    println!("focus stops reached: {reached:?}");
    assert_eq!(reached, expected);
    frame(w, path, "meetings-last-focus");

    // Space on the focused calendar writes by identifier; the checkbox follows the host.
    let before = host.log.borrow().len();
    send_key(w, " ".into());
    let after: Vec<String> = host.log.borrow()[before..].to_vec();
    println!("space on the focused calendar: {after:?}");
    assert_eq!(after.len(), 1);
    assert!(after[0].starts_with("write "), "{after:?}");
    assert!(
        rings_fit(w, &page, "ring after the write") > 0,
        "focus survives the in-place refresh"
    );
    frame(w, path, "meetings-after-toggle");

    // Unchecking every calendar in turn through the host handler: the last one is refused.
    for row in w.get_settings_calendars().iter().filter(|row| row.selected) {
        host.log.borrow_mut().clear();
        w.invoke_settings_calendar_toggled(row.id.clone());
        settle(w);
    }
    let remaining = w
        .get_settings_calendars()
        .iter()
        .filter(|row| row.selected)
        .count();
    assert_eq!(remaining, 1, "one calendar stays checked");
    assert!(
        host.log.borrow().iter().any(|l| l.contains("LastCalendar")),
        "the last checked calendar is refused: {:?}",
        host.log.borrow()
    );
    println!("selection by identifier: {:?}", host.stored.borrow());

    // Reminder: bounds from Rust, steps stop at both ends.
    let more = || {
        items(w, "KitPressable")
            .into_iter()
            .find(|e| {
                e.accessible_label().as_deref() == Some(if en { "Increase" } else { "Augmenter" })
            })
            .expect("increase")
    };
    let less = || {
        items(w, "KitPressable")
            .into_iter()
            .find(|e| {
                e.accessible_label().as_deref() == Some(if en { "Decrease" } else { "Diminuer" })
            })
            .expect("decrease")
    };
    let (min, max) = (
        w.get_settings_calendar_reminder_minutes_min(),
        w.get_settings_calendar_reminder_minutes_max(),
    );
    reveal(w, &page, "CalendarSection::delay-stepper");
    for _ in 0..(max - min + 3) {
        more().invoke_accessible_default_action();
        settle(w);
    }
    assert_eq!(w.get_settings_calendar_reminder_minutes(), max);
    frame(w, path, "meetings-reminder-max");
    for _ in 0..(max - min + 3) {
        less().invoke_accessible_default_action();
        settle(w);
    }
    assert_eq!(w.get_settings_calendar_reminder_minutes(), min);

    // The proposal switch is controlled and reachable.
    reveal(w, &page, "CalendarSection::reminder-switch");
    id(w, "CalendarSection::reminder-switch").invoke_accessible_default_action();
    settle(w);
    assert!(w.get_settings_calendar_autostart_enabled());

    // Replacing the model: an in-place update keeps the delegates, a new set replaces them.
    wheel(w, &page, -100000.);
    let first = items(w, "KitCheckbox").remove(0);
    let catalogue = meeting_calendars();
    calendar_selection::publish_rows(w, calendar_selection::rows(&catalogue, &[]));
    settle(w);
    println!(
        "delegate survives a same-calendars refresh: {}",
        first.is_valid()
    );
    assert!(first.is_valid());
    calendar_selection::publish_rows(w, calendar_selection::rows(&catalogue[..2], &[]));
    settle(w);
    assert_eq!(w.get_settings_calendars().row_count(), 2);
    calendar_selection::publish_rows(w, calendar_selection::rows(&catalogue, &[]));
    settle(w);

    // Turning the integration off hides the choices again.
    wheel(w, &page, 100000.);
    integration.invoke_accessible_default_action();
    settle(w);
    assert!(!w.get_settings_calendar_enabled());
    assert!(items(w, "KitCheckbox").is_empty());
    println!("meetings interactions verified: {:?}", host.log.borrow());
}

fn synthetic_calendars(count: usize) -> Vec<souffle_lib::calendar::CalendarInfo> {
    const SOURCES: [&str; 5] = ["iCloud", "Google", "Exchange", "Fastmail", "Local"];
    (0..count)
        .map(|index| {
            let title = if index % 7 == 0 {
                "Travail".to_string()
            } else {
                format!("Calendrier {index}")
            };
            calendar(
                &format!("synthetic-{index}"),
                &title,
                Some(SOURCES[(index * SOURCES.len() / count.max(1)).min(SOURCES.len() - 1)]),
            )
        })
        .collect()
}

/// Mount, scroll and refresh cost of the calendar list for a synthetic catalogue.
async fn meetings_scale(w: &MainWindow, path: &Path) {
    use std::time::Instant;
    let count: usize = std::env::var("SOUFFLE_FIXTURE_CALENDARS")
        .expect("calendar count")
        .parse()
        .unwrap();
    w.set_settings_tab(SettingsTab::Meetings);
    let catalogue = synthetic_calendars(count);
    let host = wire_meetings_host(w, catalogue.clone(), true);
    w.set_settings_calendar_enabled(true);
    w.set_settings_calendar_permission(crate::AccessState::Granted);
    settle(w);
    let page = id(w, "SettingsView::meetings-page");

    let started = Instant::now();
    calendar_selection::publish_rows(w, calendar_selection::rows(&catalogue, &[]));
    settle(w);
    let mount = started.elapsed();
    let at_top = items(w, "KitCheckbox").len();
    wheel(w, &page, -100000.);
    let at_end = items(w, "KitCheckbox").len();
    wheel(w, &page, 100000.);
    println!(
        "calendars {count}: mount {mount:?}, queryable checkboxes at the top {at_top}, at the end {at_end}"
    );
    assert_widths(w);

    let started = Instant::now();
    let mut steps = 0;
    let mut last_y: Option<f32> = None;
    loop {
        wheel(w, &page, -400.);
        steps += 1;
        let y = items(w, "KitCheckbox")
            .last()
            .map(|e| e.absolute_position().y);
        if (y.is_some() && y == last_y) || steps > 4000 {
            break;
        }
        last_y = y;
    }
    println!(
        "calendars {count}: scroll to the end {:?} in {steps} wheel steps",
        started.elapsed()
    );
    let last = items(w, "KitCheckbox")
        .into_iter()
        .last()
        .expect("a last calendar");
    let (p, ps) = (page.absolute_position(), page.size());
    let y = last.absolute_position().y;
    assert!(
        y >= p.y && y <= p.y + ps.height + 1.,
        "the last calendar is reachable by scrolling: {y} in {p:?} {ps:?}"
    );
    frame(w, path, "meetings-scale-end");

    let started = Instant::now();
    send_key(w, Key::Tab.into());
    println!("calendars {count}: one Tab {:?}", started.elapsed());

    let started = Instant::now();
    let mut refreshed = calendar_selection::rows(&catalogue, &[]);
    if let Some(row) = refreshed.first_mut() {
        row.selected = false;
    }
    calendar_selection::publish_rows(w, refreshed);
    settle(w);
    println!(
        "calendars {count}: in-place refresh {:?}",
        started.elapsed()
    );

    let started = Instant::now();
    w.set_settings_tab(SettingsTab::Interface);
    settle(w);
    w.set_settings_tab(SettingsTab::Meetings);
    settle(w);
    println!(
        "calendars {count}: leave and return to the tab {:?}",
        started.elapsed()
    );
    println!("calendars {count}: log {:?}", host.log.borrow());
}
