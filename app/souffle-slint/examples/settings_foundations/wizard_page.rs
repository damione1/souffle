//! The first-run wizard and the Settings permissions dialog on the real window.
//! The engine, the permission prompts, the preferences and the shortcut
//! registration are replaced by an in-memory host, so no data of the user moves.
use super::{click, save_frame, send_key, settle, shortcut_capture};
use crate::{
    AccessState, AppLocale, MainWindow, ModelPhase, OnboardingStep, PermissionKind, PermissionRow,
    SettingsOptions, SettingsTab, ShortcutRejection,
};
use i_slint_backend_testing::{AccessibleRole, ElementHandle, ElementQuery};
use slint::{ComponentHandle, Model, ModelRc, SharedString, Timer, VecModel, platform::Key};
use souffle_lib::settings::ShortcutSettings;
use std::{cell::RefCell, path::Path, rc::Rc, time::Duration};

type Log = Rc<RefCell<Vec<String>>>;

struct Host {
    log: Log,
    pair: Rc<RefCell<ShortcutSettings>>,
}

impl Host {
    fn take(&self) -> Vec<String> {
        std::mem::take(&mut *self.log.borrow_mut())
    }
}

fn frame(w: &MainWindow, path: &Path, scene: &str) {
    let stem = path.file_stem().unwrap().to_str().unwrap();
    save_frame(w, &path.with_file_name(format!("{stem}-{scene}.ppm")));
}

fn en(w: &MainWindow) -> bool {
    w.get_settings_locale() == AppLocale::En
}

fn t<'a>(w: &MainWindow, english: &'a str, french: &'a str) -> &'a str {
    if en(w) { english } else { french }
}

fn items(w: &MainWindow, kind: &str) -> Vec<ElementHandle> {
    ElementQuery::from_root(w).match_inherits(kind).find_all()
}

fn by_id(w: &MainWindow, name: &str) -> ElementHandle {
    ElementHandle::find_by_element_id(w, name)
        .next()
        .unwrap_or_else(|| panic!("missing {name}"))
}

fn button(w: &MainWindow, label: &str) -> ElementHandle {
    maybe_button(w, label).unwrap_or_else(|| panic!("no button {label:?}"))
}

fn maybe_button(w: &MainWindow, label: &str) -> Option<ElementHandle> {
    ElementHandle::find_by_accessible_label(w, label)
        .find(|e| e.accessible_role() == Some(AccessibleRole::Button))
}

fn texts(w: &MainWindow) -> Vec<String> {
    ElementQuery::from_root(w)
        .match_accessible_role(AccessibleRole::Text)
        .find_all()
        .iter()
        .filter_map(ElementHandle::accessible_label)
        .map(|s| s.to_string())
        .collect()
}

fn groups(w: &MainWindow) -> Vec<String> {
    ElementQuery::from_root(w)
        .match_accessible_role(AccessibleRole::Groupbox)
        .find_all()
        .iter()
        .filter_map(ElementHandle::accessible_label)
        .map(|s| s.to_string())
        .collect()
}

fn badges(w: &MainWindow, text: &str) -> usize {
    items(w, "KitBadge")
        .iter()
        .filter(|b| b.accessible_label().as_deref() == Some(text))
        .count()
}

fn window_width(w: &MainWindow) -> f32 {
    w.window().size().width as f32 / w.window().scale_factor()
}

fn window_height(w: &MainWindow) -> f32 {
    w.window().size().height as f32 / w.window().scale_factor()
}

fn rect(e: &ElementHandle) -> (f32, f32, f32, f32) {
    let (p, s) = (e.absolute_position(), e.size());
    (p.x, p.y, p.x + s.width, p.y + s.height)
}

fn center_in(inner: &ElementHandle, outer: (f32, f32, f32, f32)) -> bool {
    let (x0, y0, x1, y1) = rect(inner);
    let (cx, cy) = ((x0 + x1) / 2., (y0 + y1) / 2.);
    cx >= outer.0 && cx <= outer.2 && cy >= outer.1 && cy <= outer.3
}

fn within(inner: (f32, f32, f32, f32), outer: (f32, f32, f32, f32)) -> bool {
    inner.0 >= outer.0 - 1.
        && inner.1 >= outer.1 - 1.
        && inner.2 <= outer.2 + 1.
        && inner.3 <= outer.3 + 1.
}

fn assert_widths(w: &MainWindow, what: &str) {
    let limit = window_width(w);
    for kind in ["KitButton", "KitSelectTrigger", "KitBadge"] {
        for el in items(w, kind) {
            let (x0, _, x1, _) = rect(&el);
            assert!(
                el.size().width > 8. && el.size().height > 8.,
                "{what}: {kind} collapsed {:?} {:?}",
                el.accessible_label(),
                el.size()
            );
            assert!(
                x0 >= -1. && x1 <= limit + 1.,
                "{what}: {kind} {:?} leaves the window ({x0}..{x1} of {limit})",
                el.accessible_label()
            );
        }
    }
}

fn shortcut_text(value: &str) -> String {
    value
        .replace("CommandOrControl", "\u{2318}")
        .replace("Shift", "\u{21e7}")
        .replace("Alt", "\u{2325}")
        .replace('+', " ")
}

fn rejection_of(reason: Option<shortcut_capture::Rejection>) -> ShortcutRejection {
    use shortcut_capture::Rejection;
    match reason {
        None => ShortcutRejection::None,
        Some(Rejection::MissingModifier) => ShortcutRejection::MissingModifier,
        Some(Rejection::Unmappable) => ShortcutRejection::Unmappable,
        Some(Rejection::Unavailable) => ShortcutRejection::Unavailable,
        Some(Rejection::Conflict) => ShortcutRejection::Conflict,
    }
}

