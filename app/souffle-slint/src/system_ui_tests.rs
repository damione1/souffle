//! The System page driven through the real `MainWindow`, the kit controls and the
//! real settings worker on a throwaway database. The log file, the clipboard, the
//! folder dialog, the sidecar, the model files and the network are replaced by
//! recorders, so no data, preference, MCP server or model of the user is touched.
use crate::settings_io::SettingsIoCoordinator;
use crate::settings_values::{SettingsCache, SettingsValueController};
use crate::system_ui::{
    LogTail, LogUpdate, MAX_LOG_LINES, SystemBackend, SystemHost, merge_tail, split_tail, wire,
};
use crate::{
    ExportStatus, MainWindow, McpStatus, SettingsTab, TranscriptionPhase, UpdateCheckStatus,
};
use i_slint_backend_testing::{AccessibleRole, ElementHandle, ElementQuery};
use slint::platform::{
    Clipboard, Key, Platform, PointerEventButton, WindowAdapter, WindowEvent,
    software_renderer::MinimalSoftwareWindow,
};
use slint::{ComponentHandle, SharedString};
use slint_kit::{DirectoryHandle, FileLocation, HostError, SelectionOutcome};
use souffle_lib::commands::{SettingsSaveError, SettingsSaveOutcome};
use souffle_lib::db::Database;
use souffle_lib::settings::{AppSettings, Theme as SettingsTheme};
use souffle_lib::update_check::UpdateCheckResult;
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll, Waker};
use std::time::{Duration, Instant};

type LocalFuture<T> = Pin<Box<dyn Future<Output = T>>>;

const WIDTH: u32 = 1040;
const HEIGHT: u32 = 3600;
const POLL: Duration = Duration::from_millis(2000);

thread_local! {
    static TEST_WINDOW: RefCell<Option<Rc<MinimalSoftwareWindow>>> = const { RefCell::new(None) };
    static CLOCK: Cell<Duration> = const { Cell::new(Duration::ZERO) };
    static CLIPBOARD: RefCell<Option<String>> = const { RefCell::new(None) };
}

struct TestPlatform;

impl Platform for TestPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        let window = MinimalSoftwareWindow::new(Default::default());
        TEST_WINDOW.with(|w| *w.borrow_mut() = Some(window.clone()));
        Ok(window)
    }

    fn duration_since_start(&self) -> Duration {
        CLOCK.with(Cell::get)
    }

    fn set_clipboard_text(&self, text: &str, clipboard: Clipboard) {
        if matches!(clipboard, Clipboard::DefaultClipboard) {
            CLIPBOARD.with(|c| *c.borrow_mut() = Some(text.to_string()));
        }
    }

    fn clipboard_text(&self, clipboard: Clipboard) -> Option<String> {
        matches!(clipboard, Clipboard::DefaultClipboard)
            .then(|| CLIPBOARD.with(|c| c.borrow().clone()))
            .flatten()
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

fn advance(by: Duration) {
    CLOCK.with(|c| c.set(c.get() + by));
    settle();
}

fn clipboard() -> Option<String> {
    CLIPBOARD.with(|c| c.borrow().clone())
}

fn directory(path: &str) -> DirectoryHandle {
    DirectoryHandle {
        location: FileLocation::Path(PathBuf::from(path)),
        display_name: path.rsplit('/').next().unwrap_or(path).to_string(),
    }
}

fn update(available: bool, latest: Option<&str>, error: Option<&str>) -> UpdateCheckResult {
    UpdateCheckResult {
        current_version: "1.0.0".into(),
        latest_version: latest.map(str::to_string),
        update_available: available,
        release_notes: None,
        release_url: None,
        check_error: error.map(str::to_string),
    }
}

/// A future that stays pending while its gate is closed.
fn gated<T: 'static>(gate: &Rc<Cell<bool>>, value: impl FnOnce() -> T + 'static) -> LocalFuture<T> {
    let gate = gate.clone();
    Box::pin(async move {
        std::future::poll_fn(|_| {
            if gate.get() {
                Poll::Pending
            } else {
                Poll::Ready(())
            }
        })
        .await;
        value()
    })
}

struct Lab {
    window: MainWindow,
    host: Rc<SystemHost>,
    io: Rc<SettingsIoCoordinator>,
    database: PathBuf,
    tasks: Rc<RefCell<Vec<LocalFuture<()>>>>,
    tail: Rc<RefCell<Result<String, String>>>,
    tail_fetches: Rc<Cell<u32>>,
    diagnostics: Rc<RefCell<Result<String, String>>>,
    diagnostics_gate: Rc<Cell<bool>>,
    copied: Rc<RefCell<Vec<String>>>,
    picker_answers: Rc<RefCell<VecDeque<SelectionOutcome<DirectoryHandle>>>>,
    picker_calls: Rc<Cell<u32>>,
    picker_gate: Rc<Cell<bool>>,
    exports: Rc<RefCell<Vec<String>>>,
    export_result: Rc<RefCell<Result<(), String>>>,
    export_gate: Rc<Cell<bool>>,
    mcp_calls: Rc<Cell<u32>>,
    mcp_result: Rc<RefCell<Result<String, String>>>,
    mcp_gate: Rc<Cell<bool>>,
    update_calls: Rc<Cell<u32>>,
    update_result: Rc<RefCell<Result<UpdateCheckResult, String>>>,
    update_gate: Rc<Cell<bool>>,
    delete_calls: Rc<Cell<u32>>,
    delete_result: Rc<RefCell<Result<(), String>>>,
    deleted: Rc<Cell<u32>>,
    reveals: Rc<Cell<u32>>,
    autostart_observed: Arc<AtomicBool>,
    autostart_refusal: Arc<AtomicBool>,
    _directory: tempfile::TempDir,
}

use std::sync::Arc;

fn lab() -> Lab {
    lab_with(AppSettings::default())
}

