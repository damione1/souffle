//! Isolated rendering of the real settings window, without starting the host services.
use i_slint_backend_testing::ElementQuery;
use slint::{ComponentHandle, LogicalSize, Model, Timer, VecModel};
use std::{
    io::{BufWriter, Write},
    time::Duration,
};

slint::include_modules!();
#[path = "../src/settings_kit.rs"]
mod settings_kit;

#[allow(dead_code)]
#[path = "../src/audio_ui.rs"]
mod audio_ui;
#[allow(dead_code)]
#[path = "../src/ia_ui.rs"]
mod ia_ui;
// The fixture renders fixed settings; the app rule projection needs the live
// settings I/O stack that this example does not start.
mod app_branch_ui {
    pub(crate) fn populate(_: &super::MainWindow, _: &souffle_lib::settings::AppSettings) {}
}
#[allow(dead_code)]
#[path = "../src/microphone_list.rs"]
mod microphone_list;
#[path = "settings_foundations/settings_pages.rs"]
mod settings_pages;

fn settle(w: &MainWindow) {
    for _ in 0..3 {
        slint::platform::update_timers_and_animations();
        w.window().take_snapshot().unwrap();
    }
}

fn click(w: &MainWindow, item: &i_slint_backend_testing::ElementHandle) {
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
    w.window().dispatch_event(WindowEvent::PointerReleased {
        position,
        button: PointerEventButton::Left,
    });
    settle(w);
}

fn modal_interaction(w: &MainWindow, open_picker: bool) {
    use i_slint_backend_testing::{AccessibleRole, ElementHandle, ElementQuery};
    let trigger = ElementQuery::from_root(w)
        .match_inherits("KitSelectTrigger")
        .find_all()
        .into_iter()
        .find(|item| item.accessible_value().as_deref() == Some("Jamais"))
        .unwrap();
    let selections = std::rc::Rc::new(std::cell::Cell::new(0));
    let observed = selections.clone();
    w.on_settings_unload_timeout_changed(move |_| observed.set(observed.get() + 1));
    if open_picker {
        click(w, &trigger);
        assert_eq!(trigger.accessible_expanded(), Some(true));
    }
    w.set_whats_new_open(true);
    settle(w);
    let button = ElementHandle::find_by_accessible_label(w, "Compris")
        .find(|item| item.accessible_role() == Some(AccessibleRole::Button))
        .unwrap();
    println!(
        "popup previously open={open_picker}, modal button at {:?}, size {:?}",
        button.absolute_position(),
        button.size()
    );
    click(w, &button);
    println!("modal open after first click={}", w.get_whats_new_open());
    assert!(!w.get_whats_new_open(), "one click on the modal action");
    assert_ne!(trigger.accessible_expanded(), Some(true));
    w.set_whats_new_open(true);
    settle(w);
    send_key(w, slint::platform::Key::DownArrow.into());
    assert!(w.get_whats_new_open());
    send_key(w, slint::platform::Key::Return.into());
    assert!(
        !w.get_whats_new_open(),
        "modal close action owns keyboard focus"
    );
    assert_eq!(selections.get(), 0);
    assert_eq!(trigger.accessible_value().as_deref(), Some("Jamais"));
    // Announcements have no persistent opener. The header is the host's return target.
    send_key(w, slint::platform::Key::Space.into());
    assert!(
        !w.get_settings_open(),
        "Space on restored settings trigger closes the sheet"
    );
    println!(
        "modal keyboard dismissal; background choices={}",
        selections.get()
    );
}

fn send_key(w: &MainWindow, text: slint::SharedString) {
    use slint::platform::WindowEvent;
    w.window()
        .dispatch_event(WindowEvent::KeyPressed { text: text.clone() });
    w.window().dispatch_event(WindowEvent::KeyReleased { text });
    settle(w);
}

