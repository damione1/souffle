//! The real window paths behind the settings focus ring and the microphone rows.
use crate::{MainWindow, SettingsTab, audio_ui, microphone_list};
use i_slint_backend_testing::{AccessibleRole, ElementHandle, ElementQuery, ElementRoot};
use slint::platform::software_renderer::{MinimalSoftwareWindow, Rgb565Pixel};
use slint::platform::{Key, Platform, PointerEventButton, WindowAdapter, WindowEvent};
use slint::{ComponentHandle, LogicalPosition};
use souffle_lib::audio::{AudioInputDevice, InputPriority, KnownDevice, TransportType};
use std::rc::Rc;

const WIDTH: u32 = 1040;
const HEIGHT: u32 = 1900;

thread_local! {
    static TEST_WINDOW: std::cell::RefCell<Option<Rc<MinimalSoftwareWindow>>> =
        const { std::cell::RefCell::new(None) };
}

struct TestPlatform;

impl Platform for TestPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        let window = MinimalSoftwareWindow::new(Default::default());
        TEST_WINDOW.with(|slot| *slot.borrow_mut() = Some(window.clone()));
        Ok(window)
    }

    fn duration_since_start(&self) -> std::time::Duration {
        std::time::Duration::from_millis(CLOCK.with(|clock| clock.get()))
    }
}

thread_local!(static CLOCK: std::cell::Cell<u64> = const { std::cell::Cell::new(0) });

fn settle() {
    let window = TEST_WINDOW
        .with(|slot| slot.borrow().clone())
        .expect("no test window");
    let size = window.size();
    let mut frame = vec![Rgb565Pixel::default(); (size.width * size.height) as usize];
    for _ in 0..8 {
        CLOCK.with(|clock| clock.set(clock.get() + 100));
        slint::platform::update_timers_and_animations();
        window.request_redraw();
        window.draw_if_needed(|renderer| {
            renderer.render(&mut frame, size.width as usize);
        });
    }
}

fn main_window() -> MainWindow {
    let _ = slint::platform::set_platform(Box::new(TestPlatform));
    let window = crate::settings_kit::new_window().unwrap();
    crate::typography::initialize(&window).unwrap();
    window
        .window()
        .set_size(slint::PhysicalSize::new(WIDTH, HEIGHT));
    let weak = window.as_weak();
    window.on_settings_requested(move || weak.unwrap().set_settings_open(true));
    window.show().unwrap();
    settle();
    window
}

fn labelled(window: &MainWindow, role: AccessibleRole, labels: &[&str]) -> ElementHandle {
    window
        .root_element()
        .query_descendants()
        .match_predicate({
            let labels: Vec<String> = labels.iter().map(|label| label.to_string()).collect();
            move |element| {
                element.accessible_role() == Some(role)
                    && element
                        .accessible_label()
                        .is_some_and(|label| labels.iter().any(|wanted| label == wanted.as_str()))
            }
        })
        .find_first()
        .unwrap_or_else(|| panic!("no {role:?} labelled {labels:?}"))
}

fn gear(window: &MainWindow) -> ElementHandle {
    labelled(window, AccessibleRole::Button, &["Réglages", "Settings"])
}

fn centre(element: &ElementHandle) -> LogicalPosition {
    let origin = element.absolute_position();
    let size = element.size();
    LogicalPosition::new(origin.x + size.width / 2., origin.y + size.height / 2.)
}

fn click(window: &MainWindow, element: &ElementHandle) {
    let position = centre(element);
    for event in [
        WindowEvent::PointerMoved { position },
        WindowEvent::PointerPressed {
            position,
            button: PointerEventButton::Left,
        },
        WindowEvent::PointerReleased {
            position,
            button: PointerEventButton::Left,
        },
    ] {
        window.window().dispatch_event(event);
    }
    settle();
}

fn press(window: &MainWindow, key: Key) {
    window
        .window()
        .dispatch_event(WindowEvent::KeyPressed { text: key.into() });
    window
        .window()
        .dispatch_event(WindowEvent::KeyReleased { text: key.into() });
    settle();
}