fn wire(w: &MainWindow) -> Host {
    use shortcut_capture::{Modifiers, Target, decide};
    let log: Log = Default::default();
    let pair = Rc::new(RefCell::new(ShortcutSettings {
        toggle: "CommandOrControl+K".into(),
        push_to_talk: "CommandOrControl+L".into(),
    }));
    let l = log.clone();
    w.on_onboarding_grant_requested(move |k| l.borrow_mut().push(format!("grant {k:?}")));
    let l = log.clone();
    w.on_onboarding_hint_action_requested(move |k| l.borrow_mut().push(format!("settings {k:?}")));
    let l = log.clone();
    w.on_onboarding_repair_requested(move || l.borrow_mut().push("repair".into()));
    let l = log.clone();
    w.on_onboarding_review_accessibility_requested(move || l.borrow_mut().push("review".into()));
    let l = log.clone();
    w.on_onboarding_refresh_devices_requested(move || l.borrow_mut().push("refresh".into()));
    let (l, weak) = (log.clone(), w.as_weak());
    w.on_onboarding_device_changed(move |i| {
        l.borrow_mut().push(format!("device {i}"));
        weak.unwrap().set_onboarding_selected_device_index(i);
    });
    let (l, weak) = (log.clone(), w.as_weak());
    w.on_onboarding_model_picked(move |i| {
        l.borrow_mut().push(format!("model {i}"));
        weak.unwrap().set_onboarding_selected_model_index(i);
    });
    let l = log.clone();
    w.on_onboarding_back_requested(move || l.borrow_mut().push("back".into()));
    let l = log.clone();
    w.on_onboarding_continue_requested(move || l.borrow_mut().push("continue".into()));
    let (l, weak) = (log.clone(), w.as_weak());
    w.on_onboarding_locale_changed(move |locale| {
        l.borrow_mut().push(format!("locale {locale:?}"));
        let w = weak.unwrap();
        w.set_settings_locale(locale);
        slint::select_bundled_translation(if locale == AppLocale::En { "en" } else { "fr" })
            .unwrap();
        w.global::<SettingsOptions>().invoke_localize();
    });
    let (l, weak) = (log.clone(), w.as_weak());
    w.on_settings_permissions_review_requested(move || {
        l.borrow_mut().push("open dialog".into());
        weak.unwrap().set_settings_permissions_dialog_open(true);
    });
    let l = log.clone();
    w.on_settings_autostart_changed(move |v| l.borrow_mut().push(format!("autostart {v}")));
    let l = log.clone();
    w.on_theme_toggle_requested(move || l.borrow_mut().push("theme toggle".into()));
    let l = log.clone();
    w.on_settings_tab_changed(move || l.borrow_mut().push("tab".into()));

    let decision = {
        let pair = pair.clone();
        move |text: &str, c, s, a, m| {
            let current = pair.borrow().clone();
            decide(
                Some(&current),
                Target::Toggle,
                text,
                Modifiers {
                    control: c,
                    shift: s,
                    alt: a,
                    meta: m,
                },
            )
        }
    };
    w.set_onboarding_toggle_shortcut_label(shortcut_text(&pair.borrow().toggle).into());
    let weak = w.as_weak();
    w.on_onboarding_shortcut_record_requested(move || {
        let w = weak.unwrap();
        w.set_onboarding_shortcut_recording(true);
        w.set_onboarding_shortcut_error("".into());
        w.set_onboarding_shortcut_rejection(ShortcutRejection::None);
    });
    let d = decision.clone();
    w.on_onboarding_shortcut_key_allowed(move |text, c, s, a, m| d(&text, c, s, a, m).is_ok());
    let (d, weak) = (decision.clone(), w.as_weak());
    w.on_onboarding_shortcut_rejected(move |text, c, s, a, m| {
        let w = weak.unwrap();
        w.set_onboarding_shortcut_recording(true);
        w.set_onboarding_shortcut_rejection(rejection_of(d(&text, c, s, a, m).err()));
    });
    for released in [false, true] {
        let (d, weak, pair, l) = (decision.clone(), w.as_weak(), pair.clone(), log.clone());
        let apply = move |text: SharedString, c, s, a, m| {
            if let Ok(value) = d(&text, c, s, a, m) {
                let w = weak.unwrap();
                pair.borrow_mut().toggle = value.clone();
                l.borrow_mut().push(format!("save {value}"));
                w.set_onboarding_shortcut_recording(false);
                w.set_onboarding_shortcut_rejection(ShortcutRejection::None);
                w.set_onboarding_toggle_shortcut_label(shortcut_text(&value).into());
            }
        };
        if released {
            w.on_onboarding_shortcut_released(apply);
        } else {
            w.on_onboarding_shortcut_captured(apply);
        }
    }
    let (weak, pair2, l) = (w.as_weak(), pair.clone(), log.clone());
    w.on_onboarding_shortcut_cleared(move || {
        let w = weak.unwrap();
        pair2.borrow_mut().toggle.clear();
        l.borrow_mut().push("save ".into());
        w.set_onboarding_shortcut_recording(false);
        w.set_onboarding_toggle_shortcut_label("".into());
    });
    let weak = w.as_weak();
    w.on_onboarding_shortcut_cancelled(move || {
        let w = weak.unwrap();
        w.set_onboarding_shortcut_recording(false);
        w.set_onboarding_shortcut_error("".into());
        w.set_onboarding_shortcut_rejection(ShortcutRejection::None);
    });
    Host { log, pair }
}

fn rows(states: &[(PermissionKind, AccessState, bool)]) -> ModelRc<PermissionRow> {
    ModelRc::new(VecModel::from(
        states
            .iter()
            .map(|&(kind, state, busy)| PermissionRow { kind, state, busy })
            .collect::<Vec<_>>(),
    ))
}

fn uniform(state: AccessState) -> ModelRc<PermissionRow> {
    rows(&[
        (PermissionKind::Microphone, state, false),
        (PermissionKind::SystemAudio, state, false),
        (PermissionKind::Accessibility, state, false),
        (PermissionKind::Calendar, state, false),
    ])
}

fn go(w: &MainWindow, step: OnboardingStep) {
    let index = match step {
        OnboardingStep::Permissions => 0,
        OnboardingStep::Microphone => 1,
        OnboardingStep::Model => 2,
        OnboardingStep::Shortcut => 3,
    };
    w.invoke_cancel_onboarding_capture();
    w.set_onboarding_step(step);
    w.set_onboarding_step_index(index);
    w.set_onboarding_step_count(4);
    settle(w);
}

fn viewport(w: &MainWindow) -> ElementHandle {
    by_id(w, "OnboardingView::viewport")
}

/// Elements below the fold are not in the tree: scroll the scroller from its top
/// until `find` returns, leaving the match in view.
fn reveal<T>(w: &MainWindow, scroller: &ElementHandle, find: impl Fn() -> Option<T>) -> Option<T> {
    // Where the content sits: a scroll that moves nothing has reached the end.
    let signature = |w: &MainWindow| {
        [
            "KitButton",
            "KitText",
            "KitAlert",
            "KitBadge",
            "KitFieldRow",
            "KitSection",
        ]
        .into_iter()
        .flat_map(|kind| items(w, kind))
        .map(|e| e.absolute_position().y as i32)
        .collect::<Vec<_>>()
    };
    scroller.scroll(0., 100000.);
    settle(w);
    for _ in 0..80 {
        if let Some(found) = find() {
            return Some(found);
        }
        let before = signature(w);
        scroller.scroll(0., -120.);
        settle(w);
        if signature(w) == before {
            break;
        }
    }
    None
}

fn reveal_button(w: &MainWindow, scroller: &ElementHandle, label: &str) -> Option<ElementHandle> {
    reveal(w, scroller, || maybe_button(w, label))
}

