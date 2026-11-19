//! The two shortcut fields of Settings and the wizard, driven through the real
//! `MainWindow`, the kit field and the Rust handlers. The OS-facing effects are
//! replaced by a recorder, so no global shortcut, preference or TCC state moves.
use super::*;
use crate::{OnboardingStep, SettingsTab, ShortcutField, ShortcutRejection};
use i_slint_backend_testing::{AccessibleRole, ElementHandle, ElementQuery};
use slint::platform::{
    Key, Platform, PointerEventButton, WindowAdapter, WindowEvent,
    software_renderer::MinimalSoftwareWindow,
};
use slint::{ComponentHandle, SharedString};
use souffle_lib::settings::{AppSettings, ShortcutSettings};

const WIDTH: u32 = 1040;
const HEIGHT: u32 = 1700;
const TOGGLE: &str = "Basculer l'enregistrement";
const PTT: &str = "Appuyer pour parler";

thread_local! {
    static TEST_WINDOW: RefCell<Option<Rc<MinimalSoftwareWindow>>> = const { RefCell::new(None) };
}

struct TestPlatform;

impl Platform for TestPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        let window = MinimalSoftwareWindow::new(Default::default());
        TEST_WINDOW.with(|w| *w.borrow_mut() = Some(window.clone()));
        Ok(window)
    }
}

fn settle() {
    let window = TEST_WINDOW
        .with(|w| w.borrow().clone())
        .expect("no test window");
    let mut frame =
        vec![slint::platform::software_renderer::Rgb565Pixel::default(); (WIDTH * HEIGHT) as usize];
    for _ in 0..3 {
        slint::platform::update_timers_and_animations();
        window.request_redraw();
        window.draw_if_needed(|renderer| {
            renderer.render(&mut frame, WIDTH as usize);
        });
    }
}

fn pair(toggle: &str, push_to_talk: &str) -> ShortcutSettings {
    ShortcutSettings {
        toggle: toggle.into(),
        push_to_talk: push_to_talk.into(),
    }
}

/// Stands in for the system registration and the database.
#[derive(Clone, Default)]
struct Recorder {
    stored: Rc<RefCell<ShortcutSettings>>,
    saved: Rc<RefCell<Vec<ShortcutSettings>>>,
    failure: Rc<RefCell<Option<String>>>,
}

impl Recorder {
    fn new(stored: ShortcutSettings) -> Self {
        Self {
            stored: Rc::new(RefCell::new(stored)),
            ..Self::default()
        }
    }

    fn backend(&self) -> Rc<ShortcutBackend> {
        let stored = self.stored.clone();
        let save_stored = self.stored.clone();
        let saved = self.saved.clone();
        let failure = self.failure.clone();
        Rc::new(ShortcutBackend {
            load: Box::new(move || stored.borrow().clone()),
            save: Box::new(move |candidate| {
                if let Some(error) = failure.borrow_mut().take() {
                    return Err(error);
                }
                *save_stored.borrow_mut() = candidate.clone();
                saved.borrow_mut().push(candidate);
                Ok(())
            }),
            project: Box::new(|window, shortcuts| {
                crate::settings_ui::populate_shortcuts(window, shortcuts, &[], None)
            }),
        })
    }

    fn saves(&self) -> Vec<ShortcutSettings> {
        self.saved.borrow().clone()
    }
}

fn window() -> MainWindow {
    let _ = slint::platform::set_platform(Box::new(TestPlatform));
    let window = crate::settings_kit::new_window().unwrap();
    slint::select_bundled_translation("fr").unwrap();
    window.global::<crate::SettingsOptions>().invoke_localize();
    window
        .window()
        .set_size(slint::PhysicalSize::new(WIDTH, HEIGHT));
    window.set_onboarding_open(false);
    window
}

struct Settings {
    window: MainWindow,
    recorder: Recorder,
    cache: Rc<RefCell<Option<ShortcutSettings>>>,
}

fn settings(toggle: &str, push_to_talk: &str) -> Settings {
    let window = window();
    let initial = pair(toggle, push_to_talk);
    let recorder = Recorder::new(initial.clone());
    let cache = Rc::new(RefCell::new(Some(initial.clone())));
    wire_settings_shortcut_capture(&window, &cache, recorder.backend());
    crate::settings_ui::populate(&window, &AppSettings::default());
    crate::settings_ui::populate_shortcuts(&window, &initial, &[], None);
    window.set_settings_open(true);
    window.set_settings_tab(SettingsTab::Interface);
    window.show().unwrap();
    settle();
    Settings {
        window,
        recorder,
        cache,
    }
}

fn find_button(window: &MainWindow, label: &str) -> ElementHandle {
    ElementHandle::find_by_accessible_label(window, label)
        .find(|e| e.accessible_role() == Some(AccessibleRole::Button))
        .unwrap_or_else(|| panic!("no button labelled {label:?}"))
}

