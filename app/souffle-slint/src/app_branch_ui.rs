//! App rules use the shared polish catalogue and the pipeline's resolver.
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use slint::{ComponentHandle, Model, ModelRc, VecModel};
use souffle_lib::commands::SettingsSaveLane;
use souffle_lib::settings::{
    AppBranchId, AppBranchRule, AppBranchTarget, AppSettings, PolishTemplateId,
};
use souffle_lib::summary::{
    APP_BRANCH_TEST_DELAY, AppBranchFallback as DomainFallback, AppBranchResolution,
    AppBranchTestPhase as DomainTestPhase, resolve_app_branch,
};

use crate::settings_drafts::SettingsDraftController;
use crate::settings_io::{SettingsIoCoordinator, SettingsLoadToken};
use crate::settings_values::SettingsCache;
use crate::{AppBranchFallback, AppBranchRow, AppBranchTestPhase, MainWindow, ia_ui};

fn phase_to_slint(phase: DomainTestPhase) -> AppBranchTestPhase {
    match phase {
        DomainTestPhase::Idle => AppBranchTestPhase::Idle,
        DomainTestPhase::Waiting => AppBranchTestPhase::Waiting,
        DomainTestPhase::Resolving => AppBranchTestPhase::Resolving,
        DomainTestPhase::Result => AppBranchTestPhase::Result,
    }
}

#[cfg(test)]
fn phase_from_slint(phase: AppBranchTestPhase) -> DomainTestPhase {
    match phase {
        AppBranchTestPhase::Idle => DomainTestPhase::Idle,
        AppBranchTestPhase::Waiting => DomainTestPhase::Waiting,
        AppBranchTestPhase::Resolving => DomainTestPhase::Resolving,
        AppBranchTestPhase::Result => DomainTestPhase::Result,
    }
}

fn set_phase(window: &MainWindow, phase: DomainTestPhase) {
    window.set_settings_app_branch_test_phase(phase_to_slint(phase));
}

fn result_visible(window: &MainWindow) -> bool {
    window.get_settings_app_branch_test_phase() == AppBranchTestPhase::Result
}

fn sync_model<T: Clone + PartialEq + 'static>(current: ModelRc<T>, rows: Vec<T>) -> ModelRc<T> {
    if let Some(model) = current.as_any().downcast_ref::<VecModel<T>>() {
        for (index, row) in rows.iter().enumerate() {
            match model.row_data(index) {
                Some(existing) if existing == *row => {}
                Some(_) => model.set_row_data(index, row.clone()),
                None => model.push(row.clone()),
            }
        }
        while model.row_count() > rows.len() {
            model.remove(model.row_count() - 1);
        }
        current
    } else {
        Rc::new(VecModel::from(rows)).into()
    }
}

fn targets(settings: &AppSettings) -> Vec<AppBranchTarget> {
    std::iter::once(AppBranchTarget::Global)
        .chain(
            settings
                .dictation_polish_templates
                .iter()
                .map(|template| AppBranchTarget::Template(PolishTemplateId(template.id.clone()))),
        )
        .collect()
}

fn target_at(settings: &AppSettings, index: i32) -> Option<AppBranchTarget> {
    usize::try_from(index)
        .ok()
        .and_then(|index| targets(settings).get(index).cloned())
}

fn target_label(settings: &AppSettings, target: &AppBranchTarget) -> String {
    match target {
        AppBranchTarget::Global => "__DEFAULT__".into(),
        AppBranchTarget::Template(id) => settings
            .dictation_polish_templates
            .iter()
            .find(|template| template.id == id.0)
            .map(ia_ui::dictation_polish_label)
            .unwrap_or_else(|| "__MISSING_TEMPLATE__".into()),
    }
}

fn fallback_to_slint(reason: DomainFallback) -> AppBranchFallback {
    match reason {
        DomainFallback::PolishDisabled => AppBranchFallback::PolishDisabled,
        DomainFallback::AppUnavailable => AppBranchFallback::AppUnavailable,
        DomainFallback::NoMatch => AppBranchFallback::NoMatch,
        DomainFallback::InvalidPattern => AppBranchFallback::InvalidPattern,
        DomainFallback::MissingTemplate => AppBranchFallback::MissingTemplate,
    }
}

#[cfg(test)]
fn fallback_from_slint(reason: AppBranchFallback) -> DomainFallback {
    match reason {
        AppBranchFallback::PolishDisabled => DomainFallback::PolishDisabled,
        AppBranchFallback::AppUnavailable => DomainFallback::AppUnavailable,
        AppBranchFallback::NoMatch => DomainFallback::NoMatch,
        AppBranchFallback::InvalidPattern => DomainFallback::InvalidPattern,
        AppBranchFallback::MissingTemplate => DomainFallback::MissingTemplate,
    }
}

pub(crate) fn populate(window: &MainWindow, settings: &AppSettings) {
    let tested_app =
        result_visible(window).then(|| window.get_settings_app_branch_preview_app().to_string());
    let labels: Vec<slint::SharedString> = targets(settings)
        .iter()
        .map(|target| target_label(settings, target).into())
        .collect();
    window.set_settings_app_branch_target_labels(sync_model(
        window.get_settings_app_branch_target_labels(),
        labels,
    ));
    let rows: Vec<_> = settings
        .dictation_app_branches
        .iter()
        .map(|rule| AppBranchRow {
            id: rule.id.0.as_str().into(),
            app_pattern: rule.app_pattern.as_str().into(),
            enabled: rule.enabled,
            target_label: target_label(settings, &rule.target).into(),
        })
        .collect();
    window.set_settings_app_branch_rules(sync_model(window.get_settings_app_branch_rules(), rows));
    // Provider discovery can finish after Tester. Preserve that request's
    // captured app and re-resolve against the canonical settings, rather than
    // hiding its result because unrelated provider metadata arrived later.
    if let Some(app) = tested_app {
        preview(window, settings, Some(&app));
    }
}

fn preview(window: &MainWindow, settings: &AppSettings, app: Option<&str>) {
    let resolution = resolve_app_branch(settings, app);
    window.set_settings_app_branch_preview_app(app.unwrap_or_default().into());
    window.set_settings_app_branch_preview_template(
        resolution
            .template()
            .map(ia_ui::dictation_polish_label)
            .unwrap_or_default()
            .into(),
    );
    match resolution {
        AppBranchResolution::Rule { rule, .. } => {
            window.set_settings_app_branch_preview_matched(true);
            window.set_settings_app_branch_preview_pattern(rule.app_pattern.as_str().into());
        }
        AppBranchResolution::Global { reason, .. } => {
            window.set_settings_app_branch_preview_matched(false);
            window.set_settings_app_branch_preview_pattern("".into());
            window.set_settings_app_branch_preview_fallback(fallback_to_slint(reason));
        }
    }
    set_phase(window, DomainTestPhase::Result);
}

fn move_rule(settings: &mut AppSettings, id: &AppBranchId, destination: i32) {
    let Some(destination) = usize::try_from(destination)
        .ok()
        .filter(|index| *index < settings.dictation_app_branches.len())
    else {
        return;
    };
    let Some(source) = settings
        .dictation_app_branches
        .iter()
        .position(|rule| &rule.id == id)
    else {
        return;
    };
    let rule = settings.dictation_app_branches.remove(source);
    settings.dictation_app_branches.insert(destination, rule);
}

struct PatternRequest {
    session: Option<SettingsLoadToken>,
    revision: u64,
    value: String,
}

type CaptureApp = Arc<dyn Fn() -> Option<String> + Send + Sync>;

pub(crate) struct Editor {
    window: slint::Weak<MainWindow>,
    io: Rc<SettingsIoCoordinator>,
    cache: SettingsCache,
    drafts: Rc<SettingsDraftController>,
    patterns: RefCell<HashMap<AppBranchId, PatternRequest>>,
    test_generation: Cell<u64>,
    test_timer: RefCell<Option<slint::Timer>>,
    capture_app: CaptureApp,
}

impl Editor {
    fn project_routing(&self, window: &MainWindow, settings: &AppSettings) {
        let provider_available = window.get_settings_summary_unusable_message().is_empty();
        ia_ui::populate_dictation_polish(window, settings, provider_available);
        let displayed_id = window.get_settings_active_dictation_polish_id();
        self.drafts
            .reapply_polish_prompt(window, displayed_id.as_str());
    }

