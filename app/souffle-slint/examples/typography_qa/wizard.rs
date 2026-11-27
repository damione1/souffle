//! The first-run wizard on synthetic data. `onboarding-<step>[-<state>]`
//! surfaces draw one state each. Without SOU276_SNAPSHOT the window is
//! clickable: the buttons walk through the steps with simulated permissions,
//! download and shortcut capture. Nothing is saved or requested from macOS.
use super::{audio_ui, model, strings};
use slint::{ComponentHandle, Model, SharedString, Timer, TimerMode};
use souffle_slint::*;
use std::{cell::Cell, rc::Rc, time::Duration};

const STEPS: [OnboardingStep; 4] = [
    OnboardingStep::Permissions,
    OnboardingStep::Microphone,
    OnboardingStep::Model,
    OnboardingStep::Shortcut,
];

pub(super) fn step_for(surface: &str) -> OnboardingStep {
    match surface.strip_prefix("onboarding-") {
        Some(rest) if rest.starts_with("microphone") => OnboardingStep::Microphone,
        Some(rest) if rest.starts_with("model") => OnboardingStep::Model,
        Some(rest) if rest.starts_with("shortcut") => OnboardingStep::Shortcut,
        _ => OnboardingStep::Permissions,
    }
}

fn step_index(step: OnboardingStep) -> usize {
    STEPS.iter().position(|s| *s == step).unwrap_or(0)
}

fn is_english(window: &MainWindow) -> bool {
    window.get_settings_locale() == AppLocale::En
}

/// The rows `onboarding_ui::populate_permission_rows` sends. macOS never
/// reports Accessibility as unknown: until it is granted it reads as denied.
fn rows(window: &MainWindow, states: [AccessState; 3]) {
    let kinds = [
        PermissionKind::Microphone,
        PermissionKind::SystemAudio,
        PermissionKind::Accessibility,
    ];
    window.set_onboarding_permission_rows(model(
        kinds
            .into_iter()
            .zip(states)
            .map(|(kind, state)| PermissionRow {
                kind,
                state,
                busy: false,
            })
            .collect::<Vec<_>>(),
    ));
}

fn levels(phase: usize) -> Vec<f32> {
    (0..28)
        .map(|i| 0.25 + 0.6 * (((i + phase) as f32) * 0.7).sin().abs())
        .collect()
}

fn shortcut_label(window: &MainWindow) -> &'static str {
    if is_english(window) {
        "⌥ Space"
    } else {
        "⌥ Espace"
    }
}

fn show_step(window: &MainWindow, step: OnboardingStep) {
    window.set_onboarding_step(step);
    window.set_onboarding_step_index(step_index(step) as i32);
    window.set_onboarding_step_count(STEPS.len() as i32);
    window.set_onboarding_busy(false);
    window.set_onboarding_continue_enabled(true);
    window.set_onboarding_status_message("".into());
}