fn visit_tabs(w: &MainWindow) {
    use i_slint_backend_testing::{AccessibleRole, ElementHandle, ElementQuery};
    use slint::platform::Key;
    let bar = ElementQuery::from_root(w)
        .match_inherits("KitTabs")
        .find_first()
        .unwrap();
    let tab_item = |tab| {
        let label = w.global::<SettingsOptions>().invoke_tab_label(tab);
        bar.query_descendants()
            .match_accessible_role(AccessibleRole::Tab)
            .match_accessible_label(label.to_string())
            .find_first()
            .unwrap()
    };
    let tabs = [
        SettingsTab::Transcription,
        SettingsTab::Ai,
        SettingsTab::Audio,
        SettingsTab::Interface,
        SettingsTab::Meetings,
        SettingsTab::System,
    ];
    click(w, &tab_item(tabs[0]));
    let mut retained = None;
    let mut editor_y = 0.;
    for pass in 1..=2 {
        for &tab in &tabs {
            if !(pass == 1 && tab == tabs[0]) {
                send_key(w, Key::RightArrow.into());
            }
            assert_eq!(w.get_settings_tab(), tab);
            let item = tab_item(tab);
            let x = item.absolute_position().x;
            let right = x + item.size().width;
            let left = bar.absolute_position().x;
            let end = left + bar.size().width;
            println!(
                "pass {pass}, {tab:?}, item {x}..{right}, bar {left}..{end}, selected {:?}",
                item.accessible_item_selected()
            );
            assert!(
                x >= left - 1. && right <= end + 1.,
                "active tab outside strip"
            );
            if tab == SettingsTab::Ai {
                if pass == 1 {
                    let url = |w: &MainWindow| {
                        ElementHandle::find_by_element_id(w, "IntelligenceSection::url-editor")
                            .next()
                    };
                    for _ in 0..45 {
                        send_key(w, Key::Tab.into());
                        if url(w).is_some_and(|e| {
                            e.query_descendants()
                                .match_inherits("FocusOutline")
                                .find_first()
                                .is_some()
                        }) {
                            break;
                        }
                    }
                    let editor = url(w).expect("URL editor reached via Tab");
                    let ring = editor
                        .query_descendants()
                        .match_inherits("FocusOutline")
                        .find_first()
                        .unwrap();
                    let page = ElementHandle::find_by_element_id(w, "SettingsView::ai-page")
                        .next()
                        .unwrap();
                    let bottom = |e: &ElementHandle| e.absolute_position().y + e.size().height;
                    println!(
                        "AI viewport {:?}..{}, ring {:?}..{}",
                        page.absolute_position(),
                        bottom(&page),
                        ring.absolute_position(),
                        bottom(&ring)
                    );
                    assert!(
                        bottom(&ring) <= bottom(&page) + 1.
                            && ring.absolute_position().y >= page.absolute_position().y - 1.
                    );
                    send_key(w, "abcd".into());
                    send_key(w, Key::LeftArrow.into());
                    send_key(w, Key::LeftArrow.into());
                    assert_eq!(editor.accessible_value().as_deref(), Some("abcd"));
                    editor_y = editor.absolute_position().y;
                    retained = Some(editor);
                    click(w, &item);
                } else {
                    let editor = retained.as_ref().unwrap();
                    assert!(editor.is_valid());
                    assert_eq!(editor.accessible_value().as_deref(), Some("abcd"));
                    assert!((editor.absolute_position().y - editor_y).abs() < 1.);
                    let label = ElementHandle::find_by_accessible_label(w, "URL Ollama")
                        .find(|e| e.type_name().as_deref() == Some("Text"))
                        .unwrap();
                    click(w, &label);
                    send_key(w, "X".into());
                    println!(
                        "retained editor y={editor_y}, text={:?}",
                        editor.accessible_value()
                    );
                    assert_eq!(editor.accessible_value().as_deref(), Some("abXcd"));
                    click(w, &item);
                }
            }
        }
    }
    let group = || {
        ElementQuery::from_root(w)
            .match_inherits("KitFieldSet")
            .match_accessible_label("STOCKAGE DU MODÈLE")
            .find_first()
    };
    let page = ElementHandle::find_by_element_id(w, "SettingsView::system-page")
        .next()
        .unwrap();
    for _ in 0..32 {
        if group().is_some_and(|g| g.absolute_position().y < 650.) {
            break;
        }
        page.scroll(0., -260.);
        settle(w);
    }
    let action = |label: &str| {
        group()
            .expect("storage fieldset revealed")
            .query_descendants()
            .match_accessible_role(AccessibleRole::Button)
            .match_accessible_label(label)
            .find_first()
            .expect("storage action")
    };
    let delete = action("Supprimer");
    assert_eq!(delete.accessible_enabled(), Some(false));
    delete.invoke_accessible_default_action();
    assert!(delete.is_valid());
    w.set_model_runtime_phase(TranscriptionPhase::LoadRequired);
    settle(w);
    assert_eq!(delete.accessible_enabled(), Some(true));
    delete.invoke_accessible_default_action();
    settle(w);
    let confirm = action("Confirmer");
    let cancel = action("Annuler");
    assert!(
        confirm.absolute_position().x + confirm.size().width <= cancel.absolute_position().x + 1.
    );
    assert!(
        cancel.absolute_position().x + cancel.size().width
            <= w.window().size().width as f32 / w.window().scale_factor() + 1.
    );
    cancel.invoke_accessible_default_action();
    settle(w);
    assert!(action("Supprimer").is_valid());
    w.set_settings_save_error("fixture save".into());
    settle(w);
    let alert = || {
        ElementQuery::from_root(w)
            .match_inherits("KitAlert")
            .find_all()
    };
    assert_eq!(alert().len(), 1);
    w.set_settings_draft_save_error("fixture draft".into());
    settle(w);
    assert_eq!(alert().len(), 1);
    assert!(
        ElementHandle::find_by_accessible_label(w, "Brouillon conservé : fixture draft")
            .next()
            .is_some()
    );
    println!("disabled delete, conditional confirm/cancel, and retained draft alert verified");
    w.set_settings_save_error("".into());
    w.set_settings_draft_save_error("".into());
    settle(w);
    let fields = w.global::<slint_kit::ui::KitFieldService>();
    assert!(fields.invoke_begin_shortcut_session() > 0);
    let weak = w.as_weak();
    w.on_settings_closed(move || {
        assert_eq!(
            weak.unwrap()
                .global::<slint_kit::ui::KitFieldService>()
                .get_shortcut_session(),
            0
        );
    });
    let done = ElementQuery::from_root(w)
        .match_accessible_role(AccessibleRole::Button)
        .match_accessible_label("Fermer les paramètres")
        .find_first()
        .unwrap();
    click(w, &done);
    assert!(!w.get_settings_open());
    let reopened = std::rc::Rc::new(std::cell::Cell::new(false));
    let seen = reopened.clone();
    w.on_settings_requested(move || seen.set(true));
    send_key(w, Key::Space.into());
    assert!(
        reopened.get(),
        "keyboard focus returned to the settings trigger"
    );
    println!(
        "settings close expires capture synchronously and Space reactivates the header trigger"
    );
    w.set_settings_open(true);
    w.set_settings_tab(SettingsTab::System);
    settle(w);
}