fn lab_with(initial: AppSettings) -> Lab {
    let _ = slint::platform::set_platform(Box::new(TestPlatform));
    CLOCK.with(|c| c.set(Duration::ZERO));
    CLIPBOARD.with(|c| *c.borrow_mut() = None);
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
    crate::data_ui::populate(&window, &initial);
    crate::data_ui::populate_mcp(&window, &mcp_info(true));
    let cache = SettingsCache::with_observed(&window, initial.clone());

    let autostart_observed = Arc::new(AtomicBool::new(false));
    let autostart_refusal = Arc::new(AtomicBool::new(false));
    let load_path = database.clone();
    let general_path = database.clone();
    let autostart_path = database.clone();
    let observed = autostart_observed.clone();
    let refusal = autostart_refusal.clone();
    let io = SettingsIoCoordinator::with_functions(
        cache.clone(),
        move || AppSettings::load(&Database::open(&load_path)?),
        || Ok(AppSettings::default()),
        move |settings: AppSettings| {
            let database = Database::open(&general_path).unwrap();
            let result = settings
                .save(&database)
                .map_err(|message| SettingsSaveError::NotCommitted { message });
            SettingsSaveOutcome::from_results(result, AppSettings::load(&database))
        },
        move |settings: AppSettings| {
            // What macOS reports for the login item, whatever was asked for.
            let database = Database::open(&autostart_path).unwrap();
            settings.save(&database).unwrap();
            let mut seen = AppSettings::load(&database).unwrap();
            seen.autostart_enabled = observed.load(Ordering::SeqCst);
            let result = if refusal.load(Ordering::SeqCst) {
                Err(SettingsSaveError::EffectFailedAfterCommit {
                    cause: souffle_lib::commands::SettingsEffectFailure::Autostart {
                        message: "login item refused".into(),
                    },
                })
            } else {
                Ok(())
            };
            SettingsSaveOutcome::Observed {
                settings: Box::new(seen),
                result,
            }
        },
    );
    let token = io.begin_open();
    assert!(io.seed_if_current(token, io.current_revision(), initial.clone()));
    let values = SettingsValueController::new(
        &window,
        cache,
        io.clone(),
        Rc::new(RefCell::new(Vec::new())),
        Rc::new(RefCell::new(None)),
    );
    values.observe_loaded(&initial);
    crate::settings_values::wire(&window, values);

    let tasks: Rc<RefCell<Vec<LocalFuture<()>>>> = Rc::default();
    let tail = Rc::new(RefCell::new(Ok(String::new())));
    let tail_fetches = Rc::new(Cell::new(0));
    let diagnostics = Rc::new(RefCell::new(Ok("diagnostics complets".to_string())));
    let diagnostics_gate = Rc::new(Cell::new(false));
    let copied: Rc<RefCell<Vec<String>>> = Rc::default();
    let picker_answers: Rc<RefCell<VecDeque<SelectionOutcome<DirectoryHandle>>>> = Rc::default();
    let picker_calls = Rc::new(Cell::new(0));
    let picker_gate = Rc::new(Cell::new(false));
    let exports: Rc<RefCell<Vec<String>>> = Rc::default();
    let export_result = Rc::new(RefCell::new(Ok(())));
    let export_gate = Rc::new(Cell::new(false));
    let mcp_calls = Rc::new(Cell::new(0));
    let mcp_result = Rc::new(RefCell::new(Ok("search_meetings, get_meeting".to_string())));
    let mcp_gate = Rc::new(Cell::new(false));
    let update_calls = Rc::new(Cell::new(0));
    let update_result = Rc::new(RefCell::new(Ok(update(false, None, None))));
    let update_gate = Rc::new(Cell::new(false));
    let delete_calls = Rc::new(Cell::new(0));
    let delete_result = Rc::new(RefCell::new(Ok(())));
    let deleted = Rc::new(Cell::new(0));
    let reveals = Rc::new(Cell::new(0));

    let backend = SystemBackend {
        fetch_log_tail: {
            let (tail, fetches) = (tail.clone(), tail_fetches.clone());
            Rc::new(move || {
                fetches.set(fetches.get() + 1);
                let answer = tail.borrow().clone();
                Box::pin(async move { answer })
            })
        },
        diagnostics_text: {
            let (text, gate) = (diagnostics.clone(), diagnostics_gate.clone());
            Rc::new(move || {
                let text = text.clone();
                gated(&gate, move || text.borrow().clone())
            })
        },
        copy_text: {
            let copied = copied.clone();
            Rc::new(move |text| copied.borrow_mut().push(text.to_string()))
        },
        choose_export_folder: {
            let (answers, calls, gate) = (
                picker_answers.clone(),
                picker_calls.clone(),
                picker_gate.clone(),
            );
            Rc::new(move |_| {
                calls.set(calls.get() + 1);
                let answers = answers.clone();
                gated(&gate, move || {
                    answers
                        .borrow_mut()
                        .pop_front()
                        .unwrap_or(SelectionOutcome::Cancelled)
                })
            })
        },
        export_archive: {
            let (exports, result, gate) =
                (exports.clone(), export_result.clone(), export_gate.clone());
            Rc::new(move |destination| {
                exports.borrow_mut().push(destination);
                let result = result.clone();
                gated(&gate, move || result.borrow().clone())
            })
        },
        reveal_data_dir: {
            let reveals = reveals.clone();
            Rc::new(move || {
                reveals.set(reveals.get() + 1);
                Ok(())
            })
        },
        test_mcp: {
            let (calls, result, gate) = (mcp_calls.clone(), mcp_result.clone(), mcp_gate.clone());
            Rc::new(move || {
                calls.set(calls.get() + 1);
                let result = result.clone();
                gated(&gate, move || result.borrow().clone())
            })
        },
        check_updates: {
            let (calls, result, gate) = (
                update_calls.clone(),
                update_result.clone(),
                update_gate.clone(),
            );
            Rc::new(move || {
                calls.set(calls.get() + 1);
                let result = result.clone();
                gated(&gate, move || result.borrow().clone())
            })
        },
        delete_model: {
            let (calls, result) = (delete_calls.clone(), delete_result.clone());
            Rc::new(move || {
                calls.set(calls.get() + 1);
                result.borrow().clone()
            })
        },
        model_deleted: {
            let deleted = deleted.clone();
            Rc::new(move || deleted.set(deleted.get() + 1))
        },
        spawn: {
            let tasks = tasks.clone();
            Rc::new(move |future| tasks.borrow_mut().push(future))
        },
        poll_interval: POLL,
    };
    let host = SystemHost::new(&window, backend);
    wire(&window, &host);

    window.set_settings_open(true);
    window.set_settings_tab(SettingsTab::System);
    window.show().unwrap();
    let lab = Lab {
        window,
        host,
        io,
        database,
        tasks,
        tail,
        tail_fetches,
        diagnostics,
        diagnostics_gate,
        copied,
        picker_answers,
        picker_calls,
        picker_gate,
        exports,
        export_result,
        export_gate,
        mcp_calls,
        mcp_result,
        mcp_gate,
        update_calls,
        update_result,
        update_gate,
        delete_calls,
        delete_result,
        deleted,
        reveals,
        autostart_observed,
        autostart_refusal,
        _directory: directory,
    };
    pump(&lab);
    lab
}

/// Runs the page's queued work and the settings worker until nothing is left
/// that can finish. A future held by a closed gate stays queued.
fn pump(lab: &Lab) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        lab.io.drain_for_test();
        let pending: Vec<_> = lab.tasks.borrow_mut().drain(..).collect();
        let mut progressed = false;
        let mut waiting = Vec::new();
        for mut task in pending {
            let mut context = Context::from_waker(Waker::noop());
            if task.as_mut().poll(&mut context).is_pending() {
                waiting.push(task);
            } else {
                progressed = true;
            }
        }
        lab.tasks.borrow_mut().extend(waiting);
        let known = lab.tasks.borrow().len();
        settle();
        let spawned = lab.tasks.borrow().len() > known;
        if !progressed && !spawned && lab.io.is_idle_for_test() {
            return;
        }
        assert!(Instant::now() < deadline, "the page never settled");
    }
}

fn find(lab: &Lab, role: AccessibleRole, label: &str) -> ElementHandle {
    ElementQuery::from_root(&lab.window)
        .match_accessible_role(role)
        .match_accessible_label(label)
        .find_first()
        .unwrap_or_else(|| panic!("no {role:?} {label:?}"))
}

fn button(lab: &Lab, label: &str) -> ElementHandle {
    find(lab, AccessibleRole::Button, label)
}

fn copy_button(lab: &Lab, of: &str) -> ElementHandle {
    ElementQuery::from_root(&lab.window)
        .match_accessible_role(AccessibleRole::Button)
        .match_accessible_label("Copier")
        .find_all()
        .into_iter()
        .find(|e| e.accessible_description().as_deref() == Some(of))
        .unwrap_or_else(|| panic!("no Copier button of {of:?}"))
}