fn activate(window: &MainWindow, label: &str) {
    find_button(window, label).invoke_accessible_default_action();
    settle();
}

fn press(window: &MainWindow, text: impl Into<SharedString>) {
    window
        .window()
        .dispatch_event(WindowEvent::KeyPressed { text: text.into() });
    settle();
}

fn release(window: &MainWindow, text: impl Into<SharedString>) {
    window
        .window()
        .dispatch_event(WindowEvent::KeyReleased { text: text.into() });
    settle();
}

fn tap(window: &MainWindow, text: impl Into<SharedString> + Clone) {
    press(window, text.clone());
    release(window, text);
}

fn combo(window: &MainWindow, modifier: Key, text: impl Into<SharedString> + Clone) {
    press(window, modifier);
    tap(window, text);
    release(window, modifier);
}

fn click(window: &MainWindow, element: &ElementHandle) {
    let position = slint::LogicalPosition::new(
        element.absolute_position().x + element.size().width / 2.0,
        element.absolute_position().y + element.size().height / 2.0,
    );
    let button = PointerEventButton::Left;
    window
        .window()
        .dispatch_event(WindowEvent::PointerMoved { position });
    window
        .window()
        .dispatch_event(WindowEvent::PointerPressed { position, button });
    window
        .window()
        .dispatch_event(WindowEvent::PointerReleased { position, button });
    settle();
}

#[test]
fn a_valid_combination_is_saved_once_and_projected_after_success() {
    let s = settings("CommandOrControl+K", "");
    activate(&s.window, TOGGLE);
    assert_eq!(
        s.window.get_settings_recording_field(),
        ShortcutField::Toggle
    );
    combo(&s.window, Key::Control, "j");
    assert_eq!(s.recorder.saves(), vec![pair("CommandOrControl+J", "")]);
    assert_eq!(s.window.get_settings_recording_field(), ShortcutField::None);
    assert_eq!(s.window.get_settings_toggle_shortcut_label(), "\u{2318} J");
    assert_eq!(s.window.get_dictation_shortcut(), "\u{2318} J");
    assert_eq!(
        s.cache.borrow().as_ref().map(|c| c.toggle.clone()),
        Some("CommandOrControl+J".into())
    );
    assert_eq!(
        s.window.get_settings_shortcut_rejection(),
        ShortcutRejection::None
    );
    tap(&s.window, "x");
    assert_eq!(s.recorder.saves().len(), 1, "a second event has no ticket");
}

#[test]
fn a_refused_key_keeps_capturing_so_the_next_combination_needs_no_click() {
    let s = settings("CommandOrControl+K", "");
    activate(&s.window, TOGGLE);
    tap(&s.window, "d");
    assert!(s.recorder.saves().is_empty());
    assert_eq!(
        s.window.get_settings_shortcut_rejection(),
        ShortcutRejection::MissingModifier
    );
    assert_eq!(
        s.window.get_settings_recording_field(),
        ShortcutField::Toggle
    );
    assert_eq!(s.window.get_settings_shortcut_error(), "");
    assert!(
        s.cache
            .borrow()
            .as_ref()
            .is_some_and(|c| c.toggle == "CommandOrControl+K"),
        "a refusal never reaches the cache"
    );

    combo(&s.window, Key::Control, "\u{3b1}");
    assert_eq!(
        s.window.get_settings_shortcut_rejection(),
        ShortcutRejection::Unmappable
    );
    assert_eq!(
        s.window.get_settings_recording_field(),
        ShortcutField::Toggle
    );

    combo(&s.window, Key::Control, "d");
    assert_eq!(s.recorder.saves(), vec![pair("CommandOrControl+D", "")]);
    assert_eq!(
        s.window.get_settings_shortcut_rejection(),
        ShortcutRejection::None
    );
    assert_eq!(s.window.get_settings_recording_field(), ShortcutField::None);
}

#[test]
fn a_conflict_with_the_other_binding_is_refused_before_any_effect() {
    let s = settings("CommandOrControl+K", "");
    activate(&s.window, PTT);
    combo(&s.window, Key::Control, "k");
    assert!(s.recorder.saves().is_empty());
    assert_eq!(
        s.window.get_settings_shortcut_rejection(),
        ShortcutRejection::Conflict
    );
    assert_eq!(s.window.get_settings_recording_field(), ShortcutField::Ptt);
    combo(&s.window, Key::Control, "l");
    assert_eq!(
        s.recorder.saves(),
        vec![pair("CommandOrControl+K", "CommandOrControl+L")]
    );
}