fn save_frame(w: &MainWindow, path: &std::path::Path) {
    let im = w.window().take_snapshot().unwrap();
    let mut f = BufWriter::new(std::fs::File::create(path).unwrap());
    write!(f, "P6\n{} {}\n255\n", im.width(), im.height()).unwrap();
    for p in im.as_slice() {
        f.write_all(&[p.r, p.g, p.b]).unwrap();
    }
}

fn header_states(w: &MainWindow, path: &std::path::Path) {
    use strum::VariantArray;
    let header = ElementQuery::from_root(w)
        .match_inherits("AppHeader")
        .find_first()
        .unwrap();
    let check = |phase, mode| {
        w.set_model_runtime_phase(phase);
        w.set_recording_mode(mode);
        settle(w);
        let status = header
            .query_descendants()
            .match_inherits("KitBadge")
            .find_first()
            .unwrap();
        let actions = header
            .query_descendants()
            .match_inherits("KitIconButton")
            .find_all();
        assert_eq!(actions.len(), 2);
        let center_y = |item: &i_slint_backend_testing::ElementHandle| {
            item.absolute_position().y + item.size().height / 2.
        };
        let center = center_y(&actions[0]);
        let logo = header
            .query_descendants()
            .match_inherits("KitImage")
            .find_first()
            .unwrap();
        let labels = header
            .query_descendants()
            .match_inherits("KitText")
            .find_all();
        assert!(!labels.is_empty());
        let centers: Vec<_> = std::iter::once(&logo)
            .chain(labels.iter())
            .chain([&status])
            .chain(actions.iter())
            .map(center_y)
            .collect();
        println!("centers logo / texts / badge / actions {centers:?}");
        assert!(
            centers.iter().all(|y| (y - center).abs() <= 1.),
            "header cross-axis alignment"
        );
        let mut end = status.absolute_position().x + status.size().width;
        assert!(!status.accessible_label().unwrap().is_empty());
        for action in &actions {
            assert!(action.size().width > 0. && action.absolute_position().x >= end - 1.);
            end = action.absolute_position().x + action.size().width;
        }
        println!(
            "{phase:?}/{mode:?} {:?}, action right {end}, header {}",
            status.accessible_label(),
            header.size().width
        );
        assert!(end <= header.size().width + 1.);
        if phase == TranscriptionPhase::Ready && mode == RecordingMode::Idle {
            assert!(
                header
                    .query_descendants()
                    .match_accessible_label(w.get_header_model_label().to_string())
                    .find_first()
                    .is_some()
            );
        }
        if phase == TranscriptionPhase::DownloadRequired || mode != RecordingMode::Idle {
            let stem = path.file_stem().unwrap().to_string_lossy();
            save_frame(
                w,
                &path.with_file_name(format!("{stem}-{phase:?}-{mode:?}.ppm")),
            );
        }
    };
    for &phase in TranscriptionPhase::VARIANTS {
        check(phase, RecordingMode::Idle);
    }
    for &mode in RecordingMode::VARIANTS {
        check(TranscriptionPhase::Ready, mode);
    }
    w.set_recording_mode(RecordingMode::Idle);
    settle(w);
}