fn alerts(lab: &Lab) -> Vec<String> {
    ElementQuery::from_root(&lab.window)
        .match_inherits("KitAlert")
        .find_all()
        .iter()
        .filter_map(ElementHandle::accessible_label)
        .map(|label| label.to_string())
        .collect()
}

fn center(element: &ElementHandle) -> slint::LogicalPosition {
    slint::LogicalPosition::new(
        element.absolute_position().x + element.size().width / 2.0,
        element.absolute_position().y + element.size().height / 2.0,
    )
}

fn click(lab: &Lab, element: &ElementHandle) {
    let position = center(element);
    let window = lab.window.window();
    window.dispatch_event(WindowEvent::PointerMoved { position });
    window.dispatch_event(WindowEvent::PointerPressed {
        position,
        button: PointerEventButton::Left,
    });
    window.dispatch_event(WindowEvent::PointerReleased {
        position,
        button: PointerEventButton::Left,
    });
    pump(lab);
}

fn key(lab: &Lab, text: impl Into<SharedString>) {
    let text = text.into();
    lab.window
        .window()
        .dispatch_event(WindowEvent::KeyPressed { text: text.clone() });
    lab.window
        .window()
        .dispatch_event(WindowEvent::KeyReleased { text });
    pump(lab);
}

fn chord(lab: &Lab, modifier: Key, letter: &str) {
    let window = lab.window.window();
    window.dispatch_event(WindowEvent::KeyPressed {
        text: modifier.into(),
    });
    key(lab, letter);
    window.dispatch_event(WindowEvent::KeyReleased {
        text: modifier.into(),
    });
    settle();
}

/// Scrolls the System page so that a control is inside its viewport.
fn reveal(lab: &Lab, element: &ElementHandle) {
    let page = ElementHandle::find_by_element_id(&lab.window, "SettingsView::system-page")
        .next()
        .expect("the System page");
    for _ in 0..200 {
        let (p, ps) = (page.absolute_position(), page.size());
        let (e, es) = (element.absolute_position(), element.size());
        if e.y >= p.y && e.y + es.height <= p.y + ps.height {
            return;
        }
        let toward = if e.y < p.y { 120.0 } else { -120.0 };
        page.scroll(0.0, toward);
        settle();
    }
    panic!("the control never came into view");
}

fn tail_of(lines: impl IntoIterator<Item = String>) -> String {
    lines.into_iter().collect::<Vec<_>>().join("\n")
}

fn numbered(range: std::ops::Range<usize>) -> Vec<String> {
    range
        .map(|n| format!("2026-11-19T10:00:{:02}Z INFO souffle: ligne {n:04}", n % 60))
        .collect()
}

fn log_lines(lab: &Lab) -> Vec<ElementHandle> {
    ElementQuery::from_root(&lab.window)
        .match_type_name("LogLine")
        .find_all()
}

// Pure log rules -----------------------------------------------------------

fn lines(values: &[&str]) -> Vec<SharedString> {
    values.iter().map(|v| SharedString::from(*v)).collect()
}

#[test]
fn merging_a_tail_appends_only_what_is_new() {
    let shown = lines(&["a", "b", "c"]);
    assert_eq!(
        merge_tail(&shown, &lines(&["a", "b", "c"])),
        LogUpdate::Unchanged
    );
    assert_eq!(
        merge_tail(&shown, &lines(&["b", "c", "d", "e"])),
        LogUpdate::Append(2)
    );
    assert_eq!(
        merge_tail(&shown, &lines(&["c", "d"])),
        LogUpdate::Append(1)
    );
    assert_eq!(
        merge_tail(&shown, &lines(&["b", "c"])),
        LogUpdate::Unchanged
    );
    assert_eq!(merge_tail(&shown, &lines(&["x", "y"])), LogUpdate::Replace);
    assert_eq!(merge_tail(&shown, &[]), LogUpdate::Replace);
    assert_eq!(merge_tail(&[], &[]), LogUpdate::Unchanged);
    assert_eq!(merge_tail(&[], &lines(&["a"])), LogUpdate::Replace);
    assert_eq!(
        merge_tail(&lines(&["x", "x", "x"]), &lines(&["x", "x", "y"])),
        LogUpdate::Append(2)
    );
}

#[test]
fn the_kept_tail_is_bounded_and_blank_and_accented_lines_survive() {
    let _ = slint::platform::set_platform(Box::new(TestPlatform));
    let tail = LogTail::new();
    tail.apply("é à ü\n\nfin");
    assert_eq!(tail.len(), 3);
    let many = tail_of(numbered(0..MAX_LOG_LINES + 300));
    tail.apply(&many);
    assert_eq!(tail.len(), MAX_LOG_LINES + 300);
    // A refresh that follows adds lines at the end and drops the oldest ones.
    let next = tail_of(numbered(10..MAX_LOG_LINES + 400));
    tail.apply(&next);
    assert_eq!(tail.len(), MAX_LOG_LINES);
    assert_eq!(
        split_tail("a\nb\n").len(),
        2,
        "a trailing newline is not a line"
    );
}

// Autostart ---------------------------------------------------------------

fn autostart_switch(lab: &Lab) -> ElementHandle {
    find(
        lab,
        AccessibleRole::Switch,
        "Lancer Soufflé à l'ouverture de session",
    )
}

#[test]
fn autostart_shows_the_state_macos_reports_not_the_click() {
    let lab = lab_with(AppSettings {
        autostart_enabled: false,
        ..AppSettings::default()
    });
    let switch = autostart_switch(&lab);
    assert_eq!(switch.accessible_checked(), Some(false));

    // macOS accepts: the switch follows the acknowledged state.
    lab.autostart_observed.store(true, Ordering::SeqCst);
    click(&lab, &switch);
    assert!(lab.window.get_settings_autostart_enabled());
    assert_eq!(switch.accessible_checked(), Some(true));
    assert!(lab.window.get_settings_save_error().is_empty());

    // macOS accepts the write but reports the login item off: the switch returns.
    lab.autostart_observed.store(false, Ordering::SeqCst);
    click(&lab, &switch);
    assert!(!lab.window.get_settings_autostart_enabled());
    assert_eq!(switch.accessible_checked(), Some(false));

    // macOS refuses: the switch stays where it was and the failure is shown.
    lab.autostart_refusal.store(true, Ordering::SeqCst);
    click(&lab, &switch);
    assert_eq!(switch.accessible_checked(), Some(false));
    assert!(
        lab.window
            .get_settings_save_error()
            .contains("login item refused")
    );
}

#[test]
fn the_four_system_switches_persist_through_the_settings_worker() {
    let lab = lab();
    lab.autostart_observed.store(true, Ordering::SeqCst);
    lab.window.invoke_settings_autostart_changed(true);
    lab.window.invoke_settings_debug_transcription_changed(true);
    lab.window
        .invoke_settings_log_level_changed(crate::LogLevel::Debug);
    lab.window.invoke_settings_auto_update_check_changed(false);
    pump(&lab);
    let stored = AppSettings::load(&Database::open(&lab.database).unwrap()).unwrap();
    assert!(stored.autostart_enabled);
    assert!(stored.debug_transcription);
    assert_eq!(stored.log_level, souffle_lib::logging::LogLevel::Debug);
    assert!(!stored.auto_update_check_enabled);
    assert!(lab.window.get_settings_debug_transcription());
    assert_eq!(lab.window.get_settings_log_level(), crate::LogLevel::Debug);
    assert!(!lab.window.get_settings_auto_update_check());
}

