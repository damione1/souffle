//! The first-run wizard and the permissions step it shares with the Settings
//! dialog, driven through the real `MainWindow`, the kit controls and Soufflé's
//! own step functions (`show_onboarding_step`, `advance_onboarding_step`,
//! `retreat_onboarding_step`, `refresh_onboarding_devices`, the shortcut
//! handlers, the permission projection and the window close handler).
//!
//! The engine, the settings database, the TCC prompts and the preference and
//! flag files of the user are never touched: the model status is a probe, the
//! shortcut registration a recorder, and the Continue button of the wizard is
//! answered by a stand-in that takes the same branches as the live handler
//! (advance, request a download, finish) without its side effects.
use super::*;
use crate::permissions_ui::PermissionController;
use crate::shortcut_ui_tests::{
    Recorder, activate, click, combo, find_button, pair, press, release, settle, tap, window,
};
use crate::{
    AccessState, ModelPhase, OnboardingStep, PermissionKind as SlintKind, PermissionRow,
    ShortcutRejection,
};
use i_slint_backend_testing::{AccessibleRole, ElementHandle, ElementQuery};
use slint::platform::{Key, PointerEventButton, WindowEvent};
use slint::{ComponentHandle, Model};
use souffle_lib::audio::{AudioInputDevice, TransportType};
use souffle_lib::engine::TranscriptionRuntimePhase;
use souffle_lib::permissions::{PermState, PermissionStatus};

fn device(uid: &str, name: &str) -> AudioInputDevice {
    AudioInputDevice {
        uid: uid.into(),
        name: name.into(),
        transport: TransportType::Usb,
        is_default: false,
    }
}

fn option(model: &str, label: &str) -> model_ui::FlatModelOption {
    model_ui::FlatModelOption {
        engine_id: "engine".into(),
        model_id: model.into(),
        backend_id: "backend".into(),
        label: label.into(),
    }
}

struct Lab {
    window: MainWindow,
    state: Rc<RefCell<OnboardingState>>,
    permissions: Rc<PermissionController>,
    probe: ModelPhaseProbe,
    phase: Rc<Cell<Option<TranscriptionRuntimePhase>>>,
    events: Rc<RefCell<Vec<String>>>,
    recorder: Recorder,
}

impl Lab {
    fn events(&self) -> Vec<String> {
        self.events.borrow().clone()
    }

    fn clear_events(&self) {
        self.events.borrow_mut().clear();
    }

    fn advance(&self) {
        advance_onboarding_step(&self.window, &self.probe, &self.state, &self.permissions);
        settle();
    }

    fn back(&self) {
        click(&self.window, &find_button(&self.window, "Retour"));
    }

    fn proceed(&self) {
        let label = self.continue_label();
        click(&self.window, &find_button(&self.window, &label));
    }

    fn continue_label(&self) -> String {
        [
            "Télécharger et continuer",
            "Terminer",
            "Continuer",
            "Download and continue",
            "Finish",
            "Continue",
        ]
        .into_iter()
        .find(|label| maybe_button(&self.window, label).is_some())
        .expect("the footer has a primary button")
        .to_string()
    }

    fn refresh(&self, devices: Vec<AudioInputDevice>) {
        refresh_onboarding_devices(
            &self.window,
            &self.probe,
            &self.state,
            &self.permissions,
            devices,
        );
        settle();
    }
}

fn lab() -> Lab {
    let window = window();
    let initial = pair("CommandOrControl+K", "CommandOrControl+L");
    let recorder = Recorder::new(initial.clone());
    let state = Rc::new(RefCell::new(OnboardingState {
        steps: vec![
            OnboardingStep::Permissions,
            OnboardingStep::Microphone,
            OnboardingStep::Model,
            OnboardingStep::Shortcut,
        ],
        devices: vec![device("a", "Micro A"), device("b", "Micro B")],
        model_options: vec![option("m1", "Whisper"), option("m2", "Parakeet")],
        selected_model_index: Some(0),
        toggle_shortcut: initial.toggle.clone(),
        shortcuts: Some(initial),
        ..Default::default()
    }));
    let permissions = PermissionController::new(&window, Rc::new(Cell::new(false)));
    let phase = Rc::new(Cell::new(None));
    let probe: ModelPhaseProbe = {
        let phase = phase.clone();
        Rc::new(move |_| phase.get())
    };
    let events: Rc<RefCell<Vec<String>>> = Rc::default();

    let log = events.clone();
    window.on_onboarding_grant_requested(move |kind| {
        log.borrow_mut().push(format!("grant {kind:?}"))
    });
    let log = events.clone();
    window.on_onboarding_hint_action_requested(move |kind| {
        log.borrow_mut().push(format!("settings {kind:?}"))
    });
    let log = events.clone();
    window.on_onboarding_repair_requested(move || log.borrow_mut().push("repair".into()));
    let log = events.clone();
    window.on_onboarding_review_accessibility_requested(move || {
        log.borrow_mut().push("review accessibility".into())
    });
    let (weak, st) = (window.as_weak(), state.clone());
    let log = events.clone();
    window.on_onboarding_device_changed(move |index| {
        log.borrow_mut().push(format!("device {index}"));
        select_onboarding_device(&weak.unwrap(), &st, index);
    });
    let (weak, st) = (window.as_weak(), state.clone());
    let log = events.clone();
    window.on_onboarding_model_picked(move |index| {
        log.borrow_mut().push(format!("model {index}"));
        pick_onboarding_model(&weak.unwrap(), &st, index);
    });
    let (weak, st) = (window.as_weak(), state.clone());
    let (pm, perms) = (probe.clone(), permissions.clone());
    let log = events.clone();
    window.on_onboarding_refresh_devices_requested(move || {
        log.borrow_mut().push("refresh".into());
        let devices = st.borrow().devices.clone();
        refresh_onboarding_devices(&weak.unwrap(), &pm, &st, &perms, devices);
    });
    let (weak, st) = (window.as_weak(), state.clone());
    let (pm, perms) = (probe.clone(), permissions.clone());
    let log = events.clone();
    window.on_onboarding_back_requested(move || {
        log.borrow_mut().push("back".into());
        retreat_onboarding_step(&weak.unwrap(), &pm, &st, &perms);
    });
    let (weak, st) = (window.as_weak(), state.clone());
    let (pm, perms) = (probe.clone(), permissions.clone());
    let log = events.clone();
    window.on_onboarding_continue_requested(move || {
        let w = weak.unwrap();
        let step = w.get_onboarding_step();
        log.borrow_mut().push(format!("continue {step:?}"));
        match step {
            OnboardingStep::Permissions | OnboardingStep::Microphone => {
                advance_onboarding_step(&w, &pm, &st, &perms)
            }
            OnboardingStep::Model if w.get_onboarding_model_phase() == ModelPhase::Ready => {
                advance_onboarding_step(&w, &pm, &st, &perms)
            }
            OnboardingStep::Model => log.borrow_mut().push("download requested".into()),
            OnboardingStep::Shortcut => log.borrow_mut().push("finish requested".into()),
        }
    });
    let weak = window.as_weak();
    let log = events.clone();
    window.on_onboarding_locale_changed(move |locale| {
        log.borrow_mut().push(format!("locale {locale:?}"));
        let w = weak.unwrap();
        w.set_settings_locale(locale);
        select_app_locale(&w, locale);
    });
    wire_onboarding_shortcut_capture(&window, &state, recorder.backend());

    window.set_settings_locale(crate::AppLocale::Fr);
    window.set_onboarding_open(true);
    show_onboarding_step(&window, &probe, &state, &permissions);
    window.show().unwrap();
    settle();
    Lab {
        window,
        state,
        permissions,
        probe,
        phase,
        events,
        recorder,
    }
}

