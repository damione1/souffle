//! The Meetings page driven through the real `MainWindow`, the kit controls, the
//! real settings worker on a throwaway database and the real value controller.
//! EventKit, the permission prompt and System Settings are replaced by a
//! recorder, so no calendar, TCC entry or preference of the user is touched.
use super::*;
use crate::SettingsTab;
use crate::calendar_selection::publish_rows;
use crate::settings_values::SettingsValueController;
use i_slint_backend_testing::{ElementHandle, ElementQuery};
use slint::ComponentHandle;
use slint::platform::{
    Key, Platform, PointerEventButton, WindowAdapter, WindowEvent,
    software_renderer::MinimalSoftwareWindow,
};
use souffle_lib::commands::SettingsSaveError;
use souffle_lib::db::Database;
use souffle_lib::settings::{AppSettings, SettingsOptions, Theme as SettingsTheme};
use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::task::{Context, Waker};
use std::time::{Duration, Instant};

const WIDTH: u32 = 1040;
const HEIGHT: u32 = 2400;

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

fn info(id: &str, title: &str, source: Option<&str>) -> CalendarInfo {
    CalendarInfo {
        id: id.into(),
        title: title.into(),
        source_title: source.map(str::to_string),
    }
}

fn ids(values: &[&str]) -> Vec<String> {
    values.iter().map(|id| id.to_string()).collect()
}

fn work_calendars() -> Vec<CalendarInfo> {
    vec![
        info("a1", "Travail", Some("iCloud")),
        info("a2", "Travail", Some("iCloud")),
        info("g1", "Travail", Some("Google")),
        info("n1", "Personnel", None),
    ]
}

struct World {
    window: MainWindow,
    host: Rc<CalendarHost>,
    io: Rc<SettingsIoCoordinator>,
    database: std::path::PathBuf,
    authorization: Arc<Mutex<PermState>>,
    listing: Arc<Mutex<Result<Vec<CalendarInfo>, String>>>,
    access_answer: Rc<Cell<PermState>>,
    access_requests: Rc<Cell<u32>>,
    system_settings_opened: Rc<Cell<u32>>,
    tasks: Rc<RefCell<Vec<LocalFuture<()>>>>,
    upcoming: Rc<RefCell<Vec<&'static str>>>,
    hold: Arc<AtomicBool>,
    saves: Arc<AtomicUsize>,
    _directory: tempfile::TempDir,
}

fn world(initial: AppSettings, permission: PermState, listing: Vec<CalendarInfo>) -> World {
    world_with(initial, permission, listing, true)
}