// Permissions -------------------------------------------------------------

#[test]
fn the_permissions_dialog_opens_and_returns_the_focus_to_its_trigger() {
    let lab = lab();
    let trigger = button(&lab, "Vérifier les permissions");
    reveal(&lab, &trigger);

    // Keyboard path: Tab to the trigger is covered by the fixture; Space activates it.
    click(&lab, &trigger);
    assert!(lab.window.get_settings_permissions_dialog_open());
    let dialog = find(&lab, AccessibleRole::Groupbox, "Permissions macOS");
    assert!(dialog.size().width > 0.0);

    // Escape closes it and the focus is back on the trigger: Space re-opens it.
    key(&lab, Key::Escape);
    assert!(!lab.window.get_settings_permissions_dialog_open());
    key(&lab, " ");
    assert!(
        lab.window.get_settings_permissions_dialog_open(),
        "the focus returned to the trigger that opened the dialog"
    );

    // The dialog's own action closes it too, and the trigger gets the focus again.
    click(&lab, &button(&lab, "Terminé"));
    assert!(!lab.window.get_settings_permissions_dialog_open());
    key(&lab, " ");
    assert!(lab.window.get_settings_permissions_dialog_open());
    key(&lab, Key::Escape);
}

// Diagnostics: the live log -----------------------------------------------

#[test]
fn the_log_is_read_only_while_the_system_page_is_the_visible_one() {
    let lab = lab();
    lab.window.set_settings_tab(SettingsTab::Ai);
    pump(&lab);
    let baseline = lab.tail_fetches.get();
    advance(POLL + Duration::from_millis(1));
    pump(&lab);
    advance(POLL + Duration::from_millis(1));
    pump(&lab);
    assert_eq!(lab.tail_fetches.get(), baseline, "no read off the page");

    lab.window.set_settings_tab(SettingsTab::System);
    pump(&lab);
    let entered = lab.tail_fetches.get();
    assert!(entered > baseline, "entering the page reads once at once");
    advance(POLL + Duration::from_millis(1));
    pump(&lab);
    assert_eq!(lab.tail_fetches.get(), entered + 1);

    lab.window.set_settings_tab(SettingsTab::Meetings);
    pump(&lab);
    let left = lab.tail_fetches.get();
    advance(POLL + Duration::from_millis(1));
    pump(&lab);
    advance(POLL + Duration::from_millis(1));
    pump(&lab);
    assert_eq!(
        lab.tail_fetches.get(),
        left,
        "leaving the page suspends the poll"
    );

    lab.window.set_settings_tab(SettingsTab::System);
    pump(&lab);
    assert!(lab.tail_fetches.get() > left, "coming back resumes it");

    lab.window.set_settings_open(false);
    pump(&lab);
    let closed = lab.tail_fetches.get();
    advance(POLL + Duration::from_millis(1));
    pump(&lab);
    assert_eq!(
        lab.tail_fetches.get(),
        closed,
        "a closed sheet does not poll"
    );

    lab.window.set_settings_open(true);
    pump(&lab);
    assert!(lab.tail_fetches.get() > closed);
    lab.window.set_window_visible(false);
    pump(&lab);
    let hidden = lab.tail_fetches.get();
    advance(POLL + Duration::from_millis(1));
    pump(&lab);
    assert_eq!(
        lab.tail_fetches.get(),
        hidden,
        "a hidden window does not poll"
    );
}

#[test]
fn an_answer_that_arrives_after_the_page_was_left_is_dropped() {
    let lab = lab();
    *lab.tail.borrow_mut() = Ok("première ligne".into());
    lab.window.set_settings_tab(SettingsTab::Ai);
    pump(&lab);
    lab.window.set_settings_tab(SettingsTab::System);
    // The read started by entering the page is still queued: leave before it lands.
    lab.window.set_settings_tab(SettingsTab::Ai);
    pump(&lab);
    assert!(lab.host.log_len() <= 1);
    let after_leaving = lab.host.log_len();
    *lab.tail.borrow_mut() = Ok("première ligne\nseconde".into());
    advance(POLL + Duration::from_millis(1));
    pump(&lab);
    assert_eq!(lab.host.log_len(), after_leaving);
}

#[test]
fn a_long_log_builds_only_the_rows_near_the_viewport() {
    let lab = lab();
    let mut all = numbered(0..400);
    all[7] = format!("ligne très longue {}", "x".repeat(3000));
    *lab.tail.borrow_mut() = Ok(tail_of(all));
    lab.window.set_settings_tab(SettingsTab::Ai);
    pump(&lab);
    lab.window.set_settings_tab(SettingsTab::System);
    pump(&lab);
    assert_eq!(lab.host.log_len(), 400);
    let mounted = log_lines(&lab).len();
    assert!(
        (1..40).contains(&mounted),
        "{mounted} editors for 400 lines"
    );
    let list = ElementQuery::from_root(&lab.window)
        .match_inherits("ListView")
        .find_first()
        .expect("the log list");
    assert!(list.size().height > 0.0);
}

fn first_visible_line(lab: &Lab) -> (String, f32) {
    let list = ElementQuery::from_root(&lab.window)
        .match_inherits("ListView")
        .find_first()
        .unwrap();
    let top = list.absolute_position().y;
    let mut visible: Vec<_> = log_lines(lab)
        .into_iter()
        .filter(|e| e.absolute_position().y + e.size().height > top)
        .collect();
    visible.sort_by(|a, b| a.absolute_position().y.total_cmp(&b.absolute_position().y));
    let first = visible.first().expect("a line in view");
    (
        first.accessible_value().unwrap_or_default().to_string(),
        first.absolute_position().y,
    )
}

#[test]
fn a_refresh_that_appends_keeps_the_scroll_position_and_the_selection() {
    let lab = lab();
    *lab.tail.borrow_mut() = Ok(tail_of(numbered(0..120)));
    lab.window.set_settings_tab(SettingsTab::Ai);
    pump(&lab);
    lab.window.set_settings_tab(SettingsTab::System);
    pump(&lab);
    let list = ElementQuery::from_root(&lab.window)
        .match_inherits("ListView")
        .find_first()
        .unwrap();
    reveal(&lab, &list);
    for _ in 0..12 {
        list.scroll(0.0, -90.0);
        settle();
    }
    let (before_text, before_y) = first_visible_line(&lab);
    assert!(!before_text.contains("ligne 0000"), "the list did scroll");

    // Select a part of a visible line.
    let line = log_lines(&lab)
        .into_iter()
        .find(|e| e.accessible_value().as_deref() == Some(before_text.as_str()))
        .unwrap();
    click(&lab, &line);
    line.set_accessible_selection_offsets(5, 15);
    settle();

    // The tail moves on: fifteen new lines at the end, as the file grows.
    *lab.tail.borrow_mut() = Ok(tail_of(numbered(40..135)));
    advance(POLL + Duration::from_millis(1));
    pump(&lab);
    assert_eq!(lab.host.log_len(), 135);
    let (after_text, after_y) = first_visible_line(&lab);
    assert_eq!(after_text, before_text, "the reader's line did not move");
    assert!(
        (after_y - before_y).abs() < 1.0,
        "{before_y} then {after_y}"
    );

    // The selection is still the same range of the same line.
    let same = log_lines(&lab)
        .into_iter()
        .find(|e| e.accessible_value().as_deref() == Some(before_text.as_str()))
        .unwrap();
    assert!(same.is_valid());
    copy_selection(&lab);
    assert_eq!(clipboard().as_deref(), Some(&before_text[5..15]));
}