fn maybe_button(window: &MainWindow, label: &str) -> Option<ElementHandle> {
    ElementHandle::find_by_accessible_label(window, label)
        .find(|e| e.accessible_role() == Some(AccessibleRole::Button))
}

fn labelled(window: &MainWindow, role: AccessibleRole) -> Vec<String> {
    ElementQuery::from_root(window)
        .match_accessible_role(role)
        .find_all()
        .iter()
        .filter_map(ElementHandle::accessible_label)
        .map(|label| label.to_string())
        .collect()
}

fn has_text(window: &MainWindow, text: &str) -> bool {
    labelled(window, AccessibleRole::Text)
        .iter()
        .any(|label| label == text)
}

fn alerts(window: &MainWindow) -> Vec<String> {
    labelled(window, AccessibleRole::Groupbox)
}

fn trigger(window: &MainWindow, value: &str) -> ElementHandle {
    ElementQuery::from_root(window)
        .match_inherits("KitSelectTrigger")
        .find_all()
        .into_iter()
        .find(|e| e.accessible_value().as_deref() == Some(value))
        .unwrap_or_else(|| panic!("no select showing {value:?}"))
}

fn options(window: &MainWindow) -> Vec<ElementHandle> {
    ElementQuery::from_root(window)
        .match_inherits("KitOption")
        .find_all()
}

fn option_labelled(window: &MainWindow, label: &str) -> ElementHandle {
    options(window)
        .into_iter()
        .find(|e| e.accessible_label().as_deref() == Some(label))
        .unwrap_or_else(|| panic!("no option {label:?}"))
}

/// A popup row is placed by the window: the element knows where to press.
fn choose(window: &MainWindow, label: &str) {
    option_labelled(window, label).mock_single_click(PointerEventButton::Left);
    settle();
}

fn badges(window: &MainWindow, text: &str) -> usize {
    ElementQuery::from_root(window)
        .match_inherits("KitBadge")
        .find_all()
        .iter()
        .filter(|badge| badge.accessible_label().as_deref() == Some(text))
        .count()
}

fn key(window: &MainWindow, text: impl Into<slint::SharedString> + Clone) {
    tap(window, text);
}

fn status_all(state: PermState) -> PermissionStatus {
    PermissionStatus {
        microphone: state,
        system_audio: state,
        accessibility: state,
        calendar: state,
    }
}

/// The wizard projects three rows; Calendar goes through the same Slint row model.
fn project_permissions(lab: &Lab, status: &PermissionStatus, busy: [bool; 3]) {
    onboarding_ui::populate_permission_rows(&lab.window, status, busy);
    settle();
}

fn with_calendar(window: &MainWindow, state: AccessState, busy: bool) {
    let rows = window.get_onboarding_permission_rows();
    let model = rows
        .as_any()
        .downcast_ref::<slint::VecModel<PermissionRow>>()
        .expect("rows are a VecModel");
    model.push(PermissionRow {
        kind: SlintKind::Calendar,
        state,
        busy,
    });
    settle();
}

// Navigation ---------------------------------------------------------------