fn rings(window: &MainWindow) -> Vec<ElementHandle> {
    ElementQuery::from_root(window)
        .match_inherits("FocusOutline")
        .find_all()
        .into_iter()
        .filter(|ring| ring.size().width > 0.)
        .collect()
}

fn surrounds(ring: &ElementHandle, element: &ElementHandle) -> bool {
    let origin = ring.absolute_position();
    let size = ring.size();
    let point = centre(element);
    (origin.x..origin.x + size.width).contains(&point.x)
        && (origin.y..origin.y + size.height).contains(&point.y)
}

fn open_settings_and_click_a_tab(window: &MainWindow) {
    click(window, &gear(window));
    assert!(window.get_settings_open());
    click(window, &labelled(window, AccessibleRole::Tab, &["Audio"]));
    assert_eq!(window.get_settings_tab(), SettingsTab::Audio);
}

#[test]
fn a_mouse_leaves_no_ring_on_a_settings_tab_or_on_the_gear() {
    let window = main_window();
    open_settings_and_click_a_tab(&window);
    assert_eq!(rings(&window).len(), 0, "clicked tab");

    click(&window, &gear(&window));
    assert!(!window.get_settings_open());
    assert_eq!(rings(&window).len(), 0, "gear after a mouse close");
}

#[test]
fn the_keyboard_rings_the_tab_it_moves_to_and_the_gear_it_returns_to() {
    let window = main_window();
    open_settings_and_click_a_tab(&window);
    press(&window, Key::RightArrow);
    let tab = labelled(&window, AccessibleRole::Tab, &["Interface"]);
    let shown = rings(&window);
    assert_eq!(shown.len(), 1, "arrow navigation");
    assert!(surrounds(&shown[0], &tab));

    press(&window, Key::Escape);
    assert!(!window.get_settings_open());
    let shown = rings(&window);
    assert_eq!(shown.len(), 1, "gear after a keyboard close");
    assert!(surrounds(&shown[0], &gear(&window)));
}

#[test]
fn microphone_rows_show_whole_ages_and_keep_their_actions_in_columns() {
    let window = main_window();
    window.set_settings_open(true);
    window.set_settings_tab(SettingsTab::Audio);
    let now = chrono::Utc::now().timestamp();
    // None of these ages divides evenly by its unit.
    let ages = [
        ("days", 4 * 86_400 + 74_400),
        ("hours", 2 * 3_600 + 1_900),
        ("minutes", 5 * 60 + 29),
    ];
    let connected = [AudioInputDevice {
        uid: "builtin".into(),
        name: "builtin".into(),
        transport: TransportType::BuiltIn,
        is_default: true,
    }];
    let priority = InputPriority {
        priorities: vec!["days".into(), "builtin".into()],
        known: ages
            .iter()
            .map(|(uid, age)| KnownDevice {
                uid: uid.to_string(),
                name: uid.to_string(),
                last_seen: now - age,
            })
            .collect(),
    };
    let list = microphone_list::build_microphone_list(&connected, &priority);
    audio_ui::populate_microphones(&window, &list);
    settle();

    let rows = ElementQuery::from_root(&window)
        .match_inherits("MicrophonePriorityRow")
        .find_all();
    assert!(rows.len() >= 3, "mounted rows: {}", rows.len());
    let mut columns: [Vec<f32>; 3] = Default::default();
    for row in &rows {
        let name = row.accessible_label().unwrap().to_string();
        let description = row.accessible_description().unwrap().to_string();
        let expected: &[&str] = match name.as_str() {
            "days" => &["il y a 4 j", "4 d ago"],
            "hours" => &["il y a 2 h", "2 h ago"],
            "minutes" => &["il y a 5 min", "5 min ago"],
            _ => &[],
        };
        assert!(
            expected.is_empty() || expected.iter().any(|end| description.ends_with(end)),
            "{name}: {description}"
        );
        let buttons = row
            .query_descendants()
            .match_inherits("MicrophoneActionButton")
            .find_all();
        assert_eq!(
            buttons.len(),
            if name == "builtin" { 2 } else { 3 },
            "{name}"
        );
        for (column, button) in columns.iter_mut().zip(&buttons) {
            column.push(button.absolute_position().x);
        }
    }
    for column in &columns {
        assert!(
            column.windows(2).all(|pair| pair[0] == pair[1]),
            "columns: {columns:?}"
        );
    }
}