async fn catalogue_interaction(w: &MainWindow) {
    use slint::platform::{Key, PointerEventButton};
    let model = |values: &[&str]| {
        VecModel::from_slice(&values.iter().map(|v| (*v).into()).collect::<Vec<_>>())
    };
    let ids = std::rc::Rc::new(VecModel::from(vec!["alpha".into(), "beta".into()]));
    w.set_settings_summary_model_ids(ids.clone().into());
    w.set_settings_summary_model_labels(model(&["Shared", "Shared"]));
    w.set_settings_selected_summary_model_id("alpha".into());
    w.set_settings_apple_intelligence_available(true);
    w.set_settings_summary_model_picker_visible(true);
    w.set_settings_tab(SettingsTab::Ai);
    let changes = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let observed = changes.clone();
    let weak = w.as_weak();
    w.on_settings_ollama_model_changed(move |id| {
        observed.borrow_mut().push(id.clone());
        weak.unwrap().set_settings_selected_summary_model_id(id);
    });
    settle(w);
    let trigger = ElementQuery::from_root(w)
        .match_inherits("KitSelectTrigger")
        .find_all()
        .into_iter()
        .find(|e| e.accessible_value().as_deref() == Some("Shared"))
        .unwrap();
    for _ in 0..45 {
        if trigger.absolute_position().y + trigger.size().height < 735. {
            break;
        }
        send_key(w, Key::Tab.into());
    }
    click(w, &trigger);
    let rows = || {
        ElementQuery::from_root(w)
            .match_inherits("KitOption")
            .find_all()
            .into_iter()
            .filter(|e| e.accessible_label().as_deref() == Some("Shared"))
            .collect::<Vec<_>>()
    };
    let initial = rows();
    assert_eq!(initial.len(), 2, "real popup rows");
    let button = PointerEventButton::Left;
    initial[1].single_click(button).await;
    settle(w);
    assert_eq!(w.get_settings_selected_summary_model_id(), "beta");
    click(w, &trigger);
    assert_eq!(rows()[1].accessible_item_selected(), Some(true));
    changes.borrow_mut().clear();
    // single_click presses at the mapped popup position, waits 50 ms, then
    // releases. Change only the IDs during that real outstanding gesture.
    let weak = w.as_weak();
    Timer::single_shot(Duration::from_millis(10), move || {
        let w = weak.unwrap();
        ids.set_row_data(0, "beta".into());
        ids.set_row_data(1, "gamma".into());
        settle(&w);
    });
    rows()[0].single_click(button).await;
    settle(w);
    assert!(
        changes.borrow().is_empty(),
        "held gesture accepted a replacement"
    );
    if trigger.accessible_expanded() != Some(true) {
        click(w, &trigger);
    }
    let refreshed = rows();
    assert_eq!(refreshed[0].accessible_item_selected(), Some(true));
    refreshed[1].single_click(button).await;
    settle(w);
    assert_eq!(&*changes.borrow(), &[slint::SharedString::from("gamma")]);
    w.set_settings_summary_model_ids(model(&["alpha", "beta"]));
    settle(w);
    assert_eq!(
        trigger.accessible_description().as_deref(),
        Some("Option sélectionnée indisponible")
    );
    println!(
        "duplicate labels, selected second ID, stale press discarded, fresh choice gamma, missing ID announced"
    );
}