#[test]
fn the_four_steps_advance_back_and_keep_every_choice() {
    let lab = lab();
    let w = &lab.window;
    assert_eq!(w.get_onboarding_step(), OnboardingStep::Permissions);
    assert_eq!(w.get_onboarding_step_index(), 0);
    assert_eq!(w.get_onboarding_step_count(), 4);
    assert!(
        maybe_button(w, "Retour").is_none(),
        "no Back on the first step"
    );
    assert!(has_text(
        w,
        "Soufflé a besoin de ces autorisations macOS pour fonctionner."
    ));
    assert_eq!(lab.continue_label(), "Continuer");

    lab.proceed();
    assert_eq!(w.get_onboarding_step(), OnboardingStep::Microphone);
    assert_eq!(w.get_onboarding_step_index(), 1);
    assert!(has_text(
        w,
        "Vous pourrez changer ce choix plus tard dans les Réglages."
    ));
    assert!(maybe_button(w, "Retour").is_some());

    // Choose the second microphone through the real select.
    click(w, &trigger(w, "Automatique"));
    choose(w, "Micro B");
    assert_eq!(w.get_onboarding_selected_device_index(), 2);
    assert_eq!(lab.state.borrow().selected_device, "b");

    lab.proceed();
    assert_eq!(w.get_onboarding_step(), OnboardingStep::Model);
    assert_eq!(lab.continue_label(), "Télécharger et continuer");
    click(w, &trigger(w, "Whisper"));
    choose(w, "Parakeet");
    assert_eq!(lab.state.borrow().selected_model_index, Some(1));

    // Not downloaded yet: Continue asks for the download and stays on the step.
    lab.clear_events();
    lab.proceed();
    assert_eq!(lab.events(), ["continue Model", "download requested"]);
    assert_eq!(w.get_onboarding_step(), OnboardingStep::Model);
    w.set_onboarding_model_phase(ModelPhase::Ready);
    settle();
    assert_eq!(lab.continue_label(), "Continuer");
    lab.proceed();
    assert_eq!(w.get_onboarding_step(), OnboardingStep::Shortcut);
    assert_eq!(w.get_onboarding_step_index(), 3);
    assert_eq!(lab.continue_label(), "Terminer");
    assert_eq!(w.get_onboarding_toggle_shortcut_label(), "\u{2318} K");

    lab.clear_events();
    lab.proceed();
    assert_eq!(lab.events(), ["continue Shortcut", "finish requested"]);

    // Back keeps the microphone and the model, and Back is gone on the first step.
    lab.back();
    assert_eq!(w.get_onboarding_step(), OnboardingStep::Model);
    assert_eq!(w.get_onboarding_selected_model_index(), 1);
    assert_eq!(lab.state.borrow().selected_model_index, Some(1));
    lab.back();
    assert_eq!(w.get_onboarding_step(), OnboardingStep::Microphone);
    assert_eq!(w.get_onboarding_selected_device_index(), 2);
    trigger(w, "Micro B");
    lab.back();
    assert_eq!(w.get_onboarding_step(), OnboardingStep::Permissions);
    assert!(maybe_button(w, "Retour").is_none());
    lab.proceed();
    assert_eq!(w.get_onboarding_selected_device_index(), 2);
}

#[test]
fn a_resumed_wizard_starts_on_the_step_the_install_still_needs() {
    use crate::onboarding_flags::{SetupFlags, wizard_steps};
    let lab = lab();
    let w = &lab.window;
    let resume = |flags: SetupFlags| {
        {
            let mut state = lab.state.borrow_mut();
            state.steps = wizard_steps(flags);
            state.step_index = 0;
        }
        show_onboarding_step(w, &lab.probe, &lab.state, &lab.permissions);
        settle();
    };

    // Permissions already granted: the wizard opens on the microphone, three steps long.
    resume(SetupFlags {
        permissions_done: true,
        setup_done: false,
    });
    assert_eq!(w.get_onboarding_step(), OnboardingStep::Microphone);
    assert_eq!(
        (w.get_onboarding_step_index(), w.get_onboarding_step_count()),
        (0, 3)
    );
    assert!(maybe_button(w, "Retour").is_none(), "nothing to go back to");
    assert_eq!(
        ElementQuery::from_root(w)
            .match_accessible_role(AccessibleRole::ProgressIndicator)
            .find_all()
            .len(),
        1
    );
    lab.advance();
    lab.advance();
    assert_eq!(w.get_onboarding_step(), OnboardingStep::Shortcut);
    assert_eq!(lab.continue_label(), "Terminer");

    // A set-up install that lost its model: the model step alone, no progress bar.
    resume(SetupFlags {
        permissions_done: true,
        setup_done: true,
    });
    assert_eq!(w.get_onboarding_step(), OnboardingStep::Model);
    assert_eq!(
        (w.get_onboarding_step_index(), w.get_onboarding_step_count()),
        (0, 1)
    );
    assert!(maybe_button(w, "Retour").is_none());
    assert!(
        ElementQuery::from_root(w)
            .match_accessible_role(AccessibleRole::ProgressIndicator)
            .find_all()
            .is_empty(),
        "a single step shows no progress"
    );
    assert_eq!(lab.continue_label(), "Télécharger et continuer");
    lab.phase.set(Some(TranscriptionRuntimePhase::Ready));
    resume(SetupFlags {
        permissions_done: true,
        setup_done: true,
    });
    assert_eq!(w.get_onboarding_model_phase(), ModelPhase::Ready);
    assert_eq!(lab.continue_label(), "Terminer");

    // Nothing done yet: the full four steps.
    resume(SetupFlags::default());
    assert_eq!(w.get_onboarding_step(), OnboardingStep::Permissions);
    assert_eq!(w.get_onboarding_step_count(), 4);
}

#[test]
fn the_footer_buttons_follow_the_real_prerequisites() {
    let lab = lab();
    let w = &lab.window;
    lab.advance();
    let enabled = |label: &str| find_button(w, label).accessible_enabled();
    assert_eq!(enabled("Continuer"), Some(true));
    assert_eq!(enabled("Retour"), Some(true));

    w.set_onboarding_busy(true);
    settle();
    assert_eq!(enabled("Continuer"), Some(false), "busy blocks Continue");
    assert_eq!(enabled("Retour"), Some(false), "busy blocks Back");
    w.set_onboarding_busy(false);
    w.set_onboarding_continue_enabled(false);
    settle();
    assert_eq!(
        enabled("Continuer"),
        Some(false),
        "a save in flight blocks Continue"
    );
    assert_eq!(enabled("Retour"), Some(true));
    lab.clear_events();
    find_button(w, "Continuer").invoke_accessible_default_action();
    settle();
    assert!(lab.events().is_empty(), "a disabled Continue does nothing");

    // Entering a step again resets both, as the live handler does on every step.
    lab.advance();
    assert!(!w.get_onboarding_busy());
    assert!(w.get_onboarding_continue_enabled());
    assert_eq!(enabled("Télécharger et continuer"), Some(true));
}