/// `snapshot: false` is a page opened before the first settings snapshot arrived.
fn world_with(
    initial: AppSettings,
    permission: PermState,
    listing: Vec<CalendarInfo>,
    snapshot: bool,
) -> World {
    let _ = slint::platform::set_platform(Box::new(TestPlatform));
    let window = crate::settings_kit::new_window().unwrap();
    slint::select_bundled_translation("fr").unwrap();
    window.global::<crate::SettingsOptions>().invoke_localize();
    window
        .window()
        .set_size(slint::PhysicalSize::new(WIDTH, HEIGHT));
    window.set_onboarding_open(false);
    window.global::<crate::Theme>().set_dark(false);

    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("settings.db");
    let initial = AppSettings {
        theme: SettingsTheme::Light,
        locale: "fr".into(),
        ..initial
    };
    initial.save(&Database::open(&database).unwrap()).unwrap();
    let initial = AppSettings::load(&Database::open(&database).unwrap()).unwrap();

    crate::settings_ui::populate(&window, &initial);
    let cache = if snapshot {
        SettingsCache::with_observed(&window, initial.clone())
    } else {
        SettingsCache::new(&window)
    };
    let hold = Arc::new(AtomicBool::new(false));
    let saves = Arc::new(AtomicUsize::new(0));
    let load_path = database.clone();
    let save_path = database.clone();
    let save_hold = hold.clone();
    let save_count = saves.clone();
    let io = SettingsIoCoordinator::with_functions(
        cache.clone(),
        move || AppSettings::load(&Database::open(&load_path)?),
        || Ok(AppSettings::default()),
        move |settings: AppSettings| {
            while save_hold.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(1));
            }
            save_count.fetch_add(1, Ordering::SeqCst);
            let database = Database::open(&save_path).unwrap();
            let result = settings
                .save(&database)
                .map_err(|message| SettingsSaveError::NotCommitted { message });
            SettingsSaveOutcome::from_results(result, AppSettings::load(&database))
        },
        |settings| SettingsSaveOutcome::Observed {
            settings: Box::new(settings),
            result: Ok(()),
        },
    );
    let token = io.begin_open();
    if snapshot {
        assert!(io.seed_if_current(token, io.current_revision(), initial.clone()));
    }
    let values = SettingsValueController::new(
        &window,
        cache.clone(),
        io.clone(),
        Rc::new(RefCell::new(Vec::new())),
        Rc::new(RefCell::new(None)),
    );
    if snapshot {
        values.observe_loaded(&initial);
    }
    crate::settings_values::wire(&window, values);

    let authorization = Arc::new(Mutex::new(permission));
    let listing = Arc::new(Mutex::new(Ok(listing)));
    let access_answer = Rc::new(Cell::new(permission));
    let access_requests = Rc::new(Cell::new(0));
    let system_settings_opened = Rc::new(Cell::new(0));
    let tasks: Rc<RefCell<Vec<LocalFuture<()>>>> = Rc::default();
    let upcoming: Rc<RefCell<Vec<&'static str>>> = Rc::default();
    let backend = CalendarBackend {
        authorization: {
            let state = authorization.clone();
            Arc::new(move || *state.lock().unwrap())
        },
        list: {
            let state = listing.clone();
            Arc::new(move || state.lock().unwrap().clone())
        },
        request_access: {
            let answer = access_answer.clone();
            let requests = access_requests.clone();
            Rc::new(move || {
                requests.set(requests.get() + 1);
                let answer = answer.get();
                Box::pin(async move { answer })
            })
        },
        open_system_settings: {
            let opened = system_settings_opened.clone();
            Rc::new(move || opened.set(opened.get() + 1))
        },
        spawn: {
            let tasks = tasks.clone();
            Rc::new(move |future| tasks.borrow_mut().push(future))
        },
        clear_upcoming: {
            let log = upcoming.clone();
            Rc::new(move |_| log.borrow_mut().push("clear"))
        },
        refresh_upcoming: {
            let log = upcoming.clone();
            Rc::new(move |_| log.borrow_mut().push("refresh"))
        },
    };
    let host = CalendarHost::new(io.clone(), cache, backend);
    wire(&window, &host);

    window.set_settings_open(true);
    window.set_settings_tab(SettingsTab::Meetings);
    window.show().unwrap();
    settle();
    let world = World {
        window,
        host,
        io,
        database,
        authorization,
        listing,
        access_answer,
        access_requests,
        system_settings_opened,
        tasks,
        upcoming,
        hold,
        saves,
        _directory: directory,
    };
    reload(&world.window, &world.host);
    pump(&world);
    world
}

fn pump(world: &World) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        world.io.drain_for_test();
        let pending: Vec<_> = world.tasks.borrow_mut().drain(..).collect();
        let mut waiting = Vec::new();
        for mut task in pending {
            let mut context = Context::from_waker(Waker::noop());
            if task.as_mut().poll(&mut context).is_pending() {
                waiting.push(task);
            }
        }
        let idle =
            waiting.is_empty() && world.tasks.borrow().is_empty() && world.io.is_idle_for_test();
        world.tasks.borrow_mut().extend(waiting);
        settle();
        if idle {
            return;
        }
        assert!(Instant::now() < deadline, "the page never settled");
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn stored(world: &World) -> AppSettings {
    AppSettings::load(&Database::open(&world.database).unwrap()).unwrap()
}

fn enabled_with(selected: &[&str]) -> AppSettings {
    AppSettings {
        calendar_integration_enabled: true,
        calendar_autostart_enabled: false,
        calendar_selected_ids: ids(selected),
        ..AppSettings::default()
    }
}

fn checkboxes(world: &World) -> Vec<ElementHandle> {
    ElementQuery::from_root(&world.window)
        .match_inherits("KitCheckbox")
        .find_all()
}

fn checkbox(world: &World, title: &str, source: &str) -> ElementHandle {
    checkboxes(world)
        .into_iter()
        .find(|e| {
            e.accessible_label().as_deref() == Some(title)
                && e.accessible_description().unwrap_or_default().as_str() == source
        })
        .unwrap_or_else(|| panic!("no calendar {title:?} of {source:?}"))
}

fn checked(element: &ElementHandle) -> bool {
    element.accessible_checked().expect("a checkbox state")
}

fn shown(world: &World) -> Vec<(String, bool)> {
    world
        .window
        .get_settings_calendars()
        .iter()
        .map(|row| (row.id.to_string(), row.selected))
        .collect()
}

fn alerts(world: &World) -> Vec<String> {
    ElementQuery::from_root(&world.window)
        .match_inherits("KitAlert")
        .find_all()
        .iter()
        .filter_map(ElementHandle::accessible_label)
        .map(|label| label.to_string())
        .collect()
}

fn element(world: &World, id: &str) -> ElementHandle {
    ElementHandle::find_by_element_id(&world.window, id)
        .next()
        .unwrap_or_else(|| panic!("missing {id}"))
}