#[test]
fn real_catalogue_rejects_a_held_choice_after_only_the_ids_change() {
    use i_slint_backend_testing::{TestingBackend, TestingBackendOptions};
    use std::task::{Context, Waker};
    slint::platform::set_platform(Box::new(TestingBackend::new(TestingBackendOptions {
        mock_time: true,
        renderer_name: Some("software".into()),
        ..Default::default()
    })))
    .unwrap();
    let w = settings_kit::new_window().unwrap();
    slint::select_bundled_translation("fr").unwrap();
    w.global::<SettingsOptions>().invoke_localize();
    w.window().set_size(LogicalSize::new(1040., 760.));
    w.set_settings_open(true);
    w.show().unwrap();
    let mut exercise = std::pin::pin!(catalogue_interaction(&w));
    let mut cx = Context::from_waker(Waker::noop());
    for _ in 0..100 {
        if exercise.as_mut().poll(&mut cx).is_ready() {
            return;
        }
        i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(10));
    }
    panic!("catalogue gestures did not finish");
}

fn check_languages(w: &MainWindow) {
    for (locale, label, search, clear) in [
        (
            "fr",
            "Jamais",
            "Rechercher dans les options",
            "Effacer la recherche",
        ),
        ("en", "Never", "Search options", "Clear search"),
        (
            "fr",
            "Jamais",
            "Rechercher dans les options",
            "Effacer la recherche",
        ),
    ] {
        slint::select_bundled_translation(locale).unwrap();
        w.global::<SettingsOptions>().invoke_localize();
        settle(w);
        let trigger = ElementQuery::from_root(w)
            .match_inherits("KitSelectTrigger")
            .find_all()
            .into_iter()
            .find(|e| e.accessible_value().as_deref() == Some(label))
            .expect("translated trigger");
        click(w, &trigger);
        assert!(
            ElementQuery::from_root(w)
                .match_inherits("KitOption")
                .match_accessible_label(label)
                .find_first()
                .is_some()
        );
        let strings = w.global::<slint_kit::ui::KitStrings>();
        assert_eq!(strings.get_search_options_label(), search);
        assert_eq!(strings.get_clear_search_label(), clear);
        println!("{locale} trigger and popup {label}; search {search}, clear {clear}");
        send_key(w, slint::platform::Key::Escape.into());
        w.set_settings_open(false);
        w.set_search_query("fixture".into());
        settle(w);
        let (search_name, clear_name) = if locale == "fr" {
            ("Rechercher les transcriptions et réunions", "Effacer")
        } else {
            ("Search transcripts and meetings", "Clear")
        };
        let field = ElementQuery::from_root(w)
            .match_inherits("KitSearchField")
            .find_first()
            .unwrap();
        assert_eq!(field.accessible_label().as_deref(), Some(search_name));
        let clear_action = field
            .query_descendants()
            .match_accessible_label(clear_name)
            .find_first()
            .unwrap();
        clear_action.invoke_accessible_default_action();
        settle(w);
        assert!(w.get_search_query().is_empty());
        println!("search field {search_name}; accessible action {clear_name} cleared its value");
        w.set_settings_open(true);
        settle(w);
    }
}