const KINDS: [PermissionKind; 4] = [
    PermissionKind::Microphone,
    PermissionKind::SystemAudio,
    PermissionKind::Accessibility,
    PermissionKind::Calendar,
];
const STATES: [AccessState; 5] = [
    AccessState::Unknown,
    AccessState::Denied,
    AccessState::NoDevice,
    AccessState::Granted,
    AccessState::Unsupported,
];

/// Every state of every permission, one row at a time so each fits in view:
/// the action offered, the hint and its Settings detour, the badge, and the
/// event each action reports.
fn permission_matrix(
    w: &MainWindow,
    host: &Host,
    scroller: &ElementHandle,
    scene: &str,
    path: &Path,
) {
    let (allow, open) = (
        t(w, "Allow", "Autoriser"),
        t(w, "Open Settings", "Ouvrir les Réglages"),
    );
    let settings = t(w, "System Settings", "Réglages Système");
    let name = |kind: PermissionKind| match kind {
        PermissionKind::Microphone => t(w, "Mic", "Microphone"),
        PermissionKind::SystemAudio => t(w, "System audio", "Audio système"),
        PermissionKind::Accessibility => t(w, "Accessibility", "Accessibilité"),
        PermissionKind::Calendar => t(w, "Calendar", "Calendrier"),
    };
    let hint = |kind: PermissionKind, state: AccessState| match (kind, state) {
        (PermissionKind::Accessibility, AccessState::Denied) => Some(t(
            w,
            "Denied. Open System Settings > Privacy & Security > Accessibility.",
            "Refusé. Ouvrez Réglages Système > Confidentialité et sécurité > Accessibilité.",
        )),
        (PermissionKind::Microphone, AccessState::Denied) => Some(t(
            w,
            "Denied. Open System Settings to allow it.",
            "Refusé. Ouvrez Réglages Système pour l'autoriser.",
        )),
        (PermissionKind::Microphone, AccessState::NoDevice) => {
            Some(t(w, "No microphone detected.", "Aucun microphone détecté."))
        }
        _ => None,
    };
    host.take();
    for kind in KINDS {
        for state in STATES {
            w.set_onboarding_permission_rows(rows(&[(kind, state, false)]));
            settle(w);
            let tag = format!("{scene} {kind:?}/{state:?}");
            let action = format!(
                "{}, {}",
                name(kind),
                if kind == PermissionKind::Accessibility {
                    open
                } else {
                    allow
                }
            );
            let actionable = matches!(
                state,
                AccessState::Unknown | AccessState::Denied | AccessState::NoDevice
            );
            let found = reveal_button(w, scroller, &action);
            assert_eq!(found.is_some(), actionable, "{tag}: action {action:?}");
            if let Some(found) = found {
                assert_eq!(found.accessible_enabled(), Some(true), "{tag}");
                found.invoke_accessible_default_action();
                settle(w);
                assert_eq!(host.take(), [format!("grant {kind:?}")], "{tag}");
            }
            assert_eq!(
                badges(w, t(w, "Granted", "Accordé")),
                usize::from(state == AccessState::Granted),
                "{tag}"
            );
            assert_eq!(
                badges(w, t(w, "Unsupported", "Non pris en charge")),
                usize::from(state == AccessState::Unsupported),
                "{tag}"
            );
            let expected_hint = hint(kind, state);
            let shown = reveal(w, scroller, || {
                groups(w)
                    .into_iter()
                    .find(|g| Some(g.as_str()) == expected_hint)
            });
            assert_eq!(shown.is_some(), expected_hint.is_some(), "{tag}: hint");
            let detour = expected_hint.is_some() && state == AccessState::Denied;
            let button_found = reveal_button(w, scroller, settings);
            assert_eq!(button_found.is_some(), detour, "{tag}: Settings button");
            if let Some(button_found) = button_found {
                button_found.invoke_accessible_default_action();
                settle(w);
                assert_eq!(host.take(), [format!("settings {kind:?}")], "{tag}");
            }
            assert_widths(w, &tag);
            if matches!(
                (kind, state),
                (PermissionKind::Microphone, AccessState::Denied)
                    | (PermissionKind::Accessibility, AccessState::Denied)
                    | (PermissionKind::Microphone, AccessState::NoDevice)
                    | (PermissionKind::Calendar, AccessState::Granted)
                    | (PermissionKind::SystemAudio, AccessState::Unsupported)
            ) {
                scroller.scroll(0., 100000.);
                settle(w);
                frame(w, path, &format!("{scene}-{kind:?}-{state:?}"));
            }
        }
    }
    // Checking: a request in flight disables its own button only.
    for kind in KINDS {
        w.set_onboarding_permission_rows(rows(&[(kind, AccessState::Unknown, true)]));
        settle(w);
        let checking = format!("{}, {}", name(kind), t(w, "Checking...", "Vérification…"));
        let found =
            reveal_button(w, scroller, &checking).unwrap_or_else(|| panic!("{scene}: {checking}"));
        assert_eq!(
            found.accessible_enabled(),
            Some(false),
            "{scene}: {checking}"
        );
        found.invoke_accessible_default_action();
        settle(w);
        assert!(
            host.take().is_empty(),
            "{scene}: a checking row does not request again"
        );
    }
}

fn rb(w: &MainWindow, label: &str) -> ElementHandle {
    reveal_button(w, &viewport(w), label)
        .unwrap_or_else(|| panic!("no button {label:?} in the step"))
}

/// The control that owns the focus ring, by kind and name: a scroll moves the
/// ring, so its position cannot say when a walk has come back round.
fn focus_owner(w: &MainWindow) -> Option<String> {
    focus_owner_labelled(w).map(|(key, _)| key)
}

fn focus_owner_labelled(w: &MainWindow) -> Option<(String, Option<String>)> {
    [
        "KitButton",
        "KitIconButton",
        "KitSelectTrigger",
        "KitSegmentedControl",
        "KitSwitch",
        "KitScrollArea",
    ]
    .into_iter()
    .flat_map(|kind| items(w, kind).into_iter().map(move |e| (kind, e)))
    .find(|(_, e)| {
        e.query_descendants()
            .match_inherits("FocusOutline")
            .find_first()
            .is_some()
    })
    .map(|(kind, e)| {
        (
            format!(
                "{kind} {:?} {:?}",
                e.accessible_label(),
                e.accessible_value()
            ),
            e.accessible_label().map(|l| l.to_string()),
        )
    })
}