/// Copies the focused editor's selection with the platform shortcut.
fn copy_selection(lab: &Lab) {
    CLIPBOARD.with(|c| *c.borrow_mut() = None);
    chord(lab, Key::Meta, "c");
    if clipboard().is_some() {
        println!("native copy: Meta+C");
        return;
    }
    chord(lab, Key::Control, "c");
    println!("native copy: Control+C, Meta+C copied nothing");
}

#[test]
fn a_partial_selection_copies_natively_and_the_whole_copy_is_the_hosts() {
    let lab = lab();
    *lab.diagnostics.borrow_mut() = Ok(format!(
        "{}\nfin des diagnostics",
        tail_of(numbered(0..300))
    ));
    *lab.tail.borrow_mut() = Ok(tail_of(numbered(220..300)));
    lab.window.set_settings_tab(SettingsTab::Ai);
    pump(&lab);
    lab.window.set_settings_tab(SettingsTab::System);
    pump(&lab);
    let list = ElementQuery::from_root(&lab.window)
        .match_inherits("ListView")
        .find_first()
        .unwrap();
    reveal(&lab, &list);

    let line = log_lines(&lab).into_iter().next().expect("a mounted line");
    let text = line.accessible_value().unwrap().to_string();
    click(&lab, &line);
    line.set_accessible_selection_offsets(11, 21);
    settle();
    copy_selection(&lab);
    assert_eq!(clipboard().as_deref(), Some(&text[11..21]));
    assert!(lab.copied.borrow().is_empty(), "the host was not involved");

    // The whole copy is the host's text, however little of it is mounted.
    let copy = button(&lab, "Copier les diagnostics");
    reveal(&lab, &copy);
    lab.diagnostics_gate.set(true);
    click(&lab, &copy);
    assert_eq!(
        copy.accessible_enabled(),
        Some(false),
        "busy while it builds"
    );
    lab.diagnostics_gate.set(false);
    pump(&lab);
    let copied = lab.copied.borrow();
    assert_eq!(copied.len(), 1);
    assert!(
        copied[0].contains("ligne 0000"),
        "lines that are not mounted"
    );
    assert!(copied[0].ends_with("fin des diagnostics"));
    assert!(log_lines(&lab).len() < 40);
}

#[test]
fn copying_the_diagnostics_twice_fast_copies_once_and_a_failure_does_not() {
    let lab = lab();
    lab.diagnostics_gate.set(true);
    let copy = button(&lab, "Copier les diagnostics");
    reveal(&lab, &copy);
    lab.window.invoke_settings_copy_diagnostics_requested();
    lab.window.invoke_settings_copy_diagnostics_requested();
    lab.diagnostics_gate.set(false);
    pump(&lab);
    assert_eq!(lab.copied.borrow().len(), 1);
    assert!(!lab.window.get_settings_copying_diagnostics());

    *lab.diagnostics.borrow_mut() = Err("pas de bundle".into());
    lab.window.invoke_settings_copy_diagnostics_requested();
    pump(&lab);
    assert_eq!(lab.copied.borrow().len(), 1, "nothing copied on failure");
    assert!(!lab.window.get_settings_copying_diagnostics());
}

#[test]
fn a_failed_read_keeps_the_lines_already_shown_and_the_next_one_recovers() {
    let lab = lab();
    *lab.tail.borrow_mut() = Ok(tail_of(numbered(0..20)));
    lab.window.set_settings_tab(SettingsTab::Ai);
    pump(&lab);
    lab.window.set_settings_tab(SettingsTab::System);
    pump(&lab);
    assert_eq!(lab.host.log_len(), 20);
    *lab.tail.borrow_mut() = Err("fichier illisible".into());
    advance(POLL + Duration::from_millis(1));
    pump(&lab);
    assert_eq!(lab.host.log_len(), 20);
    *lab.tail.borrow_mut() = Ok(tail_of(numbered(0..25)));
    advance(POLL + Duration::from_millis(1));
    pump(&lab);
    assert_eq!(lab.host.log_len(), 25);
    // The file was replaced by a new one: nothing overlaps.
    *lab.tail.borrow_mut() = Ok("nouveau fichier".into());
    advance(POLL + Duration::from_millis(1));
    pump(&lab);
    assert_eq!(lab.host.log_len(), 1);
}

#[test]
fn an_empty_log_shows_its_empty_state_and_a_line_shows_no_placeholder() {
    let lab = lab();
    assert!(
        ElementQuery::from_root(&lab.window)
            .match_accessible_label("Aucune entrée de journal pour l'instant.")
            .find_first()
            .is_some()
    );
    *lab.tail.borrow_mut() = Ok("une ligne".into());
    lab.window.set_settings_tab(SettingsTab::Ai);
    pump(&lab);
    lab.window.set_settings_tab(SettingsTab::System);
    pump(&lab);
    assert!(
        ElementQuery::from_root(&lab.window)
            .match_accessible_label("Aucune entrée de journal pour l'instant.")
            .find_first()
            .is_none()
    );
}

// Data: retention and export ------------------------------------------------

#[test]
fn the_retention_choice_is_persisted_with_the_exact_value() {
    let lab = lab_with(AppSettings {
        meeting_audio_retention: souffle_lib::settings::MeetingAudioRetention::Off,
        ..AppSettings::default()
    });
    lab.window
        .invoke_settings_meeting_audio_retention_changed(crate::AudioRetention::Keep30d);
    pump(&lab);
    let stored = AppSettings::load(&Database::open(&lab.database).unwrap()).unwrap();
    assert_eq!(
        stored.meeting_audio_retention,
        souffle_lib::settings::MeetingAudioRetention::Keep30d
    );
    assert_eq!(
        lab.window.get_settings_meeting_audio_retention(),
        crate::AudioRetention::Keep30d
    );
}

#[test]
fn the_retention_picker_chooses_through_its_popup() {
    let lab = lab_with(AppSettings {
        meeting_audio_retention: souffle_lib::settings::MeetingAudioRetention::Off,
        ..AppSettings::default()
    });
    let trigger = ElementQuery::from_root(&lab.window)
        .match_inherits("KitSelectTrigger")
        .find_all()
        .into_iter()
        .find(|e| e.accessible_label().as_deref() == Some("Conservation audio des réunions"))
        .expect("the retention picker");
    reveal(&lab, &trigger);
    click(&lab, &trigger);
    assert_eq!(trigger.accessible_expanded(), Some(true));
    let option = ElementQuery::from_root(&lab.window)
        .match_inherits("KitOption")
        .match_accessible_label("Conserver 30 jours")
        .find_first()
        .expect("the 30 days option");
    // The popup is placed by the window: the element knows where to press.
    option.mock_single_click(PointerEventButton::Left);
    pump(&lab);
    assert_eq!(
        lab.window.get_settings_meeting_audio_retention(),
        crate::AudioRetention::Keep30d
    );
    let stored = AppSettings::load(&Database::open(&lab.database).unwrap()).unwrap();
    assert_eq!(
        stored.meeting_audio_retention,
        souffle_lib::settings::MeetingAudioRetention::Keep30d
    );
    assert_ne!(
        trigger.accessible_expanded(),
        Some(true),
        "the popup closed"
    );
}