    fn save(self: &Rc<Self>, mutation: impl FnOnce(&mut AppSettings) + Send + 'static) -> u64 {
        self.cancel_test();
        let token = self.io.current_load_token();
        let revision = self
            .io
            .submit(SettingsSaveLane::General, mutation, move |_, outcome| {
                crate::log_settings_save_outcome("app branch", outcome)
            });
        if let Some(token) = token {
            let weak = Rc::downgrade(self);
            self.io
                .observe_current_open_snapshot(token, move |settings| {
                    if let Some(editor) = weak.upgrade()
                        && let Some(window) = editor.window.upgrade()
                    {
                        editor.project_routing(&window, &settings);
                    }
                });
        }
        revision
    }

    pub(crate) fn cancel_test(&self) {
        self.test_generation
            .set(self.test_generation.get().wrapping_add(1));
        self.test_timer.borrow_mut().take();
        if let Some(window) = self.window.upgrade() {
            set_phase(&window, DomainTestPhase::Idle);
        }
    }

    fn accepts_test(&self, token: SettingsLoadToken, generation: u64) -> bool {
        self.io.accepts_load(token) && self.test_generation.get() == generation
    }

    fn request_test(self: &Rc<Self>) {
        self.cancel_test();
        let Some(token) = self.io.current_load_token() else {
            return;
        };
        let generation = self.test_generation.get();
        if let Some(window) = self.window.upgrade() {
            set_phase(&window, DomainTestPhase::Waiting);
        }
        let weak = Rc::downgrade(self);
        let timer = slint::Timer::default();
        timer.start(
            slint::TimerMode::SingleShot,
            APP_BRANCH_TEST_DELAY,
            move || {
                if let Some(editor) = weak.upgrade() {
                    editor.capture_test(token, generation);
                }
            },
        );
        *self.test_timer.borrow_mut() = Some(timer);
    }

    fn capture_test(self: &Rc<Self>, token: SettingsLoadToken, generation: u64) {
        self.test_timer.borrow_mut().take();
        if !self.accepts_test(token, generation) {
            return;
        }
        if let Some(window) = self.window.upgrade() {
            set_phase(&window, DomainTestPhase::Resolving);
        }
        let capture = self.capture_app.clone();
        // Read NSWorkspace on a worker only after the focus-switch interval,
        // then freeze the app before waiting for ordered Settings writes.
        let worker = souffle_lib::async_runtime::spawn_blocking(move || capture());
        let weak = Rc::downgrade(self);
        slint::spawn_local(async move {
            let app = worker.await.ok().flatten();
            let Some(editor) = weak.upgrade() else { return };
            if !editor.accepts_test(token, generation) {
                return;
            }
            let weak = Rc::downgrade(&editor);
            editor
                .io
                .observe_current_open_snapshot(token, move |settings| {
                    let Some(editor) = weak.upgrade() else { return };
                    if let Some(window) = editor.window.upgrade() {
                        // Invalidating a test must still allow its terminal
                        // snapshot to settle all canonical routing controls.
                        editor.project_routing(&window, &settings);
                        if editor.accepts_test(token, generation) {
                            preview(&window, &settings, app.as_deref());
                        }
                    }
                });
        })
        .expect("Slint event loop is unavailable for app rule capture");
    }

    fn pattern_is_current(&self, id: &AppBranchId, pattern: &str) -> bool {
        let session = self.io.current_load_token();
        self.patterns.borrow_mut().retain(|_, request| {
            request.session == session && !self.io.revision_is_observed(request.revision)
        });
        if let Some(request) = self.patterns.borrow().get(id) {
            return request.value == pattern;
        }
        // A reopened session can still have writes from its predecessor in
        // flight. Its observed cache is not a safe deduplication baseline.
        self.io.snapshot_revision_is_current()
            && self.cache.borrow().as_ref().is_some_and(|settings| {
                settings
                    .dictation_app_branches
                    .iter()
                    .any(|rule| &rule.id == id && rule.app_pattern == pattern)
            })
    }
}

/// Wire the real global routing controls alongside the rule editor. Their
/// changes invalidate capture requests, then settle all canonical routing
/// controls/rows through the same terminal observation as rule mutations.
pub(crate) fn wire_polish_routing_callbacks(window: &MainWindow, editor: Rc<Editor>) {
    let current = editor.clone();
    window.on_settings_dictation_polish_enabled_changed(move |enabled| {
        current.save(move |settings| settings.dictation_polish_enabled = enabled);
    });
    let current = editor.clone();
    window.on_settings_dictation_polish_prompt_changed(move |text| {
        let active_id = current
            .window
            .upgrade()
            .map(|window| window.get_settings_active_dictation_polish_id().to_string());
        if let Some(active_id) = active_id.filter(|id| !id.is_empty()) {
            current
                .drafts
                .edit_polish_prompt(active_id, text.to_string());
        }
    });
    window.on_settings_dictation_polish_template_changed(move |template_id| {
        let template_id = template_id.to_string();
        if !editor.cache.borrow().as_ref().is_some_and(|settings| {
            ia_ui::contains_dictation_polish_id(&settings.dictation_polish_templates, &template_id)
        }) {
            return;
        }
        editor.save(move |settings| settings.dictation_polish_template_id = template_id);
    });
}

pub(crate) fn register(
    window: &MainWindow,
    io: Rc<SettingsIoCoordinator>,
    cache: SettingsCache,
    drafts: Rc<SettingsDraftController>,
) -> Rc<Editor> {
    register_with_capture(
        window,
        io,
        cache,
        drafts,
        Arc::new(|| souffle_lib::commands::frontmost_app_name().unwrap_or(None)),
    )
}

fn register_with_capture(
    window: &MainWindow,
    io: Rc<SettingsIoCoordinator>,
    cache: SettingsCache,
    drafts: Rc<SettingsDraftController>,
    capture_app: CaptureApp,
) -> Rc<Editor> {
    window.set_settings_app_branch_test_delay_seconds(
        i32::try_from(APP_BRANCH_TEST_DELAY.as_secs()).expect("app rule delay must fit Slint int"),
    );
    let editor = Rc::new(Editor {
        window: window.as_weak(),
        io,
        cache,
        drafts,
        patterns: RefCell::new(HashMap::new()),
        test_generation: Cell::new(0),
        test_timer: RefCell::new(None),
        capture_app,
    });
    let current = editor.clone();
    window.on_settings_app_branch_added(move |pattern, target_index| {
        let pattern = pattern.trim().to_string();
        if pattern.is_empty() {
            return;
        }
        current.save(move |settings| {
            if let Some(target) = target_at(settings, target_index) {
                settings
                    .dictation_app_branches
                    .push(AppBranchRule::new(pattern, target));
            }
        });
    });
    let current = editor.clone();
    window.on_settings_app_branch_deleted(move |id| {
        let id = AppBranchId(id.to_string());
        current.save(move |settings| settings.dictation_app_branches.retain(|rule| rule.id != id));
    });
    let current = editor.clone();
    window.on_settings_app_branch_enabled(move |id, enabled| {
        let id = AppBranchId(id.to_string());
        current.save(move |settings| {
            if let Some(rule) = settings
                .dictation_app_branches
                .iter_mut()
                .find(|rule| rule.id == id)
            {
                rule.enabled = enabled;
            }
        });
    });
    let current = editor.clone();
    window.on_settings_app_branch_pattern_changed(move |id, pattern| {
        let id = AppBranchId(id.to_string());
        let pattern = pattern.trim().to_string();
        // Empty edits are rejected and the canonical row is restored.
        if pattern.is_empty() {
            let settings = current.cache.known_snapshot();
            if let Some(window) = current.window.upgrade()
                && let Some(settings) = settings
            {
                populate(&window, &settings);
            }
            return;
        }
        if current.pattern_is_current(&id, &pattern) {
            return;
        }
        let worker_id = id.clone();
        let worker_pattern = pattern.clone();
        let revision = current.save(move |settings| {
            if let Some(rule) = settings
                .dictation_app_branches
                .iter_mut()
                .find(|rule| rule.id == worker_id)
            {
                rule.app_pattern = worker_pattern;
            }
        });
        current.patterns.borrow_mut().insert(
            id,
            PatternRequest {
                session: current.io.current_load_token(),
                revision,
                value: pattern,
            },
        );
    });
    let current = editor.clone();
    window.on_settings_app_branch_target_changed(move |id, index| {
        let id = AppBranchId(id.to_string());
        current.save(move |settings| {
            if let Some(target) = target_at(settings, index)
                && let Some(rule) = settings
                    .dictation_app_branches
                    .iter_mut()
                    .find(|rule| rule.id == id)
            {
                rule.target = target;
            }
        });
    });
    let current = editor.clone();
    window.on_settings_app_branch_moved(move |id, destination| {
        let id = AppBranchId(id.to_string());
        current.save(move |settings| move_rule(settings, &id, destination));
    });
    let current = editor.clone();
    window.on_settings_app_branch_test_requested(move || current.request_test());
    editor
}