fn press_at(world: &World, element: &ElementHandle) -> slint::LogicalPosition {
    let position = slint::LogicalPosition::new(
        element.absolute_position().x + element.size().width / 2.0,
        element.absolute_position().y + element.size().height / 2.0,
    );
    world
        .window
        .window()
        .dispatch_event(WindowEvent::PointerMoved { position });
    world
        .window
        .window()
        .dispatch_event(WindowEvent::PointerPressed {
            position,
            button: PointerEventButton::Left,
        });
    settle();
    position
}

fn release_at(world: &World, position: slint::LogicalPosition) {
    world
        .window
        .window()
        .dispatch_event(WindowEvent::PointerReleased {
            position,
            button: PointerEventButton::Left,
        });
    settle();
}

fn click(world: &World, element: &ElementHandle) {
    let position = press_at(world, element);
    release_at(world, position);
}

fn tap(world: &World, text: impl Into<slint::SharedString> + Clone) {
    world
        .window
        .window()
        .dispatch_event(WindowEvent::KeyPressed {
            text: text.clone().into(),
        });
    world
        .window
        .window()
        .dispatch_event(WindowEvent::KeyReleased { text: text.into() });
    settle();
}

fn key_down(world: &World, text: impl Into<slint::SharedString>) {
    world
        .window
        .window()
        .dispatch_event(WindowEvent::KeyPressed { text: text.into() });
    settle();
}

fn key_up(world: &World, text: impl Into<slint::SharedString>) {
    world
        .window
        .window()
        .dispatch_event(WindowEvent::KeyReleased { text: text.into() });
    settle();
}

// Pure selection rules

fn catalogue() -> Vec<CalendarInfo> {
    vec![
        info("a", "A", Some("iCloud")),
        info("b", "B", Some("iCloud")),
        info("c", "C", Some("Google")),
    ]
}

#[test]
fn an_empty_stored_list_means_every_calendar_and_unchecking_one_keeps_the_others() {
    let change = change_selection(&[], &catalogue(), "b", false);
    assert_eq!(change, SelectionChange::Write(ids(&["a", "c"])));
}

#[test]
fn checking_the_last_unchecked_calendar_returns_to_the_empty_all_convention() {
    let change = change_selection(&ids(&["a", "c"]), &catalogue(), "b", true);
    assert_eq!(change, SelectionChange::Write(Vec::new()));
}

#[test]
fn the_last_checked_calendar_cannot_be_unchecked() {
    let change = change_selection(&ids(&["b"]), &catalogue(), "b", false);
    assert_eq!(change, SelectionChange::Unchanged(Refusal::LastCalendar));
}

#[test]
fn an_unknown_calendar_and_an_already_applied_state_write_nothing() {
    assert_eq!(
        change_selection(&[], &catalogue(), "ghost", true),
        SelectionChange::Unchanged(Refusal::UnknownCalendar)
    );
    assert_eq!(
        change_selection(&[], &catalogue(), "a", true),
        SelectionChange::Unchanged(Refusal::AlreadyApplied)
    );
    assert_eq!(
        change_selection(&ids(&["a"]), &catalogue(), "b", false),
        SelectionChange::Unchanged(Refusal::AlreadyApplied)
    );
}

#[test]
fn an_obsolete_stored_identifier_is_kept_but_never_counted_as_selected() {
    // Two stored entries against a catalogue of three: a size comparison would
    // call "a + ghost + c" complete, the effective sets do not.
    let partial = change_selection(&ids(&["a", "ghost"]), &catalogue(), "c", true);
    assert_eq!(partial, SelectionChange::Write(ids(&["a", "ghost", "c"])));

    // The effective set {a, b} equals the two calendars of this catalogue.
    let two = &catalogue()[..2];
    let all = change_selection(&ids(&["a", "ghost"]), two, "b", true);
    assert_eq!(all, SelectionChange::Write(Vec::new()));

    // Only the obsolete identifier is stored: nothing visible is selected, so
    // unchecking is already applied and checking starts a fresh selection.
    assert_eq!(
        change_selection(&ids(&["ghost"]), &catalogue(), "a", false),
        SelectionChange::Unchanged(Refusal::AlreadyApplied)
    );
    assert_eq!(
        change_selection(&ids(&["ghost"]), &catalogue(), "a", true),
        SelectionChange::Write(ids(&["ghost", "a"]))
    );
}

#[test]
fn homonymous_calendars_are_told_apart_by_identifier_and_ordered_stably() {
    let rows = rows(&work_calendars(), &[]);
    let order: Vec<_> = rows.iter().map(|row| row.id.to_string()).collect();
    assert_eq!(order, ["n1", "g1", "a1", "a2"]);
    let headers: Vec<_> = rows
        .iter()
        .map(|row| (row.id.to_string(), row.show_header))
        .collect();
    assert_eq!(
        headers,
        [
            ("n1".to_string(), true),
            ("g1".to_string(), true),
            ("a1".to_string(), true),
            ("a2".to_string(), false)
        ]
    );
    let toggled = change_selection(&[], &work_calendars(), "a2", false);
    assert_eq!(toggled, SelectionChange::Write(ids(&["a1", "g1", "n1"])));
}