pub(super) fn apply(window: &MainWindow, surface: &str) {
    use AccessState::{Denied, Granted, NoDevice, Unknown};
    let step = step_for(surface);
    show_step(window, step);
    rows(window, [Granted, Unknown, Denied]);

    window.set_onboarding_device_labels(strings(&["__AUTOMATIC__", "MacBook Pro Microphone"]));
    window.set_onboarding_device_ids(strings(&["", "builtin"]));
    window.set_onboarding_default_device_index(audio_ui::default_input_index(&[
        souffle_lib::audio::AudioInputDevice {
            uid: "builtin".into(),
            name: "MacBook Pro Microphone".into(),
            transport: souffle_lib::audio::TransportType::BuiltIn,
            is_default: true,
        },
    ]));
    window.set_onboarding_selected_device_index(0);
    window.set_onboarding_audio_level(0.6);
    window.set_onboarding_audio_levels(model(levels(0)));
    window.set_onboarding_model_labels(strings(&["Kyutai STT 1B · FR + EN · streaming"]));
    window.set_onboarding_model_ids(strings(&["kyutai"]));
    window.set_onboarding_selected_model_index(0);
    window.set_onboarding_model_phase(ModelPhase::Pick);
    window.set_onboarding_toggle_shortcut_label(shortcut_label(window).into());
    window.set_onboarding_accessibility_granted(false);
    window.set_onboarding_autostart_enabled(false);
    window.set_onboarding_autostart_failed(false);

    let downloading = || {
        window.set_onboarding_model_phase(ModelPhase::Downloading);
        window.set_onboarding_download_progress_label("62 %".into());
        window.set_onboarding_download_progress_fraction(0.62);
        window.set_onboarding_busy(true);
        window.set_onboarding_continue_enabled(false);
    };
    let error = |french: &str, english: &str| {
        let message = if is_english(window) { english } else { french };
        window.set_onboarding_status_message(message.into());
    };
    let state = surface
        .split_once('-')
        .and_then(|(_, rest)| rest.split_once('-'))
        .map_or("", |(_, state)| state);
    match (step, state) {
        (OnboardingStep::Permissions, "fresh") => rows(window, [Unknown, Unknown, Denied]),
        (OnboardingStep::Permissions, "granted") => rows(window, [Granted; 3]),
        (OnboardingStep::Permissions, "denied") => rows(window, [Denied; 3]),
        (OnboardingStep::Permissions, "no-device") => rows(window, [NoDevice, Unknown, Denied]),
        (OnboardingStep::Permissions, "checking") => {
            let rows = window.get_onboarding_permission_rows();
            let mut row = rows.row_data(1).expect("system audio row");
            row.busy = true;
            rows.set_row_data(1, row);
        }
        (OnboardingStep::Permissions, "repair-busy") => window.set_onboarding_repair_busy(true),
        (OnboardingStep::Permissions, "repair-done") => {
            window.set_onboarding_repair_success(true);
            window.set_onboarding_repair_cooldown(true);
        }
        (OnboardingStep::Permissions, "error") => window.set_onboarding_permissions_error(
            if is_english(window) {
                "Synthetic error: the permission status could not be read."
            } else {
                "Erreur fictive : impossible de lire l'état des autorisations."
            }
            .into(),
        ),
        (OnboardingStep::Microphone, "silent") => {
            window.set_onboarding_audio_level(0.0);
            window.set_onboarding_audio_levels(model(vec![0.0; 28]));
        }
        (OnboardingStep::Microphone, "none") => {
            window.set_onboarding_device_labels(strings(&["__AUTOMATIC__"]));
            window.set_onboarding_device_ids(strings(&[""]));
            window.set_onboarding_default_device_index(-1);
            window.set_onboarding_audio_level(0.0);
            window.set_onboarding_audio_levels(model(Vec::new()));
        }
        (OnboardingStep::Microphone, "saving") => {
            window.set_onboarding_busy(true);
            window.set_onboarding_continue_enabled(false);
        }
        (OnboardingStep::Microphone, "error") => error(
            "Erreur fictive : le microphone n'a pas pu être sélectionné.",
            "Synthetic error: the microphone could not be selected.",
        ),
        // The bare surface keeps the state the design board draws.
        (OnboardingStep::Model, "") | (OnboardingStep::Model, "downloading") => downloading(),
        (OnboardingStep::Model, "pick") => {}
        (OnboardingStep::Model, "loading") => {
            window.set_onboarding_model_phase(ModelPhase::Loading);
            window.set_onboarding_busy(true);
            window.set_onboarding_continue_enabled(false);
        }
        (OnboardingStep::Model, "ready") => window.set_onboarding_model_phase(ModelPhase::Ready),
        (OnboardingStep::Model, "error") => error(
            "Erreur fictive : le téléchargement a été interrompu.",
            "Synthetic error: the download was interrupted.",
        ),
        (OnboardingStep::Model, "recovery") => {
            window.set_onboarding_step_index(0);
            window.set_onboarding_step_count(1);
        }
        (OnboardingStep::Shortcut, "granted") => window.set_onboarding_accessibility_granted(true),
        (OnboardingStep::Shortcut, "autostart-failed") => {
            window.set_onboarding_shortcut_notice(ShortcutNotice::TypesCharacter);
            window.set_onboarding_autostart_failed(true);
        }
        (OnboardingStep::Shortcut, "recording") => window.set_onboarding_shortcut_recording(true),
        (OnboardingStep::Shortcut, "unset") => {
            window.set_onboarding_toggle_shortcut_label("".into())
        }
        (OnboardingStep::Shortcut, "missing-modifier") => {
            window.set_onboarding_shortcut_notice(ShortcutNotice::MissingModifier)
        }
        (OnboardingStep::Shortcut, "unusable-key") => {
            window.set_onboarding_shortcut_notice(ShortcutNotice::UnusableKey)
        }
        (OnboardingStep::Shortcut, "types-character") => {
            window.set_onboarding_accessibility_granted(true);
            window.set_onboarding_shortcut_notice(ShortcutNotice::TypesCharacter)
        }
        (OnboardingStep::Shortcut, "error") => window.set_onboarding_shortcut_error(
            if is_english(window) {
                "Synthetic error: the shortcut could not be registered."
            } else {
                "Erreur fictive : impossible d'enregistrer le raccourci."
            }
            .into(),
        ),
        (_, "") => {}
        (_, unknown) => panic!("unknown onboarding state: {unknown}"),
    }
}