#[test]
fn a_modifier_alone_is_proposed_on_its_release_and_two_modifiers_are_not() {
    let s = settings("CommandOrControl+K", "");
    activate(&s.window, PTT);
    press(&s.window, Key::Shift);
    assert!(s.recorder.saves().is_empty(), "nothing before the release");
    release(&s.window, Key::Shift);
    assert_eq!(
        s.recorder.saves(),
        vec![pair("CommandOrControl+K", "ShiftLeft")]
    );

    activate(&s.window, PTT);
    press(&s.window, Key::Shift);
    press(&s.window, Key::Control);
    release(&s.window, Key::Control);
    release(&s.window, Key::Shift);
    assert_eq!(s.recorder.saves().len(), 1);
    assert_eq!(s.window.get_settings_recording_field(), ShortcutField::Ptt);

    press(&s.window, Key::Shift);
    tap(&s.window, "a");
    release(&s.window, Key::Shift);
    assert_eq!(
        s.recorder.saves().last(),
        Some(&pair("CommandOrControl+K", "Shift+A")),
        "a modifier followed by a key is that combination, not the modifier"
    );
}

#[test]
fn backspace_clears_only_without_a_modifier_and_the_icon_clears_too() {
    let s = settings("CommandOrControl+K", "CommandOrControl+L");
    activate(&s.window, PTT);
    combo(&s.window, Key::Control, Key::Backspace);
    assert_eq!(
        s.recorder.saves(),
        vec![pair("CommandOrControl+K", "CommandOrControl+Backspace")]
    );

    activate(&s.window, PTT);
    tap(&s.window, Key::Backspace);
    assert_eq!(
        s.recorder.saves().last(),
        Some(&pair("CommandOrControl+K", ""))
    );

    activate(&s.window, TOGGLE);
    tap(&s.window, Key::Delete);
    assert_eq!(s.recorder.saves().last(), Some(&pair("", "")));

    let s = settings("CommandOrControl+K", "CommandOrControl+L");
    let clear = find_button(&s.window, &format!("Effacer le raccourci « {PTT} »"));
    clear.invoke_accessible_default_action();
    settle();
    assert_eq!(s.recorder.saves(), vec![pair("CommandOrControl+K", "")]);
    assert_eq!(s.window.get_settings_ptt_shortcut_label(), "");
    assert_eq!(s.window.get_settings_recording_field(), ShortcutField::None);
}

#[test]
fn escape_and_focus_loss_leave_the_initial_value() {
    let s = settings("CommandOrControl+K", "");
    activate(&s.window, TOGGLE);
    tap(&s.window, Key::Escape);
    assert_eq!(s.window.get_settings_recording_field(), ShortcutField::None);
    assert!(s.recorder.saves().is_empty());

    activate(&s.window, TOGGLE);
    assert_eq!(
        s.window.get_settings_recording_field(),
        ShortcutField::Toggle
    );
    tap(&s.window, Key::Tab);
    assert_eq!(
        s.window.get_settings_shortcut_rejection(),
        ShortcutRejection::MissingModifier,
        "Tab is a proposal while capturing, not a way out"
    );
    click(
        &s.window,
        &find_button_by_name(&s.window, "Masquer l'overlay d'enregistrement"),
    );
    assert_eq!(s.window.get_settings_recording_field(), ShortcutField::None);
    combo(&s.window, Key::Control, "j");
    assert!(
        s.recorder.saves().is_empty(),
        "the cancelled capture has no ticket"
    );
    assert_eq!(s.window.get_settings_toggle_shortcut_label(), "\u{2318} K");
}

#[test]
fn activating_with_the_keyboard_does_not_record_the_activation_key() {
    let s = settings("CommandOrControl+K", "");
    activate(&s.window, TOGGLE);
    tap(&s.window, Key::Escape);
    for activation in [Key::Space, Key::Return] {
        tap(&s.window, activation);
        assert_eq!(
            s.window.get_settings_recording_field(),
            ShortcutField::Toggle,
            "{activation:?} starts the capture"
        );
        s.window
            .window()
            .dispatch_event(WindowEvent::KeyPressRepeated {
                text: activation.into(),
            });
        settle();
        assert!(s.recorder.saves().is_empty());
        assert_eq!(
            s.window.get_settings_shortcut_rejection(),
            ShortcutRejection::None,
            "{activation:?} and its repeat are not proposals"
        );
        tap(&s.window, Key::Escape);
    }
}

#[test]
fn two_fields_capture_independently() {
    let s = settings("CommandOrControl+K", "CommandOrControl+L");
    activate(&s.window, TOGGLE);
    activate(&s.window, PTT);
    assert_eq!(s.window.get_settings_recording_field(), ShortcutField::Ptt);
    combo(&s.window, Key::Control, "m");
    assert_eq!(
        s.recorder.saves(),
        vec![pair("CommandOrControl+K", "CommandOrControl+M")]
    );
    assert_eq!(s.window.get_settings_toggle_shortcut_label(), "\u{2318} K");
    assert_eq!(s.window.get_settings_ptt_shortcut_label(), "\u{2318} M");
}