// Permission flow

#[test]
fn a_refusal_shows_the_alert_and_its_action_then_the_grant_recovers_the_page() {
    let w = world(AppSettings::default(), PermState::Denied, work_calendars());
    w.access_answer.set(PermState::Denied);
    assert!(
        alerts(&w).iter().any(|a| a.contains("refusé")),
        "a refusal is explained even while the integration is off"
    );

    element(&w, "CalendarSection::calendar-switch").invoke_accessible_default_action();
    pump(&w);
    assert_eq!(w.access_requests.get(), 1);
    assert_eq!(
        w.window.get_settings_calendar_permission(),
        AccessState::Denied
    );
    assert!(!w.window.get_settings_calendar_enabled());
    assert!(!stored(&w).calendar_integration_enabled);
    assert!(
        alerts(&w)
            .iter()
            .any(|a| a == "L'accès au calendrier a été refusé."),
        "{:?}",
        alerts(&w)
    );
    assert!(checkboxes(&w).is_empty());

    let open = ElementQuery::from_root(&w.window)
        .match_accessible_label("Ouvrir les Réglages Système")
        .find_first()
        .expect("the recovery action");
    open.invoke_accessible_default_action();
    settle();
    assert_eq!(w.system_settings_opened.get(), 1);

    // The user grants access in System Settings, then turns the integration on.
    *w.authorization.lock().unwrap() = PermState::Granted;
    w.access_answer.set(PermState::Granted);
    element(&w, "CalendarSection::calendar-switch").invoke_accessible_default_action();
    pump(&w);
    assert_eq!(w.access_requests.get(), 2);
    assert!(w.window.get_settings_calendar_enabled());
    assert!(stored(&w).calendar_integration_enabled);
    assert_eq!(
        w.window.get_settings_calendar_permission(),
        AccessState::Granted
    );
    assert!(alerts(&w).is_empty());
    assert_eq!(checkboxes(&w).len(), 4);
    assert_eq!(*w.upcoming.borrow(), ["refresh"]);
}

#[test]
fn every_other_permission_state_keeps_the_integration_off_without_an_alert() {
    for permission in [
        PermState::Unknown,
        PermState::Unsupported,
        PermState::NoDevice,
    ] {
        let w = world(AppSettings::default(), permission, work_calendars());
        w.access_answer.set(permission);
        element(&w, "CalendarSection::calendar-switch").invoke_accessible_default_action();
        pump(&w);
        assert!(!w.window.get_settings_calendar_enabled(), "{permission:?}");
        assert!(!stored(&w).calendar_integration_enabled, "{permission:?}");
        assert!(alerts(&w).is_empty(), "{permission:?}");
        assert!(checkboxes(&w).is_empty(), "{permission:?}");
        assert!(w.upcoming.borrow().is_empty(), "{permission:?}");
    }
}

#[test]
fn a_second_activation_during_the_prompt_asks_only_once() {
    let w = world(AppSettings::default(), PermState::Unknown, work_calendars());
    w.access_answer.set(PermState::Granted);
    *w.authorization.lock().unwrap() = PermState::Granted;
    w.window.invoke_settings_calendar_enabled_changed(true);
    w.window.invoke_settings_calendar_enabled_changed(true);
    pump(&w);
    assert_eq!(w.access_requests.get(), 1);
    assert!(stored(&w).calendar_integration_enabled);
    assert_eq!(checkboxes(&w).len(), 4);
}

#[test]
fn turning_the_integration_off_hides_the_choices_and_clears_the_agenda() {
    let w = world(enabled_with(&[]), PermState::Granted, work_calendars());
    assert_eq!(checkboxes(&w).len(), 4);
    element(&w, "CalendarSection::calendar-switch").invoke_accessible_default_action();
    pump(&w);
    assert!(!w.window.get_settings_calendar_enabled());
    assert!(!stored(&w).calendar_integration_enabled);
    assert!(checkboxes(&w).is_empty());
    assert!(shown(&w).is_empty());
    assert_eq!(*w.upcoming.borrow(), ["clear"]);
}

#[test]
fn a_read_that_answers_after_the_integration_was_turned_off_is_dropped() {
    let w = world(enabled_with(&[]), PermState::Granted, work_calendars());
    reload(&w.window, &w.host);
    element(&w, "CalendarSection::calendar-switch").invoke_accessible_default_action();
    pump(&w);
    assert!(shown(&w).is_empty(), "{:?}", shown(&w));
    assert!(checkboxes(&w).is_empty());
}