fn export_button(lab: &Lab) -> ElementHandle {
    ElementQuery::from_root(&lab.window)
        .match_accessible_role(AccessibleRole::Button)
        .find_all()
        .into_iter()
        .find(|e| {
            matches!(
                e.accessible_label().as_deref(),
                Some("Exporter…" | "Export en cours…")
            )
        })
        .expect("the export button")
}

#[test]
fn a_cancelled_export_says_nothing_and_starts_nothing() {
    let lab = lab();
    let export = export_button(&lab);
    reveal(&lab, &export);
    lab.picker_answers
        .borrow_mut()
        .push_back(SelectionOutcome::Cancelled);
    lab.picker_gate.set(true);
    click(&lab, &export);
    assert_eq!(
        lab.window.get_settings_data_export_status(),
        ExportStatus::Choosing
    );
    assert_eq!(
        export.accessible_label().as_deref(),
        Some("Export en cours…")
    );
    assert_eq!(export.accessible_enabled(), Some(false));
    lab.picker_gate.set(false);
    pump(&lab);
    assert_eq!(
        lab.window.get_settings_data_export_status(),
        ExportStatus::Idle
    );
    assert!(lab.exports.borrow().is_empty());
    assert!(alerts(&lab).is_empty());
    assert_eq!(export.accessible_enabled(), Some(true));
}

#[test]
fn a_successful_export_reports_its_destination() {
    let lab = lab();
    let export = export_button(&lab);
    reveal(&lab, &export);
    lab.picker_answers
        .borrow_mut()
        .push_back(SelectionOutcome::Selected(directory("/tmp/exports/été é")));
    lab.export_gate.set(true);
    click(&lab, &export);
    assert_eq!(
        lab.window.get_settings_data_export_status(),
        ExportStatus::Running
    );
    assert_eq!(
        lab.window.get_settings_data_export_detail(),
        "/tmp/exports/été é"
    );
    assert!(
        alerts(&lab)
            .iter()
            .any(|a| a.contains("Export en cours vers /tmp/exports/été é"))
    );
    assert_eq!(lab.picker_calls.get(), 1);
    // A second press while it runs is ignored: one picker, one export.
    lab.window.invoke_settings_data_export_requested();
    pump(&lab);
    assert_eq!(lab.picker_calls.get(), 1);
    lab.export_gate.set(false);
    pump(&lab);
    assert_eq!(
        *lab.exports.borrow(),
        vec!["/tmp/exports/été é".to_string()]
    );
    assert_eq!(
        lab.window.get_settings_data_export_status(),
        ExportStatus::Succeeded
    );
    assert!(
        alerts(&lab)
            .iter()
            .any(|a| a == "Export terminé vers /tmp/exports/été é"),
        "{:?}",
        alerts(&lab)
    );
    assert_eq!(export.accessible_enabled(), Some(true));
}

#[test]
fn a_failed_export_and_a_failed_picker_are_shown_and_leave_the_button_usable() {
    let lab = lab();
    let export = export_button(&lab);
    reveal(&lab, &export);
    *lab.export_result.borrow_mut() = Err("disque plein".into());
    lab.picker_answers
        .borrow_mut()
        .push_back(SelectionOutcome::Selected(directory("/tmp/plein")));
    click(&lab, &export);
    assert_eq!(
        lab.window.get_settings_data_export_status(),
        ExportStatus::Failed
    );
    assert!(alerts(&lab).iter().any(|a| a == "disque plein"));
    assert_eq!(export.accessible_enabled(), Some(true));

    lab.picker_answers
        .borrow_mut()
        .push_back(SelectionOutcome::Failed(HostError::Platform(
            "panneau absent".into(),
        )));
    click(&lab, &export);
    assert_eq!(
        lab.window.get_settings_data_export_status(),
        ExportStatus::Failed
    );
    assert!(alerts(&lab).iter().any(|a| a.contains("panneau absent")));
    assert_eq!(lab.exports.borrow().len(), 1, "no export without a folder");
    assert_eq!(export.accessible_enabled(), Some(true));

    // A new attempt clears the previous outcome.
    lab.picker_gate.set(true);
    click(&lab, &export);
    assert!(alerts(&lab).is_empty());
    lab.picker_gate.set(false);
    pump(&lab);
}

#[test]
fn the_real_archive_export_writes_into_a_temporary_folder() {
    let lab = lab();
    let destination = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let database = Arc::new(Database::open(&db_dir.path().join("fixture.db")).unwrap());
    let export = {
        let database = database.clone();
        move |dest: String| {
            souffle_lib::archive::run_archive_export(
                &database,
                std::path::Path::new(&dest),
                chrono::Utc::now(),
                |_, _| {},
            )
            .map(|_| ())
        }
    };
    let dest = destination.path().to_string_lossy().into_owned();
    let result = export(dest.clone());
    assert!(result.is_ok(), "{result:?}");
    let written: Vec<_> = std::fs::read_dir(destination.path())
        .unwrap()
        .filter_map(Result::ok)
        .collect();
    assert_eq!(written.len(), 1, "one souffle-export folder");
    assert!(
        written[0]
            .file_name()
            .to_string_lossy()
            .starts_with("souffle-export-")
    );
    // The page reports the same destination for the same call.
    lab.picker_answers
        .borrow_mut()
        .push_back(SelectionOutcome::Selected(directory(&dest)));
    let button = export_button(&lab);
    reveal(&lab, &button);
    click(&lab, &button);
    assert_eq!(lab.window.get_settings_data_export_detail(), dest.as_str());
}

#[test]
fn revealing_the_data_folder_goes_through_the_host() {
    let lab = lab();
    let reveal_button = button(&lab, "Afficher dans le Finder");
    reveal(&lab, &reveal_button);
    click(&lab, &reveal_button);
    assert_eq!(lab.reveals.get(), 1);
}

// MCP -----------------------------------------------------------------------

fn mcp_info(exists: bool) -> souffle_lib::commands::McpSetupInfo {
    souffle_lib::commands::McpSetupInfo {
        binary_path: "/Applications/Soufflé.app/Contents/MacOS/souffle-mcp".into(),
        exists,
        claude_desktop_snippet: "{\n  \"mcpServers\": {\n    \"souffle\": { \"command\": \"/Applications/Soufflé.app/Contents/MacOS/souffle-mcp\" }\n  }\n}".into(),
        claude_code_command: "claude mcp add souffle '/Applications/Soufflé.app/Contents/MacOS/souffle-mcp'".into(),
    }
}

fn test_connection(lab: &Lab) -> ElementHandle {
    ElementQuery::from_root(&lab.window)
        .match_accessible_role(AccessibleRole::Button)
        .find_all()
        .into_iter()
        .find(|e| {
            matches!(
                e.accessible_label().as_deref(),
                Some("Tester la connexion" | "Test en cours…")
            )
        })
        .expect("the connection button")
}