#[test]
fn a_failed_save_shows_the_error_and_keeps_the_accepted_value() {
    let s = settings("CommandOrControl+K", "");
    *s.recorder.failure.borrow_mut() = Some("registration refused".into());
    activate(&s.window, TOGGLE);
    combo(&s.window, Key::Control, "j");
    assert!(s.recorder.saves().is_empty());
    assert_eq!(
        s.window.get_settings_shortcut_error(),
        "registration refused"
    );
    assert_eq!(s.window.get_settings_toggle_shortcut_label(), "\u{2318} K");
    assert_eq!(
        s.cache.borrow().as_ref().map(|c| c.toggle.clone()),
        Some("CommandOrControl+K".into())
    );
    assert_eq!(s.window.get_settings_recording_field(), ShortcutField::None);

    activate(&s.window, TOGGLE);
    assert_eq!(
        s.window.get_settings_shortcut_error(),
        "",
        "a new capture clears it"
    );
    combo(&s.window, Key::Control, "j");
    assert_eq!(s.recorder.saves(), vec![pair("CommandOrControl+J", "")]);
}

#[test]
fn every_departure_expires_the_ticket_even_inside_one_event() {
    let s = settings("CommandOrControl+K", "");

    activate(&s.window, TOGGLE);
    s.window.invoke_cancel_settings_capture();
    combo(&s.window, Key::Control, "j");
    assert!(s.recorder.saves().is_empty(), "cancel then immediate key");
    assert_eq!(s.window.get_settings_recording_field(), ShortcutField::None);

    activate(&s.window, TOGGLE);
    s.window
        .set_shortcut_generation(s.window.get_shortcut_generation() + 1);
    combo(&s.window, Key::Control, "j");
    assert!(s.recorder.saves().is_empty(), "a replaced target identity");

    activate(&s.window, TOGGLE);
    s.window.set_settings_tab(SettingsTab::Ai);
    s.window.set_settings_tab(SettingsTab::Interface);
    s.window.invoke_cancel_settings_capture();
    combo(&s.window, Key::Control, "j");
    assert!(s.recorder.saves().is_empty(), "tab round trip");

    activate(&s.window, TOGGLE);
    s.window.set_settings_tab(SettingsTab::Ai);
    settle();
    assert_eq!(s.window.get_settings_recording_field(), ShortcutField::None);
    s.window.set_settings_tab(SettingsTab::Interface);
    settle();
    combo(&s.window, Key::Control, "j");
    assert!(s.recorder.saves().is_empty(), "back on the tab, no ticket");

    activate(&s.window, TOGGLE);
    combo(&s.window, Key::Control, "j");
    assert_eq!(s.recorder.saves().len(), 1, "a new gesture works again");
}

#[test]
fn closing_settings_refuses_a_validation_before_the_flush_barrier_ends() {
    let s = settings("CommandOrControl+K", "");
    let weak = s.window.as_weak();
    s.window.on_settings_closed(move || {
        let window = weak.unwrap();
        assert!(!window.get_settings_shortcut_eligible());
    });
    activate(&s.window, TOGGLE);
    assert!(s.window.get_settings_shortcut_eligible());
    s.window.invoke_close_settings();
    assert!(!s.window.get_settings_open());
    assert!(!s.window.get_settings_shortcut_eligible());
    combo(&s.window, Key::Control, "j");
    assert!(s.recorder.saves().is_empty());

    s.window.set_settings_open(true);
    settle();
    combo(&s.window, Key::Control, "j");
    assert!(
        s.recorder.saves().is_empty(),
        "the old ticket does not come back"
    );
    assert_eq!(s.window.get_settings_recording_field(), ShortcutField::None);
    activate(&s.window, TOGGLE);
    combo(&s.window, Key::Control, "j");
    assert_eq!(s.recorder.saves().len(), 1);
}

#[test]
fn a_hidden_window_refuses_the_proposal_and_a_shown_one_starts_fresh() {
    let s = settings("CommandOrControl+K", "");
    activate(&s.window, TOGGLE);
    slint_kit::hide_window(&s.window).unwrap();
    s.window.show().unwrap();
    settle();
    combo(&s.window, Key::Control, "j");
    assert!(s.recorder.saves().is_empty());
    activate(&s.window, TOGGLE);
    combo(&s.window, Key::Control, "j");
    assert_eq!(s.recorder.saves().len(), 1);
}

#[test]
fn eligibility_follows_the_real_window_tab_and_modal_state() {
    let s = settings("CommandOrControl+K", "");
    let w = &s.window;
    assert!(w.get_settings_shortcut_eligible());
    w.set_settings_tab(SettingsTab::System);
    assert!(!w.get_settings_shortcut_eligible());
    w.set_settings_tab(SettingsTab::Interface);
    w.set_window_visible(false);
    assert!(!w.get_settings_shortcut_eligible());
    w.set_window_visible(true);
    w.set_onboarding_open(true);
    assert!(!w.get_settings_shortcut_eligible());
    w.set_onboarding_open(false);
    assert!(w.get_settings_shortcut_eligible());
    w.set_settings_open(false);
    assert!(!w.get_settings_shortcut_eligible());
}