#[test]
fn a_read_failing_because_access_was_revoked_shows_the_real_state() {
    let w = world(enabled_with(&[]), PermState::Granted, work_calendars());
    *w.authorization.lock().unwrap() = PermState::Denied;
    *w.listing.lock().unwrap() = Err("Calendar access not granted".into());
    reload(&w.window, &w.host);
    pump(&w);
    assert_eq!(
        w.window.get_settings_calendar_permission(),
        AccessState::Denied
    );
    assert!(checkboxes(&w).is_empty());
    assert!(alerts(&w).iter().any(|a| a.contains("refusé")));

    *w.authorization.lock().unwrap() = PermState::Granted;
    *w.listing.lock().unwrap() = Ok(work_calendars());
    reload(&w.window, &w.host);
    pump(&w);
    assert_eq!(
        w.window.get_settings_calendar_permission(),
        AccessState::Granted
    );
    assert_eq!(checkboxes(&w).len(), 4);
    assert!(alerts(&w).is_empty());
}

#[test]
fn a_failed_worker_keeps_the_last_projection() {
    let w = world(enabled_with(&[]), PermState::Granted, work_calendars());
    let before = shown(&w);
    let request = w.host.catalog.begin_request();
    finish_reload(&w.window, &w.host, request, None);
    assert_eq!(shown(&w), before);
    assert_eq!(
        w.window.get_settings_calendar_permission(),
        AccessState::Granted
    );
}

#[test]
fn an_empty_catalogue_shows_the_settings_without_a_list() {
    let w = world(enabled_with(&[]), PermState::Granted, Vec::new());
    assert!(checkboxes(&w).is_empty());
    assert!(alerts(&w).is_empty());
    assert!(
        ElementQuery::from_root(&w.window)
            .match_accessible_label("Délai du rappel en minutes")
            .find_first()
            .is_some(),
        "the reminder stays available with no calendar"
    );
}

#[test]
fn the_last_projection_stays_while_a_new_read_is_in_flight() {
    let w = world(enabled_with(&[]), PermState::Granted, work_calendars());
    *w.listing.lock().unwrap() = Ok(vec![info("z", "Seul", None)]);
    reload(&w.window, &w.host);
    settle();
    assert_eq!(checkboxes(&w).len(), 4, "still the previous list");
    pump(&w);
    assert_eq!(checkboxes(&w).len(), 1);
    assert_eq!(shown(&w), [("z".to_string(), true)]);
}

#[test]
fn only_the_latest_read_is_published() {
    let w = world(enabled_with(&[]), PermState::Granted, work_calendars());
    let older = w.host.catalog.begin_request();
    let newer = w.host.catalog.begin_request();
    finish_reload(
        &w.window,
        &w.host,
        newer,
        Some(Ok(vec![info("n", "Nouveau", None)])),
    );
    finish_reload(&w.window, &w.host, older, Some(Ok(work_calendars())));
    assert_eq!(shown(&w), [("n".to_string(), true)]);
}

// Selection through the checkboxes

#[test]
fn several_sources_are_grouped_and_homonymous_calendars_write_their_own_identifier() {
    let w = world(enabled_with(&[]), PermState::Granted, work_calendars());
    assert_eq!(
        shown(&w)
            .iter()
            .map(|(id, _)| id.as_str())
            .collect::<Vec<_>>(),
        ["n1", "g1", "a1", "a2"]
    );
    let headers: Vec<_> = ElementQuery::from_root(&w.window)
        .match_inherits("KitText")
        .find_all()
        .iter()
        .filter_map(ElementHandle::accessible_label)
        .filter(|label| ["Google", "iCloud"].contains(&label.as_str()))
        .map(|label| label.to_string())
        .collect();
    assert_eq!(headers, ["Google", "iCloud"]);

    let second = checkbox(&w, "Travail", "iCloud");
    assert_eq!(checkboxes(&w).len(), 4);
    let homonyms: Vec<_> = checkboxes(&w)
        .into_iter()
        .filter(|e| e.accessible_label().as_deref() == Some("Travail"))
        .collect();
    assert_eq!(homonyms.len(), 3);
    // The second homonym of iCloud is the one that is shown last.
    let last_icloud = homonyms
        .into_iter()
        .filter(|e| e.accessible_description().as_deref() == Some("iCloud"))
        .max_by(|a, b| a.absolute_position().y.total_cmp(&b.absolute_position().y))
        .unwrap();
    assert!(last_icloud.absolute_position().y >= second.absolute_position().y);
    last_icloud.invoke_accessible_default_action();
    pump(&w);
    let mut written = stored(&w).calendar_selected_ids;
    written.sort();
    assert_eq!(written, ids(&["a1", "g1", "n1"]));
    let unchecked: Vec<_> = shown(&w).into_iter().filter(|(_, on)| !on).collect();
    assert_eq!(unchecked, [("a2".to_string(), false)]);
}