fn main() -> Result<(), slint::PlatformError> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let [output, locale, palette, width, scale] = args.as_slice() else {
        panic!("expected output.ppm locale light|dark width text-scale");
    };
    let width: f32 = width.parse().unwrap();
    let scale: f32 = scale.parse().unwrap();
    let selector = slint::BackendSelector::new().renderer_name("skia".into());
    #[cfg(target_os = "macos")]
    let selector = selector
        .require_metal()
        .with_winit_window_attributes_hook(|attrs| {
            use slint::winit_030::winit::platform::macos::WindowAttributesExtMacOS;
            attrs
                .with_titlebar_transparent(true)
                .with_title_hidden(true)
                .with_fullsize_content_view(true)
                .with_movable_by_window_background(true)
        });
    selector.select()?;
    #[cfg(target_os = "macos")]
    println!(
        "backend: Skia renderer, Metal required (BackendSelector::require_metal accepted by select())"
    );
    let w = settings_kit::new_window()?;
    w.window().set_size(LogicalSize::new(width, 760.));
    slint::select_bundled_translation(locale).unwrap();
    w.global::<SettingsOptions>().invoke_localize();
    w.global::<Theme>().set_dark(palette == "dark");
    w.global::<Theme>().invoke_apply_kit_theme();
    w.set_settings_locale(if locale == "en" {
        AppLocale::En
    } else {
        AppLocale::Fr
    });
    w.set_model_runtime_phase(TranscriptionPhase::Ready);
    w.set_header_model_label(
        "Whisper Large V3 Turbo · Multilingual transcription model for meetings and interviews"
            .into(),
    );
    w.set_settings_open(true);
    w.set_settings_unload_timeout_labels(VecModel::from_slice(&[
        "__NEVER__".into(),
        "15 min".into(),
    ]));
    w.set_settings_unload_timeout_label("__NEVER__".into());
    w.show()?;
    let weak = w.as_weak();
    let output = output.clone();
    Timer::single_shot(Duration::from_millis(150), move || {
        let w = weak.unwrap();
        let ctx = w.global::<slint_kit::ui::KitThemeContext>();
        let mut theme = ctx.get_current();
        theme.text_scale = scale;
        if std::env::var_os("SOUFFLE_FIXTURE_TABS").is_some()
            || std::env::var_os("SOUFFLE_FIXTURE_PAGES").is_some()
        {
            theme.reduced_motion = true;
        }
        ctx.set_current(theme);
        let weak = w.as_weak();
        Timer::single_shot(Duration::from_millis(350), move || {
            let w = weak.unwrap();
            slint::spawn_local(async move {
                if std::env::var_os("SOUFFLE_FIXTURE_TABS").is_some() {
                    visit_tabs(&w);
                }
                if let Ok(mode) = std::env::var("SOUFFLE_FIXTURE_MODAL") {
                    modal_interaction(&w, mode == "picker");
                }
                let path = std::path::Path::new(&output);
                if std::env::var_os("SOUFFLE_FIXTURE_HEADER").is_some() {
                    header_states(&w, path);
                }
                if std::env::var_os("SOUFFLE_FIXTURE_CATALOGUE").is_some() {
                    catalogue_interaction(&w).await;
                }
                if std::env::var_os("SOUFFLE_FIXTURE_LANG").is_some() {
                    check_languages(&w);
                }
                if std::env::var_os("SOUFFLE_FIXTURE_PAGES").is_some() {
                    settings_pages::exercise(&w, path).await;
                }
                save_frame(&w, path);
                slint::quit_event_loop().unwrap();
            })
            .unwrap();
        });
    });
    slint::run_event_loop()
}