#[test]
fn a_modal_over_the_page_blocks_the_capture() {
    let s = settings("CommandOrControl+K", "");
    activate(&s.window, TOGGLE);
    s.window.set_whats_new_open(true);
    settle();
    combo(&s.window, Key::Control, "j");
    assert!(s.recorder.saves().is_empty());
    s.window.set_whats_new_open(false);
    settle();
    combo(&s.window, Key::Control, "j");
    assert!(s.recorder.saves().is_empty(), "the old ticket stays dead");
}

struct Wizard {
    window: MainWindow,
    recorder: Recorder,
    state: Rc<RefCell<OnboardingState>>,
}

fn wizard(toggle: &str, push_to_talk: &str) -> Wizard {
    let window = window();
    let initial = pair(toggle, push_to_talk);
    let recorder = Recorder::new(initial.clone());
    let state = Rc::new(RefCell::new(OnboardingState {
        toggle_shortcut: initial.toggle.clone(),
        shortcuts: Some(initial.clone()),
        ..Default::default()
    }));
    wire_onboarding_shortcut_capture(&window, &state, recorder.backend());
    window.set_onboarding_toggle_shortcut_label(format_shortcut_label(&initial.toggle).into());
    window.set_onboarding_open(true);
    window.set_onboarding_step(OnboardingStep::Shortcut);
    window.show().unwrap();
    settle();
    Wizard {
        window,
        recorder,
        state,
    }
}

const WIZARD_FIELD: &str = "Raccourci clavier";

#[test]
fn the_wizard_checks_the_push_to_talk_binding_and_retries_without_a_click() {
    let w = wizard("CommandOrControl+K", "CommandOrControl+L");
    assert!(w.window.get_onboarding_shortcut_eligible());
    activate(&w.window, WIZARD_FIELD);
    assert!(w.window.get_onboarding_shortcut_recording());
    combo(&w.window, Key::Control, "l");
    assert!(w.recorder.saves().is_empty());
    assert_eq!(
        w.window.get_onboarding_shortcut_rejection(),
        ShortcutRejection::Conflict
    );
    assert!(w.window.get_onboarding_shortcut_recording());
    tap(&w.window, "d");
    assert_eq!(
        w.window.get_onboarding_shortcut_rejection(),
        ShortcutRejection::MissingModifier
    );
    combo(&w.window, Key::Control, "j");
    assert_eq!(
        w.recorder.saves(),
        vec![pair("CommandOrControl+J", "CommandOrControl+L")]
    );
    assert_eq!(
        w.window.get_onboarding_shortcut_rejection(),
        ShortcutRejection::None
    );
    assert!(!w.window.get_onboarding_shortcut_recording());
    assert_eq!(
        w.window.get_onboarding_toggle_shortcut_label(),
        "\u{2318} J"
    );
    assert_eq!(w.state.borrow().toggle_shortcut, "CommandOrControl+J");
}

#[test]
fn a_failed_wizard_save_keeps_the_previous_toggle() {
    let w = wizard("CommandOrControl+K", "");
    *w.recorder.failure.borrow_mut() = Some("registration refused".into());
    activate(&w.window, WIZARD_FIELD);
    combo(&w.window, Key::Control, "j");
    assert_eq!(
        w.window.get_onboarding_shortcut_error(),
        "registration refused"
    );
    assert_eq!(w.state.borrow().toggle_shortcut, "CommandOrControl+K");
    assert_eq!(
        w.window.get_onboarding_toggle_shortcut_label(),
        "\u{2318} K"
    );
    assert_eq!(
        w.state
            .borrow()
            .shortcuts
            .as_ref()
            .map(|s| s.toggle.clone()),
        Some("CommandOrControl+K".into())
    );
}

#[test]
fn leaving_the_shortcut_step_or_finishing_expires_the_wizard_ticket() {
    let w = wizard("CommandOrControl+K", "");
    activate(&w.window, WIZARD_FIELD);
    w.window.invoke_cancel_onboarding_capture();
    combo(&w.window, Key::Control, "j");
    assert!(w.recorder.saves().is_empty());
    assert!(!w.window.get_onboarding_shortcut_recording());

    activate(&w.window, WIZARD_FIELD);
    w.window.set_onboarding_busy(true);
    assert!(!w.window.get_onboarding_shortcut_eligible());
    combo(&w.window, Key::Control, "j");
    assert!(
        w.recorder.saves().is_empty(),
        "no validation while finishing"
    );
    w.window.set_onboarding_busy(false);
    settle();
    combo(&w.window, Key::Control, "j");
    assert!(w.recorder.saves().is_empty(), "and none after coming back");

    activate(&w.window, WIZARD_FIELD);
    set_onboarding_step_departing(&w.window, OnboardingStep::Model);
    assert!(!w.window.get_onboarding_shortcut_eligible());
    set_onboarding_step_departing(&w.window, OnboardingStep::Shortcut);
    combo(&w.window, Key::Control, "j");
    assert!(w.recorder.saves().is_empty());
}