#[test]
fn the_checkbox_is_controlled_and_follows_the_stored_selection_only() {
    let w = world(enabled_with(&[]), PermState::Granted, catalogue());
    let b = checkbox(&w, "B", "iCloud");
    assert!(checked(&b));
    w.hold.store(true, Ordering::SeqCst);
    b.invoke_accessible_default_action();
    settle();
    assert!(
        checked(&b),
        "nothing changes before Rust persisted the choice"
    );
    assert!(stored(&w).calendar_selected_ids.is_empty());
    w.hold.store(false, Ordering::SeqCst);
    pump(&w);
    assert!(!checked(&checkbox(&w, "B", "iCloud")));
    assert_eq!(stored(&w).calendar_selected_ids, ids(&["a", "c"]));
}

#[test]
fn the_last_checked_calendar_stays_checked() {
    let w = world(enabled_with(&["b"]), PermState::Granted, catalogue());
    let b = checkbox(&w, "B", "iCloud");
    assert!(checked(&b));
    b.invoke_accessible_default_action();
    pump(&w);
    assert!(checked(&checkbox(&w, "B", "iCloud")));
    assert_eq!(stored(&w).calendar_selected_ids, ids(&["b"]));
}

#[test]
fn checking_everything_returns_to_the_empty_convention_even_with_an_obsolete_identifier() {
    let w = world(
        enabled_with(&["a", "ghost"]),
        PermState::Granted,
        catalogue()[..2].to_vec(),
    );
    assert!(checked(&checkbox(&w, "A", "iCloud")));
    assert!(!checked(&checkbox(&w, "B", "iCloud")));
    checkbox(&w, "B", "iCloud").invoke_accessible_default_action();
    pump(&w);
    assert!(stored(&w).calendar_selected_ids.is_empty());
    assert!(checkboxes(&w).iter().all(checked));
}

#[test]
fn a_reload_projects_the_stored_selection_and_follows_a_change_made_elsewhere() {
    let w = world(enabled_with(&["b"]), PermState::Granted, catalogue());
    assert_eq!(
        shown(&w),
        [
            ("c".to_string(), false),
            ("a".to_string(), false),
            ("b".to_string(), true)
        ]
    );
    let mut next = stored(&w);
    next.calendar_selected_ids = ids(&["a", "c"]);
    w.io.submit(
        SettingsSaveLane::General,
        move |settings| settings.calendar_selected_ids = next.calendar_selected_ids.clone(),
        |_, _| {},
    );
    pump(&w);
    reload(&w.window, &w.host);
    pump(&w);
    assert_eq!(
        shown(&w),
        [
            ("c".to_string(), true),
            ("a".to_string(), true),
            ("b".to_string(), false)
        ]
    );
}

#[test]
fn a_calendar_deleted_while_the_choice_waits_to_be_written_changes_nothing() {
    let w = world(enabled_with(&["a", "b"]), PermState::Granted, catalogue());
    w.hold.store(true, Ordering::SeqCst);
    w.io.submit(
        SettingsSaveLane::General,
        |settings| settings.calendar_autostart_enabled = true,
        |_, _| {},
    );
    checkbox(&w, "C", "Google").invoke_accessible_default_action();
    settle();
    // EventKit reports the calendar gone before the queued write runs.
    *w.listing.lock().unwrap() = Ok(catalogue()[..2].to_vec());
    w.host
        .catalog
        .replace(PermState::Granted, catalogue()[..2].to_vec());
    w.hold.store(false, Ordering::SeqCst);
    pump(&w);
    let after = stored(&w);
    assert!(
        after.calendar_autostart_enabled,
        "the earlier write still happened"
    );
    assert_eq!(after.calendar_selected_ids, ids(&["a", "b"]));
    assert_eq!(
        shown(&w),
        [("a".to_string(), true), ("b".to_string(), true)],
        "the page reconciles with what exists"
    );
}

#[test]
fn a_choice_written_after_the_permission_was_revoked_changes_nothing() {
    let w = world(enabled_with(&[]), PermState::Granted, catalogue());
    w.hold.store(true, Ordering::SeqCst);
    w.io.submit(
        SettingsSaveLane::General,
        |settings| settings.calendar_autostart_enabled = true,
        |_, _| {},
    );
    checkbox(&w, "B", "iCloud").invoke_accessible_default_action();
    settle();
    w.host.catalog.replace(PermState::Denied, Vec::new());
    w.hold.store(false, Ordering::SeqCst);
    pump(&w);
    assert!(stored(&w).calendar_selected_ids.is_empty());
}