#[test]
fn the_mcp_states_absent_idle_failed_and_succeeded() {
    let lab = lab();
    crate::data_ui::populate_mcp(&lab.window, &mcp_info(false));
    pump(&lab);
    let test = test_connection(&lab);
    reveal(&lab, &test);
    assert_eq!(lab.window.get_settings_mcp_status(), McpStatus::Absent);
    assert_eq!(test.accessible_enabled(), Some(false));
    assert!(
        alerts(&lab)
            .iter()
            .any(|a| a.starts_with("Binaire introuvable"))
    );
    lab.window.invoke_settings_mcp_test_requested();
    pump(&lab);
    assert_eq!(lab.mcp_calls.get(), 0, "an absent sidecar is never spawned");

    crate::data_ui::populate_mcp(&lab.window, &mcp_info(true));
    pump(&lab);
    assert_eq!(lab.window.get_settings_mcp_status(), McpStatus::Idle);
    assert_eq!(test.accessible_enabled(), Some(true));
    assert!(
        !alerts(&lab)
            .iter()
            .any(|a| a.starts_with("Binaire introuvable"))
    );

    *lab.mcp_result.borrow_mut() = Err("Spawn sidecar: permission refusée".into());
    click(&lab, &test);
    assert_eq!(lab.window.get_settings_mcp_status(), McpStatus::Failed);
    assert!(
        alerts(&lab)
            .iter()
            .any(|a| a == "Spawn sidecar: permission refusée")
    );

    lab.mcp_gate.set(true);
    *lab.mcp_result.borrow_mut() = Ok("search_meetings, get_meeting".into());
    click(&lab, &test);
    assert_eq!(lab.window.get_settings_mcp_status(), McpStatus::Testing);
    assert_eq!(test.accessible_label().as_deref(), Some("Test en cours…"));
    assert_eq!(test.accessible_enabled(), Some(false));
    assert!(alerts(&lab).is_empty(), "the previous failure is gone");
    lab.window.invoke_settings_mcp_test_requested();
    pump(&lab);
    assert_eq!(lab.mcp_calls.get(), 2, "one test at a time");
    lab.mcp_gate.set(false);
    pump(&lab);
    assert_eq!(lab.window.get_settings_mcp_status(), McpStatus::Succeeded);
    assert!(
        alerts(&lab)
            .iter()
            .any(|a| a == "Connexion réussie (search_meetings, get_meeting).")
    );
}

#[test]
fn the_json_and_the_command_are_copied_exactly_and_can_be_selected() {
    let lab = lab();
    let info = mcp_info(true);
    crate::data_ui::populate_mcp(&lab.window, &info);
    pump(&lab);
    let desktop = copy_button(&lab, "Config Claude Desktop");
    reveal(&lab, &desktop);
    click(&lab, &desktop);
    let code = copy_button(&lab, "Claude Code");
    reveal(&lab, &code);
    click(&lab, &code);
    assert_eq!(
        *lab.copied.borrow(),
        vec![
            info.claude_desktop_snippet.clone(),
            info.claude_code_command.clone()
        ]
    );

    // The blocks are read-only editors holding the same text, selectable natively.
    let block = ElementQuery::from_root(&lab.window)
        .match_inherits("KitTextArea")
        .match_accessible_label("Claude Code")
        .find_first()
        .expect("the command block");
    assert_eq!(block.accessible_read_only(), Some(true));
    assert_eq!(
        block.accessible_value().as_deref(),
        Some(info.claude_code_command.as_str())
    );
    reveal(&lab, &block);
    click(&lab, &block);
    block.set_accessible_selection_offsets(0, 10);
    settle();
    copy_selection(&lab);
    assert_eq!(
        clipboard().as_deref(),
        Some(&info.claude_code_command[..10])
    );
}

#[test]
fn a_very_long_path_and_command_stay_inside_the_page_width() {
    let lab = lab();
    let long = format!(
        "/Users/utilisateur/{}/souffle-mcp",
        "très-long-dossier/".repeat(14)
    );
    let mut info = mcp_info(true);
    info.binary_path = long.clone();
    info.claude_code_command = format!("claude mcp add souffle '{long}'");
    info.claude_desktop_snippet = format!("{{\n  \"command\": \"{long}\"\n}}");
    crate::data_ui::populate_mcp(&lab.window, &info);
    pump(&lab);
    let page = ElementHandle::find_by_element_id(&lab.window, "SettingsView::system-page")
        .next()
        .unwrap();
    let right = page.absolute_position().x + page.size().width;
    for block in ElementQuery::from_root(&lab.window)
        .match_inherits("KitTextArea")
        .find_all()
    {
        assert!(
            block.absolute_position().x + block.size().width <= right + 1.0,
            "{:?} is wider than its page",
            block.accessible_label()
        );
        assert!(block.size().height > 0.0);
    }
}

// Model deletion ----------------------------------------------------------

fn delete_trigger(lab: &Lab) -> ElementHandle {
    let group = ElementQuery::from_root(&lab.window)
        .match_inherits("KitFieldSet")
        .match_accessible_label("STOCKAGE DU MODÈLE")
        .find_first()
        .expect("the storage group");
    group
        .query_descendants()
        .match_accessible_role(AccessibleRole::Button)
        .match_accessible_label("Supprimer")
        .find_first()
        .expect("the delete button")
}

#[test]
fn deleting_a_model_in_use_is_not_offered_and_a_refusal_is_shown_verbatim() {
    let lab = lab();
    lab.window
        .set_model_runtime_phase(TranscriptionPhase::Ready);
    pump(&lab);
    let delete = delete_trigger(&lab);
    reveal(&lab, &delete);
    assert_eq!(delete.accessible_enabled(), Some(false), "a loaded model");
    click(&lab, &delete);
    assert!(!lab.window.get_settings_model_delete_confirm_open());
    assert_eq!(lab.delete_calls.get(), 0);

    // The page offers it for a downloaded, unloaded model; the command still decides.
    lab.window
        .set_model_runtime_phase(TranscriptionPhase::LoadRequired);
    pump(&lab);
    assert_eq!(delete.accessible_enabled(), Some(true));
    *lab.delete_result.borrow_mut() =
        Err("Cannot delete the currently loaded model. Unload it first.".into());
    click(&lab, &delete);
    assert!(lab.window.get_settings_model_delete_confirm_open());
    click(&lab, &button(&lab, "Confirmer"));
    assert_eq!(lab.delete_calls.get(), 1);
    assert_eq!(lab.deleted.get(), 0, "a refusal refreshes nothing");
    assert!(
        alerts(&lab)
            .iter()
            .any(|a| a == "Cannot delete the currently loaded model. Unload it first.")
    );
    assert!(!lab.window.get_settings_model_delete_confirm_open());
}