#[test]
fn the_permissions_step_needs_no_scrolling_in_an_860px_window() {
    use crate::{AccessState, OnboardingStep, PermissionKind, PermissionRow};
    let window = main_window();
    window
        .window()
        .set_size(slint::PhysicalSize::new(WIDTH, 860));
    window.set_onboarding_open(true);
    window.set_onboarding_step(OnboardingStep::Permissions);
    window.set_onboarding_step_count(4);
    let rows = |states: [AccessState; 3]| {
        let kinds = [
            PermissionKind::Microphone,
            PermissionKind::SystemAudio,
            PermissionKind::Accessibility,
        ];
        let rows: Vec<_> = kinds
            .into_iter()
            .zip(states)
            .map(|(kind, state)| PermissionRow {
                kind,
                state,
                busy: false,
            })
            .collect();
        slint::ModelRc::new(slint::VecModel::from(rows))
    };
    // The overflow of the scrolled column past its viewport, in logical pixels.
    let overflow = || {
        settle();
        let height = |id: &str| {
            ElementHandle::find_by_element_id(&window, id)
                .next()
                .unwrap_or_else(|| panic!("{id}"))
                .size()
                .height
        };
        height("OnboardingView::content") - height("OnboardingView::viewport")
    };
    use AccessState::{Denied, NoDevice, Unknown};
    for (name, states, error) in [
        ("first run", [Unknown, Unknown, Denied], ""),
        ("everything denied", [Denied, Denied, Denied], ""),
        ("no microphone", [NoDevice, Unknown, Denied], ""),
        (
            "status error",
            [Unknown, Unknown, Denied],
            "The permission status could not be read.",
        ),
    ] {
        window.set_onboarding_permission_rows(rows(states));
        window.set_onboarding_permissions_error(error.into());
        let overflow = overflow();
        assert!(overflow <= 0.5, "{name} overflows by {overflow}px");
    }
}

#[test]
fn the_last_wizard_step_needs_no_scrolling_in_an_860px_window() {
    use crate::{OnboardingStep, ShortcutNotice};
    let window = main_window();
    window
        .window()
        .set_size(slint::PhysicalSize::new(WIDTH, 860));
    window.set_onboarding_open(true);
    window.set_onboarding_step(OnboardingStep::Shortcut);
    window.set_onboarding_step_count(4);
    window.set_onboarding_step_index(3);
    window.set_onboarding_toggle_shortcut_label("⌘ ⇧ Space".into());
    for locale in ["fr", "en"] {
        slint::select_bundled_translation(locale).expect("bundled translation");
        // The tallest state: no Accessibility, a shortcut notice and a refused
        // launch at login.
        for (granted, notice, failed) in [
            (true, ShortcutNotice::None, false),
            (false, ShortcutNotice::None, false),
            (false, ShortcutNotice::TypesCharacter, true),
        ] {
            window.set_onboarding_accessibility_granted(granted);
            window.set_onboarding_shortcut_notice(notice);
            window.set_onboarding_autostart_failed(failed);
            settle();
            let height = |id: &str| {
                ElementHandle::find_by_element_id(&window, id)
                    .next()
                    .unwrap_or_else(|| panic!("{id}"))
                    .size()
                    .height
            };
            let overflow = height("OnboardingView::content") - height("OnboardingView::viewport");
            assert!(
                overflow <= 0.5,
                "{locale}, granted {granted}, failed {failed}: overflows by {overflow}px"
            );
        }
    }
}