#[test]
fn the_language_choice_retranslates_the_wizard_in_place() {
    let lab = lab();
    let w = &lab.window;
    let radios = |label: &str| {
        ElementQuery::from_root(w)
            .match_accessible_role(AccessibleRole::RadioButton)
            .match_accessible_label(label)
            .find_first()
    };
    let english = radios("English").expect("English segment");
    assert!(radios("Français").is_some());
    click(w, &english);
    assert_eq!(lab.events(), ["locale En"]);
    assert_eq!(w.get_settings_locale(), crate::AppLocale::En);
    assert!(has_text(
        w,
        "Soufflé needs these macOS permissions to work."
    ));
    assert_eq!(lab.continue_label(), "Continue");
    click(
        w,
        &radios("French").expect("the French segment, named in English"),
    );
    assert!(has_text(
        w,
        "Soufflé a besoin de ces autorisations macOS pour fonctionner."
    ));
    // The segment is a first-step choice only.
    lab.advance();
    assert!(radios("English").is_none());
}

// Permissions ---------------------------------------------------------------

fn grant_name(kind: &str, action: &str) -> String {
    format!("{kind}, {action}")
}

#[test]
fn every_permission_state_shows_its_action_hint_and_settings_shortcut() {
    let lab = lab();
    let w = &lab.window;
    let grant_buttons = |actions: [(&str, &str); 4]| -> Vec<String> {
        actions
            .iter()
            .map(|(kind, action)| grant_name(kind, action))
            .collect()
    };
    let unknown_or_denied = grant_buttons([
        ("Microphone", "Autoriser"),
        ("Audio système", "Autoriser"),
        ("Accessibilité", "Ouvrir les Réglages"),
        ("Calendrier", "Autoriser"),
    ]);

    // Not asked yet: every row offers its action, nothing says "denied".
    project_permissions(&lab, &status_all(PermState::Unknown), [false; 3]);
    with_calendar(w, AccessState::Unknown, false);
    for name in &unknown_or_denied {
        assert!(maybe_button(w, name).is_some(), "{name}");
    }
    assert!(alerts(w).iter().all(|a| !a.starts_with("Refusé")));

    // Denied: the same actions, plus the way to the matching System Settings pane.
    project_permissions(&lab, &status_all(PermState::Denied), [false; 3]);
    with_calendar(w, AccessState::Denied, false);
    for name in &unknown_or_denied {
        assert!(maybe_button(w, name).is_some(), "{name}");
    }
    let hints = alerts(w);
    assert!(hints.contains(&"Refusé. Ouvrez Réglages Système pour l'autoriser.".to_string()));
    assert!(
        hints.contains(
            &"Refusé. Ouvrez Réglages Système > Confidentialité et sécurité > Accessibilité."
                .to_string()
        )
    );
    let settings_buttons = ElementQuery::from_root(w)
        .match_accessible_role(AccessibleRole::Button)
        .match_accessible_label("Réglages Système")
        .find_all();
    assert_eq!(settings_buttons.len(), 2, "microphone and accessibility");
    lab.clear_events();
    for button in &settings_buttons {
        button.invoke_accessible_default_action();
    }
    settle();
    assert_eq!(
        lab.events(),
        ["settings Microphone", "settings Accessibility"]
    );

    // Every action reports its kind and nothing else.
    lab.clear_events();
    for name in &unknown_or_denied {
        find_button(w, name).invoke_accessible_default_action();
    }
    settle();
    assert_eq!(
        lab.events(),
        [
            "grant Microphone",
            "grant SystemAudio",
            "grant Accessibility",
            "grant Calendar"
        ]
    );

    // No device: a microphone hint without a Settings detour.
    project_permissions(&lab, &status_all(PermState::NoDevice), [false; 3]);
    with_calendar(w, AccessState::NoDevice, false);
    assert!(alerts(w).contains(&"Aucun microphone détecté.".to_string()));
    assert!(
        ElementQuery::from_root(w)
            .match_accessible_label("Réglages Système")
            .find_all()
            .is_empty()
    );

    // Granted: a badge each, no action, no hint.
    project_permissions(&lab, &status_all(PermState::Granted), [false; 3]);
    with_calendar(w, AccessState::Granted, false);
    assert_eq!(badges(w, "Accordé"), 4);
    for name in &unknown_or_denied {
        assert!(maybe_button(w, name).is_none(), "{name}");
    }
    assert!(
        alerts(w)
            .iter()
            .all(|a| !a.contains("Refusé") && !a.contains("Aucun"))
    );

    // Unsupported by this macOS: said, not offered.
    project_permissions(&lab, &status_all(PermState::Unsupported), [false; 3]);
    with_calendar(w, AccessState::Unsupported, false);
    assert_eq!(badges(w, "Non pris en charge"), 4);
    for name in &unknown_or_denied {
        assert!(maybe_button(w, name).is_none(), "{name}");
    }
}

#[test]
fn a_permission_in_flight_is_checking_and_cannot_be_requested_twice() {
    let lab = lab();
    let w = &lab.window;
    project_permissions(&lab, &status_all(PermState::Unknown), [true, false, true]);
    for name in ["Microphone, Vérification…", "Accessibilité, Vérification…"] {
        let button = find_button(w, name);
        assert_eq!(button.accessible_enabled(), Some(false), "{name}");
    }
    assert_eq!(
        find_button(w, "Audio système, Autoriser").accessible_enabled(),
        Some(true)
    );
    lab.clear_events();
    find_button(w, "Microphone, Vérification…").invoke_accessible_default_action();
    settle();
    assert!(lab.events().is_empty());
    project_permissions(&lab, &status_all(PermState::Granted), [false; 3]);
    assert!(maybe_button(w, "Microphone, Vérification…").is_none());
}