#[test]
fn the_wizard_clear_action_publishes_the_empty_candidate() {
    let w = wizard("CommandOrControl+K", "");
    find_button(
        &w.window,
        &format!("Effacer le raccourci « {WIZARD_FIELD} »"),
    )
    .invoke_accessible_default_action();
    settle();
    assert_eq!(w.recorder.saves(), vec![pair("", "")]);
    assert_eq!(w.window.get_onboarding_toggle_shortcut_label(), "");
}

#[test]
fn a_rejection_message_follows_the_language() {
    let s = settings("CommandOrControl+K", "");
    activate(&s.window, TOGGLE);
    tap(&s.window, "d");
    let message = |text: &str| {
        ElementQuery::from_root(&s.window)
            .match_predicate({
                let text = text.to_string();
                move |e| {
                    e.accessible_description()
                        .is_some_and(|d| d.as_str() == text)
                }
            })
            .find_first()
    };
    let french = "Le raccourci doit inclure une touche de modification (Cmd, Ctrl, Maj, Alt) ou être une touche de fonction.";
    assert!(
        message(french).is_some(),
        "French description on the recorder"
    );
    select_app_locale(&s.window, crate::AppLocale::En);
    settle();
    let english =
        "The shortcut must include a modifier key (Cmd, Ctrl, Shift, Alt) or be a function key.";
    assert!(
        message(english).is_some(),
        "the message on screen was retranslated"
    );
    select_app_locale(&s.window, crate::AppLocale::Fr);
    settle();
    assert!(message(french).is_some());
    assert_eq!(
        s.window.get_settings_shortcut_rejection(),
        ShortcutRejection::MissingModifier
    );
}

#[test]
fn language_and_theme_switch_keep_the_open_capture_and_the_labels_follow() {
    let s = settings("CommandOrControl+K", "");
    activate(&s.window, TOGGLE);
    let trigger_value = |id: &str| {
        ElementHandle::find_by_element_id(&s.window, id)
            .next()
            .and_then(|e| {
                e.query_descendants()
                    .match_inherits("KitSelectTrigger")
                    .find_first()
            })
            .and_then(|t| t.accessible_value())
            .map(|v| v.to_string())
            .unwrap_or_default()
    };
    s.window.set_settings_locale(crate::AppLocale::Fr);
    s.window.set_settings_theme(crate::AppTheme::Dark);
    settle();
    assert_eq!(trigger_value("InterfaceSection::theme-picker"), "Sombre");
    assert_eq!(
        trigger_value("InterfaceSection::language-picker"),
        "Français"
    );
    s.window.set_settings_locale(crate::AppLocale::En);
    select_app_locale(&s.window, crate::AppLocale::En);
    settle();
    assert_eq!(trigger_value("InterfaceSection::theme-picker"), "Dark");
    assert_eq!(
        trigger_value("InterfaceSection::language-picker"),
        "English"
    );
    s.window.set_settings_theme(crate::AppTheme::Light);
    settle();
    assert_eq!(trigger_value("InterfaceSection::theme-picker"), "Light");
    s.window.set_settings_locale(crate::AppLocale::Fr);
    select_app_locale(&s.window, crate::AppLocale::Fr);
    settle();
    assert_eq!(trigger_value("InterfaceSection::theme-picker"), "Clair");
    assert_eq!(
        trigger_value("InterfaceSection::language-picker"),
        "Français"
    );
    assert_eq!(
        s.window.get_settings_recording_field(),
        ShortcutField::Toggle
    );
    combo(&s.window, Key::Control, "j");
    assert_eq!(
        s.recorder.saves(),
        vec![pair("CommandOrControl+J", "")],
        "a presentation change does not drop the capture"
    );
}

fn feedback_window() -> (MainWindow, Rc<RefCell<Vec<i32>>>) {
    let s = settings("CommandOrControl+K", "");
    let window = s.window;
    let options = souffle_lib::settings::SettingsOptions::current();
    assert_eq!(
        window.get_settings_feedback_sounds_volume_step(),
        options.feedback_sounds_volume_step as i32
    );
    window.set_settings_feedback_sounds_enabled(true);
    let proposals = Rc::new(RefCell::new(Vec::new()));
    let log = proposals.clone();
    let weak = window.as_weak();
    window.on_settings_feedback_sounds_volume_changed(move |value| {
        log.borrow_mut().push(value);
        weak.unwrap().set_settings_feedback_sounds_volume(value);
    });
    settle();
    std::mem::forget(s.recorder);
    (window, proposals)
}