/// Timers of the clickable wizard; they stop when dropped.
pub(super) struct Interactive {
    _meter: Timer,
}

fn after(delay: u64, run: impl FnOnce() + 'static) {
    Timer::single_shot(Duration::from_millis(delay), run);
}

pub(super) fn interactive(window: &MainWindow) -> Interactive {
    // Starts the download from the pick state whatever surface was asked for.
    if window.get_onboarding_step() == OnboardingStep::Model {
        apply(window, "onboarding-model-pick");
    }
    // Bumped on every step change so a pending simulation cannot touch another step.
    let generation = Rc::new(Cell::new(0u32));

    let weak = window.as_weak();
    window.on_onboarding_locale_changed(move |locale| {
        let window = weak.unwrap();
        window.set_settings_locale(locale);
        slint::select_bundled_translation(if locale == AppLocale::En { "en" } else { "fr" })
            .expect("bundled translation");
        window.global::<SettingsOptions>().invoke_localize();
        window.set_onboarding_toggle_shortcut_label(shortcut_label(&window).into());
    });

    let weak = window.as_weak();
    window.on_onboarding_grant_requested(move |kind| {
        let rows = weak.unwrap().get_onboarding_permission_rows();
        let Some(index) =
            (0..rows.row_count()).find(|i| rows.row_data(*i).is_some_and(|row| row.kind == kind))
        else {
            return;
        };
        let set = move |rows: &slint::ModelRc<PermissionRow>, state, busy| {
            let mut row = rows.row_data(index).expect("permission row");
            row.state = state;
            row.busy = busy;
            rows.set_row_data(index, row);
        };
        let current = rows.row_data(index).expect("permission row").state;
        set(&rows, current, true);
        after(700, move || set(&rows, AccessState::Granted, false));
    });
    window.on_onboarding_hint_action_requested(|kind| println!("would open Settings for {kind:?}"));

    let weak = window.as_weak();
    window.on_onboarding_repair_requested(move || {
        let window = weak.unwrap();
        window.set_onboarding_repair_busy(true);
        let weak = window.as_weak();
        after(900, move || {
            let window = weak.unwrap();
            window.set_onboarding_repair_busy(false);
            window.set_onboarding_repair_success(true);
        });
    });

    let weak = window.as_weak();
    window.on_onboarding_device_changed(move |index| {
        weak.unwrap().set_onboarding_selected_device_index(index)
    });
    window.on_onboarding_refresh_devices_requested(|| println!("device list refreshed"));
    let weak = window.as_weak();
    window.on_onboarding_model_picked(move |index| {
        weak.unwrap().set_onboarding_selected_model_index(index)
    });

    let weak = window.as_weak();
    window.on_onboarding_shortcut_record_requested(move || {
        let window = weak.unwrap();
        window.set_onboarding_shortcut_notice(ShortcutNotice::None);
        window.set_onboarding_shortcut_recording(true);
    });
    let weak = window.as_weak();
    window.on_onboarding_shortcut_captured(move |text, ctrl, shift, alt, meta| {
        let window = weak.unwrap();
        // Slint reports Command as `control` on macOS and Control as `meta`.
        if !(ctrl || alt || meta) {
            window.set_onboarding_shortcut_notice(ShortcutNotice::MissingModifier);
            return;
        }
        let key = if text == " " {
            shortcut_label(&window).trim_start_matches("⌥ ").to_string()
        } else {
            text.to_uppercase()
        };
        let mut parts: Vec<&str> = [(meta, "⌃"), (alt, "⌥"), (shift, "⇧"), (ctrl, "⌘")]
            .into_iter()
            .filter_map(|(held, symbol)| held.then_some(symbol))
            .collect();
        parts.push(&key);
        window.set_onboarding_toggle_shortcut_label(SharedString::from(parts.join(" ")));
        window.set_onboarding_shortcut_notice(ShortcutNotice::None);
        window.set_onboarding_shortcut_recording(false);
    });
    let weak = window.as_weak();
    window.on_onboarding_shortcut_cancelled(move || {
        weak.unwrap().set_onboarding_shortcut_recording(false)
    });
    let weak = window.as_weak();
    window.on_onboarding_shortcut_cleared(move || {
        let window = weak.unwrap();
        window.set_onboarding_toggle_shortcut_label("".into());
        window.set_onboarding_shortcut_recording(false);
    });
    let weak = window.as_weak();
    window.on_onboarding_autostart_changed(move |enabled| {
        weak.unwrap().set_onboarding_autostart_enabled(enabled)
    });
    let weak = window.as_weak();
    window.on_onboarding_review_accessibility_requested(move || {
        weak.unwrap().set_onboarding_accessibility_granted(true)
    });

    let weak = window.as_weak();
    let steps = generation.clone();
    window.on_onboarding_back_requested(move || {
        let window = weak.unwrap();
        steps.set(steps.get() + 1);
        let index = step_index(window.get_onboarding_step());
        show_step(&window, STEPS[index.saturating_sub(1)]);
    });

    let weak = window.as_weak();
    window.on_onboarding_continue_requested(move || {
        let window = weak.unwrap();
        let step = window.get_onboarding_step();
        if step == OnboardingStep::Model && window.get_onboarding_model_phase() != ModelPhase::Ready
        {
            simulate_download(&window, generation.clone());
            return;
        }
        generation.set(generation.get() + 1);
        match STEPS.get(step_index(step) + 1) {
            Some(next) => show_step(&window, *next),
            None => window.set_onboarding_open(false),
        }
    });

    let meter = Timer::default();
    let weak = window.as_weak();
    let phase = Cell::new(0usize);
    meter.start(TimerMode::Repeated, Duration::from_millis(120), move || {
        let window = weak.unwrap();
        if window.get_onboarding_step() == OnboardingStep::Microphone {
            phase.set(phase.get() + 1);
            window.set_onboarding_audio_levels(model(levels(phase.get())));
        }
    });
    Interactive { _meter: meter }
}

fn simulate_download(window: &MainWindow, generation: Rc<Cell<u32>>) {
    window.set_onboarding_busy(true);
    window.set_onboarding_continue_enabled(false);
    window.set_onboarding_model_phase(ModelPhase::Downloading);
    let started = generation.get();
    for tick in 0..=20u64 {
        let weak = window.as_weak();
        let generation = generation.clone();
        after(150 * tick, move || {
            if generation.get() != started {
                return;
            }
            let window = weak.unwrap();
            window.set_onboarding_download_progress_fraction(tick as f32 / 20.);
            window.set_onboarding_download_progress_label(format!("{} %", tick * 5).into());
        });
    }
    for (delay, phase) in [(3300, ModelPhase::Loading), (4800, ModelPhase::Ready)] {
        let weak = window.as_weak();
        let generation = generation.clone();
        after(delay, move || {
            if generation.get() != started {
                return;
            }
            let window = weak.unwrap();
            window.set_onboarding_model_phase(phase);
            if phase == ModelPhase::Ready {
                window.set_onboarding_busy(false);
                window.set_onboarding_continue_enabled(true);
            }
        });
    }
}