#[test]
fn a_press_on_a_permission_button_survives_the_poll_that_lands_before_the_release() {
    let lab = lab();
    let w = &lab.window;
    project_permissions(&lab, &status_all(PermState::Unknown), [false; 3]);
    let button = find_button(w, "Microphone, Autoriser");
    let position = slint::LogicalPosition::new(
        button.absolute_position().x + button.size().width / 2.,
        button.absolute_position().y + button.size().height / 2.,
    );
    lab.clear_events();
    w.window()
        .dispatch_event(WindowEvent::PointerMoved { position });
    w.window().dispatch_event(WindowEvent::PointerPressed {
        position,
        button: PointerEventButton::Left,
    });
    settle();
    // The 600 ms poll projects the same status again between press and release.
    project_permissions(&lab, &status_all(PermState::Unknown), [false; 3]);
    w.window().dispatch_event(WindowEvent::PointerReleased {
        position,
        button: PointerEventButton::Left,
    });
    settle();
    assert_eq!(lab.events(), ["grant Microphone"]);
}

#[test]
fn the_repair_row_shows_busy_cooldown_success_and_the_error() {
    let lab = lab();
    let w = &lab.window;
    project_permissions(&lab, &status_all(PermState::Denied), [false; 3]);
    let text = "Un problème persiste ? Réparez l'entrée Accessibilité.";
    assert!(has_text(w, text));
    assert_eq!(find_button(w, "Réparer").accessible_enabled(), Some(true));
    lab.clear_events();
    find_button(w, "Réparer").invoke_accessible_default_action();
    settle();
    assert_eq!(lab.events(), ["repair"]);

    w.set_onboarding_repair_busy(true);
    settle();
    assert_eq!(
        find_button(w, "Vérification…").accessible_enabled(),
        Some(false)
    );
    w.set_onboarding_repair_busy(false);
    w.set_onboarding_repair_success(true);
    w.set_onboarding_repair_cooldown(true);
    settle();
    assert!(has_text(
        w,
        "Réparation effectuée, en attente de confirmation macOS."
    ));
    assert!(!has_text(w, text));
    assert_eq!(find_button(w, "Réparer").accessible_enabled(), Some(false));
    w.set_onboarding_repair_cooldown(false);
    settle();
    assert_eq!(find_button(w, "Réparer").accessible_enabled(), Some(true));

    assert!(!alerts(w).contains(&"macOS a refusé la réparation".to_string()));
    w.set_onboarding_permissions_error("macOS a refusé la réparation".into());
    settle();
    assert!(alerts(w).contains(&"macOS a refusé la réparation".to_string()));
    w.set_onboarding_permissions_error("".into());
    settle();
    assert!(!alerts(w).contains(&"macOS a refusé la réparation".to_string()));
}

// Microphone ----------------------------------------------------------------

fn open_microphone_step(lab: &Lab) {
    lab.advance();
    assert_eq!(lab.window.get_onboarding_step(), OnboardingStep::Microphone);
}

#[test]
fn no_device_offers_only_automatic_and_a_refresh_brings_the_devices() {
    let lab = lab();
    let w = &lab.window;
    lab.state.borrow_mut().devices.clear();
    open_microphone_step(&lab);
    click(w, &trigger(w, "Automatique"));
    assert_eq!(options(w).len(), 1, "only Automatic without a device");
    key(w, Key::Escape);
    lab.clear_events();
    click(w, &find_button(w, "Actualiser"));
    assert_eq!(lab.events(), ["refresh"]);

    lab.refresh(vec![device("a", "Micro A")]);
    click(w, &trigger(w, "Automatique"));
    let labels: Vec<_> = options(w)
        .iter()
        .filter_map(ElementHandle::accessible_label)
        .collect();
    assert_eq!(labels, ["Automatique", "Micro A"]);
    key(w, Key::Escape);
}

#[test]
fn a_pinned_microphone_that_vanishes_shows_and_saves_automatic() {
    let lab = lab();
    let w = &lab.window;
    open_microphone_step(&lab);
    click(w, &trigger(w, "Automatique"));
    choose(w, "Micro B");
    assert_eq!(lab.state.borrow().selected_device, "b");

    lab.refresh(vec![device("a", "Micro A")]);
    assert_eq!(w.get_onboarding_selected_device_index(), 0);
    assert_eq!(
        lab.state.borrow().selected_device,
        "",
        "what the select shows is what Continue saves"
    );
    trigger(w, "Automatique");

    // A device that is still there keeps its place across a refresh.
    click(w, &trigger(w, "Automatique"));
    choose(w, "Micro A");
    lab.refresh(vec![device("b", "Micro B"), device("a", "Micro A")]);
    assert_eq!(w.get_onboarding_selected_device_index(), 2);
    assert_eq!(lab.state.borrow().selected_device, "a");
    trigger(w, "Micro A");
}

#[test]
fn a_refresh_under_an_open_menu_resolves_the_choice_in_the_new_list() {
    let lab = lab();
    let w = &lab.window;
    open_microphone_step(&lab);
    click(w, &trigger(w, "Automatique"));
    assert_eq!(options(w).len(), 3);
    lab.clear_events();
    // Same labels, other identities: the row labelled Micro B is another device now.
    lab.refresh(vec![device("c", "Micro A"), device("d", "Micro B")]);
    assert_eq!(lab.state.borrow().selected_device, "");
    if options(w).is_empty() {
        click(w, &trigger(w, "Automatique"));
    }
    choose(w, "Micro B");
    assert_eq!(lab.events(), ["device 2"]);
    assert_eq!(lab.state.borrow().selected_device, "d", "never the stale b");

    // A device that disappears while the menu is open leaves one row fewer.
    click(w, &trigger(w, "Micro B"));
    lab.refresh(vec![device("c", "Micro A")]);
    assert_eq!(lab.state.borrow().selected_device, "");
    assert!(
        options(w)
            .iter()
            .all(|o| o.accessible_label().as_deref() != Some("Micro B"))
    );
    key(w, Key::Escape);
}

// Model ---------------------------------------------------------------------

fn open_model_step(lab: &Lab) {
    lab.advance();
    lab.advance();
    assert_eq!(lab.window.get_onboarding_step(), OnboardingStep::Model);
}