fn focus_volume(window: &MainWindow) -> ElementHandle {
    let row = ElementHandle::find_by_element_id(window, "FeedbackSoundsSection::volume-row")
        .next()
        .expect("volume row");
    let label = row
        .query_descendants()
        .match_inherits("KitFieldLabel")
        .find_first()
        .expect("volume label");
    click(window, &label);
    ElementHandle::find_by_element_id(window, "FeedbackSoundsSection::volume-slider")
        .next()
        .expect("volume slider")
}

#[test]
fn volume_keyboard_steps_extremes_and_label_stay_in_agreement() {
    let (window, proposals) = feedback_window();
    let slider = focus_volume(&window);
    let start = window.get_settings_feedback_sounds_volume();
    tap(&window, Key::RightArrow);
    tap(&window, Key::RightArrow);
    tap(&window, Key::LeftArrow);
    assert_eq!(window.get_settings_feedback_sounds_volume(), start + 1);
    tap(&window, Key::End);
    assert_eq!(window.get_settings_feedback_sounds_volume(), 100);
    tap(&window, Key::RightArrow);
    assert_eq!(
        window.get_settings_feedback_sounds_volume(),
        100,
        "no step past the maximum"
    );
    tap(&window, Key::Home);
    assert_eq!(window.get_settings_feedback_sounds_volume(), 0);
    tap(&window, Key::LeftArrow);
    assert_eq!(window.get_settings_feedback_sounds_volume(), 0);
    assert!(proposals.borrow().iter().all(|v| (0..=100).contains(v)));
    let percent = ElementQuery::from_root(&window)
        .match_predicate(|e| e.accessible_label().is_some_and(|l| l.as_str() == "0%"))
        .find_first();
    assert!(
        percent.is_some(),
        "the visible percentage follows the accepted volume"
    );
    println!("slider accessible value {:?}", slider.accessible_value());
}

#[test]
fn volume_pointer_proposes_one_whole_step_on_release_and_a_cancelled_gesture_proposes_nothing() {
    let (window, proposals) = feedback_window();
    let slider = focus_volume(&window);
    let at = |fraction: f32| {
        slint::LogicalPosition::new(
            slider.absolute_position().x + slider.size().width * fraction,
            slider.absolute_position().y + slider.size().height / 2.0,
        )
    };
    let button = PointerEventButton::Left;
    let drag = |from: slint::LogicalPosition, to: slint::LogicalPosition, release: bool| {
        window
            .window()
            .dispatch_event(WindowEvent::PointerMoved { position: from });
        window.window().dispatch_event(WindowEvent::PointerPressed {
            position: from,
            button,
        });
        window
            .window()
            .dispatch_event(WindowEvent::PointerMoved { position: to });
        settle();
        if release {
            window
                .window()
                .dispatch_event(WindowEvent::PointerReleased {
                    position: to,
                    button,
                });
            settle();
        }
    };
    proposals.borrow_mut().clear();
    drag(at(0.3), at(0.5), true);
    let first = proposals.borrow().clone();
    assert_eq!(first.len(), 1, "one proposal, on release: {first:?}");
    assert!((0..=100).contains(&first[0]));
    assert_eq!(window.get_settings_feedback_sounds_volume(), first[0]);

    // A move of about half a step around the same point never yields a fraction.
    let one_step = slider.size().width / 100.0;
    let base = at(0.5);
    let near = slint::LogicalPosition::new(base.x + one_step / 2.0, base.y);
    drag(base, near, true);
    let second = *proposals.borrow().last().unwrap();
    assert!((second - first[0]).abs() <= 1, "{first:?} then {second}");

    let before = proposals.borrow().len();
    let accepted = window.get_settings_feedback_sounds_volume();
    drag(at(0.2), at(0.8), false);
    window.window().dispatch_event(WindowEvent::KeyPressed {
        text: Key::Escape.into(),
    });
    window.window().dispatch_event(WindowEvent::KeyReleased {
        text: Key::Escape.into(),
    });
    settle();
    window
        .window()
        .dispatch_event(WindowEvent::PointerMoved { position: at(0.9) });
    window
        .window()
        .dispatch_event(WindowEvent::PointerReleased {
            position: at(0.9),
            button,
        });
    settle();
    assert_eq!(
        proposals.borrow().len(),
        before,
        "a cancelled gesture proposes nothing"
    );
    assert_eq!(window.get_settings_feedback_sounds_volume(), accepted);
}

#[test]
fn the_sounds_switch_stays_usable_with_sounds_off_and_the_volume_row_follows() {
    let (window, _) = feedback_window();
    assert!(
        ElementHandle::find_by_element_id(&window, "FeedbackSoundsSection::volume-slider")
            .next()
            .is_some()
    );
    window.set_settings_feedback_sounds_enabled(false);
    settle();
    assert!(
        ElementHandle::find_by_element_id(&window, "FeedbackSoundsSection::volume-slider")
            .next()
            .is_none()
    );
    let toggles = Rc::new(RefCell::new(Vec::new()));
    let log = toggles.clone();
    window.on_settings_feedback_sounds_enabled_changed(move |value| log.borrow_mut().push(value));
    find_button_by_name(&window, "Activer les sons").invoke_accessible_default_action();
    settle();
    assert_eq!(&*toggles.borrow(), &[true]);
}