#[test]
fn a_confirmed_deletion_runs_once_even_when_confirmed_twice() {
    let lab = lab();
    lab.window
        .set_model_runtime_phase(TranscriptionPhase::LoadRequired);
    lab.window
        .set_settings_selected_model_label("Whisper fixture".into());
    pump(&lab);
    let delete = delete_trigger(&lab);
    reveal(&lab, &delete);
    click(&lab, &delete);
    assert!(lab.window.get_settings_model_delete_confirm_open());
    let confirm = button(&lab, "Confirmer");
    let position = center(&confirm);
    // Two presses delivered before the page gets to re-evaluate anything.
    for _ in 0..2 {
        let window = lab.window.window();
        window.dispatch_event(WindowEvent::PointerMoved { position });
        window.dispatch_event(WindowEvent::PointerPressed {
            position,
            button: PointerEventButton::Left,
        });
        window.dispatch_event(WindowEvent::PointerReleased {
            position,
            button: PointerEventButton::Left,
        });
    }
    pump(&lab);
    assert_eq!(lab.delete_calls.get(), 1);
    assert_eq!(lab.deleted.get(), 1);
    assert!(!lab.window.get_settings_model_delete_confirm_open());
    assert!(alerts(&lab).is_empty());

    // The request alone, without the dialog, does nothing either.
    lab.window.invoke_settings_delete_model_requested();
    assert_eq!(lab.delete_calls.get(), 1);

    // A new dialog allows one new deletion.
    click(&lab, &delete);
    click(&lab, &button(&lab, "Confirmer"));
    assert_eq!(lab.delete_calls.get(), 2);
}

#[test]
fn cancelling_or_escaping_the_dialog_deletes_nothing_and_returns_the_focus() {
    let lab = lab();
    lab.window
        .set_model_runtime_phase(TranscriptionPhase::LoadRequired);
    pump(&lab);
    let delete = delete_trigger(&lab);
    reveal(&lab, &delete);

    click(&lab, &delete);
    assert!(lab.window.get_settings_model_delete_confirm_open());
    let dialog = ElementQuery::from_root(&lab.window)
        .match_accessible_role(AccessibleRole::Groupbox)
        .match_accessible_label("Supprimer le modèle téléchargé")
        .find_all();
    assert!(!dialog.is_empty(), "the dialog is on screen");
    // Cancel has the initial focus: Return cancels instead of deleting.
    key(&lab, Key::Return);
    assert!(!lab.window.get_settings_model_delete_confirm_open());
    assert_eq!(lab.delete_calls.get(), 0);
    key(&lab, " ");
    assert!(
        lab.window.get_settings_model_delete_confirm_open(),
        "the focus returned to the delete button"
    );

    key(&lab, Key::Escape);
    assert!(!lab.window.get_settings_model_delete_confirm_open());
    assert_eq!(lab.delete_calls.get(), 0);
    click(&lab, &delete);
    click(&lab, &button(&lab, "Annuler"));
    assert!(!lab.window.get_settings_model_delete_confirm_open());
    assert_eq!(lab.delete_calls.get(), 0);
    assert_eq!(lab.deleted.get(), 0);
}

#[test]
fn while_the_confirmation_is_open_the_page_behind_it_does_not_act() {
    let lab = lab();
    lab.window
        .set_model_runtime_phase(TranscriptionPhase::LoadRequired);
    pump(&lab);
    let delete = delete_trigger(&lab);
    reveal(&lab, &delete);
    click(&lab, &delete);
    let behind = button(&lab, "Afficher dans le Finder");
    click(&lab, &behind);
    assert_eq!(lab.reveals.get(), 0, "an inert control behind the modal");
    key(&lab, Key::Escape);
}

// About -------------------------------------------------------------------

fn update_button(lab: &Lab) -> ElementHandle {
    ElementQuery::from_root(&lab.window)
        .match_accessible_role(AccessibleRole::Button)
        .find_all()
        .into_iter()
        .find(|e| {
            matches!(
                e.accessible_label().as_deref(),
                Some("Vérifier les mises à jour" | "Vérification…")
            )
        })
        .expect("the update button")
}

#[test]
fn the_version_is_shown_and_the_update_check_reports_each_outcome() {
    let lab = lab();
    lab.window.set_settings_app_version("local build".into());
    pump(&lab);
    assert!(
        ElementQuery::from_root(&lab.window)
            .match_accessible_label("Build local")
            .find_first()
            .is_some()
    );
    lab.window.set_settings_app_version("1.4.2".into());
    pump(&lab);
    assert!(
        ElementQuery::from_root(&lab.window)
            .match_accessible_label("v1.4.2")
            .find_first()
            .is_some()
    );

    let check = update_button(&lab);
    reveal(&lab, &check);
    lab.update_gate.set(true);
    click(&lab, &check);
    assert_eq!(
        lab.window.get_settings_update_status(),
        UpdateCheckStatus::Checking
    );
    assert_eq!(check.accessible_label().as_deref(), Some("Vérification…"));
    assert_eq!(check.accessible_enabled(), Some(false));
    lab.window.invoke_settings_check_updates_requested();
    pump(&lab);
    assert_eq!(lab.update_calls.get(), 1, "one check at a time");
    lab.update_gate.set(false);
    pump(&lab);
    assert_eq!(
        lab.window.get_settings_update_status(),
        UpdateCheckStatus::UpToDate
    );
    assert!(alerts(&lab).iter().any(|a| a == "À jour."));
    assert_eq!(check.accessible_enabled(), Some(true));

    *lab.update_result.borrow_mut() = Ok(update(true, Some("1.5.0"), None));
    click(&lab, &check);
    assert_eq!(
        lab.window.get_settings_update_status(),
        UpdateCheckStatus::Available
    );
    assert!(
        alerts(&lab)
            .iter()
            .any(|a| a == "Mise à jour disponible : v1.5.0")
    );
    assert!(!alerts(&lab).iter().any(|a| a == "À jour."));

    *lab.update_result.borrow_mut() = Ok(update(false, None, Some("réseau indisponible")));
    click(&lab, &check);
    assert_eq!(
        lab.window.get_settings_update_status(),
        UpdateCheckStatus::Failed
    );
    assert!(alerts(&lab).iter().any(|a| a == "réseau indisponible"));

    *lab.update_result.borrow_mut() = Err("tâche interrompue".into());
    click(&lab, &check);
    assert_eq!(
        lab.window.get_settings_update_status(),
        UpdateCheckStatus::Failed
    );
    assert!(alerts(&lab).iter().any(|a| a == "tâche interrompue"));
}

#[test]
fn the_page_speaks_english_when_the_language_is_english() {
    let lab = lab();
    slint::select_bundled_translation("en").unwrap();
    lab.window
        .global::<crate::SettingsOptions>()
        .invoke_localize();
    pump(&lab);
    let check = ElementQuery::from_root(&lab.window)
        .match_accessible_role(AccessibleRole::Button)
        .match_accessible_label("Check for updates")
        .find_first()
        .expect("the update button in English");
    reveal(&lab, &check);
    *lab.update_result.borrow_mut() = Ok(update(true, Some("2.0.0"), None));
    click(&lab, &check);
    assert!(alerts(&lab).iter().any(|a| a == "Update available: v2.0.0"));
    crate::data_ui::populate_mcp(&lab.window, &mcp_info(true));
    pump(&lab);
    let test = ElementQuery::from_root(&lab.window)
        .match_accessible_role(AccessibleRole::Button)
        .match_accessible_label("Test connection")
        .find_first()
        .expect("the connection button in English");
    reveal(&lab, &test);
    click(&lab, &test);
    assert!(
        alerts(&lab)
            .iter()
            .any(|a| a == "Connection successful (search_meetings, get_meeting).")
    );
    let copy = button(&lab, "Copy diagnostics");
    reveal(&lab, &copy);
    slint::select_bundled_translation("fr").unwrap();
}