#[test]
fn the_model_step_shows_each_phase_and_the_engine_probe_decides_on_entry() {
    let lab = lab();
    let w = &lab.window;
    open_model_step(&lab);
    assert_eq!(w.get_onboarding_model_phase(), ModelPhase::Pick);
    assert_eq!(trigger(w, "Whisper").accessible_enabled(), Some(true));
    assert_eq!(lab.continue_label(), "Télécharger et continuer");

    // The picker is locked while the choice is being saved.
    w.set_onboarding_busy(true);
    settle();
    assert_ne!(trigger(w, "Whisper").accessible_enabled(), Some(true));
    w.set_onboarding_busy(false);
    settle();

    w.set_onboarding_model_phase(ModelPhase::Downloading);
    w.set_onboarding_download_progress_fraction(0.25);
    w.set_onboarding_download_progress_label("model.bin".into());
    settle();
    let bar = ElementQuery::from_root(w)
        .match_accessible_role(AccessibleRole::ProgressIndicator)
        .match_accessible_label("Téléchargement du modèle…")
        .find_first()
        .expect("download progress");
    assert_eq!(bar.accessible_value().as_deref(), Some("25"));
    assert!(
        ElementQuery::from_root(w)
            .match_inherits("KitSelectTrigger")
            .find_all()
            .is_empty(),
        "no picker while downloading"
    );

    w.set_onboarding_model_phase(ModelPhase::Loading);
    settle();
    assert!(
        ElementQuery::from_root(w)
            .match_accessible_role(AccessibleRole::ProgressIndicator)
            .match_accessible_label("Chargement du modèle…")
            .find_first()
            .is_some()
    );

    w.set_onboarding_model_phase(ModelPhase::Ready);
    settle();
    assert!(has_text(w, "Modèle prêt."));
    assert_eq!(lab.continue_label(), "Continuer");

    // An engine error comes back as the banner and the step can be retried.
    w.set_onboarding_model_phase(ModelPhase::Pick);
    w.set_onboarding_status_message("Le modèle est en erreur.".into());
    settle();
    assert!(alerts(w).contains(&"Le modèle est en erreur.".to_string()));
    assert_eq!(lab.continue_label(), "Télécharger et continuer");
    assert_eq!(
        find_button(w, "Télécharger et continuer").accessible_enabled(),
        Some(true)
    );

    // Entering the step reads the engine: a ready model skips the download wording.
    lab.back();
    lab.phase.set(Some(TranscriptionRuntimePhase::Ready));
    lab.advance();
    assert_eq!(w.get_onboarding_model_phase(), ModelPhase::Ready);
    assert_eq!(
        w.get_onboarding_status_message(),
        "",
        "a step starts without the last error"
    );
    lab.back();
    lab.phase.set(Some(TranscriptionRuntimePhase::Downloading));
    lab.advance();
    assert_eq!(w.get_onboarding_model_phase(), ModelPhase::Downloading);
    lab.back();
    lab.phase.set(Some(TranscriptionRuntimePhase::Loading));
    lab.advance();
    assert_eq!(w.get_onboarding_model_phase(), ModelPhase::Loading);
    lab.back();
    lab.phase.set(Some(TranscriptionRuntimePhase::Failed));
    lab.advance();
    assert_eq!(w.get_onboarding_model_phase(), ModelPhase::Pick);
}

// Shortcut ------------------------------------------------------------------

const FIELD: &str = "Raccourci clavier";

fn open_shortcut_step(lab: &Lab) {
    for _ in 0..3 {
        lab.advance();
    }
    assert_eq!(lab.window.get_onboarding_step(), OnboardingStep::Shortcut);
}

#[test]
fn the_shortcut_step_captures_cancels_and_clears_with_the_kit_field() {
    let lab = lab();
    let w = &lab.window;
    open_shortcut_step(&lab);
    assert!(has_text(
        w,
        "Utilisé pour démarrer et arrêter la dictée depuis n'importe quelle application."
    ));
    assert_eq!(
        find_button(w, FIELD).accessible_value().as_deref(),
        Some("\u{2318} K")
    );

    activate(w, FIELD);
    assert!(w.get_onboarding_shortcut_recording());
    key(w, Key::Escape);
    assert!(!w.get_onboarding_shortcut_recording(), "Escape cancels");
    assert!(lab.recorder.saves().is_empty());
    assert_eq!(w.get_onboarding_toggle_shortcut_label(), "\u{2318} K");

    activate(w, FIELD);
    combo(w, Key::Control, "j");
    assert_eq!(lab.recorder.saves().len(), 1);
    assert_eq!(w.get_onboarding_toggle_shortcut_label(), "\u{2318} J");

    find_button(w, &format!("Effacer le raccourci « {FIELD} »")).invoke_accessible_default_action();
    settle();
    assert_eq!(
        lab.recorder.saves().last(),
        Some(&pair("", "CommandOrControl+L"))
    );
    assert_eq!(w.get_onboarding_toggle_shortcut_label(), "");
}

#[test]
fn a_refused_wizard_shortcut_shows_its_message_and_the_next_try_needs_no_click() {
    let lab = lab();
    let w = &lab.window;
    open_shortcut_step(&lab);
    activate(w, FIELD);
    tap(w, "d");
    assert_eq!(
        w.get_onboarding_shortcut_rejection(),
        ShortcutRejection::MissingModifier
    );
    assert!(w.get_onboarding_shortcut_recording());
    let message = "Le raccourci doit inclure une touche de modification (Cmd, Ctrl, Maj, Alt) ou être une touche de fonction.";
    assert!(
        has_text(w, message),
        "the refusal is a kit error, not a raw text"
    );
    assert_eq!(
        find_button(w, FIELD).accessible_description().as_deref(),
        Some(message)
    );
    combo(w, Key::Control, "l");
    assert_eq!(
        w.get_onboarding_shortcut_rejection(),
        ShortcutRejection::Conflict
    );
    assert!(lab.recorder.saves().is_empty());
    combo(w, Key::Control, "j");
    assert_eq!(lab.recorder.saves().len(), 1);
    assert_eq!(
        w.get_onboarding_shortcut_rejection(),
        ShortcutRejection::None
    );
    assert!(!has_text(w, message));

    // A failed registration is an error row too, and the old value stays.
    *lab.recorder.failure.borrow_mut() = Some("enregistrement refusé".into());
    activate(w, FIELD);
    combo(w, Key::Control, "m");
    assert!(has_text(w, "enregistrement refusé"));
    assert_eq!(w.get_onboarding_toggle_shortcut_label(), "\u{2318} J");
}