/// Walks the whole step with Tab: every focus ring must sit entirely inside the
/// scrolled viewport, or inside the footer, and the walk must come back round.
fn tab_walk(w: &MainWindow, what: &str, footer: bool) -> usize {
    let vp = rect(&viewport(w));
    let ft = items(w, "KitFooter").first().map(rect);
    let height = window_height(w);
    let mut first: Option<String> = None;
    let mut stops = 0;
    for _ in 0..60 {
        send_key(w, Key::Tab.into());
        let Some(owner) = focus_owner(w) else {
            continue;
        };
        if first.as_ref() == Some(&owner) && stops > 1 {
            return stops;
        }
        first.get_or_insert(owner.clone());
        stops += 1;
        let ring = items(w, "FocusOutline")
            .into_iter()
            .next()
            .expect("a ring for the owner");
        let r = rect(&ring);
        let in_view = center_in(&ring, vp);
        let in_footer = footer && ft.is_some_and(|f| center_in(&ring, f));
        assert!(
            in_view || in_footer,
            "{what}: ring of {owner} {r:?} is neither in the viewport {vp:?} nor the footer {ft:?}"
        );
        if in_view {
            assert!(
                within(r, vp),
                "{what}: ring of {owner} {r:?} is clipped by the viewport {vp:?}"
            );
        } else {
            assert!(
                within(r, (0., ft.unwrap().1, window_width(w), height)),
                "{what}: footer ring of {owner} {r:?} leaves the window"
            );
        }
    }
    panic!("{what}: Tab never came back round ({stops} stops)");
}

fn column_checks(w: &MainWindow, what: &str) {
    let width = window_width(w);
    let expected = (width - 48.).min(512.);
    let bar = items(w, "KitProgress")
        .into_iter()
        .next()
        .expect("progress bar");
    let (x0, _, x1, _) = rect(&bar);
    assert!(
        (bar.size().width - expected).abs() <= 1.,
        "{what}: column {} instead of {expected}",
        bar.size().width
    );
    assert!(
        ((x0 + x1) / 2. - width / 2.).abs() <= 1.,
        "{what}: column not centred ({x0}..{x1} in {width})"
    );
    let primary = items(w, "KitButton")
        .into_iter()
        .filter(|b| center_in(b, rect(&items(w, "KitFooter")[0])))
        .map(|b| rect(&b).2)
        .fold(0f32, f32::max);
    assert!(
        (primary - x1).abs() <= 1.,
        "{what}: footer buttons end at {primary}, the column at {x1}"
    );
    let image = items(w, "KitImage").into_iter().next().expect("brand mark");
    let (ix0, _, ix1, _) = rect(&image);
    assert!(
        ((ix0 + ix1) / 2. - width / 2.).abs() <= 1.,
        "{what}: brand mark not centred"
    );
    assert!(
        (image.size().width - 64.).abs() <= 1.,
        "{what}: brand mark size"
    );
}

pub(super) async fn exercise(w: &MainWindow, path: &Path, mode: &str) {
    let host = wire(w);
    if mode == "permissions-dialog" {
        dialog(w, path, &host).await;
    } else {
        wizard(w, path, &host).await;
    }
}