#[test]
fn two_quick_activations_apply_one_intention_not_a_double_flip() {
    let w = world(enabled_with(&[]), PermState::Granted, catalogue());
    w.hold.store(true, Ordering::SeqCst);
    let b = checkbox(&w, "B", "iCloud");
    b.invoke_accessible_default_action();
    settle();
    b.invoke_accessible_default_action();
    settle();
    w.hold.store(false, Ordering::SeqCst);
    pump(&w);
    assert_eq!(stored(&w).calendar_selected_ids, ids(&["a", "c"]));
    assert!(!checked(&checkbox(&w, "B", "iCloud")));
}

#[test]
fn a_gesture_started_on_a_row_that_a_homonym_replaced_writes_nothing() {
    let w = world(enabled_with(&[]), PermState::Granted, catalogue());
    let saves_before = w.saves.load(Ordering::SeqCst);
    let held = checkbox(&w, "B", "iCloud");
    let position = press_at(&w, &held);
    // A refresh swaps the calendars under the pointer.
    let replaced = vec![
        info("a", "A", Some("iCloud")),
        info("x", "B", Some("iCloud")),
        info("c", "C", Some("Google")),
    ];
    publish_rows(&w.window, rows(&replaced, &[]));
    settle();
    release_at(&w, position);
    pump(&w);
    assert_eq!(w.saves.load(Ordering::SeqCst), saves_before);
    assert!(stored(&w).calendar_selected_ids.is_empty());

    // A new gesture on the replacement is a different intention and applies.
    w.host.catalog.replace(PermState::Granted, replaced);
    click(&w, &checkbox(&w, "B", "iCloud"));
    pump(&w);
    assert_eq!(w.saves.load(Ordering::SeqCst), saves_before + 1);
    assert_eq!(stored(&w).calendar_selected_ids, ids(&["a", "c"]));
}

#[test]
fn a_refresh_with_the_same_calendars_keeps_the_rows_and_a_new_set_replaces_them() {
    let w = world(enabled_with(&[]), PermState::Granted, catalogue());
    let before = checkbox(&w, "B", "iCloud");
    publish_rows(&w.window, rows(&catalogue(), &ids(&["a", "c"])));
    settle();
    assert!(before.is_valid(), "same calendars update in place");
    assert!(!checked(&before));
    publish_rows(&w.window, rows(&catalogue()[..2], &[]));
    settle();
    assert!(!before.is_valid() || checkboxes(&w).len() == 2);
    assert_eq!(checkboxes(&w).len(), 2);
}

// Reminder

fn stepper_editor(world: &World) -> ElementHandle {
    element(world, "CalendarSection::delay-stepper")
        .query_descendants()
        .match_accessible_label("Délai du rappel en minutes")
        .find_first()
        .expect("the reminder editor")
}

fn step(world: &World, label: &str) {
    ElementQuery::from_root(&world.window)
        .match_inherits("KitPressable")
        .find_all()
        .into_iter()
        .find(|e| e.accessible_label().as_deref() == Some(label))
        .unwrap_or_else(|| panic!("no {label} action"))
        .invoke_accessible_default_action();
    pump(world);
}

#[test]
fn the_reminder_bounds_come_from_the_rust_contract() {
    let w = world(enabled_with(&[]), PermState::Granted, catalogue());
    let options = SettingsOptions::current();
    assert_eq!(
        w.window.get_settings_calendar_reminder_minutes_min(),
        options.calendar_reminder_minutes_min as i32
    );
    assert_eq!(
        w.window.get_settings_calendar_reminder_minutes_max(),
        options.calendar_reminder_minutes_max as i32
    );
}

#[test]
fn the_reminder_steps_stop_at_both_bounds_and_persist() {
    let w = world(enabled_with(&[]), PermState::Granted, catalogue());
    let min = w.window.get_settings_calendar_reminder_minutes_min();
    let max = w.window.get_settings_calendar_reminder_minutes_max();
    let start = w.window.get_settings_calendar_reminder_minutes();
    step(&w, "Augmenter");
    assert_eq!(w.window.get_settings_calendar_reminder_minutes(), start + 1);
    assert_eq!(stored(&w).calendar_reminder_minutes as i32, start + 1);
    for _ in 0..(max - start + 2) {
        step(&w, "Augmenter");
    }
    assert_eq!(w.window.get_settings_calendar_reminder_minutes(), max);
    assert_eq!(stored(&w).calendar_reminder_minutes as i32, max);
    for _ in 0..(max - min + 2) {
        step(&w, "Diminuer");
    }
    assert_eq!(w.window.get_settings_calendar_reminder_minutes(), min);
    assert_eq!(stored(&w).calendar_reminder_minutes as i32, min);
}