#[test]
fn a_hidden_window_and_a_left_step_make_the_wizard_ticket_obsolete() {
    let lab = lab();
    let w = &lab.window;
    open_shortcut_step(&lab);

    // Refusal in progress, then the window is hidden and shown again.
    activate(w, FIELD);
    tap(w, "d");
    assert!(w.get_onboarding_shortcut_recording());
    slint_kit::hide_window(w).unwrap();
    show_main_window(w).unwrap();
    settle();
    combo(w, Key::Control, "j");
    assert!(lab.recorder.saves().is_empty(), "the old ticket is dead");
    assert!(!w.get_onboarding_shortcut_recording());
    activate(w, FIELD);
    combo(w, Key::Control, "j");
    assert_eq!(lab.recorder.saves().len(), 1, "and a new capture works");

    // Back and forward inside the same step visit: the capture does not come back.
    activate(w, FIELD);
    lab.back();
    assert_eq!(w.get_onboarding_step(), OnboardingStep::Model);
    assert!(!w.get_onboarding_shortcut_eligible());
    w.set_onboarding_model_phase(ModelPhase::Ready);
    lab.proceed();
    assert_eq!(w.get_onboarding_step(), OnboardingStep::Shortcut);
    assert!(!w.get_onboarding_shortcut_recording());
    combo(w, Key::Control, "k");
    assert_eq!(lab.recorder.saves().len(), 1);
    activate(w, FIELD);
    combo(w, Key::Control, "k");
    assert_eq!(lab.recorder.saves().len(), 2);
}

#[test]
fn the_real_close_handler_expires_the_wizard_capture_across_an_immediate_reshow() {
    let lab = lab();
    let w = &lab.window;
    open_shortcut_step(&lab);
    let cache = settings_values::SettingsCache::with_observed(w, AppSettings::default());
    let io = settings_io::SettingsIoCoordinator::with_functions(
        cache,
        || Ok(AppSettings::default()),
        || Ok(AppSettings::default()),
        |settings| SettingsSaveOutcome::Observed {
            settings: Box::new(settings),
            result: Ok(()),
        },
        |settings| SettingsSaveOutcome::Observed {
            settings: Box::new(settings),
            result: Ok(()),
        },
    );
    wire_main_window_close(w, io);
    activate(w, FIELD);
    tap(w, "d");
    assert!(w.get_onboarding_shortcut_recording());
    w.window().dispatch_event(WindowEvent::CloseRequested);
    assert!(!w.window().is_visible());
    assert!(!w.get_onboarding_shortcut_recording());
    show_main_window(w).unwrap();
    settle();
    combo(w, Key::Control, "j");
    assert!(lab.recorder.saves().is_empty());
    activate(w, FIELD);
    combo(w, Key::Control, "j");
    assert_eq!(lab.recorder.saves().len(), 1);
}

#[test]
fn the_accessibility_notice_and_its_review_action_follow_the_real_permission() {
    let lab = lab();
    let w = &lab.window;
    open_shortcut_step(&lab);
    let notice = "Le collage automatique a besoin de l'Accessibilité, qui n'est pas encore accordée. Le texte ira dans votre historique à la place.";
    assert!(alerts(w).contains(&notice.to_string()));
    lab.clear_events();
    activate(w, "Vérifier les permissions");
    assert_eq!(lab.events(), ["review accessibility"]);
    w.set_onboarding_accessibility_granted(true);
    settle();
    assert!(!alerts(w).contains(&notice.to_string()));
    assert!(maybe_button(w, "Vérifier les permissions").is_none());
    // Entering the step reads the polled status, never a stale value.
    lab.back();
    lab.advance();
    assert!(
        w.get_onboarding_accessibility_granted()
            == (lab.permissions.status().accessibility == PermState::Granted)
    );
}

// The same step in the Settings dialog ---------------------------------------

struct DialogLab {
    lab: Lab,
    switches: Rc<RefCell<Vec<String>>>,
}

fn dialog_lab() -> DialogLab {
    let lab = lab();
    let w = &lab.window;
    w.set_onboarding_open(false);
    w.set_settings_open(true);
    w.set_settings_tab(crate::SettingsTab::System);
    let switches: Rc<RefCell<Vec<String>>> = Rc::default();
    let log = switches.clone();
    let weak = w.as_weak();
    w.on_settings_permissions_review_requested(move || {
        weak.unwrap().set_settings_permissions_dialog_open(true)
    });
    w.on_settings_autostart_changed(move |value| {
        log.borrow_mut().push(format!("autostart {value}"))
    });
    let log = switches.clone();
    w.on_settings_tab_changed(move || log.borrow_mut().push("tab".into()));
    settle();
    DialogLab { lab, switches }
}

fn focus_rings(window: &MainWindow) -> Vec<ElementHandle> {
    ElementQuery::from_root(window)
        .match_inherits("FocusOutline")
        .find_all()
}

fn inside(inner: &ElementHandle, outer: &ElementHandle) -> bool {
    let (p, s) = (inner.absolute_position(), inner.size());
    let (q, t) = (outer.absolute_position(), outer.size());
    let (cx, cy) = (p.x + s.width / 2., p.y + s.height / 2.);
    cx >= q.x && cx <= q.x + t.width && cy >= q.y && cy <= q.y + t.height
}