async fn wizard(w: &MainWindow, path: &Path, host: &Host) {
    use slint::platform::{PointerEventButton, WindowEvent};
    w.set_settings_open(false);
    w.set_onboarding_open(true);
    w.set_onboarding_permission_rows(rows(&[
        (PermissionKind::Microphone, AccessState::Denied, false),
        (PermissionKind::SystemAudio, AccessState::Granted, false),
        (PermissionKind::Accessibility, AccessState::Denied, false),
    ]));
    w.set_onboarding_device_labels(ModelRc::new(VecModel::from(vec![
        "Automatique".into(),
        "Studio · USB".into(),
        "Station de conférence à très longue désignation pour les réunions".into(),
    ])));
    w.set_onboarding_device_ids(ModelRc::new(VecModel::from(vec![
        "".into(),
        "a".into(),
        "b".into(),
    ])));
    w.set_onboarding_model_labels(ModelRc::new(VecModel::from(vec![
        "Whisper Large V3 Turbo · Multilingual transcription model for meetings and interviews"
            .into(),
        "Parakeet".into(),
    ])));
    w.set_onboarding_model_ids(ModelRc::new(VecModel::from(vec!["m1".into(), "m2".into()])));
    w.set_onboarding_accessibility_granted(false);
    go(w, OnboardingStep::Permissions);

    // 1. Permissions: the shell, then every state of every row.
    column_checks(w, "permissions");
    assert_widths(w, "permissions");
    assert!(maybe_button(w, t(w, "Back", "Retour")).is_none());
    assert!(
        texts(w)
            .iter()
            .any(|s| s == t(w, "App Permissions", "Permissions"))
    );
    frame(w, path, "permissions-mixed");

    let vp = viewport(w);
    permission_matrix(w, host, &vp, "permissions", path);
    for (state, scene) in [
        (AccessState::Unknown, "unknown"),
        (AccessState::Denied, "denied"),
        (AccessState::NoDevice, "no-device"),
        (AccessState::Granted, "granted"),
        (AccessState::Unsupported, "unsupported"),
    ] {
        w.set_onboarding_permission_rows(uniform(state));
        settle(w);
        assert_widths(w, scene);
        frame(w, path, &format!("permissions-all-{scene}"));
    }
    let mic = t(w, "Mic", "Microphone");
    let checking = t(w, "Checking...", "Vérification…");
    w.set_onboarding_permission_rows(rows(&[
        (PermissionKind::Microphone, AccessState::Unknown, true),
        (PermissionKind::SystemAudio, AccessState::Granted, false),
        (PermissionKind::Accessibility, AccessState::Unknown, true),
        (PermissionKind::Calendar, AccessState::Denied, false),
    ]));
    settle(w);
    assert_eq!(
        button(w, &format!("{mic}, {checking}")).accessible_enabled(),
        Some(false)
    );
    frame(w, path, "permissions-checking");

    // Repair: idle, busy, cooldown with its confirmation, then a refusal.
    w.set_onboarding_permission_rows(rows(&[(
        PermissionKind::Accessibility,
        AccessState::Denied,
        false,
    )]));
    settle(w);
    let repair = t(w, "Repair", "Réparer");
    let repair_button = |w: &MainWindow| reveal_button(w, &vp, repair).expect("the repair button");
    assert_eq!(repair_button(w).accessible_enabled(), Some(true));
    repair_button(w).invoke_accessible_default_action();
    assert_eq!(host.take(), ["repair"]);
    w.set_onboarding_repair_busy(true);
    settle(w);
    assert_eq!(
        reveal_button(w, &vp, checking)
            .expect("repair while busy")
            .accessible_enabled(),
        Some(false)
    );
    w.set_onboarding_repair_busy(false);
    w.set_onboarding_repair_success(true);
    w.set_onboarding_repair_cooldown(true);
    w.set_onboarding_permissions_error(
        t(
            w,
            "macOS refused the repair",
            "macOS a refusé la réparation",
        )
        .into(),
    );
    settle(w);
    assert_eq!(repair_button(w).accessible_enabled(), Some(false));
    assert!(
        reveal(w, &vp, || groups(w)
            .into_iter()
            .find(|g| g.starts_with("macOS")))
        .is_some()
    );
    vp.scroll(0., -100000.);
    settle(w);
    frame(w, path, "permissions-repair");
    w.set_onboarding_repair_success(false);
    w.set_onboarding_repair_cooldown(false);
    w.set_onboarding_permissions_error("".into());
    w.set_onboarding_permission_rows(rows(&[
        (PermissionKind::Microphone, AccessState::Denied, false),
        (PermissionKind::SystemAudio, AccessState::Granted, false),
        (PermissionKind::Accessibility, AccessState::Denied, false),
    ]));
    settle(w);
    vp.scroll(0., 100000.);
    settle(w);

    // The language segment: a first-step choice that retranslates in place.
    let radio = |label: &str| {
        ElementQuery::from_root(w)
            .match_accessible_role(AccessibleRole::RadioButton)
            .match_accessible_label(label)
            .find_first()
    };
    let (english, french) = ("English", t(w, "French", "Français"));
    let was_en = en(w);
    click(
        w,
        &radio(if was_en { french } else { english }).expect("the other language"),
    );
    assert_eq!(en(w), !was_en);
    host.take();
    frame(w, path, "permissions-switched-language");
    let (english, french) = ("English", t(w, "French", "Français"));
    click(
        w,
        &radio(if was_en { english } else { french }).expect("the first language back"),
    );
    assert_eq!(en(w), was_en);
    host.take();
    assert!(
        texts(w)
            .iter()
            .any(|s| s == t(w, "App Permissions", "Permissions"))
    );

    let stops = tab_walk(w, "permissions", true);
    println!("permissions: {stops} focus stops, every ring whole");
    frame(w, path, "permissions-focus-end");

    // 2. Microphone: devices, the open menu, a refresh and the held press.
    go(w, OnboardingStep::Microphone);
    column_checks(w, "microphone");
    assert_widths(w, "microphone");
    assert!(maybe_button(w, t(w, "Back", "Retour")).is_some());
    frame(w, path, "microphone");
    let refresh = t(w, "Refresh", "Actualiser");
    button(w, refresh).invoke_accessible_default_action();
    assert_eq!(host.take(), ["refresh"]);
    let stops = tab_walk(w, "microphone", true);
    println!("microphone: {stops} focus stops");
    let trigger = |value: &str| {
        items(w, "KitSelectTrigger")
            .into_iter()
            .find(|e| e.accessible_value().as_deref() == Some(value))
            .unwrap_or_else(|| panic!("no select showing {value:?}"))
    };
    click(w, &trigger("Automatique"));
    frame(w, path, "microphone-menu");
    let rows_of = |label: &str| {
        items(w, "KitOption")
            .into_iter()
            .filter(|e| e.accessible_label().as_deref() == Some(label))
            .collect::<Vec<_>>()
    };
    assert_eq!(items(w, "KitOption").len(), 3);
    rows_of("Studio · USB")[0]
        .single_click(PointerEventButton::Left)
        .await;
    settle(w);
    assert_eq!(host.take(), ["device 1"]);
    assert_eq!(
        trigger("Studio · USB").accessible_value().as_deref(),
        Some("Studio · USB")
    );
    // A held press whose list is replaced under the finger resolves to nothing.
    let ids = Rc::new(VecModel::from(vec!["".into(), "a".into(), "b".into()]));
    w.set_onboarding_device_ids(ids.clone().into());
    settle(w);
    click(w, &trigger("Studio · USB"));
    let weak = w.as_weak();
    Timer::single_shot(Duration::from_millis(10), move || {
        ids.set_row_data(1, "x".into());
        ids.set_row_data(2, "y".into());
        settle(&weak.unwrap());
    });
    host.take();
    rows_of("Automatique")[0]
        .single_click(PointerEventButton::Left)
        .await;
    settle(w);
    assert!(
        host.take().iter().all(|e| !e.starts_with("device")),
        "a press held while the ids changed was accepted"
    );
    println!("microphone: held press under a replaced list discarded");
    if items(w, "KitOption").is_empty() {
        click(w, &trigger("Studio · USB"));
    }
    rows_of("Automatique")[0]
        .single_click(PointerEventButton::Left)
        .await;
    settle(w);
    assert_eq!(host.take(), ["device 0"]);
    // No microphone at all: Automatic stays and says nothing more.
    w.set_onboarding_device_labels(ModelRc::new(VecModel::from(vec!["Automatique".into()])));
    w.set_onboarding_device_ids(ModelRc::new(VecModel::from(vec!["".into()])));
    w.set_onboarding_selected_device_index(0);
    settle(w);
    frame(w, path, "microphone-none");
    w.set_onboarding_device_labels(ModelRc::new(VecModel::from(vec![
        "Automatique".into(),
        "Studio · USB".into(),
        "Station de conférence à très longue désignation pour les réunions".into(),
    ])));
    w.set_onboarding_device_ids(ModelRc::new(VecModel::from(vec![
        "".into(),
        "a".into(),
        "b".into(),
    ])));
    settle(w);

    // 3. Model: every phase.
    go(w, OnboardingStep::Model);
    column_checks(w, "model");
    assert_widths(w, "model");
    assert_eq!(
        w.get_onboarding_model_phase(),
        ModelPhase::Pick,
        "the fixture starts the step unpicked"
    );
    let download = t(w, "Download and continue", "Télécharger et continuer");
    assert_eq!(button(w, download).accessible_enabled(), Some(true));
    frame(w, path, "model-pick");
    let stops = tab_walk(w, "model pick", true);
    println!("model: {stops} focus stops");
    w.set_onboarding_busy(true);
    w.set_onboarding_continue_enabled(false);
    settle(w);
    assert_eq!(button(w, download).accessible_enabled(), Some(false));
    assert_eq!(
        button(w, t(w, "Back", "Retour")).accessible_enabled(),
        Some(false)
    );
    assert_ne!(
        items(w, "KitSelectTrigger")[0].accessible_enabled(),
        Some(true)
    );
    frame(w, path, "model-busy");
    w.set_onboarding_model_phase(ModelPhase::Downloading);
    w.set_onboarding_download_progress_fraction(0.42);
    w.set_onboarding_download_progress_label("encoder.onnx".into());
    settle(w);
    let progress = ElementQuery::from_root(w)
        .match_accessible_role(AccessibleRole::ProgressIndicator)
        .match_accessible_label(t(w, "Downloading model…", "Téléchargement du modèle…"))
        .find_first()
        .expect("download progress");
    assert_eq!(progress.accessible_value().as_deref(), Some("42"));
    assert!(items(w, "KitSelectTrigger").is_empty());
    frame(w, path, "model-downloading");
    w.set_onboarding_model_phase(ModelPhase::Loading);
    settle(w);
    frame(w, path, "model-loading");
    w.set_onboarding_model_phase(ModelPhase::Ready);
    w.set_onboarding_busy(false);
    w.set_onboarding_continue_enabled(true);
    settle(w);
    assert!(
        texts(w)
            .iter()
            .any(|s| s == t(w, "Model ready.", "Modèle prêt."))
    );
    assert_eq!(
        button(w, t(w, "Continue", "Continuer")).accessible_enabled(),
        Some(true)
    );
    frame(w, path, "model-ready");
    w.set_onboarding_model_phase(ModelPhase::Pick);
    w.set_onboarding_status_message(
        t(w, "The model is in error.", "Le modèle est en erreur.").into(),
    );
    settle(w);
    assert!(
        groups(w)
            .iter()
            .any(|g| g == t(w, "The model is in error.", "Le modèle est en erreur."))
    );
    frame(w, path, "model-error");
    let picker = reveal(w, &viewport(w), || {
        items(w, "KitSelectTrigger").into_iter().next()
    })
    .expect("the model picker");
    click(w, &picker);
    items(w, "KitOption")
        .iter()
        .find(|e| e.accessible_label().as_deref() == Some("Parakeet"))
        .expect("second model")
        .mock_single_click(PointerEventButton::Left);
    settle(w);
    assert_eq!(host.take(), ["model 1"]);
    w.set_onboarding_status_message("".into());

    // 4. Shortcut: capture, refusal, retry without a click, Escape, clear.
    go(w, OnboardingStep::Shortcut);
    column_checks(w, "shortcut");
    assert_widths(w, "shortcut");
    assert_eq!(
        button(w, t(w, "Finish", "Terminer")).accessible_enabled(),
        Some(true)
    );
    let field = t(w, "Keyboard shortcut", "Raccourci clavier");
    let recorder = by_id(w, "OnboardingView::shortcut-recorder");
    assert!(recorder.size().width > 24.);
    frame(w, path, "shortcut");
    let notice = t(
        w,
        "Automatic paste needs Accessibility, which isn't granted yet. Your dictation will go to your history instead.",
        "Le collage automatique a besoin de l'Accessibilité, qui n'est pas encore accordée. Le texte ira dans votre historique à la place.",
    );
    assert!(groups(w).iter().any(|g| g == notice));
    let review = t(w, "Review permissions", "Vérifier les permissions");
    rb(w, review).invoke_accessible_default_action();
    assert_eq!(host.take(), ["review"]);
    click(w, &rb(w, field));
    assert!(w.get_onboarding_shortcut_recording());
    frame(w, path, "shortcut-recording");
    send_key(w, "d".into());
    assert_eq!(
        w.get_onboarding_shortcut_rejection(),
        ShortcutRejection::MissingModifier
    );
    assert!(
        w.get_onboarding_shortcut_recording(),
        "a refusal keeps the capture"
    );
    let ring = items(w, "FocusOutline")
        .into_iter()
        .next()
        .expect("the recorder keeps the focus ring while capturing");
    assert!(
        within(rect(&ring), rect(&viewport(w))),
        "the recorder ring {:?} is whole in the viewport {:?} the moment the refusal shows",
        rect(&ring),
        rect(&viewport(w))
    );
    frame(w, path, "shortcut-refused");
    let message = t(
        w,
        "The shortcut must include a modifier key (Cmd, Ctrl, Shift, Alt) or be a function key.",
        "Le raccourci doit inclure une touche de modification (Cmd, Ctrl, Maj, Alt) ou être une touche de fonction.",
    );
    assert!(texts(w).iter().any(|s| s == message), "{:?}", texts(w));
    assert_eq!(
        rb(w, field).accessible_description().as_deref(),
        Some(message)
    );
    assert_widths(w, "shortcut refusal");
    click(w, &rb(w, field));
    if !w.get_onboarding_shortcut_recording() {
        click(w, &rb(w, field));
    }
    send_key(w, "d".into());
    {
        let ctrl: SharedString = Key::Control.into();
        w.window()
            .dispatch_event(WindowEvent::KeyPressed { text: ctrl.clone() });
        w.window()
            .dispatch_event(WindowEvent::KeyPressed { text: "l".into() });
        w.window()
            .dispatch_event(WindowEvent::KeyReleased { text: "l".into() });
        settle(w);
        assert_eq!(
            w.get_onboarding_shortcut_rejection(),
            ShortcutRejection::Conflict,
            "the push-to-talk binding is refused"
        );
        assert!(w.get_onboarding_shortcut_recording());
        w.window()
            .dispatch_event(WindowEvent::KeyPressed { text: "j".into() });
        w.window()
            .dispatch_event(WindowEvent::KeyReleased { text: "j".into() });
        w.window()
            .dispatch_event(WindowEvent::KeyReleased { text: ctrl });
        settle(w);
    }
    assert_eq!(host.pair.borrow().toggle, "CommandOrControl+J");
    assert_eq!(host.take(), ["save CommandOrControl+J"]);
    assert!(!w.get_onboarding_shortcut_recording());
    assert!(!texts(w).iter().any(|s| s == message));
    click(w, &rb(w, field));
    assert!(w.get_onboarding_shortcut_recording());
    send_key(w, Key::Escape.into());
    assert!(!w.get_onboarding_shortcut_recording(), "Escape cancels");
    assert_eq!(host.pair.borrow().toggle, "CommandOrControl+J");
    let clear = if en(w) {
        format!("Clear shortcut \u{201c}{field}\u{201d}")
    } else {
        format!("Effacer le raccourci \u{ab} {field} \u{bb}")
    };
    rb(w, &clear).invoke_accessible_default_action();
    settle(w);
    assert_eq!(host.pair.borrow().toggle, "");
    assert_eq!(host.take(), ["save "]);
    frame(w, path, "shortcut-cleared");
    w.set_onboarding_accessibility_granted(true);
    settle(w);
    assert!(
        reveal_button(w, &viewport(w), review).is_none(),
        "the notice follows the permission"
    );
    frame(w, path, "shortcut-granted");
    let stops = tab_walk(w, "shortcut", true);
    println!("shortcut: {stops} focus stops");

    // Footer: Back and Finish report themselves; a save in flight blocks both.
    button(w, t(w, "Back", "Retour")).invoke_accessible_default_action();
    button(w, t(w, "Finish", "Terminer")).invoke_accessible_default_action();
    assert_eq!(host.take(), ["back", "continue"]);
    w.set_onboarding_busy(true);
    w.set_onboarding_continue_enabled(false);
    settle(w);
    assert_eq!(
        button(w, t(w, "Finish", "Terminer")).accessible_enabled(),
        Some(false)
    );
    assert_eq!(
        button(w, t(w, "Back", "Retour")).accessible_enabled(),
        Some(false)
    );
    w.set_onboarding_busy(false);
    w.set_onboarding_continue_enabled(true);

    // Long content scrolls and the page starts at the top again on a new step.
    go(w, OnboardingStep::Permissions);
    w.set_onboarding_permission_rows(uniform(AccessState::Denied));
    settle(w);
    let vp = viewport(w);
    let bar_y = |w: &MainWindow| items(w, "KitProgress")[0].absolute_position().y;
    let step_y = |w: &MainWindow| items(w, "PermissionsStep")[0].absolute_position().y;
    go(w, OnboardingStep::Microphone);
    let top = bar_y(w);
    go(w, OnboardingStep::Permissions);
    let step_top = step_y(w);
    vp.scroll(0., -100000.);
    settle(w);
    let step_scrolled = step_y(w);
    println!("permissions: step y {step_top} -> {step_scrolled} after scrolling to the end");
    if step_scrolled < step_top - 1. {
        go(w, OnboardingStep::Microphone);
        let again = bar_y(w);
        assert!(
            (again - top).abs() <= 1.,
            "a new step starts at the top ({again} vs {top})"
        );
    }
    go(w, OnboardingStep::Permissions);
    w.set_onboarding_permission_rows(rows(&[
        (PermissionKind::Microphone, AccessState::Denied, false),
        (PermissionKind::SystemAudio, AccessState::Granted, false),
        (PermissionKind::Accessibility, AccessState::Denied, false),
    ]));
    settle(w);
}