fn find_button_by_name(window: &MainWindow, label: &str) -> ElementHandle {
    ElementHandle::find_by_accessible_label(window, label)
        .find(|e| e.accessible_role() != Some(AccessibleRole::Text))
        .unwrap_or_else(|| panic!("no control labelled {label:?}"))
}

fn delay_window() -> (MainWindow, Rc<RefCell<Vec<i32>>>) {
    let s = settings("CommandOrControl+K", "");
    let window = s.window;
    window.set_settings_auto_paste(true);
    let proposals = Rc::new(RefCell::new(Vec::new()));
    let log = proposals.clone();
    let weak = window.as_weak();
    window.on_settings_paste_delay_changed(move |value| {
        log.borrow_mut().push(value);
        weak.unwrap().set_settings_paste_delay_ms(value);
    });
    settle();
    std::mem::forget(s.recorder);
    (window, proposals)
}

#[test]
fn the_delay_stepper_moves_by_the_host_step_and_never_leaves_its_bounds() {
    let (window, proposals) = delay_window();
    let options = souffle_lib::settings::SettingsOptions::current();
    assert_eq!(
        window.get_settings_paste_delay_step(),
        options.paste_delay_ms_step as i32
    );
    let (min, max, step) = (
        window.get_settings_paste_delay_min(),
        window.get_settings_paste_delay_max(),
        window.get_settings_paste_delay_step(),
    );
    assert_eq!((min, max, step), (50, 1000, 50));
    let more = || find_button_by_name(&window, "Augmenter");
    let less = || find_button_by_name(&window, "Diminuer");
    assert_eq!(window.get_settings_paste_delay_ms(), 100);
    more().invoke_accessible_default_action();
    settle();
    assert_eq!(window.get_settings_paste_delay_ms(), 150);
    less().invoke_accessible_default_action();
    less().invoke_accessible_default_action();
    settle();
    assert_eq!(
        window.get_settings_paste_delay_ms(),
        50,
        "stops at the minimum"
    );
    less().invoke_accessible_default_action();
    settle();
    assert_eq!(window.get_settings_paste_delay_ms(), 50);
    window.set_settings_paste_delay_ms(1000);
    settle();
    more().invoke_accessible_default_action();
    settle();
    assert_eq!(
        window.get_settings_paste_delay_ms(),
        1000,
        "stops at the maximum"
    );
    window.set_settings_paste_delay_ms(125);
    settle();
    more().invoke_accessible_default_action();
    settle();
    let up = window.get_settings_paste_delay_ms();
    println!("125 + one step = {up}");
    assert!((126..=1000).contains(&up));
    window.set_settings_paste_delay_ms(125);
    settle();
    less().invoke_accessible_default_action();
    settle();
    let down = window.get_settings_paste_delay_ms();
    println!("125 - one step = {down}");
    assert!((50..=124).contains(&down));
    assert!(
        proposals.borrow().iter().all(|v| (min..=max).contains(v)),
        "{:?}",
        proposals.borrow()
    );
}

#[test]
fn a_typed_out_of_range_delay_is_not_proposed_and_a_draft_survives_language_changes() {
    let (window, proposals) = delay_window();
    let stepper = ElementHandle::find_by_element_id(&window, "InterfaceSection::delay-stepper")
        .next()
        .expect("delay stepper");
    let input = stepper
        .query_descendants()
        .match_accessible_label("Délai de collage en millisecondes")
        .find_first()
        .expect("stepper editor");
    click(&window, &input);
    for ch in ["7", "0", "0", "0"] {
        tap(&window, ch);
    }
    let draft = input
        .accessible_value()
        .map(|v| v.to_string())
        .unwrap_or_default();
    println!("typed draft {draft:?}");
    assert!(draft.ends_with("7000"), "{draft}");
    select_app_locale(&window, crate::AppLocale::En);
    window.set_settings_locale(crate::AppLocale::En);
    settle();
    window.set_settings_theme(crate::AppTheme::Dark);
    settle();
    assert_eq!(
        input.accessible_value().map(|v| v.to_string()),
        Some(draft.clone()),
        "the draft survives language and theme"
    );
    tap(&window, Key::Return);
    println!(
        "proposals after commit: {:?}, host value {}",
        proposals.borrow(),
        window.get_settings_paste_delay_ms()
    );
    assert!(proposals.borrow().iter().all(|v| (50..=1000).contains(v)));
    select_app_locale(&window, crate::AppLocale::Fr);
}