#[cfg(test)]
mod tests {
    use super::*;
    use slint::Model;
    use slint::platform::{
        EventLoopProxy, Platform, WindowAdapter, software_renderer::MinimalSoftwareWindow,
    };
    use std::sync::mpsc::{Receiver, Sender};
    use std::time::Duration;

    type UiEvent = Box<dyn FnOnce() + Send>;
    thread_local! {
        static CLOCK: Cell<Duration> = const { Cell::new(Duration::ZERO) };
        static EVENTS: (Sender<UiEvent>, Receiver<UiEvent>) = std::sync::mpsc::channel();
    }

    struct TestProxy(Sender<UiEvent>);
    impl EventLoopProxy for TestProxy {
        fn quit_event_loop(&self) -> Result<(), slint::EventLoopError> {
            Ok(())
        }
        fn invoke_from_event_loop(&self, event: UiEvent) -> Result<(), slint::EventLoopError> {
            self.0
                .send(event)
                .map_err(|_| slint::EventLoopError::EventLoopTerminated)
        }
    }

    struct TestPlatform {
        registering: Cell<bool>,
    }
    impl Platform for TestPlatform {
        fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
            Ok(MinimalSoftwareWindow::new(Default::default()))
        }
        fn new_event_loop_proxy(&self) -> Option<Box<dyn EventLoopProxy>> {
            // Unit fixtures run on independent libtest threads. Keep the
            // process-global proxy unset during platform registration; local
            // futures then obtain the proxy belonging to their own context.
            if self.registering.replace(false) {
                return None;
            }
            Some(Box::new(TestProxy(EVENTS.with(|events| events.0.clone()))))
        }
        fn duration_since_start(&self) -> Duration {
            CLOCK.get()
        }
    }

    fn test_window() -> MainWindow {
        let _ = slint::platform::set_platform(Box::new(TestPlatform {
            registering: Cell::new(true),
        }));
        MainWindow::new().unwrap()
    }

    fn pump() {
        slint::platform::update_timers_and_animations();
        for _ in 0..64 {
            let event = EVENTS.with(|events| events.1.try_recv().ok());
            let Some(event) = event else { break };
            event();
        }
    }

    fn advance(duration: Duration) {
        CLOCK.set(CLOCK.get() + duration);
        pump();
    }

    fn wait_until(mut done: impl FnMut() -> bool) {
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while !done() {
            assert!(
                std::time::Instant::now() < deadline,
                "app rule operation timed out"
            );
            pump();
            std::thread::yield_now();
        }
    }

    fn settle(io: &SettingsIoCoordinator, window: &MainWindow) {
        if window.get_settings_app_branch_test_phase() == AppBranchTestPhase::Waiting {
            advance(APP_BRANCH_TEST_DELAY);
        }
        let revision_limit = io.current_revision() + 16;
        wait_until(|| {
            assert!(
                io.current_revision() <= revision_limit,
                "app rule observations must be bounded"
            );
            io.drain_one_for_test();
            io.is_idle_for_test()
                && window.get_settings_app_branch_test_phase() != AppBranchTestPhase::Resolving
        });
    }

    fn isolated_editor() -> (
        MainWindow,
        Rc<SettingsIoCoordinator>,
        std::sync::Arc<souffle_lib::db::Database>,
        tempfile::TempDir,
        Rc<Editor>,
    ) {
        use souffle_lib::commands::SettingsSaveOutcome;
        use std::sync::Arc;
        let dir = tempfile::tempdir().unwrap();
        let db = Arc::new(souffle_lib::db::Database::open(&dir.path().join("rules.db")).unwrap());
        let initial = AppSettings::default();
        initial.save(&db).unwrap();
        let window = test_window();
        let cache = SettingsCache::with_observed(&window, initial.clone());
        let load = Arc::clone(&db);
        let load_autostart = Arc::clone(&db);
        let save = Arc::clone(&db);
        let io = SettingsIoCoordinator::with_functions(
            cache.clone(),
            move || AppSettings::load(&load),
            move || AppSettings::load(&load_autostart),
            move |settings| {
                settings.save(&save).unwrap();
                SettingsSaveOutcome::Observed {
                    settings: Box::new(AppSettings::load(&save).unwrap()),
                    result: Ok(()),
                }
            },
            |_| panic!("app branches must not use the autostart lane"),
        );
        io.begin_open();
        let values = crate::settings_values::SettingsValueController::new(
            &window,
            cache.clone(),
            io.clone(),
            Rc::new(std::cell::RefCell::new(Vec::new())),
            Rc::new(std::cell::RefCell::new(None)),
        );
        crate::settings_values::wire(&window, values);
        let editor = register_with_capture(
            &window,
            io.clone(),
            cache,
            SettingsDraftController::new(&window, io.clone()),
            Arc::new(|| Some("Terminal".into())),
        );
        wire_polish_routing_callbacks(&window, editor.clone());
        populate(&window, &initial);
        (window, io, db, dir, editor)
    }

    struct BlockedEditor {
        window: MainWindow,
        io: Rc<SettingsIoCoordinator>,
        db: Arc<souffle_lib::db::Database>,
        editor: Rc<Editor>,
        _dir: tempfile::TempDir,
        release: Sender<()>,
        frontmost: Arc<std::sync::Mutex<Option<String>>>,
        captures: Arc<std::sync::atomic::AtomicUsize>,
    }

    impl BlockedEditor {
        fn new() -> Self {
            use souffle_lib::commands::SettingsSaveOutcome;
            use std::sync::{
                Mutex,
                atomic::{AtomicUsize, Ordering},
            };
            let dir = tempfile::tempdir().unwrap();
            let db =
                Arc::new(souffle_lib::db::Database::open(&dir.path().join("rules.db")).unwrap());
            let initial = AppSettings::default();
            initial.save(&db).unwrap();
            let window = test_window();
            let cache = SettingsCache::with_observed(&window, initial.clone());
            let load = db.clone();
            let effective = db.clone();
            let save = db.clone();
            let (started_tx, started_rx) = std::sync::mpsc::channel();
            let (release, release_rx) = std::sync::mpsc::channel();
            let release_rx = Mutex::new(release_rx);
            let saves = AtomicUsize::new(0);
            let io = SettingsIoCoordinator::with_functions(
                cache.clone(),
                move || AppSettings::load(&load),
                move || AppSettings::load(&effective),
                move |settings| {
                    if saves.fetch_add(1, Ordering::SeqCst) == 0 {
                        started_tx.send(()).unwrap();
                        release_rx
                            .lock()
                            .unwrap()
                            .recv_timeout(Duration::from_secs(2))
                            .unwrap();
                    }
                    settings.save(&save).unwrap();
                    SettingsSaveOutcome::Observed {
                        settings: Box::new(AppSettings::load(&save).unwrap()),
                        result: Ok(()),
                    }
                },
                |_| panic!("app rules must not use the autostart lane"),
            );
            io.begin_open();
            let frontmost = Arc::new(Mutex::new(Some("Soufflé Nightly".into())));
            let captures = Arc::new(AtomicUsize::new(0));
            let app = frontmost.clone();
            let count = captures.clone();
            let editor = register_with_capture(
                &window,
                io.clone(),
                cache,
                SettingsDraftController::new(&window, io.clone()),
                Arc::new(move || {
                    count.fetch_add(1, Ordering::SeqCst);
                    app.lock().unwrap().clone()
                }),
            );
            wire_polish_routing_callbacks(&window, editor.clone());
            populate(&window, &initial);
            let rule = window.get_settings_app_branch_rules().row_data(0).unwrap();
            window.invoke_settings_app_branch_enabled(rule.id, true);
            started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
            Self {
                window,
                io,
                db,
                editor,
                _dir: dir,
                release,
                frontmost,
                captures,
            }
        }

        fn capture_terminal(&self) {
            *self.frontmost.lock().unwrap() = Some("Terminal".into());
            let before = self.io.current_open_snapshot_request_for_test();
            self.window.invoke_settings_app_branch_test_requested();
            advance(APP_BRANCH_TEST_DELAY);
            wait_until(|| self.io.current_open_snapshot_request_for_test() > before);
        }

        fn finish(&self) {
            self.release.send(()).unwrap();
            settle(&self.io, &self.window);
            assert!(
                self.window
                    .get_settings_app_branch_rules()
                    .row_data(0)
                    .unwrap()
                    .enabled
            );
            assert!(AppSettings::load(&self.db).unwrap().dictation_app_branches[0].enabled);
        }
    }

    fn assert_polish_controls_match(window: &MainWindow, canonical: &AppSettings) {
        let active = canonical
            .dictation_polish_templates
            .iter()
            .find(|template| template.id == canonical.dictation_polish_template_id)
            .unwrap();
        assert_eq!(
            window.get_settings_active_dictation_polish_id().as_str(),
            active.id
        );
        assert_eq!(
            window.get_settings_active_dictation_polish_label().as_str(),
            ia_ui::dictation_polish_label(active)
        );
        assert_eq!(
            window
                .get_settings_active_dictation_polish_prompt()
                .as_str(),
            active.prompt
        );
        assert_eq!(
            window.get_settings_dictation_polish_enabled(),
            canonical.dictation_polish_enabled
        );
        assert_eq!(
            window.get_settings_app_branch_rules().row_count(),
            canonical.dictation_app_branches.len()
        );
        for (index, rule) in canonical.dictation_app_branches.iter().enumerate() {
            let row = window
                .get_settings_app_branch_rules()
                .row_data(index)
                .unwrap();
            assert_eq!(row.id.as_str(), rule.id.0);
            assert_eq!(row.enabled, rule.enabled);
            assert_eq!(row.app_pattern.as_str(), rule.app_pattern);
        }
    }

    #[test]
    fn global_template_then_rule_publishes_canonical_polish_controls() {
        let test = BlockedEditor::new();
        ia_ui::populate_dictation_polish(&test.window, &AppSettings::load(&test.db).unwrap(), true);
        assert_eq!(
            test.window.get_settings_active_dictation_polish_label(),
            "__CLEANUP__"
        );
        test.window
            .invoke_settings_dictation_polish_template_changed(
                souffle_lib::summary::TEMPLATE_EMAIL.into(),
            );
        let row = test
            .window
            .get_settings_app_branch_rules()
            .row_data(0)
            .unwrap();
        test.window.invoke_settings_app_branch_enabled(row.id, true);
        test.finish();
        let canonical = AppSettings::load(&test.db).unwrap();
        assert_eq!(
            canonical.dictation_polish_template_id,
            souffle_lib::summary::TEMPLATE_EMAIL
        );
        assert_polish_controls_match(&test.window, &canonical);
    }

    #[test]
    fn global_template_then_tester_publishes_canonical_polish_controls() {
        let test = BlockedEditor::new();
        ia_ui::populate_dictation_polish(&test.window, &AppSettings::load(&test.db).unwrap(), true);
        test.window
            .invoke_settings_dictation_polish_template_changed(
                souffle_lib::summary::TEMPLATE_EMAIL.into(),
            );
        test.capture_terminal();
        test.finish();
        let canonical = AppSettings::load(&test.db).unwrap();
        assert_eq!(
            canonical.dictation_polish_template_id,
            souffle_lib::summary::TEMPLATE_EMAIL
        );
        assert_polish_controls_match(&test.window, &canonical);
        assert_eq!(
            test.window.get_settings_app_branch_preview_app(),
            "Terminal"
        );
        assert_eq!(
            test.window.get_settings_app_branch_test_phase(),
            AppBranchTestPhase::Result
        );
    }

    #[test]
    fn polish_enabled_then_rule_publishes_canonical_polish_controls() {
        let test = BlockedEditor::new();
        ia_ui::populate_dictation_polish(&test.window, &AppSettings::load(&test.db).unwrap(), true);
        assert!(test.window.get_settings_dictation_polish_enabled());
        test.window
            .invoke_settings_dictation_polish_enabled_changed(false);
        let row = test
            .window
            .get_settings_app_branch_rules()
            .row_data(0)
            .unwrap();
        test.window.invoke_settings_app_branch_enabled(row.id, true);
        test.finish();
        let canonical = AppSettings::load(&test.db).unwrap();
        assert!(!canonical.dictation_polish_enabled);
        assert_polish_controls_match(&test.window, &canonical);
    }

    #[test]
    fn coalesced_routing_preserves_shared_prompt_drafts_and_routes_visible_edits() {
        let (window, io, db, _dir, editor) = isolated_editor();
        let initial = AppSettings::load(&db).unwrap();
        ia_ui::populate_dictation_polish(&window, &initial, true);
        window.invoke_settings_dictation_polish_template_changed(
            souffle_lib::summary::TEMPLATE_EMAIL.into(),
        );
        settle(&io, &window);
        window.set_settings_active_dictation_polish_prompt("Email draft".into());
        window.invoke_settings_dictation_polish_prompt_changed("Email draft".into());
        window.invoke_settings_dictation_polish_template_changed(
            souffle_lib::summary::TEMPLATE_CLEAN.into(),
        );
        settle(&io, &window);
        assert_polish_controls_match(&window, &AppSettings::load(&db).unwrap());

        // No mock-clock advance: the shared prompt is still a pending draft
        // when the rule observer supersedes the global-template observer.
        window.invoke_settings_dictation_polish_template_changed(
            souffle_lib::summary::TEMPLATE_EMAIL.into(),
        );
        let row = window.get_settings_app_branch_rules().row_data(0).unwrap();
        window.invoke_settings_app_branch_enabled(row.id.clone(), true);
        settle(&io, &window);
        assert_eq!(
            window.get_settings_active_dictation_polish_label(),
            "__PROFESSIONAL_EMAIL__"
        );
        assert_eq!(
            window.get_settings_active_dictation_polish_prompt(),
            "Email draft"
        );

        window.set_settings_active_dictation_polish_prompt("New email draft".into());
        window.invoke_settings_dictation_polish_prompt_changed("New email draft".into());
        editor.drafts.flush_explicit(|result| {
            assert_eq!(result, crate::settings_drafts::DraftFlushResult::Committed)
        });
        settle(&io, &window);
        window.invoke_settings_app_branch_enabled(row.id, false);
        settle(&io, &window);
        let canonical = AppSettings::load(&db).unwrap();
        let email = canonical
            .dictation_polish_templates
            .iter()
            .find(|template| template.id == souffle_lib::summary::TEMPLATE_EMAIL)
            .unwrap();
        assert_eq!(email.prompt, "New email draft");
        let cleanup = canonical
            .dictation_polish_templates
            .iter()
            .find(|template| template.id == souffle_lib::summary::TEMPLATE_CLEAN)
            .unwrap();
        assert_eq!(
            cleanup.prompt,
            initial
                .dictation_polish_templates
                .iter()
                .find(|template| template.id == souffle_lib::summary::TEMPLATE_CLEAN)
                .unwrap()
                .prompt
        );
        assert_polish_controls_match(&window, &canonical);
    }

    #[test]
    fn prompt_edit_during_intermediate_global_response_targets_displayed_template() {
        use souffle_lib::commands::SettingsSaveOutcome;
        use std::sync::{
            Mutex,
            atomic::{AtomicBool, Ordering},
        };
        let dir = tempfile::tempdir().unwrap();
        let db = Arc::new(souffle_lib::db::Database::open(&dir.path().join("rules.db")).unwrap());
        let initial = AppSettings::default();
        initial.save(&db).unwrap();
        let original_email_prompt = initial
            .dictation_polish_templates
            .iter()
            .find(|template| template.id == souffle_lib::summary::TEMPLATE_EMAIL)
            .unwrap()
            .prompt
            .clone();
        let window = test_window();
        let cache = SettingsCache::with_observed(&window, initial.clone());
        let load = db.clone();
        let effective = db.clone();
        let save = db.clone();
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let release_rx = Mutex::new(release_rx);
        let blocked = AtomicBool::new(false);
        let io = SettingsIoCoordinator::with_functions(
            cache.clone(),
            move || AppSettings::load(&load),
            move || AppSettings::load(&effective),
            move |settings| {
                if settings.dictation_app_branches[0].enabled
                    && !blocked.swap(true, Ordering::SeqCst)
                {
                    started_tx.send(()).unwrap();
                    release_rx
                        .lock()
                        .unwrap()
                        .recv_timeout(Duration::from_secs(2))
                        .unwrap();
                }
                settings.save(&save).unwrap();
                SettingsSaveOutcome::Observed {
                    settings: Box::new(AppSettings::load(&save).unwrap()),
                    result: Ok(()),
                }
            },
            |_| panic!("unexpected autostart save"),
        );
        io.begin_open();
        let editor = register_with_capture(
            &window,
            io.clone(),
            cache.clone(),
            SettingsDraftController::new(&window, io.clone()),
            Arc::new(|| Some("Terminal".into())),
        );
        wire_polish_routing_callbacks(&window, editor.clone());
        ia_ui::populate_dictation_polish(&window, &initial, true);
        window.invoke_settings_dictation_polish_template_changed(
            souffle_lib::summary::TEMPLATE_EMAIL.into(),
        );
        let rule = window.get_settings_app_branch_rules().row_data(0).unwrap();
        window.invoke_settings_app_branch_enabled(rule.id, true);
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();

        // The global save response is Intermediate because the rule save
        // remains blocked. Durable/cache ID advances, but its prompt is not
        // the one the user is editing until terminal publication happens.
        io.drain_one_for_test();
        assert_eq!(
            cache.known_snapshot().unwrap().dictation_polish_template_id,
            souffle_lib::summary::TEMPLATE_EMAIL
        );
        assert_eq!(
            window.get_settings_active_dictation_polish_label(),
            "__CLEANUP__"
        );
        assert_eq!(
            AppSettings::load(&db).unwrap().dictation_polish_template_id,
            souffle_lib::summary::TEMPLATE_EMAIL
        );
        window.set_settings_active_dictation_polish_prompt("Edited displayed Cleanup".into());
        window.invoke_settings_dictation_polish_prompt_changed("Edited displayed Cleanup".into());
        editor.drafts.flush_explicit(|result| {
            assert_eq!(result, crate::settings_drafts::DraftFlushResult::Committed)
        });
        release_tx.send(()).unwrap();
        settle(&io, &window);
        let canonical = AppSettings::load(&db).unwrap();
        assert!(canonical.dictation_app_branches[0].enabled);
        assert_eq!(
            canonical.dictation_polish_template_id,
            souffle_lib::summary::TEMPLATE_EMAIL
        );
        let cleanup = canonical
            .dictation_polish_templates
            .iter()
            .find(|template| template.id == souffle_lib::summary::TEMPLATE_CLEAN)
            .unwrap();
        let email = canonical
            .dictation_polish_templates
            .iter()
            .find(|template| template.id == souffle_lib::summary::TEMPLATE_EMAIL)
            .unwrap();
        assert_eq!(cleanup.prompt, "Edited displayed Cleanup");
        assert_eq!(email.prompt, original_email_prompt);
        assert_polish_controls_match(&window, &canonical);
    }

    #[test]
    fn ordinary_tester_waits_for_focus_switch_and_freezes_captured_app() {
        use std::sync::atomic::Ordering;
        let test = BlockedEditor::new();
        let before = test.io.current_open_snapshot_request_for_test();
        test.window.invoke_settings_app_branch_test_requested();
        assert_eq!(
            test.window.get_settings_app_branch_test_phase(),
            AppBranchTestPhase::Waiting
        );
        assert_eq!(
            test.window.get_settings_app_branch_test_delay_seconds() as u64,
            APP_BRANCH_TEST_DELAY.as_secs()
        );
        advance(APP_BRANCH_TEST_DELAY - Duration::from_millis(1));
        assert_eq!(test.captures.load(Ordering::SeqCst), 0);
        *test.frontmost.lock().unwrap() = Some("Terminal".into());
        advance(Duration::from_millis(1));
        wait_until(|| test.io.current_open_snapshot_request_for_test() > before);
        assert_eq!(test.captures.load(Ordering::SeqCst), 1);
        assert_eq!(
            test.window.get_settings_app_branch_test_phase(),
            AppBranchTestPhase::Resolving
        );
        // Frontmost changes while Settings writes still block; this request
        // must retain Terminal and use the same canonical resolver as dictation.
        *test.frontmost.lock().unwrap() = Some("Mail".into());
        test.finish();
        assert_eq!(
            test.window.get_settings_app_branch_preview_app(),
            "Terminal"
        );
        let canonical = AppSettings::load(&test.db).unwrap();
        let resolution = resolve_app_branch(&canonical, Some("Terminal"));
        assert_eq!(
            test.window
                .get_settings_app_branch_preview_template()
                .as_str(),
            ia_ui::dictation_polish_label(resolution.template().unwrap())
        );
        assert_eq!(
            test.window.get_settings_app_branch_test_phase(),
            AppBranchTestPhase::Result
        );
    }

    #[test]
    fn newer_tester_restarts_timer_and_supersedes_captured_request() {
        use std::sync::atomic::Ordering;
        let test = BlockedEditor::new();
        test.window.invoke_settings_app_branch_test_requested();
        advance(APP_BRANCH_TEST_DELAY / 2);
        test.window.invoke_settings_app_branch_test_requested();
        advance(APP_BRANCH_TEST_DELAY / 2);
        assert_eq!(test.captures.load(Ordering::SeqCst), 0);
        let before = test.io.current_open_snapshot_request_for_test();
        *test.frontmost.lock().unwrap() = Some("Terminal".into());
        advance(APP_BRANCH_TEST_DELAY / 2);
        wait_until(|| test.io.current_open_snapshot_request_for_test() > before);
        assert_eq!(test.captures.load(Ordering::SeqCst), 1);
        *test.frontmost.lock().unwrap() = Some("Mail".into());
        let before = test.io.current_open_snapshot_request_for_test();
        test.window.invoke_settings_app_branch_test_requested();
        advance(APP_BRANCH_TEST_DELAY);
        wait_until(|| test.io.current_open_snapshot_request_for_test() > before);
        test.finish();
        assert_eq!(test.captures.load(Ordering::SeqCst), 2);
        assert_eq!(test.window.get_settings_app_branch_preview_app(), "Mail");
    }

    #[test]
    fn pending_tester_is_cancelled_by_actual_global_template_callback() {
        let test = BlockedEditor::new();
        test.capture_terminal();
        test.window
            .invoke_settings_dictation_polish_template_changed(
                souffle_lib::summary::TEMPLATE_EMAIL.into(),
            );
        assert_eq!(
            test.window.get_settings_app_branch_test_phase(),
            AppBranchTestPhase::Idle
        );
        test.finish();
        assert_eq!(
            test.window.get_settings_app_branch_test_phase(),
            AppBranchTestPhase::Idle
        );
        assert_eq!(
            AppSettings::load(&test.db)
                .unwrap()
                .dictation_polish_template_id,
            souffle_lib::summary::TEMPLATE_EMAIL
        );
        assert_eq!(
            test.window.get_settings_active_dictation_polish_label(),
            "__PROFESSIONAL_EMAIL__"
        );
    }

    #[test]
    fn pending_tester_is_cancelled_by_actual_polish_enabled_callback() {
        let test = BlockedEditor::new();
        test.capture_terminal();
        test.window
            .invoke_settings_dictation_polish_enabled_changed(false);
        assert_eq!(
            test.window.get_settings_app_branch_test_phase(),
            AppBranchTestPhase::Idle
        );
        test.finish();
        assert_eq!(
            test.window.get_settings_app_branch_test_phase(),
            AppBranchTestPhase::Idle
        );
        assert!(
            !AppSettings::load(&test.db)
                .unwrap()
                .dictation_polish_enabled
        );
        assert!(!test.window.get_settings_dictation_polish_enabled());
    }

    #[test]
    fn routing_callbacks_cancel_waiting_timer_without_capturing() {
        use std::sync::atomic::Ordering;
        let test = BlockedEditor::new();
        test.window.invoke_settings_app_branch_test_requested();
        test.window
            .invoke_settings_dictation_polish_template_changed(
                souffle_lib::summary::TEMPLATE_EMAIL.into(),
            );
        advance(APP_BRANCH_TEST_DELAY);
        assert_eq!(test.captures.load(Ordering::SeqCst), 0);
        test.window.invoke_settings_app_branch_test_requested();
        test.window
            .invoke_settings_dictation_polish_enabled_changed(false);
        advance(APP_BRANCH_TEST_DELAY);
        assert_eq!(test.captures.load(Ordering::SeqCst), 0);
        test.finish();
        assert_eq!(
            test.window.get_settings_app_branch_test_phase(),
            AppBranchTestPhase::Idle
        );
    }

    #[test]
    fn pending_tester_is_cancelled_by_rule_mutation_with_canonical_rows() {
        let test = BlockedEditor::new();
        test.capture_terminal();
        let original = test
            .window
            .get_settings_app_branch_rules()
            .row_data(0)
            .unwrap();
        test.window
            .invoke_settings_app_branch_pattern_changed(original.id, "Messages".into());
        test.finish();
        assert_eq!(
            test.window.get_settings_app_branch_test_phase(),
            AppBranchTestPhase::Idle
        );
        assert_eq!(
            test.window
                .get_settings_app_branch_rules()
                .row_data(0)
                .unwrap()
                .app_pattern,
            "Messages"
        );
    }

    #[test]
    fn reopened_pattern_editor_cannot_deduplicate_against_unsettled_old_session() {
        let test = BlockedEditor::new();
        let original = test
            .window
            .get_settings_app_branch_rules()
            .row_data(0)
            .unwrap();
        test.window
            .invoke_settings_app_branch_pattern_changed(original.id.clone(), "Messages".into());
        test.editor.cancel_test();
        test.io.close();
        test.io.begin_open();
        test.editor.cancel_test();
        test.window
            .invoke_settings_app_branch_pattern_changed(original.id, original.app_pattern.clone());
        test.finish();
        assert_eq!(
            AppSettings::load(&test.db).unwrap().dictation_app_branches[0].app_pattern,
            original.app_pattern.as_str()
        );
        assert_eq!(
            test.window
                .get_settings_app_branch_rules()
                .row_data(0)
                .unwrap()
                .app_pattern,
            original.app_pattern
        );
    }

    #[test]
    fn closed_or_reopened_session_rejects_waiting_and_captured_tester() {
        use std::sync::atomic::Ordering;
        for captured in [false, true] {
            let test = BlockedEditor::new();
            if captured {
                test.capture_terminal();
            } else {
                test.window.invoke_settings_app_branch_test_requested();
            }
            // Same explicit invalidation as the real main.rs open/close hooks.
            test.editor.cancel_test();
            test.io.close();
            test.io.begin_open();
            test.editor.cancel_test();
            advance(APP_BRANCH_TEST_DELAY);
            assert_eq!(test.captures.load(Ordering::SeqCst), usize::from(captured));
            test.release.send(()).unwrap();
            settle(&test.io, &test.window);
            assert_eq!(
                test.window.get_settings_app_branch_test_phase(),
                AppBranchTestPhase::Idle
            );
            assert!(
                !test
                    .window
                    .get_settings_app_branch_rules()
                    .row_data(0)
                    .unwrap()
                    .enabled
            );
            assert!(AppSettings::load(&test.db).unwrap().dictation_app_branches[0].enabled);
            // A request in the new session remains usable after rejection.
            *test.frontmost.lock().unwrap() = Some("Mail".into());
            test.window.invoke_settings_app_branch_test_requested();
            settle(&test.io, &test.window);
            assert_eq!(test.window.get_settings_app_branch_preview_app(), "Mail");
            assert!(
                test.window
                    .get_settings_app_branch_rules()
                    .row_data(0)
                    .unwrap()
                    .enabled
            );
        }
    }

    #[test]
    fn capturing_worker_does_not_retain_window_and_late_completion_is_cancelled() {
        use std::sync::Mutex;
        let (window, io, _db, _dir, original_editor) = isolated_editor();
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let release_rx = Mutex::new(release_rx);
        let editor = register_with_capture(
            &window,
            io.clone(),
            original_editor.cache.clone(),
            original_editor.drafts.clone(),
            Arc::new(move || {
                started_tx.send(()).unwrap();
                release_rx
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(2))
                    .unwrap();
                Some("Terminal".into())
            }),
        );
        window.invoke_settings_app_branch_test_requested();
        advance(APP_BRANCH_TEST_DELAY);
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let before = io.current_open_snapshot_request_for_test();
        let weak = window.as_weak();
        drop(window);
        assert!(
            weak.upgrade().is_none(),
            "an awaited capture must hold only a weak Window"
        );
        editor.cancel_test();
        release_tx.send(()).unwrap();
        // The worker wakes exactly this context's future. Drive that event,
        // proving its late completion does not enqueue an observation.
        let event = EVENTS.with(|events| events.1.recv_timeout(Duration::from_secs(2)).unwrap());
        event();
        pump();
        assert_eq!(io.current_open_snapshot_request_for_test(), before);
    }

    #[test]
    fn pattern_a_b_a_while_save_is_blocked_preserves_latest_requested_value() {
        use souffle_lib::commands::SettingsSaveOutcome;
        use std::sync::{Arc, Mutex};
        use std::time::{Duration, Instant};
        let dir = tempfile::tempdir().unwrap();
        let db = Arc::new(souffle_lib::db::Database::open(&dir.path().join("rules.db")).unwrap());
        let initial = AppSettings::default();
        initial.save(&db).unwrap();
        let window = test_window();
        let cache = SettingsCache::with_observed(&window, initial.clone());
        let load = Arc::clone(&db);
        let effective = Arc::clone(&db);
        let save = Arc::clone(&db);
        let (started_tx, started_rx) = std::sync::mpsc::sync_channel(1);
        let (release_tx, release_rx) = std::sync::mpsc::sync_channel(1);
        let release_rx = Mutex::new(release_rx);
        let io = SettingsIoCoordinator::with_functions(
            cache.clone(),
            move || AppSettings::load(&load),
            move || AppSettings::load(&effective),
            move |settings| {
                if settings.dictation_app_branches[0].app_pattern == "Messages" {
                    started_tx.send(()).unwrap();
                    release_rx
                        .lock()
                        .unwrap()
                        .recv_timeout(Duration::from_secs(2))
                        .unwrap();
                }
                settings.save(&save).unwrap();
                SettingsSaveOutcome::Observed {
                    settings: Box::new(AppSettings::load(&save).unwrap()),
                    result: Ok(()),
                }
            },
            |_| panic!("app branches must not use the autostart lane"),
        );
        io.begin_open();
        register(
            &window,
            io.clone(),
            cache,
            SettingsDraftController::new(&window, io.clone()),
        );
        populate(&window, &initial);
        let original = window.get_settings_app_branch_rules().row_data(0).unwrap();
        window.invoke_settings_app_branch_pattern_changed(original.id.clone(), "Messages".into());
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        window.invoke_settings_app_branch_pattern_changed(
            original.id.clone(),
            original.app_pattern.clone(),
        );
        release_tx.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while !io.is_idle_for_test() {
            assert!(io.current_revision() <= 16 && Instant::now() < deadline);
            io.drain_one_for_test();
            std::thread::yield_now();
        }
        assert_eq!(
            AppSettings::load(&db).unwrap().dictation_app_branches[0].app_pattern,
            original.app_pattern.as_str()
        );
        assert_eq!(
            window
                .get_settings_app_branch_rules()
                .row_data(0)
                .unwrap()
                .app_pattern,
            original.app_pattern
        );
        // Re-publication/focus-lost echoes of the canonical value must not
        // create another write, even after a rapid round trip.
        let settled_revision = io.current_revision();
        window.invoke_settings_app_branch_pattern_changed(original.id, original.app_pattern);
        assert_eq!(io.current_revision(), settled_revision);
    }

    #[test]
    fn delayed_open_and_immediate_rule_test_publish_one_terminal_snapshot() {
        use souffle_lib::commands::SettingsSaveOutcome;
        use std::cell::Cell;
        use std::sync::{Arc, Mutex};
        use std::time::{Duration, Instant};

        let dir = tempfile::tempdir().unwrap();
        let db = Arc::new(souffle_lib::db::Database::open(&dir.path().join("rules.db")).unwrap());
        let initial = AppSettings::default();
        initial.save(&db).unwrap();
        let window = test_window();
        let cache = SettingsCache::with_observed(&window, initial.clone());
        let load = Arc::clone(&db);
        let load_effective = Arc::clone(&db);
        let save = Arc::clone(&db);
        let (started_tx, started_rx) = std::sync::mpsc::sync_channel(1);
        let (release_tx, release_rx) = std::sync::mpsc::sync_channel(1);
        let release_rx = Mutex::new(release_rx);
        let io = SettingsIoCoordinator::with_functions(
            cache.clone(),
            move || AppSettings::load(&load),
            move || {
                started_tx.send(()).unwrap();
                release_rx
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(2))
                    .unwrap();
                AppSettings::load(&load_effective)
            },
            move |settings| {
                settings.save(&save).unwrap();
                SettingsSaveOutcome::Observed {
                    settings: Box::new(AppSettings::load(&save).unwrap()),
                    result: Ok(()),
                }
            },
            |_| panic!("app branches must not use the autostart lane"),
        );
        let token = io.begin_open();
        register_with_capture(
            &window,
            io.clone(),
            cache,
            SettingsDraftController::new(&window, io.clone()),
            Arc::new(|| Some("Terminal".into())),
        );
        populate(&window, &initial);
        let opened = Rc::new(Cell::new(0));
        let open_count = opened.clone();
        let weak = window.as_weak();
        io.load_effective_snapshot(token, move |settings| {
            open_count.set(open_count.get() + 1);
            if let Some(window) = weak.upgrade() {
                populate(&window, &settings);
            }
        });
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();

        // Invoke the actual editor callbacks without draining the worker.
        let original = window.get_settings_app_branch_rules().row_data(0).unwrap();
        window.invoke_settings_app_branch_enabled(original.id, true);
        let before_test = io.current_open_snapshot_request_for_test();
        window.invoke_settings_app_branch_test_requested();
        advance(APP_BRANCH_TEST_DELAY);
        // Allow the capture future to queue its terminal observation while
        // Open remains blocked; neither audience may restart the other.
        wait_until(|| io.current_open_snapshot_request_for_test() > before_test);
        release_tx.send(()).unwrap();

        let deadline = Instant::now() + Duration::from_secs(2);
        while !io.is_idle_for_test() {
            assert!(
                io.current_revision() <= 32 && Instant::now() < deadline,
                "Open and rule/Tester observations must settle: revision={}, open publications={}",
                io.current_revision(),
                opened.get(),
            );
            pump();
            io.drain_one_for_test();
            std::thread::yield_now();
        }
        assert_eq!(opened.get(), 1);
        assert!(AppSettings::load(&db).unwrap().dictation_app_branches[0].enabled);
        assert!(
            window
                .get_settings_app_branch_rules()
                .row_data(0)
                .unwrap()
                .enabled
        );
        assert!(result_visible(&window));
        // Holding the coordinator must not hold a strong Window handle.
        let weak = window.as_weak();
        drop(window);
        assert!(weak.upgrade().is_none());
    }

    #[test]
    fn provider_refresh_after_tester_preserves_result_until_rule_mutation() {
        let (window, io, db, _dir, _editor) = isolated_editor();
        window.invoke_settings_app_branch_test_requested();
        settle(&io, &window);
        let tested_app = window.get_settings_app_branch_preview_app();
        let tested_template = window.get_settings_app_branch_preview_template();
        let mut settings = AppSettings::load(&db).unwrap();
        ia_ui::populate_dictation_polish(&window, &settings, true);
        ia_ui::populate_dictation_polish(&window, &settings, false);
        assert!(result_visible(&window));
        assert_eq!(window.get_settings_app_branch_preview_app(), tested_app);
        assert_eq!(
            window.get_settings_app_branch_preview_template(),
            tested_template
        );

        // If canonical routing itself changes, a late provider publication
        // must re-resolve the captured app instead of retaining stale output.
        settings.dictation_polish_template_id = souffle_lib::summary::TEMPLATE_EMAIL.into();
        ia_ui::populate_dictation_polish(&window, &settings, true);
        assert_eq!(
            window.get_settings_app_branch_preview_template(),
            "__PROFESSIONAL_EMAIL__"
        );
        settings.dictation_polish_enabled = false;
        ia_ui::populate_dictation_polish(&window, &settings, true);
        assert!(!window.get_settings_app_branch_preview_matched());
        assert_eq!(
            window.get_settings_app_branch_preview_fallback(),
            AppBranchFallback::PolishDisabled
        );

        let original = window.get_settings_app_branch_rules().row_data(0).unwrap();
        window.invoke_settings_app_branch_enabled(original.id, true);
        assert!(!result_visible(&window));
        settle(&io, &window);
        assert!(!result_visible(&window));
    }

    #[test]
    fn immediate_test_after_mutation_publishes_terminal_rule_rows() {
        let (window, io, db, _dir, _editor) = isolated_editor();
        let original = window.get_settings_app_branch_rules().row_data(0).unwrap();
        window.invoke_settings_app_branch_enabled(original.id.clone(), true);
        window.invoke_settings_app_branch_test_requested();
        settle(&io, &window);
        assert!(AppSettings::load(&db).unwrap().dictation_app_branches[0].enabled);
        assert!(
            window
                .get_settings_app_branch_rules()
                .row_data(0)
                .unwrap()
                .enabled,
            "terminal test snapshot must refresh the rule switch"
        );

        window.invoke_settings_app_branch_enabled(original.id.clone(), false);
        window.invoke_settings_app_branch_test_requested();
        settle(&io, &window);
        assert!(
            !window
                .get_settings_app_branch_rules()
                .row_data(0)
                .unwrap()
                .enabled
        );
        window.invoke_settings_app_branch_target_changed(original.id.clone(), 2);
        window.invoke_settings_app_branch_test_requested();
        settle(&io, &window);
        assert_eq!(
            window
                .get_settings_app_branch_rules()
                .row_data(0)
                .unwrap()
                .target_label,
            "__PROFESSIONAL_EMAIL__"
        );
        let old_count = window.get_settings_app_branch_rules().row_count();
        window.invoke_settings_app_branch_added("Immediate addition".into(), 0);
        window.invoke_settings_app_branch_test_requested();
        settle(&io, &window);
        assert_eq!(
            window.get_settings_app_branch_rules().row_count(),
            old_count + 1
        );
        assert!(result_visible(&window));
    }

    #[test]
    fn unrelated_settings_save_cannot_hide_rule_publication() {
        let (window, io, db, _dir, _editor) = isolated_editor();
        let original = window.get_settings_app_branch_rules().row_data(0).unwrap();
        let paste_delay = AppSettings::load(&db).unwrap().paste_delay_ms as i32;
        window.invoke_settings_app_branch_enabled(original.id.clone(), true);
        window.invoke_settings_paste_delay_changed(600);
        settle(&io, &window);
        let durable = AppSettings::load(&db).unwrap();
        assert!(durable.dictation_app_branches[0].enabled);
        assert_eq!(durable.paste_delay_ms, 600);
        assert!(
            window
                .get_settings_app_branch_rules()
                .row_data(0)
                .unwrap()
                .enabled
        );
        assert_eq!(window.get_settings_paste_delay_ms(), 600);
        window.invoke_settings_app_branch_test_requested();
        window.invoke_settings_app_branch_target_changed(original.id.clone(), 2);
        window.invoke_settings_paste_delay_changed(paste_delay);
        settle(&io, &window);
        assert!(
            !result_visible(&window),
            "superseded preview must not publish"
        );
        assert_eq!(
            window
                .get_settings_app_branch_rules()
                .row_data(0)
                .unwrap()
                .target_label,
            "__PROFESSIONAL_EMAIL__"
        );
        window.invoke_settings_app_branch_test_requested();
        settle(&io, &window);
        assert!(result_visible(&window));
    }

    #[test]
    fn rule_snapshots_do_not_publish_after_close_or_into_reopened_session() {
        for reopen in [false, true] {
            let (window, io, db, _dir, editor) = isolated_editor();
            let initial = AppSettings::load(&db).unwrap();
            let original = window.get_settings_app_branch_rules().row_data(0).unwrap();
            window.invoke_settings_app_branch_enabled(original.id.clone(), true);
            window.invoke_settings_app_branch_test_requested();
            editor.cancel_test();
            io.close();
            if reopen {
                io.begin_open();
            }
            populate(&window, &initial);
            settle(&io, &window);
            assert!(AppSettings::load(&db).unwrap().dictation_app_branches[0].enabled);
            assert!(
                !window
                    .get_settings_app_branch_rules()
                    .row_data(0)
                    .unwrap()
                    .enabled
            );
            assert!(!result_visible(&window));
        }
    }

    #[test]
    fn rule_controls_persist_to_isolated_database_and_preview_matches_pipeline() {
        use souffle_lib::commands::SettingsSaveOutcome;
        use std::sync::Arc;
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("rules.db");
        let db = Arc::new(souffle_lib::db::Database::open(&db_path).unwrap());
        let mut initial = AppSettings::default();
        initial.dictation_app_branches.clear();
        initial.save(&db).unwrap();
        let window = test_window();
        let cache = SettingsCache::with_observed(&window, initial.clone());
        let load = Arc::clone(&db);
        let load_autostart = Arc::clone(&db);
        let save = Arc::clone(&db);
        let io = SettingsIoCoordinator::with_functions(
            cache.clone(),
            move || AppSettings::load(&load),
            move || AppSettings::load(&load_autostart),
            move |settings| {
                settings.save(&save).unwrap();
                SettingsSaveOutcome::Observed {
                    settings: Box::new(AppSettings::load(&save).unwrap()),
                    result: Ok(()),
                }
            },
            |_| panic!("app branches must not use the autostart lane"),
        );
        io.begin_open();
        register(
            &window,
            io.clone(),
            cache,
            SettingsDraftController::new(&window, io.clone()),
        );
        populate(&window, &initial);
        window.invoke_settings_app_branch_added("Mail".into(), 0);
        window.invoke_settings_app_branch_added("mail".into(), 2);
        settle(&io, &window);
        let first = window.get_settings_app_branch_rules().row_data(0).unwrap();
        let second = window.get_settings_app_branch_rules().row_data(1).unwrap();
        assert!(!first.enabled && !second.enabled);
        window.invoke_settings_app_branch_enabled(first.id.clone(), true);
        window.invoke_settings_app_branch_enabled(second.id.clone(), true);
        window.invoke_settings_app_branch_pattern_changed(second.id.clone(), "Apple Mail".into());
        window.invoke_settings_app_branch_moved(second.id.clone(), 0);
        settle(&io, &window);
        let durable = AppSettings::load(&db).unwrap();
        preview(&window, &durable, Some("Apple Mail"));
        assert!(window.get_settings_app_branch_preview_matched());
        assert_eq!(
            window.get_settings_app_branch_preview_pattern(),
            "Apple Mail"
        );
        assert_eq!(
            window.get_settings_app_branch_preview_template(),
            "__PROFESSIONAL_EMAIL__"
        );
        assert_eq!(
            resolve_app_branch(&durable, Some("Apple Mail"))
                .template()
                .unwrap()
                .id,
            "email"
        );
        window.invoke_settings_app_branch_target_changed(second.id.clone(), 0);
        settle(&io, &window);
        let durable = AppSettings::load(&db).unwrap();
        preview(&window, &durable, Some("Apple Mail"));
        assert_eq!(
            window.get_settings_app_branch_preview_template(),
            "__CLEANUP__"
        );
        window.invoke_settings_app_branch_pattern_changed(first.id.clone(), "   ".into());
        window.invoke_settings_app_branch_added("   ".into(), 0);
        window.invoke_settings_app_branch_deleted(first.id.clone());
        settle(&io, &window);
        assert_eq!(
            AppSettings::load(&db).unwrap().dictation_app_branches.len(),
            1
        );
        drop(io);
        drop(db);
        let restarted = souffle_lib::db::Database::open(&db_path).unwrap();
        let restored = AppSettings::load(&restarted).unwrap();
        assert_eq!(restored.dictation_app_branches[0].id.0, second.id.as_str());
        assert!(restored.dictation_app_branches[0].enabled);
        assert_eq!(
            restored.dictation_app_branches[0].target,
            AppBranchTarget::Global
        );
        preview(&window, &restored, None);
        assert_eq!(
            window.get_settings_app_branch_preview_fallback(),
            AppBranchFallback::AppUnavailable
        );
        assert!(!window.get_settings_app_branch_preview_matched());
    }

    #[test]
    fn fallback_contract_round_trips_exhaustively() {
        for reason in [
            DomainFallback::PolishDisabled,
            DomainFallback::AppUnavailable,
            DomainFallback::NoMatch,
            DomainFallback::InvalidPattern,
            DomainFallback::MissingTemplate,
        ] {
            assert_eq!(fallback_from_slint(fallback_to_slint(reason)), reason);
        }
    }

    #[test]
    fn test_phase_contract_round_trips_exhaustively() {
        for phase in [
            DomainTestPhase::Idle,
            DomainTestPhase::Waiting,
            DomainTestPhase::Resolving,
            DomainTestPhase::Result,
        ] {
            assert_eq!(phase_from_slint(phase_to_slint(phase)), phase);
        }
    }

    #[test]
    fn canonical_repopulation_preserves_models_and_does_not_echo_writes() {
        let (window, io, db, _dir, _editor) = isolated_editor();
        let rules = window.get_settings_app_branch_rules();
        let labels = window.get_settings_app_branch_target_labels();
        let canonical = AppSettings::load(&db).unwrap();
        let revision = io.current_revision();
        populate(&window, &canonical);
        assert!(std::ptr::eq(
            rules.as_any(),
            window.get_settings_app_branch_rules().as_any()
        ));
        assert!(std::ptr::eq(
            labels.as_any(),
            window.get_settings_app_branch_target_labels().as_any()
        ));
        let row = rules.row_data(0).unwrap();
        window.invoke_settings_app_branch_pattern_changed(row.id, row.app_pattern);
        assert_eq!(io.current_revision(), revision);
    }

    #[test]
    fn rejected_pattern_save_restores_canonical_value_and_allows_retry() {
        use souffle_lib::commands::{SettingsSaveError, SettingsSaveOutcome};
        use std::sync::atomic::{AtomicBool, Ordering};
        let dir = tempfile::tempdir().unwrap();
        let db = Arc::new(souffle_lib::db::Database::open(&dir.path().join("rules.db")).unwrap());
        let initial = AppSettings::default();
        initial.save(&db).unwrap();
        let window = test_window();
        let cache = SettingsCache::with_observed(&window, initial.clone());
        let load = db.clone();
        let effective = db.clone();
        let save = db.clone();
        let reject = AtomicBool::new(true);
        let io = SettingsIoCoordinator::with_functions(
            cache.clone(),
            move || AppSettings::load(&load),
            move || AppSettings::load(&effective),
            move |settings| {
                let result = if reject.swap(false, Ordering::SeqCst) {
                    Err(SettingsSaveError::NotCommitted {
                        message: "injected rollback".into(),
                    })
                } else {
                    settings.save(&save).unwrap();
                    Ok(())
                };
                SettingsSaveOutcome::Observed {
                    settings: Box::new(AppSettings::load(&save).unwrap()),
                    result,
                }
            },
            |_| panic!("unexpected autostart save"),
        );
        io.begin_open();
        register(
            &window,
            io.clone(),
            cache,
            SettingsDraftController::new(&window, io.clone()),
        );
        populate(&window, &initial);
        let original = window.get_settings_app_branch_rules().row_data(0).unwrap();
        window.invoke_settings_app_branch_pattern_changed(original.id.clone(), "Messages".into());
        settle(&io, &window);
        assert_eq!(
            window
                .get_settings_app_branch_rules()
                .row_data(0)
                .unwrap()
                .app_pattern,
            original.app_pattern
        );
        let revision = io.current_revision();
        window.invoke_settings_app_branch_pattern_changed(
            original.id.clone(),
            original.app_pattern.clone(),
        );
        assert_eq!(io.current_revision(), revision);
        window.invoke_settings_app_branch_pattern_changed(original.id, "Messages".into());
        settle(&io, &window);
        assert_eq!(
            AppSettings::load(&db).unwrap().dictation_app_branches[0].app_pattern,
            "Messages"
        );
        assert_eq!(
            window
                .get_settings_app_branch_rules()
                .row_data(0)
                .unwrap()
                .app_pattern,
            "Messages"
        );
    }

    #[test]
    fn target_catalogue_tracks_shared_templates_and_global() {
        let mut settings = AppSettings::default();
        settings
            .dictation_polish_templates
            .push(souffle_lib::settings::DictationPolishTemplate {
                id: "user-template".into(),
                label: "My shared template".into(),
                prompt: "Custom instruction".into(),
            });
        assert_eq!(target_at(&settings, 0), Some(AppBranchTarget::Global));
        assert_eq!(
            targets(&settings).len(),
            settings.dictation_polish_templates.len() + 1
        );
        assert_eq!(target_at(&settings, -1), None);
        assert_eq!(target_at(&settings, 100), None);
        for (index, template) in settings.dictation_polish_templates.iter().enumerate() {
            assert_eq!(
                target_at(&settings, (index + 1) as i32),
                Some(AppBranchTarget::Template(PolishTemplateId(
                    template.id.clone()
                )))
            );
        }
        assert_eq!(
            target_label(
                &settings,
                &AppBranchTarget::Template(PolishTemplateId("user-template".into()))
            ),
            "My shared template"
        );
    }

    #[test]
    fn editor_reorders_by_stable_id_and_preserves_other_fields() {
        let mut settings = AppSettings::default();
        let first = settings.dictation_app_branches[0].clone();
        move_rule(&mut settings, &first.id, 2);
        assert_eq!(settings.dictation_app_branches[2], first);
        let before = settings.dictation_app_branches.clone();
        move_rule(&mut settings, &first.id, -1);
        move_rule(&mut settings, &AppBranchId("missing".into()), 0);
        assert_eq!(settings.dictation_app_branches, before);
    }
}