async fn dialog(w: &MainWindow, path: &Path, host: &Host) {
    w.set_onboarding_open(false);
    w.set_settings_open(true);
    w.set_settings_tab(SettingsTab::System);
    w.set_onboarding_permission_rows(rows(&[
        (PermissionKind::Microphone, AccessState::Denied, false),
        (PermissionKind::SystemAudio, AccessState::Granted, false),
        (PermissionKind::Accessibility, AccessState::Denied, false),
    ]));
    settle(w);
    let trigger = by_id(w, "PermissionsSection::permissions-button");
    let title = t(w, "macOS permissions", "Permissions macOS");
    let (close, done) = (t(w, "Close", "Fermer"), t(w, "Done", "Terminé"));
    let panel = || {
        ElementHandle::find_by_accessible_label(w, title)
            .find(|e| e.accessible_role() == Some(AccessibleRole::Groupbox))
    };
    let has_ring = |e: &ElementHandle| {
        e.query_descendants()
            .match_inherits("FocusOutline")
            .find_first()
            .is_some()
    };

    // The trigger is reached with Tab and opened with Space.
    let page = by_id(w, "SettingsView::system-page");
    page.scroll(0., 100000.);
    settle(w);
    page.scroll(0., -100000.);
    for _ in 0..40 {
        if has_ring(&trigger) {
            break;
        }
        send_key(w, Key::Tab.into());
    }
    assert!(has_ring(&trigger), "Tab reaches the permissions trigger");
    send_key(w, " ".into());
    assert!(w.get_settings_permissions_dialog_open());
    assert_eq!(host.take(), ["open dialog"]);
    let panel_el = panel().expect("the dialog panel");
    let pr = rect(&panel_el);
    let (width, height) = (window_width(w), window_height(w));
    assert!(
        within(pr, (0., 0., width, height)),
        "the panel fits the window {pr:?}"
    );
    assert!(
        (((pr.0 + pr.2) / 2.) - width / 2.).abs() <= 1.5,
        "the panel is centred {pr:?} in {width}"
    );
    assert_widths(w, "dialog");
    assert!(
        panel_el
            .query_descendants()
            .match_inherits("PermissionsStep")
            .find_first()
            .is_some(),
        "the shared step is the dialog body"
    );
    frame(w, path, "dialog-open");

    // Inert background: nothing behind the panel answers. The panel can cover the
    // whole window, so what is "in the dialog" is decided by its descendants.
    let in_dialog: std::collections::BTreeSet<String> = ["KitButton", "KitIconButton"]
        .into_iter()
        .flat_map(|kind| panel_el.query_descendants().match_inherits(kind).find_all())
        .filter_map(|e| e.accessible_label())
        .map(|l| l.to_string())
        .collect();
    assert!(in_dialog.contains(close) && in_dialog.contains(done));
    // Controls scrolled out of the dialog body are not in the tree: name them too.
    let mut in_dialog = in_dialog;
    for kind in KINDS {
        let name = match kind {
            PermissionKind::Microphone => t(w, "Mic", "Microphone"),
            PermissionKind::SystemAudio => t(w, "System audio", "Audio système"),
            PermissionKind::Accessibility => t(w, "Accessibility", "Accessibilité"),
            PermissionKind::Calendar => t(w, "Calendar", "Calendrier"),
        };
        for action in [
            t(w, "Allow", "Autoriser"),
            t(w, "Open Settings", "Ouvrir les Réglages"),
            t(w, "Checking...", "Vérification…"),
        ] {
            in_dialog.insert(format!("{name}, {action}"));
        }
    }
    for label in [
        t(w, "System Settings", "Réglages Système"),
        t(w, "Repair", "Réparer"),
        t(w, "Checking...", "Vérification…"),
    ] {
        in_dialog.insert(label.to_string());
    }
    let behind: Vec<_> = items(w, "KitButton")
        .into_iter()
        .chain(items(w, "KitIconButton"))
        .filter(|b| {
            !b.accessible_label()
                .is_some_and(|l| in_dialog.contains(l.as_str()))
        })
        .collect();
    assert!(!behind.is_empty());
    for control in &behind {
        assert_eq!(
            control.accessible_enabled(),
            Some(false),
            "{:?}",
            control.accessible_label()
        );
        control.invoke_accessible_default_action();
    }
    for control in items(w, "KitSwitch") {
        assert_eq!(control.accessible_enabled(), Some(false));
        control.invoke_accessible_default_action();
    }
    for control in items(w, "KitSelectTrigger") {
        assert_ne!(control.accessible_enabled(), Some(true));
    }
    settle(w);
    assert!(host.take().is_empty(), "no background control answered");

    // Tab stays inside the panel and goes round.
    let mut seen = std::collections::BTreeSet::new();
    let mut first = None;
    let mut round = false;
    for _ in 0..40 {
        send_key(w, Key::Tab.into());
        let Some((owner, label)) = focus_owner_labelled(w) else {
            continue;
        };
        let ring = items(w, "FocusOutline")
            .into_iter()
            .next()
            .expect("a ring for the owner");
        assert!(
            label.is_some_and(|l| in_dialog.contains(l.as_str())),
            "the focus left the dialog for {owner}"
        );
        assert!(
            center_in(&ring, pr),
            "the ring of {owner} left the dialog panel"
        );
        assert!(
            within(rect(&ring), pr),
            "the ring of {owner} is clipped by the panel {:?}",
            rect(&ring)
        );
        if first.as_ref() == Some(&owner) && seen.len() > 1 {
            round = true;
            break;
        }
        first.get_or_insert(owner.clone());
        seen.insert(owner);
    }
    assert!(round, "Tab goes round inside the dialog: {seen:?}");
    println!(
        "dialog: {} focus stops, all inside the panel, the walk comes back round",
        seen.len()
    );
    assert!(w.get_settings_permissions_dialog_open());
    assert!(host.take().is_empty());

    // Escape, the close icon, Done: each closes it and the trigger has the focus.
    send_key(w, Key::Escape.into());
    assert!(!w.get_settings_permissions_dialog_open());
    assert!(has_ring(&trigger), "the focus came back to the trigger");
    send_key(w, " ".into());
    assert!(w.get_settings_permissions_dialog_open());
    click(w, &button(w, close));
    assert!(!w.get_settings_permissions_dialog_open());
    assert!(has_ring(&trigger));
    send_key(w, " ".into());
    assert!(w.get_settings_permissions_dialog_open());
    click(w, &button(w, done));
    assert!(!w.get_settings_permissions_dialog_open());
    assert!(has_ring(&trigger));
    host.take();

    // A press outside the panel only dismisses; the control under it stays unchanged.
    send_key(w, " ".into());
    assert!(w.get_settings_permissions_dialog_open());
    host.take();
    let pr = rect(&panel().expect("the dialog panel"));
    assert!(pr.0 > 4., "a scrim margin left of the panel to press on");
    let outside = slint::LogicalPosition::new(pr.0 / 2., (pr.1 + pr.3) / 2.);
    let settings_were_open = w.get_settings_open();
    let tab_before = w.get_settings_tab();
    {
        use slint::platform::{PointerEventButton, WindowEvent};
        w.window()
            .dispatch_event(WindowEvent::PointerMoved { position: outside });
        w.window().dispatch_event(WindowEvent::PointerPressed {
            position: outside,
            button: PointerEventButton::Left,
        });
        w.window().dispatch_event(WindowEvent::PointerReleased {
            position: outside,
            button: PointerEventButton::Left,
        });
        settle(w);
    }
    assert!(!w.get_settings_permissions_dialog_open());
    assert_eq!(w.get_settings_open(), settings_were_open);
    assert_eq!(w.get_settings_tab(), tab_before);
    assert!(host.take().is_empty());

    // Every state inside the dialog, and its actions.
    send_key(w, " ".into());
    assert!(w.get_settings_permissions_dialog_open());
    let body = || {
        ElementHandle::find_by_accessible_label(w, title)
            .filter(|e| e.accessible_role() == Some(AccessibleRole::Groupbox))
            .min_by(|a, b| a.size().height.total_cmp(&b.size().height))
            .expect("the dialog body")
    };
    permission_matrix(w, host, &body(), "dialog", path);
    for (state, scene) in [
        (AccessState::Unknown, "unknown"),
        (AccessState::Denied, "denied"),
        (AccessState::NoDevice, "no-device"),
        (AccessState::Granted, "granted"),
        (AccessState::Unsupported, "unsupported"),
    ] {
        w.set_onboarding_permission_rows(uniform(state));
        settle(w);
        body().scroll(0., 100000.);
        settle(w);
        assert_widths(w, scene);
        let pr = rect(&panel().expect("the dialog is open"));
        assert!(
            within(pr, (0., 0., width, height)),
            "{scene}: the panel fits {pr:?}"
        );
        frame(w, path, &format!("dialog-all-{scene}"));
    }

    // The whole step with its repair row, the repair states and an error.
    w.set_onboarding_permission_rows(rows(&[
        (PermissionKind::Microphone, AccessState::Denied, false),
        (PermissionKind::SystemAudio, AccessState::Granted, false),
        (PermissionKind::Accessibility, AccessState::Denied, false),
    ]));
    settle(w);
    host.take();
    let mic = t(w, "Mic", "Microphone");
    let allow = t(w, "Allow", "Autoriser");
    let repair = t(w, "Repair", "Réparer");
    let scroller = body();
    reveal_button(w, &scroller, &format!("{mic}, {allow}"))
        .expect("the microphone action")
        .invoke_accessible_default_action();
    reveal_button(w, &scroller, repair)
        .expect("the repair action")
        .invoke_accessible_default_action();
    settle(w);
    assert_eq!(host.take(), ["grant Microphone", "repair"]);
    w.set_onboarding_repair_busy(true);
    settle(w);
    assert_eq!(
        reveal_button(w, &scroller, t(w, "Checking...", "Vérification…"))
            .expect("repair while busy")
            .accessible_enabled(),
        Some(false)
    );
    w.set_onboarding_repair_busy(false);
    w.set_onboarding_repair_success(true);
    w.set_onboarding_repair_cooldown(true);
    w.set_onboarding_permissions_error(
        t(
            w,
            "macOS refused the repair",
            "macOS a refusé la réparation",
        )
        .into(),
    );
    settle(w);
    assert_eq!(
        reveal_button(w, &scroller, repair)
            .expect("the repair action")
            .accessible_enabled(),
        Some(false)
    );
    assert!(
        reveal(w, &scroller, || groups(w)
            .into_iter()
            .find(|g| g.starts_with("macOS")))
        .is_some()
    );
    assert_widths(w, "dialog repair");
    frame(w, path, "dialog-repair");
    w.set_onboarding_repair_success(false);
    w.set_onboarding_repair_cooldown(false);
    w.set_onboarding_permissions_error("".into());
    settle(w);

    // Another modal above it makes the dialog inert and keeps Escape for itself.
    let under = reveal_button(w, &scroller, repair).expect("the repair action");
    w.set_whats_new_open(true);
    settle(w);
    assert_ne!(
        under.accessible_enabled(),
        Some(true),
        "the repair action under the modal"
    );
    under.invoke_accessible_default_action();
    settle(w);
    assert!(
        host.take().is_empty(),
        "nothing under the second modal answered"
    );
    send_key(w, Key::Escape.into());
    assert!(!w.get_whats_new_open());
    assert!(
        w.get_settings_permissions_dialog_open(),
        "Escape closed the top modal only"
    );
    settle(w);
    // The keyboard is back in the dialog: Tab reaches its controls, inside the panel.
    send_key(w, Key::Tab.into());
    let owner = focus_owner(w).expect("a dialog control has the focus after the top modal closed");
    let ring = items(w, "FocusOutline")
        .into_iter()
        .next()
        .expect("its ring");
    assert!(
        center_in(&ring, rect(&panel().unwrap())),
        "{owner} is outside the dialog"
    );
    send_key(w, Key::Escape.into());
    assert!(!w.get_settings_permissions_dialog_open());
    frame(w, path, "dialog-closed");
}