#[test]
fn the_dialog_is_modal_confines_the_keyboard_and_gives_the_focus_back() {
    let DialogLab { lab, switches } = dialog_lab();
    let w = &lab.window;
    project_permissions(&lab, &status_all(PermState::Denied), [false; 3]);
    activate(w, "Vérifier les permissions");
    assert!(w.get_settings_permissions_dialog_open());
    let panel = ElementHandle::find_by_accessible_label(w, "Permissions macOS")
        .find(|e| e.accessible_role() == Some(AccessibleRole::Groupbox))
        .expect("the dialog panel");

    // The shared step is the body, with its rows, hints and repair row.
    assert!(
        ElementQuery::from_root(w)
            .match_inherits("PermissionsStep")
            .find_first()
            .is_some_and(|step| inside(&step, &panel))
    );
    assert!(has_text(w, "Vous pourrez les accorder plus tard."));

    // Background controls are inert under the dialog: disabled for assistive
    // technology, deaf to their default action, and a press outside only dismisses.
    lab.clear_events();
    let background: Vec<_> = ElementQuery::from_root(w)
        .match_inherits("KitSwitch")
        .find_all()
        .into_iter()
        .filter(|e| !inside(e, &panel))
        .collect();
    assert!(!background.is_empty(), "the System page has switches");
    for control in &background {
        assert_eq!(control.accessible_enabled(), Some(false));
        control.invoke_accessible_default_action();
    }
    for button in ElementQuery::from_root(w)
        .match_accessible_role(AccessibleRole::Button)
        .find_all()
        .into_iter()
        .filter(|e| !inside(e, &panel))
    {
        assert_eq!(
            button.accessible_enabled(),
            Some(false),
            "{:?}",
            button.accessible_label()
        );
    }
    settle();
    assert!(switches.borrow().is_empty(), "{:?}", switches.borrow());
    assert!(w.get_settings_permissions_dialog_open());
    click(w, &background[0]);
    assert!(
        switches.borrow().is_empty(),
        "the outside press never reached the switch"
    );
    assert!(
        !w.get_settings_permissions_dialog_open(),
        "a press outside the panel dismisses it"
    );
    activate(w, "Vérifier les permissions");
    assert!(w.get_settings_permissions_dialog_open());

    // Tab goes round inside the panel and never reaches the page.
    let mut stops = std::collections::BTreeSet::new();
    for _ in 0..24 {
        key(w, Key::Tab);
        for ring in focus_rings(w) {
            assert!(inside(&ring, &panel), "a focus ring left the dialog");
            stops.insert((
                ring.absolute_position().x as i32,
                ring.absolute_position().y as i32,
            ));
        }
    }
    assert!(
        stops.len() >= 4,
        "close, grant buttons, repair, Done: {stops:?}"
    );
    key(w, " ");
    assert!(
        switches.borrow().is_empty(),
        "no background control was activated"
    );

    // Escape, the close icon and Done each close it and return the focus.
    key(w, Key::Escape);
    if w.get_settings_permissions_dialog_open() {
        // Space above may have activated a dialog button; Escape then closes it.
        key(w, Key::Escape);
    }
    assert!(!w.get_settings_permissions_dialog_open());
    key(w, " ");
    assert!(
        w.get_settings_permissions_dialog_open(),
        "the focus went back to the trigger"
    );
    click(w, &find_button(w, "Fermer"));
    assert!(!w.get_settings_permissions_dialog_open());
    key(w, " ");
    assert!(w.get_settings_permissions_dialog_open());
    click(w, &find_button(w, "Terminé"));
    assert!(!w.get_settings_permissions_dialog_open());
    key(w, " ");
    assert!(w.get_settings_permissions_dialog_open());
    key(w, Key::Escape);
}

#[test]
fn the_dialog_rows_report_their_kind_and_go_inert_under_a_higher_modal() {
    let DialogLab { lab, .. } = dialog_lab();
    let w = &lab.window;
    project_permissions(&lab, &status_all(PermState::Denied), [false; 3]);
    activate(w, "Vérifier les permissions");
    lab.clear_events();
    click(w, &find_button(w, "Microphone, Autoriser"));
    click(w, &find_button(w, "Réparer"));
    assert_eq!(lab.events(), ["grant Microphone", "repair"]);

    // What's New opens above the dialog: its buttons are inert until it closes.
    w.set_whats_new_open(true);
    settle();
    lab.clear_events();
    for name in ["Microphone, Autoriser", "Réparer"] {
        let button = find_button(w, name);
        assert_eq!(button.accessible_enabled(), Some(false), "{name}");
        button.invoke_accessible_default_action();
    }
    settle();
    assert!(lab.events().is_empty(), "{:?}", lab.events());
    // Escape belongs to the top modal; once it is gone the dialog has the keyboard again.
    key(w, Key::Escape);
    assert!(!w.get_whats_new_open());
    assert!(w.get_settings_permissions_dialog_open());
    settle();
    settle();
    key(w, Key::Escape);
    assert!(
        !w.get_settings_permissions_dialog_open(),
        "Escape closes the dialog after the announcement above it"
    );
    activate(w, "Vérifier les permissions");
    click(w, &find_button(w, "Réparer"));
    assert_eq!(lab.events(), ["repair"]);
    key(w, Key::Escape);
}

#[test]
fn the_dialog_follows_the_language_and_the_live_status() {
    let DialogLab { lab, .. } = dialog_lab();
    let w = &lab.window;
    activate(w, "Vérifier les permissions");
    project_permissions(&lab, &status_all(PermState::Unknown), [false; 3]);
    assert!(maybe_button(w, "Microphone, Autoriser").is_some());
    project_permissions(&lab, &status_all(PermState::Granted), [false; 3]);
    assert!(maybe_button(w, "Microphone, Autoriser").is_none());
    assert_eq!(badges(w, "Accordé"), 3);
    select_app_locale(w, crate::AppLocale::En);
    settle();
    assert_eq!(badges(w, "Granted"), 3);
    assert!(maybe_button(w, "Done").is_some());
    select_app_locale(w, crate::AppLocale::Fr);
    key(w, Key::Escape);
    press(w, Key::Shift);
    release(w, Key::Shift);
}