fn type_reminder(world: &World, text: &str) {
    let input = stepper_editor(world);
    click(world, &input);
    key_down(world, Key::Control);
    tap(world, "a");
    key_up(world, Key::Control);
    if text.is_empty() {
        tap(world, Key::Backspace);
    }
    for ch in text.chars() {
        tap(world, ch.to_string());
    }
    tap(world, Key::Return);
    pump(world);
}

#[test]
fn a_typed_reminder_inside_the_range_is_persisted_and_one_outside_is_never_proposed() {
    let w = world(enabled_with(&[]), PermState::Granted, catalogue());
    let max = w.window.get_settings_calendar_reminder_minutes_max();
    type_reminder(&w, "7");
    assert_eq!(w.window.get_settings_calendar_reminder_minutes(), 7);
    assert_eq!(stored(&w).calendar_reminder_minutes, 7);

    let outside = (max + 40).to_string();
    type_reminder(&w, &outside);
    assert_eq!(
        stored(&w).calendar_reminder_minutes,
        7,
        "an out of range draft is not a value"
    );
    assert_eq!(w.window.get_settings_calendar_reminder_minutes(), 7);
}

#[test]
fn an_emptied_reminder_falls_back_to_the_lower_bound_of_the_contract() {
    let w = world(enabled_with(&[]), PermState::Granted, catalogue());
    let min = w.window.get_settings_calendar_reminder_minutes_min();
    type_reminder(&w, "9");
    assert_eq!(stored(&w).calendar_reminder_minutes, 9);
    type_reminder(&w, "");
    assert_eq!(stored(&w).calendar_reminder_minutes as i32, min);
    assert_eq!(w.window.get_settings_calendar_reminder_minutes(), min);
    assert_eq!(
        stepper_editor(&w)
            .accessible_value()
            .map(|value| value.to_string()),
        Some(min.to_string())
    );
}

#[test]
fn the_reminder_is_offered_only_with_the_integration_on_and_access_granted() {
    let w = world(AppSettings::default(), PermState::Granted, catalogue());
    assert!(
        ElementHandle::find_by_element_id(&w.window, "CalendarSection::delay-stepper")
            .next()
            .is_none()
    );
    let w = world(enabled_with(&[]), PermState::Granted, catalogue());
    assert!(
        ElementHandle::find_by_element_id(&w.window, "CalendarSection::delay-stepper")
            .next()
            .is_some()
    );
}

#[test]
fn the_proposal_switch_persists_through_the_controlled_state() {
    let w = world(enabled_with(&[]), PermState::Granted, catalogue());
    assert!(!w.window.get_settings_calendar_autostart_enabled());
    element(&w, "CalendarSection::reminder-switch").invoke_accessible_default_action();
    pump(&w);
    assert!(w.window.get_settings_calendar_autostart_enabled());
    assert!(stored(&w).calendar_autostart_enabled);
}

#[test]
fn a_reminder_step_without_a_first_snapshot_is_persisted_and_then_projected() {
    let w = world_with(enabled_with(&[]), PermState::Granted, catalogue(), false);
    let start = stored(&w).calendar_reminder_minutes as i32;
    w.window.set_settings_calendar_reminder_minutes(start);
    settle();
    step(&w, "Augmenter");
    assert_eq!(stored(&w).calendar_reminder_minutes as i32, start + 1);
    assert_eq!(w.window.get_settings_calendar_reminder_minutes(), start + 1);
}

#[test]
fn a_save_result_arriving_while_a_new_value_is_typed_does_not_disturb_the_draft() {
    let w = world(enabled_with(&[]), PermState::Granted, catalogue());
    let start = w.window.get_settings_calendar_reminder_minutes();
    w.hold.store(true, Ordering::SeqCst);
    ElementQuery::from_root(&w.window)
        .match_inherits("KitPressable")
        .find_all()
        .into_iter()
        .find(|e| e.accessible_label().as_deref() == Some("Augmenter"))
        .expect("increase")
        .invoke_accessible_default_action();
    settle();
    assert_eq!(w.window.get_settings_calendar_reminder_minutes(), start + 1);
    let input = stepper_editor(&w);
    click(&w, &input);
    key_down(&w, Key::Control);
    tap(&w, "a");
    key_up(&w, Key::Control);
    tap(&w, "1");
    tap(&w, "7");
    w.hold.store(false, Ordering::SeqCst);
    pump(&w);
    assert_eq!(stored(&w).calendar_reminder_minutes as i32, start + 1);
    assert_eq!(
        input.accessible_value().map(|value| value.to_string()),
        Some("17".to_string()),
        "the observed value equals the preview, so the draft stays"
    );
    tap(&w, Key::Return);
    pump(&w);
    assert_eq!(stored(&w).calendar_reminder_minutes, 17);
}
